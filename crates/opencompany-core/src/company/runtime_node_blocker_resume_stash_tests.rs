//! Runtime node-blocker resume: cancelled-answer stash pruning and stranded-stash reconciliation.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::company::CompanyManifest;
use crate::company::runtime::CompanyRuntime;
use crate::company::task_intent::BlockerReplyIntent;
use crate::ports::blockers::{BlockerKind, BlockerPayload, BlockerSource, BlockerStep};
use crate::ports::types::{CompanyId, Effect, EffectGroup};
use crate::ports::{WorkflowRun, WorkflowRunContext, WorkflowRunner};
use crate::runtime::RuntimeBuilder;
use crate::runtime::journal::{ApprovalConversation, TaskLink};
use crate::runtime::workflow_resume::{CONTINUATION_BLOCKER_KEY, workflow_node_turn_key};

const RUN_ID: &str = "run-that-blocked";
const NODE_ID: &str = "draft";
const WORKFLOW_TOML: &str = r#"
id = "reporting"
name = "Reporting"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "draft"
kind = "output"
name = "Draft"
[[edge]]
from = "start"
to = "draft"
"#;

#[derive(Clone, Debug)]
struct StartedRun {
    // Not read by any assertion in this file — kept to match the shape
    // `RecordingRunner` shares with `runtime_node_blocker_resume_retry_tests.rs`,
    // where `workflow_id` is the field under test.
    #[allow(dead_code)]
    workflow_id: String,
    input: Value,
}

#[derive(Default)]
struct RecordingRunner {
    started: Mutex<Vec<StartedRun>>,
}

impl RecordingRunner {
    fn started(&self) -> Vec<StartedRun> {
        self.started.lock().expect("recording runner").clone()
    }
}

#[async_trait]
impl WorkflowRunner for RecordingRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        workflow: &crate::company::WorkflowFile,
        input: Value,
        _ctx: &WorkflowRunContext,
    ) -> crate::Result<WorkflowRun> {
        self.started
            .lock()
            .expect("recording runner")
            .push(StartedRun {
                workflow_id: workflow.id.clone(),
                input,
            });
        Ok(WorkflowRun {
            output: json!({ "ok": true }),
            pending_approvals: Vec::new(),
            deliveries: Vec::new(),
            cancelled: false,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        })
    }
}

fn manifest() -> CompanyManifest {
    toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n\
         [[agent]]\nid = \"eng\"\nrole = \"Engineer\"\n",
    )
    .expect("manifest")
}

fn seed_home() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("opencompany-node-blocker-")
        .tempdir()
        .expect("tempdir");
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).expect("workflows dir");
    std::fs::write(workflows.join("reporting.toml"), WORKFLOW_TOML).expect("seed graph");
    dir
}

async fn runtime(
    home: &std::path::Path,
    with_runner: bool,
) -> (Arc<CompanyRuntime>, Arc<RecordingRunner>) {
    let mut rt = RuntimeBuilder::new(home.to_path_buf(), manifest())
        .with_id(CompanyId::new("acme"))
        .with_seed_dir(home.to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let runner = Arc::new(RecordingRunner::default());
    if with_runner {
        rt.set_workflow_runner(runner.clone());
    }
    (Arc::new(rt), runner)
}

/// Parks a node blocker exactly as `park_node_blocker_as` does, and
/// arms the same per-(run, node) stash its `stash_node_blocker_resume`
/// writes at park time.
async fn park_node_blocker(rt: &Arc<CompanyRuntime>, input: Value) -> String {
    park_node_blocker_stashed(rt, input, true, None).await
}

async fn park_node_blocker_stashed(
    rt: &Arc<CompanyRuntime>,
    input: Value,
    stash: bool,
    thread_id: Option<&str>,
) -> String {
    let payload = BlockerPayload {
        kind: BlockerKind::Infrastructure,
        source: BlockerSource::Provider,
        step: Some(BlockerStep::Node {
            run_id: RUN_ID.to_string(),
            node_id: NODE_ID.to_string(),
        }),
        reason: "the model id `gpt-nope` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    let effect = Effect {
        kind: payload.effect_kind(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::to_value(&payload).expect("payload"),
        agent: None,
        run_id: Some(RUN_ID.to_string()),
    };
    let id = rt
        .approvals
        .park(rt.id(), effect.clone())
        .await
        .expect("parks");
    rt.journal()
        .record_parked(
            &id,
            &effect,
            crate::ports::now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .expect("journals");
    if stash {
        let turn = workflow_node_turn_key(RUN_ID, NODE_ID);
        rt.blocked_nodes.arm_checkpointed(
            &turn,
            "reporting",
            &input,
            &crate::ports::types::StartedBy::from_scheduled(false),
            thread_id,
            None,
        );
        rt.journal()
            .record_blocked_node_stashed_checkpointed(
                &turn,
                "reporting",
                &input,
                &crate::ports::types::StartedBy::from_scheduled(false),
                thread_id,
                None,
            )
            .await
            .expect("stashes");
    }
    id.to_string()
}

async fn answer(rt: &Arc<CompanyRuntime>, id: &str, intent: BlockerReplyIntent, text: &str) {
    let ids = vec![crate::ports::types::ApprovalId::from(id.to_string())];
    rt.apply_blocker_reply(&ids, intent, text, None)
        .await
        .expect("applies");
}

/// The reserved key is never written for a verdict that starts no run.
#[tokio::test]
async fn a_cancelled_answer_never_reaches_a_trigger_input() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_node_blocker(&rt, json!({ "topic": "quarterly numbers" })).await;

    answer(&rt, &id, BlockerReplyIntent::Cancel, "cancel that").await;

    assert!(
        runner
            .started()
            .iter()
            .all(|run| run.input.get(CONTINUATION_BLOCKER_KEY).is_none()),
        "a cancel writes no answer onto any trigger input"
    );
}

/// A cancel must retire the per-node stash it parked with — in memory
/// and durably — and prune the checkpoint lineage that stash names,
/// the same as every other terminal outcome on this queue.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_cancelled_answer_retires_the_stash_and_prunes_its_checkpoint() {
    use tinyflows::graph::Checkpointer;

    let home = seed_home();
    let mut rt = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_id(CompanyId::new("acme"))
        .with_seed_dir(home.path().to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let checkpoints = std::sync::Arc::new(
        crate::workflows::checkpoint_store::WorkflowCheckpointStore::new(
            home.path().join("checkpoints"),
        ),
    );
    checkpoints
        .put(tinyflows::graph::Checkpoint {
            thread_id: RUN_ID.to_string(),
            checkpoint_id: "c1".to_string(),
            run_id: Some(RUN_ID.to_string()),
            parent_checkpoint_id: None,
            namespace: Vec::new(),
            state: json!({}),
            next_nodes: vec![tinyflows::graph::ids::NodeId::new(NODE_ID)],
            completed_tasks: Vec::new(),
            pending_writes: Vec::new(),
            interrupts: Vec::new(),
            pending_activations: None,
            barrier_arrivals: Vec::new(),
            metadata: Value::Null,
        })
        .await
        .expect("seed checkpoint");
    rt.set_workflow_checkpoints(checkpoints.clone());
    let rt = Arc::new(rt);

    let payload = BlockerPayload {
        kind: BlockerKind::Infrastructure,
        source: BlockerSource::Provider,
        step: Some(BlockerStep::Node {
            run_id: RUN_ID.to_string(),
            node_id: NODE_ID.to_string(),
        }),
        reason: "the model id `gpt-nope` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    let effect = Effect {
        kind: payload.effect_kind(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::to_value(&payload).expect("payload"),
        agent: None,
        run_id: Some(RUN_ID.to_string()),
    };
    let id = rt
        .approvals
        .park(rt.id(), effect.clone())
        .await
        .expect("parks");
    rt.journal()
        .record_parked(
            &id,
            &effect,
            crate::ports::now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .expect("journals");
    let turn = workflow_node_turn_key(RUN_ID, NODE_ID);
    let input = json!({ "topic": "quarterly numbers" });
    rt.blocked_nodes.arm_checkpointed(
        &turn,
        "reporting",
        &input,
        &crate::ports::types::StartedBy::from_scheduled(false),
        Some(RUN_ID),
        None,
    );
    rt.journal()
        .record_blocked_node_stashed_checkpointed(
            &turn,
            "reporting",
            &input,
            &crate::ports::types::StartedBy::from_scheduled(false),
            Some(RUN_ID),
            None,
        )
        .await
        .expect("stashes");

    rt.apply_blocker_reply(&[id], BlockerReplyIntent::Cancel, "cancel that", None)
        .await
        .expect("applies");

    assert!(
        !rt.blocked_nodes.is_armed(&turn),
        "a cancel must retire the stash it parked with, not leave it stranded until \
         restart"
    );
    assert!(
        rt.journal()
            .blocked_stashes()
            .iter()
            .all(|(recorded_turn, ..)| recorded_turn != &turn),
        "the durable stash mirror must be retired too"
    );
    let remaining = checkpoints
        .get_thread(RUN_ID)
        .await
        .expect("checkpoint read");
    assert!(
        remaining.is_empty(),
        "a cancel must prune the checkpoint lineage its stash named, the same as every \
         other terminal outcome: {remaining:?}"
    );
}

/// The restart reconciler's own retire path for an unapproved stranded
/// stash (a cancel that crashed before its own cleanup ran, or any
/// other resolved-with-nothing-approved shape) must prune that stash's
/// checkpoint lineage too, not only release the stash.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn reconcile_stranded_blocked_nodes_prunes_checkpoint_lineage_for_an_unapproved_stash() {
    use tinyflows::graph::Checkpointer;

    let home = seed_home();
    let mut rt = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_id(CompanyId::new("acme"))
        .with_seed_dir(home.path().to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let checkpoints = std::sync::Arc::new(
        crate::workflows::checkpoint_store::WorkflowCheckpointStore::new(
            home.path().join("checkpoints"),
        ),
    );
    checkpoints
        .put(tinyflows::graph::Checkpoint {
            thread_id: RUN_ID.to_string(),
            checkpoint_id: "c1".to_string(),
            run_id: Some(RUN_ID.to_string()),
            parent_checkpoint_id: None,
            namespace: Vec::new(),
            state: json!({}),
            next_nodes: vec![tinyflows::graph::ids::NodeId::new(NODE_ID)],
            completed_tasks: Vec::new(),
            pending_writes: Vec::new(),
            interrupts: Vec::new(),
            pending_activations: None,
            barrier_arrivals: Vec::new(),
            metadata: Value::Null,
        })
        .await
        .expect("seed checkpoint");
    rt.set_workflow_checkpoints(checkpoints.clone());

    // Stashed but never parked (or already resolved with nothing left
    // in the journal's live set) — the same "stranded, unapproved"
    // shape a crash mid-cleanup leaves behind.
    let turn = workflow_node_turn_key(RUN_ID, NODE_ID);
    rt.blocked_nodes.arm_checkpointed(
        &turn,
        "reporting",
        &json!({ "topic": "quarterly numbers" }),
        &crate::ports::types::StartedBy::from_scheduled(false),
        Some(RUN_ID),
        None,
    );

    rt.reconcile_stranded_blocked_nodes().await;

    assert!(
        !rt.blocked_nodes.is_armed(&turn),
        "an unapproved stranded stash must be retired"
    );
    let remaining = checkpoints
        .get_thread(RUN_ID)
        .await
        .expect("checkpoint read");
    assert!(
        remaining.is_empty(),
        "the reconciler must prune the checkpoint lineage an unapproved stranded stash \
         names, not only release the stash: {remaining:?}"
    );
}

/// Codex review finding on PR #2140 (`3951723403`): a stash stranded by
/// [`CompanyRuntime::reconcile_stranded_blocked_nodes`]'s own
/// emergency-stop guard while the company was stopped previously stayed
/// armed until the next full restart — that function's own doc says
/// "this runs again on the boot after the release", true only because
/// nothing ran it any sooner. `emergency_resume` now runs it itself, so
/// releasing the stop catches this up on the still-live process instead
/// of requiring an operator to restart the host.
#[tokio::test]
async fn emergency_resume_reconciles_a_stranded_stash_without_a_restart() {
    let home = seed_home();
    let rt = Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
            .with_id(CompanyId::new("acme"))
            .with_seed_dir(home.path().to_path_buf())
            .build()
            .await
            .expect("runtime builds"),
    );

    let operator = crate::ports::types::Actor {
        kind: crate::ports::types::ActorKind::Operator,
        id: "owner".into(),
    };
    rt.emergency_pause(operator.clone(), None)
        .await
        .expect("pause");

    // Stashed but never approved — the same "stranded, unapproved" shape
    // a crash mid-cleanup leaves behind, here left behind by the stop
    // instead of a restart.
    let turn = workflow_node_turn_key(RUN_ID, NODE_ID);
    rt.blocked_nodes.arm_checkpointed(
        &turn,
        "reporting",
        &json!({ "topic": "quarterly numbers" }),
        &crate::ports::types::StartedBy::from_scheduled(false),
        Some(RUN_ID),
        None,
    );
    assert!(
        rt.blocked_nodes.is_armed(&turn),
        "the stash exists while the company is stopped"
    );

    rt.emergency_resume(operator, None).await.expect("resume");

    assert!(
        !rt.blocked_nodes.is_armed(&turn),
        "releasing the stop must reconcile the stranded stash immediately, without \
         waiting for a restart"
    );
}
