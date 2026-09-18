use super::tests_resolution::{overlay, overlay_resolver, parent_of, store_with};
use super::*;

// ---- Issue #617: gating child calls -----------------------------------

/// A child graph whose one working node is a `tool_call` the policy parks.
fn child_with_shell(id: &str) -> String {
    format!(
        r#"
id = "{id}"
name = "{id}"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "run"
kind = "tool_call"
name = "Run"
[node.config]
slug = "shell"
[node.config.args]
# An ACTING command. Since issue #875 `shell` is classified by what it was
# handed, so a read would be a call the policy does not park — and the
# gate this fixture exists to exercise only happens for one that would.
command = "rm -rf ."
[[edge]]
from = "start"
to = "run"
"#
    )
}

fn gated_resolver(overlays: Vec<OverlayWorkflow>, mode: &str) -> StoreWorkflowResolver {
    let policy: crate::company::Policy =
        toml::from_str(&format!("mode = \"{mode}\"\nalways_approve = []\n"))
            .expect("valid [policy]");
    StoreWorkflowResolver::new(
        None,
        store_with(overlays),
        CompanyId::new("acme"),
        "root".to_string(),
        Some(ChildPolicyGates {
            policy_hitl_enabled: true,
            policy,
            run_id: "run-1".to_string(),
            grants: crate::runtime::grants::GrantSet::default(),
            registry: Arc::new(ChildGateRegistry::default()),
        }),
    )
}

/// Issue #617. The child's `shell` call is one the policy parks at the top
/// level, so the resolver must mark it before tinyflows runs the child.
#[tokio::test]
async fn a_policy_gated_child_call_is_marked_before_the_engine_runs_it() {
    let resolver = gated_resolver(
        vec![overlay("child", child_with_shell("child"))],
        "supervised",
    );

    let graph = resolver.resolve("child").await.expect("child resolves");

    let run = graph
        .nodes
        .iter()
        .find(|n| n.id == "run")
        .expect("the child's node survives");
    assert!(
        run.config["requires_approval"].as_bool().unwrap_or(false),
        "the child call must be gated: {:?}",
        run.config
    );
}

/// A company whose policy does not park the call leaves the child runnable.
#[tokio::test]
async fn a_child_the_policy_would_not_park_is_not_marked() {
    let resolver = gated_resolver(vec![overlay("child", child_with_shell("child"))], "full");

    let graph = resolver.resolve("child").await.expect("child resolves");
    let run = graph
        .nodes
        .iter()
        .find(|n| n.id == "run")
        .expect("the child's node survives");

    assert!(
        run.config.get("requires_approval").is_none(),
        "an ungated child call remains runnable: {:?}",
        run.config
    );
}

/// A dry run executes nothing, so its resolver receives no gate context.
#[tokio::test]
async fn a_resolver_without_policy_gates_leaves_the_child_unmarked() {
    let resolver = overlay_resolver(vec![overlay("child", child_with_shell("child"))], "root");
    let graph = resolver.resolve("child").await.expect("child resolves");
    let run = graph
        .nodes
        .iter()
        .find(|n| n.id == "run")
        .expect("the child's node survives");
    assert!(run.config.get("requires_approval").is_none());
}

// ---- Issue #617: routing the child's gate pass back to the parent -------

/// A child graph whose one working node is a `file_write` — the grantable
/// call the standing-permission tests below exercise (`shell` is
/// `Standing::PerCall`, so no grant can ever admit it; a resolvable
/// `web_fetch` is `Reach::ExternalRead`, so it never parks in the first
/// place and a grant would have nothing to admit).
fn child_with_file_write(id: &str) -> String {
    format!(
        r#"
id = "{id}"
name = "{id}"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "write"
kind = "tool_call"
name = "Write"
[node.config]
slug = "file_write"
[node.config.args]
path = "notes/child.md"
content = "hello"
[[edge]]
from = "start"
to = "write"
"#
    )
}

/// A live standing permission for `file_write` held by one workflow.
fn file_write_grant(workflow: &str) -> crate::runtime::grants::GrantSet {
    let grants = crate::runtime::grants::GrantSet::default();
    grants.grant_standing(crate::runtime::grants::StandingGrant {
        id: crate::runtime::grants::GrantId::new("g-write"),
        agent: String::new(),
        workflow: Some(workflow.to_string()),
        tool: "file_write".to_string(),
        verdict: crate::ports::types::Verdict::Approve,
        granted_by: crate::ports::types::Actor {
            kind: crate::ports::types::ActorKind::User,
            id: "user-1".to_string(),
        },
        approval_id: crate::ports::types::ApprovalId::new("approval-1"),
        at_millis: 1_000,
        expires_at_millis: crate::ports::now_millis() + 60 * 60 * 1000,
        origin_thread: None,
        origin_parent: None,
        origin_task: None,
        scope: None,
    });
    grants
}

/// Like [`gated_resolver`], but hands back the registry too so a test can
/// assert what the resolver recorded, and accepts the live grant set.
fn gated_resolver_with_grants(
    overlays: Vec<OverlayWorkflow>,
    mode: &str,
    grants: crate::runtime::grants::GrantSet,
) -> (StoreWorkflowResolver, Arc<ChildGateRegistry>) {
    let policy: crate::company::Policy =
        toml::from_str(&format!("mode = \"{mode}\"\nalways_approve = []\n"))
            .expect("valid [policy]");
    let registry = Arc::new(ChildGateRegistry::default());
    let resolver = StoreWorkflowResolver::new(
        None,
        store_with(overlays),
        CompanyId::new("acme"),
        "root".to_string(),
        Some(ChildPolicyGates {
            policy_hitl_enabled: true,
            policy,
            run_id: "run-1".to_string(),
            grants,
            registry: registry.clone(),
        }),
    );
    (resolver, registry)
}

/// Issue #617. The child's policy check is bound to the run's **root**
/// workflow id — the id the parked card is minted under — so a permission
/// the operator granted the top-level workflow is honoured inside the
/// child. Bound to the child's own id instead, the grant would not match
/// and the child call would park again under a permission that should have
/// admitted it.
#[tokio::test]
async fn a_standing_grant_for_the_root_workflow_admits_a_child_call() {
    let (resolver, _) = gated_resolver_with_grants(
        vec![overlay("child", child_with_file_write("child"))],
        "supervised",
        file_write_grant("root"),
    );

    let graph = resolver.resolve("child").await.expect("child resolves");
    let write = graph
        .nodes
        .iter()
        .find(|n| n.id == "write")
        .expect("the child's node survives");
    assert!(
        write.config.get("requires_approval").is_none(),
        "a grant for the root workflow must admit the child's call: {:?}",
        write.config
    );
}

/// The other direction of the subject decision: a grant bound to the
/// child's own id does **not** admit it, because the child's checks run
/// under the root workflow. No card path mints a child-bound grant — cards
/// are minted with the root — so this pins the decision rather than a
/// reachable state, and keeps the two ids from being confused again.
#[tokio::test]
async fn a_grant_bound_to_the_child_id_does_not_admit_the_child_call() {
    let (resolver, _) = gated_resolver_with_grants(
        vec![overlay("child", child_with_file_write("child"))],
        "supervised",
        file_write_grant("child"),
    );

    let graph = resolver.resolve("child").await.expect("child resolves");
    let write = graph
        .nodes
        .iter()
        .find(|n| n.id == "write")
        .expect("the child's node survives");
    assert!(
        write.config["requires_approval"].as_bool().unwrap_or(false),
        "a grant bound to the child's own id must not admit it: {:?}",
        write.config
    );
}

/// Issue #617. The resolver records each gated child — the graph the engine
/// is actually running and the calls the policy raised on it — so the
/// parent's parking path can name a child pause after the run settles.
/// A namespaced pending id (`sub::work`) resolves through the parent graph
/// and the registry back to the child's own classification.
#[tokio::test]
async fn a_policy_gated_child_is_recorded_for_the_parents_parking_path() {
    let (resolver, registry) = gated_resolver_with_grants(
        vec![overlay("child", child_with_shell("child"))],
        "supervised",
        crate::runtime::grants::GrantSet::default(),
    );
    resolver.resolve("child").await.expect("child resolves");

    let record = registry
        .get("child")
        .expect("the resolver recorded the gated child");
    // The recorded graph is the one the engine runs — post-gate-pass.
    assert!(
        record.graph.nodes.iter().any(|n| n.id == "run"),
        "the recorded graph is the gated child graph"
    );
    // The gated list names the child's OWN node ids (un-namespaced), so the
    // parent can match them against the stripped namespace.
    let gate = record
        .gated
        .iter()
        .find(|g| g.node_id == "run")
        .expect("the shell call was gated");
    assert_eq!(gate.slug, "shell");
    assert!(
        gate.reason.contains("shell"),
        "the reason names the call: {}",
        gate.reason
    );

    // The test graph is the post-gate graph. Populate a registry record
    // manually so this unit test exercises the namespace lookup itself,
    // rather than depending on a second engine resolve to reach a nested
    // child.
    let registry = Arc::new(ChildGateRegistry::default());
    let child = crate::workflows::translate::translate(
        &crate::company::parse_workflow(&child_with_shell("child")).expect("child parses"),
    );
    let gated = child
        .nodes
        .iter()
        .find(|node| node.id == "run")
        .map(|node| crate::workflows::gate::GatedCall {
            node_id: node.id.clone(),
            slug: "shell".to_string(),
            reason: "shell requires approval".to_string(),
            args: node.config.get("args").cloned().unwrap_or(Value::Null),
            target: None,
        })
        .into_iter()
        .collect();
    registry.record(
        "child",
        ChildGateRecord {
            graph: child,
            gated,
        },
    );

    let parent = crate::workflows::translate::translate(
        &crate::company::parse_workflow(&parent_of("parent", "child")).expect("parent parses"),
    );
    let described = child_gate_call(&registry, &parent, "sub::run", None)
        .expect("a namespaced child gate resolves through the registry");
    assert_eq!(described.node_id, "run");
    assert_eq!(described.slug, "shell");
}

/// Issue #617, the nested half. A gate two levels down is reported as
/// `sub::nested::work`; the parent graph resolves only the first hop, so
/// the parking path must descend the registry — resolving each intermediate
/// `sub_workflow` node's `workflow_id` to the child that actually ran it —
/// to reach the grandchild's own classification.
#[tokio::test]
async fn a_two_level_child_gate_resolves_through_the_registry() {
    let (_resolver, _unused_registry) = gated_resolver_with_grants(
        vec![
            // `a` runs `b` from a node named `nested`.
            overlay("a", parent_of("a", "b")),
            overlay("b", child_with_shell("b")),
        ],
        "supervised",
        crate::runtime::grants::GrantSet::default(),
    );
    // `a`'s child record is the graph that contains `nested`; `b`'s record
    // is the graph that contains the gated `work` node. Populate both
    // explicitly so the lookup test mirrors the engine's call order.
    let registry = Arc::new(ChildGateRegistry::default());
    let a = crate::workflows::translate::translate(
        &crate::company::parse_workflow(
            r#"
id = "a"
name = "a"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "nested"
kind = "sub_workflow"
name = "Nested"
[node.config]
workflow_id = "b"
[[edge]]
from = "start"
to = "nested"
"#,
        )
        .expect("a parses"),
    );
    let b = crate::workflows::translate::translate(
        &crate::company::parse_workflow(&child_with_shell("b")).expect("b parses"),
    );
    let gated = b
        .nodes
        .iter()
        .find(|node| node.id == "run")
        .map(|node| crate::workflows::gate::GatedCall {
            node_id: node.id.clone(),
            slug: "shell".to_string(),
            reason: "shell requires approval".to_string(),
            args: node.config.get("args").cloned().unwrap_or(Value::Null),
            target: None,
        })
        .into_iter()
        .collect();
    registry.record(
        "a",
        ChildGateRecord {
            graph: a,
            gated: Vec::new(),
        },
    );
    registry.record("b", ChildGateRecord { graph: b, gated });
    let parent = crate::workflows::translate::translate(
        &crate::company::parse_workflow(&parent_of("parent", "a")).expect("parent parses"),
    );
    let described = child_gate_call(&registry, &parent, "sub::nested::run", None)
        .expect("a two-level namespaced child gate resolves through the registry");
    assert_eq!(described.node_id, "run");
    assert_eq!(described.slug, "shell");
}

/// Issue #617, the dynamic half. A `workflow_id = "=item.target"` child is
/// resolved by the engine at run time, so the registry is keyed by the
/// RESOLVED id (`child`), not the authored expression. The parking path
/// must resolve the same expression against the trigger input to find the
/// record and describe the gate.
#[tokio::test]
async fn an_expr_bound_child_gate_resolves_through_the_registry() {
    let registry = Arc::new(ChildGateRegistry::default());
    let child = crate::workflows::translate::translate(
        &crate::company::parse_workflow(&child_with_shell("child")).expect("child parses"),
    );
    let gated = child
        .nodes
        .iter()
        .find(|node| node.id == "run")
        .map(|node| crate::workflows::gate::GatedCall {
            node_id: node.id.clone(),
            slug: "shell".to_string(),
            reason: "shell requires approval".to_string(),
            args: node.config.get("args").cloned().unwrap_or(Value::Null),
            target: None,
        })
        .into_iter()
        .collect();
    registry.record(
        "child",
        ChildGateRecord {
            graph: child,
            gated,
        },
    );

    let parent = r#"
id = "parent"
name = "Parent"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "sub"
kind = "sub_workflow"
name = "Sub"
[node.config]
workflow_id = "=item.target"
[[edge]]
from = "start"
to = "sub"
"#;
    let file = crate::company::parse_workflow(parent).expect("parent parses");
    let parent = crate::workflows::translate::translate(&file);
    let described = child_gate_call(
        &registry,
        &parent,
        "sub::run",
        Some(&serde_json::json!({ "target": "child" })),
    )
    .expect("an expression-bound child gate resolves through the registry");
    assert_eq!(described.node_id, "run");
    assert_eq!(described.slug, "shell");
}
