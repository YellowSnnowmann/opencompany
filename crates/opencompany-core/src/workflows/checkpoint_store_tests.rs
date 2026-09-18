use std::sync::{Arc, Mutex};

use serde_json::json;
use tinyflows::caps::mock::mock_capabilities;
use tinyflows::graph::ids::NodeId;
use tinyflows::graph::{Checkpoint, Checkpointer};
use tinyflows::model::{Edge, Node, NodeKind, WorkflowGraph};
use tinyflows::observability::RunObserver;

use super::*;

fn checkpoint(id: &str, namespace: &[&str], value: u64) -> Checkpoint<Value> {
    Checkpoint {
        thread_id: "lineage".to_string(),
        checkpoint_id: id.to_string(),
        run_id: Some("attempt".to_string()),
        parent_checkpoint_id: None,
        namespace: namespace.iter().map(|part| (*part).to_string()).collect(),
        state: json!({ "value": value }),
        next_nodes: vec![NodeId::new("next")],
        completed_tasks: Vec::new(),
        pending_writes: Vec::new(),
        interrupts: Vec::new(),
        pending_activations: None,
        barrier_arrivals: Vec::new(),
        metadata: Value::Null,
    }
}

/// [`checkpoint`], with an explicit `thread_id` rather than the hardcoded
/// `"lineage"` — for the thread-id validation tests below, where the
/// thread id itself is what is under test.
fn checkpoint_for_thread(thread_id: &str, id: &str) -> Checkpoint<Value> {
    Checkpoint {
        thread_id: thread_id.to_string(),
        checkpoint_id: id.to_string(),
        run_id: Some("attempt".to_string()),
        parent_checkpoint_id: None,
        namespace: Vec::new(),
        state: json!({ "value": 1 }),
        next_nodes: vec![NodeId::new("next")],
        completed_tasks: Vec::new(),
        pending_writes: Vec::new(),
        interrupts: Vec::new(),
        pending_activations: None,
        barrier_arrivals: Vec::new(),
        metadata: Value::Null,
    }
}

/// PR #1991 review (`3903802623`, `tinysweeper/security`). Every producer
/// in this codebase mints `thread_id` from
/// [`crate::ports::generate_id`] (see `validate_thread_id`'s doc
/// comment for the traced call sites), so this is defence-in-depth rather
/// than a live path — but it is cheap, and it is the failing gate, so it
/// gets enforced here too rather than resting solely on
/// `FileCheckpointer`'s own percent-encoding.
#[tokio::test]
async fn a_path_traversal_thread_id_is_refused_before_touching_the_filesystem() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = WorkflowCheckpointStore::new(temp.path());
    let escape_target = temp
        .path()
        .parent()
        .expect("tempdir has a parent")
        .join("escaped-checkpoint-marker.jsonl");

    for hostile in [
        "../../../../../../../tmp/escaped-checkpoint-marker",
        "..",
        ".",
        "a/b",
        "a\\b",
        "thread/../../escape",
        "",
    ] {
        let err = store
            .get(hostile, None)
            .await
            .expect_err("a hostile thread id must be refused, not resolved to a path");
        assert!(
            err.to_string().contains("thread id"),
            "hostile={hostile:?} err={err}"
        );

        let err = store
            .put(checkpoint_for_thread(hostile, "c1"))
            .await
            .expect_err("put must refuse the same way get does");
        assert!(err.to_string().contains("thread id"), "{err}");
    }

    assert!(
        !escape_target.exists(),
        "no file may ever be written outside the store's base_dir"
    );
}

/// The validation must not reject the shape every real `thread_id` in
/// this codebase actually has — a plain run id.
#[tokio::test]
async fn an_ordinary_generated_thread_id_still_round_trips() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = WorkflowCheckpointStore::new(temp.path());
    let thread_id = crate::ports::generate_id();

    store
        .put(checkpoint_for_thread(&thread_id, "c1"))
        .await
        .expect("an ordinary generated thread id must still be accepted");
    assert!(store.has_resume_point(&thread_id).await.unwrap());
}

#[tokio::test]
async fn survives_reopen_and_round_trips_scoped_namespaces() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = WorkflowCheckpointStore::new(temp.path());
    store
        .put(checkpoint("shared", &[], 1))
        .await
        .expect("put root");
    store
        .put(checkpoint("shared", &["loop", "child"], 2))
        .await
        .expect("put child");

    let reopened = WorkflowCheckpointStore::new(temp.path());
    let root = reopened
        .get_scoped("lineage", Some("shared"), &[])
        .await
        .expect("get root")
        .expect("root checkpoint");
    let child_namespace = vec!["loop".to_string(), "child".to_string()];
    let child = reopened
        .get_scoped("lineage", Some("shared"), &child_namespace)
        .await
        .expect("get child")
        .expect("child checkpoint");

    assert_eq!(root.state, json!({ "value": 1 }));
    assert_eq!(child.state, json!({ "value": 2 }));
    assert!(reopened.has_resume_point("lineage").await.unwrap());
}

#[tokio::test]
async fn clean_settle_prunes_the_whole_lineage() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = WorkflowCheckpointStore::new(temp.path());
    store.put(checkpoint("one", &[], 1)).await.expect("put");

    store.prune_settled("lineage").await.expect("prune");

    assert!(store.list("lineage").await.unwrap().is_empty());
    assert!(!store.has_resume_point("lineage").await.unwrap());
}

#[derive(Default)]
struct FinishedNodes(Mutex<Vec<String>>);

impl RunObserver for FinishedNodes {
    fn on_step_finish(&self, step: &tinyflows::observability::ExecutionStep) {
        self.0.lock().unwrap().push(step.node_id.clone());
    }
}

fn parser_node(id: &str) -> Node {
    Node {
        id: id.to_string(),
        kind: NodeKind::OutputParser,
        type_version: 1,
        name: id.to_string(),
        config: Value::Null,
        ports: Vec::new(),
        position: None,
    }
}

#[tokio::test]
async fn reopens_at_node_seven_without_replaying_completed_nodes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = WorkflowCheckpointStore::new(temp.path());
    let mut nodes = vec![Node {
        id: "trigger".to_string(),
        kind: NodeKind::Trigger,
        type_version: 1,
        name: "trigger".to_string(),
        config: Value::Null,
        ports: Vec::new(),
        position: None,
    }];
    nodes.extend((1..=9).map(|index| {
        let mut node = parser_node(&format!("node-{index}"));
        if index == 7 {
            node.config = json!({ "requires_approval": true });
        }
        node
    }));
    let mut edges = vec![Edge {
        from_node: "trigger".to_string(),
        from_port: "main".to_string(),
        to_node: "node-1".to_string(),
        to_port: "main".to_string(),
    }];
    edges.extend((1..9).map(|index| Edge {
        from_node: format!("node-{index}"),
        from_port: "main".to_string(),
        to_node: format!("node-{}", index + 1),
        to_port: "main".to_string(),
    }));
    let compiled = tinyflows::compiler::compile(&WorkflowGraph {
        nodes,
        edges,
        ..Default::default()
    })
    .expect("compile");
    let checkpointer: Arc<dyn Checkpointer<Value>> = Arc::new(store);
    let paused = tinyflows::engine::run_with_checkpointer(
        &compiled,
        json!({ "request": "restart" }),
        &mock_capabilities(),
        checkpointer,
        "node-seven-lineage",
    )
    .await
    .expect("pause at node seven");
    assert_eq!(paused.pending_approvals, vec!["node-7".to_string()]);

    let reopened: Arc<dyn Checkpointer<Value>> =
        Arc::new(WorkflowCheckpointStore::new(temp.path()));
    let observer = Arc::new(FinishedNodes::default());
    let dyn_observer: Arc<dyn RunObserver> = observer.clone();
    tinyflows::engine::resume_with_checkpointer_journaled_observed(
        &compiled,
        &mock_capabilities(),
        reopened,
        "node-seven-lineage",
        vec!["node-7".to_string()],
        Vec::new(),
        Arc::new(tinyflows::engine::InMemoryGraphEventJournal::new()),
        &dyn_observer,
    )
    .await
    .expect("resume from durable node boundary");

    assert_eq!(*observer.0.lock().unwrap(), ["node-7", "node-8", "node-9"]);
}
