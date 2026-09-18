use super::tests_recording::{graph, node};
use super::*;
use crate::workflows::caps::resolver::ChildGateRecord;

// ---- Issue #617: child-gate repeat warnings ---------------------------

/// A parent graph running one child from a node named `sub`.
fn child_parent_graph(child_id: &str) -> WorkflowGraph {
    graph(vec![node(
        "sub",
        NodeKind::SubWorkflow,
        json!({ "workflow_id": child_id }),
    )])
}

/// A child graph with an ungated POST then two sequential gated POSTs —
/// the shape the repeat warning exists for.
fn two_gate_child_graph() -> WorkflowGraph {
    let mut g = graph(vec![
        node(
            "notify",
            NodeKind::HttpRequest,
            json!({ "method": "POST", "url": "https://api.test/notify" }),
        ),
        node(
            "work",
            NodeKind::HttpRequest,
            json!({
                "method": "POST",
                "url": "https://api.test/work",
                "requires_approval": true,
            }),
        ),
        node(
            "work2",
            NodeKind::HttpRequest,
            json!({
                "method": "POST",
                "url": "https://api.test/work2",
                "requires_approval": true,
            }),
        ),
        node("done", NodeKind::Transform, json!({})),
    ]);
    g.edges = vec![
        tinyflows::model::Edge {
            from_node: "notify".into(),
            from_port: "main".into(),
            to_node: "work".into(),
            to_port: "main".into(),
        },
        tinyflows::model::Edge {
            from_node: "work".into(),
            from_port: "main".into(),
            to_node: "work2".into(),
            to_port: "main".into(),
        },
        tinyflows::model::Edge {
            from_node: "work2".into(),
            from_port: "main".into(),
            to_node: "done".into(),
            to_port: "main".into(),
        },
    ];
    g
}

/// A registry holding `child`'s gated record.
fn child_registry(graph: WorkflowGraph) -> ChildGateRegistry {
    let registry = ChildGateRegistry::default();
    registry.record(
        "child",
        ChildGateRecord {
            graph,
            gated: Vec::new(),
        },
    );
    registry
}

/// Issue #617. Two sequential gated nodes in one child: approving the first
/// makes it execute on the continuation (the engine skips the interrupt
/// only when the id is listed), the child then pauses at the second, and
/// approving that re-runs the child with BOTH approvals — so the first
/// gate's call fires on every hop. A `requires_approval` exclusion that
/// treats every such node as still-blocked would omit exactly the call the
/// operator must be warned about.
#[test]
fn an_approved_child_gate_that_will_fire_again_is_reported() {
    let parent = child_parent_graph("child");
    let pending = vec!["sub::work2".to_string()];
    let input = json!({ "approvals": ["sub::work"] });

    let warned = child_calls_to_repeat(
        &parent,
        &pending,
        &child_registry(two_gate_child_graph()),
        &input,
    );

    let ids: Vec<&str> = warned.iter().map(|w| w.node_id.as_str()).collect();
    assert_eq!(ids, vec!["notify", "work"], "{warned:?}");
    assert!(
        !warned.iter().any(|w| w.node_id == "work2"),
        "the gate this run paused on is not yet approved, so it is excluded: {warned:?}"
    );
}

/// The negative control: a not-yet-approved gate stays excluded. On the run
/// that parks it the child restarts and pauses at it again — it does not
/// execute — so reporting it would be a warning for a call that will not
/// happen.
#[test]
fn a_gate_this_run_has_not_cleared_stays_excluded() {
    let parent = child_parent_graph("child");
    let pending = vec!["sub::work".to_string()];

    let warned = child_calls_to_repeat(
        &parent,
        &pending,
        &child_registry(two_gate_child_graph()),
        &json!({}),
    );

    let ids: Vec<&str> = warned.iter().map(|w| w.node_id.as_str()).collect();
    assert_eq!(ids, vec!["notify"], "{warned:?}");
}

/// Issue #617, nested ancestor calls are included too. A call in the
/// intermediate child has already happened before that child enters its
/// nested workflow, so approving the grandchild gate restarts and repeats
/// the intermediate call as well.
#[test]
fn a_nested_gate_warns_for_calls_in_ancestor_children() {
    let mut ancestor = graph(vec![
        node(
            "notify_parent_child",
            NodeKind::HttpRequest,
            json!({ "method": "POST", "url": "https://api.test/ancestor" }),
        ),
        node(
            "nested",
            NodeKind::SubWorkflow,
            json!({ "workflow_id": "b" }),
        ),
    ]);
    ancestor.edges = vec![tinyflows::model::Edge {
        from_node: "notify_parent_child".into(),
        from_port: "main".into(),
        to_node: "nested".into(),
        to_port: "main".into(),
    }];

    let registry = ChildGateRegistry::default();
    registry.record(
        "a",
        ChildGateRecord {
            graph: ancestor,
            gated: Vec::new(),
        },
    );
    registry.record(
        "b",
        ChildGateRecord {
            graph: two_gate_child_graph(),
            gated: Vec::new(),
        },
    );

    let warned = child_calls_to_repeat(
        &child_parent_graph("a"),
        &["sub::nested::work2".to_string()],
        &registry,
        &json!({ "approvals": ["sub::nested::work"] }),
    );

    let ids: Vec<&str> = warned.iter().map(|w| w.node_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["sub::notify_parent_child", "notify", "work"],
        "{warned:?}"
    );
}

/// `sub::nested::work`; the child whose graph holds `work` is the
/// grandchild, reachable only by descending the registry through the
/// intermediate `sub_workflow` node's `workflow_id`. The approved first
/// gate then reads `sub::nested::work` off the same namespace.
#[test]
fn a_two_level_child_namespace_warns_for_the_grandchilds_upstream() {
    let registry = ChildGateRegistry::default();
    registry.record(
        "a",
        ChildGateRecord {
            graph: graph(vec![node(
                "nested",
                NodeKind::SubWorkflow,
                json!({ "workflow_id": "b" }),
            )]),
            gated: Vec::new(),
        },
    );
    registry.record(
        "b",
        ChildGateRecord {
            graph: two_gate_child_graph(),
            gated: Vec::new(),
        },
    );
    let parent = child_parent_graph("a");
    let pending = vec!["sub::nested::work2".to_string()];
    let input = json!({ "approvals": ["sub::nested::work"] });

    let warned = child_calls_to_repeat(&parent, &pending, &registry, &input);

    let ids: Vec<&str> = warned.iter().map(|w| w.node_id.as_str()).collect();
    assert_eq!(ids, vec!["notify", "work"], "{warned:?}");
}

/// Issue #617, the dynamic half. A `workflow_id = "=item.target"` child is
/// keyed in the registry by the RESOLVED id, so the repeat walk must
/// resolve the same expression against the trigger input — the same way
/// [`child_gate_call`](crate::workflows::caps::resolver::child_gate_call)
/// does for the card — or the warning is silently dropped for a dynamic
/// child.
#[test]
fn an_expr_bound_child_gate_warns_for_the_resolved_children_calls() {
    let parent = graph(vec![node(
        "sub",
        NodeKind::SubWorkflow,
        json!({ "workflow_id": "=item.target" }),
    )]);
    let pending = vec!["sub::work".to_string()];
    let input = json!({ "target": "child" });

    let warned = child_calls_to_repeat(
        &parent,
        &pending,
        &child_registry(two_gate_child_graph()),
        &input,
    );

    let ids: Vec<&str> = warned.iter().map(|w| w.node_id.as_str()).collect();
    assert_eq!(ids, vec!["notify"], "{warned:?}");
}

/// A per-item expression-bound child cannot be reconstructed — the paused
/// id carries no item index to say which element's scope resolved the id —
/// so the walk falls back rather than describing the wrong child.
#[test]
fn a_per_item_expr_bound_child_id_falls_back_to_nothing() {
    let parent = graph(vec![node(
        "sub",
        NodeKind::SubWorkflow,
        json!({ "workflow_id": "=item.target", "execution": "per_item" }),
    )]);
    let pending = vec!["sub::work".to_string()];

    let warned = child_calls_to_repeat(
        &parent,
        &pending,
        &child_registry(two_gate_child_graph()),
        &json!({ "target": "child" }),
    );

    assert!(
        warned.is_empty(),
        "falls back rather than guessing: {warned:?}"
    );
}
