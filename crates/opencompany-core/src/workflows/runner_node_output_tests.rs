use super::tests_capped_halt::{GREET, deps, record, tools_record};
use super::*;

use crate::company::parse_workflow;
use crate::ports::run_output::WorkflowRunOutputStore;
use crate::store::FsOps;

/// CodeRabbit review on #1937 (issue #1866) — a downstream binding of the
/// SAME value the postcondition gate certified.
///
/// `agent = "ceo"` replies with the literal JSON text `{"items":[1,2,3]}`.
/// `field_present`/`field = "json.items"` certifies it. `reflect`'s
/// `=item.json.items` binding is the "downstream" this issue is about:
/// it reads straight off `ceo`'s emitted item exactly the way
/// `translate.rs`'s own doc comment says a downstream node must be able
/// to ("a downstream node reads `=item.text` / `=item.json.<field>`").
/// Before the emitted-output fix, the gate passed while this bound to
/// `null` — the postcondition envelope's parsed value never reached the
/// node's own emitted `json`, only a transient local used for the check.
/// This is the real engine (full graph execution, real expression
/// resolution), not a unit-level inspection of the returned tuple.
const STRUCTURED_REPLY_WF: &str = r#"
id = "structured_wf"
name = "Structured WF"

[[node]]
id = "start"
kind = "trigger"
name = "Start"

[[node]]
id = "ceo"
kind = "agent"
name = "CEO"
agent = "ceo"

[node.postcondition]
require = "field_present"
field = "json.items"

[[node]]
id = "reflect"
kind = "transform"
name = "Reflect"

[node.config.set]
wrapped = "=item.json.items"

[[node]]
id = "done"
kind = "output"
name = "Done"

[[edge]]
from = "start"
to = "ceo"

[[edge]]
from = "ceo"
to = "reflect"

[[edge]]
from = "reflect"
to = "done"
"#;

/// A [`RunTurn`](crate::runtime::delegation::RunTurn) that always answers
/// with the literal JSON text of `{"items":[1,2,3]}`, for any agent —
/// the engine's own `run_background_workflow` default chain
/// (`run_background_workflow` -> `run_background` -> `run`) reaches
/// `run` below, so overriding just the three required methods is enough
/// to stand in for the full workflow-node dispatch path, not only the
/// direct chat one.
struct StructuredJsonReplyTurn;
#[async_trait::async_trait]
impl crate::runtime::delegation::RunTurn for StructuredJsonReplyTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
    ) -> Result<crate::harness::TurnOutcome> {
        Ok(crate::harness::TurnOutcome {
            reply: "{\"items\": [1, 2, 3]}".to_string(),
            steps: Vec::new(),
            hit_iteration_cap: false,
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        })
    }

    async fn run_steered(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.run(
            _company,
            _agent_id,
            _message,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
    }

    async fn run_steered_background(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.run(
            _company,
            _agent_id,
            _message,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
    }
}

#[tokio::test]
async fn a_structured_agent_reply_is_readable_by_a_downstream_json_binding() {
    let dir = tempfile::tempdir().unwrap();
    let file = parse_workflow(STRUCTURED_REPLY_WF).expect("workflow parses");

    let run = run_workflow_lane_aware(
        Arc::new(StructuredJsonReplyTurn),
        deps(dir.path()),
        &record(),
        &file,
        serde_json::json!({}),
        &WorkflowRunContext::new(false),
    )
    .await
    .expect("workflow runs");

    // The gate itself: `ceo`'s postcondition (field_present on json.items)
    // must have let the node succeed, not halted the run.
    assert!(
        !run.output["nodes"]["ceo"]["items"].is_null(),
        "the postcondition must have passed — ceo should have emitted: {}",
        run.output
    );

    // The actual finding: `reflect`'s `=item.json.items` binding — reading
    // `ceo`'s own emitted item downstream, the same way any real workflow
    // node would — must resolve to the SAME [1, 2, 3] the gate certified,
    // not null.
    let wrapped = &run.output["nodes"]["reflect"]["items"][0]["json"]["wrapped"];
    assert_eq!(
        wrapped,
        &serde_json::json!([1, 2, 3]),
        "a downstream `=item.json.items` binding must resolve to the same \
         structured value the postcondition gate certified, not null: {}",
        run.output
    );
}

/// A graph identical in shape to `STRUCTURED_REPLY_WF` above, but the
/// declared `field_present` targets the bare `json` root (not
/// `json.items`) and the scripted reply is a bare JSON scalar rather
/// than an object — see
/// `a_scalar_reply_cannot_satisfy_field_present_on_the_bare_json_root`
/// below for what this proves.
const SCALAR_REPLY_WF: &str = r#"
id = "scalar_wf"
name = "Scalar WF"

[[node]]
id = "start"
kind = "trigger"
name = "Start"

[[node]]
id = "ceo"
kind = "agent"
name = "CEO"
agent = "ceo"

[node.postcondition]
require = "field_present"
field = "json"

[[node]]
id = "reflect"
kind = "transform"
name = "Reflect"

[node.config.set]
wrapped = "=item.json"

[[node]]
id = "done"
kind = "output"
name = "Done"

[[edge]]
from = "start"
to = "ceo"

[[edge]]
from = "ceo"
to = "reflect"

[[edge]]
from = "reflect"
to = "done"
"#;

/// A [`RunTurn`] that always answers with the literal JSON text `"42"` —
/// a bare scalar, not an object or array.
struct ScalarJsonReplyTurn;

#[async_trait::async_trait]
impl crate::runtime::delegation::RunTurn for ScalarJsonReplyTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
    ) -> Result<crate::harness::TurnOutcome> {
        Ok(crate::harness::TurnOutcome {
            reply: "42".to_string(),
            steps: Vec::new(),
            hit_iteration_cap: false,
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        })
    }

    async fn run_steered(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.run(
            _company,
            _agent_id,
            _message,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
    }

    async fn run_steered_background(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.run(
            _company,
            _agent_id,
            _message,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
    }
}

/// Codex #3894162757 on #1937 — verified through the REAL engine, the
/// same technique that proved the original certify-vs-consume bug (see
/// `a_structured_agent_reply_is_readable_by_a_downstream_json_binding`
/// above). A prior round added an emission arm that replaced `value`
/// wholesale for a scalar reply too, mirroring the array case, and
/// asserted only `run_turn`'s OWN return value (`workflows::caps::tests_field_present`)
/// — which DID come back as the bare `42`. That test missed the actual
/// defect: tinyflows' own envelope construction
/// (`finish_agent_run`/`envelope::structured_of`, vendored) clamps
/// `AgentRunOutcome.json` to `Value::Null` for anything that is not an
/// `Object`/`Array` — "scalars carry no structure" is that crate's own
/// stated invariant. Run against the code as it stood after that round
/// (gate passes, `value` = `42`), this exact graph produced:
/// `ceo.items[0].json = {"json": null, "text": "42", "raw": 42, "meta":
/// {...}}` and `reflect.items[0].json.wrapped = null` — the gate had
/// certified `42`, and `=item.json` downstream got `null` anyway, one
/// layer further out than the original bug this PR started from.
///
/// The fix moves to the gate itself: `field_present` on the bare `json`
/// root now refuses to certify a scalar at all (see
/// `postcondition::evaluate_postcondition`'s `field_present` arm), so
/// this run must fail outright rather than silently passing a value
/// nothing downstream can read.
#[tokio::test]
async fn a_scalar_reply_cannot_satisfy_field_present_on_the_bare_json_root() {
    let dir = tempfile::tempdir().unwrap();
    let file = parse_workflow(SCALAR_REPLY_WF).expect("workflow parses");

    let result = run_workflow_lane_aware(
        Arc::new(ScalarJsonReplyTurn),
        deps(dir.path()),
        &record(),
        &file,
        serde_json::json!({}),
        &WorkflowRunContext::new(false),
    )
    .await;

    let err = result.expect_err(
        "a bare scalar reply must not satisfy field_present on the bare `json` \
         root — the run must halt at `ceo` rather than let `reflect` (and \
         `done`) advance on a `wrapped` binding that resolves to null",
    );
    let message = err.to_string();
    assert!(
        message.contains("ceo")
            && message.contains("postcondition")
            && message.contains("bare scalar"),
        "the halting error should name the node and the reason: {message}"
    );
}

/// A GREET-shaped graph whose agent node carries a config `=`-expression
/// pointing at a trigger field that does not exist (issue #1014). The engine
/// resolves the expression to `null` and records a `NullResolution`, which
/// this test asserts reaches the run's per-node timeline.
const AGENT_NULL_BINDING: &str = r#"
id = "greet"
name = "Greet"

[[node]]
id = "start"
kind = "trigger"
name = "Start"

[[node]]
id = "ceo"
kind = "agent"
name = "CEO"
summary = "say hello-marker"
agent = "ceo"

[node.config]
recipient = "=item.missing_field"

[[node]]
id = "done"
kind = "output"
name = "Report back"

[[edge]]
from = "start"
to = "ceo"

[[edge]]
from = "ceo"
to = "done"
"#;

/// Issue #1014: a node whose config `=`-expression resolves to `null` yields
/// a [`WorkflowRunNodeRow`](crate::ports::WorkflowRunNodeRow) whose
/// `diagnostics` carries that config **path** — the engine's own broken-wiring
/// list, surfaced on the run response so an operator sees the unresolved
/// binding behind a bad step.
///
/// The discriminating half is *paths only*: `diagnostics` carries the config
/// location (`recipient`) and never the expression text (`=item.missing_field`)
/// nor any resolved value — the no-payload stance the rest of the row takes.
#[tokio::test]
async fn a_null_resolved_config_expression_surfaces_as_a_node_diagnostic_path() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(HarnessPool::new());
    let rec = record();
    let deps = deps(dir.path());
    pool.ensure(&rec, &deps).await.expect("roster builds");

    let file = parse_workflow(AGENT_NULL_BINDING).expect("workflow parses");
    let run = run_workflow(
        pool,
        deps,
        &rec,
        &file,
        // No `missing_field` on the trigger payload, so `=item.missing_field`
        // resolves to `null` and the engine records the miss.
        serde_json::json!({ "brief": "launch" }),
        &WorkflowRunContext::new(false),
    )
    .await
    .expect("workflow runs");

    let ceo = run
        .nodes
        .iter()
        .find(|n| n.node_id == "ceo")
        .expect("the agent node finished and produced a row");

    // The config path of the null-resolved binding rode all the way to the
    // run's per-node timeline.
    assert!(
        ceo.diagnostics.iter().any(|d| d.contains("recipient")),
        "expected the `recipient` config path in diagnostics, got {:?}",
        ceo.diagnostics
    );
    // Paths only: neither the expression text nor a resolved value leaks.
    assert!(
        !ceo.diagnostics.iter().any(|d| d.contains("=item")),
        "diagnostics must carry config paths, not expression text: {:?}",
        ceo.diagnostics
    );
}

// --- Durable per-node output persist-at-settle (issue #596) ---------------

/// A completed run persists its per-node output to the durable store, so a
/// later console read can show what each node produced. The agent node's
/// text is present in the stored snapshot.
#[tokio::test]
async fn a_completed_run_persists_its_per_node_output() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsOps::new(dir.path()));
    let pool = Arc::new(HarnessPool::new());
    let rec = record();
    let mut deps = deps(dir.path());
    deps.run_output_store = Some(store.clone());
    // The GREET graph has an agent node, so the roster must be resident and
    // the record loadable — exactly like `agent_node_runs_on_the_harness_pool`.
    pool.ensure(&rec, &deps).await.expect("roster builds");
    let file = parse_workflow(GREET).expect("parses");
    let ctx = WorkflowRunContext::new(false);
    let run_id = ctx.run_id.clone();

    run_workflow(
        pool,
        deps,
        &rec,
        &file,
        serde_json::json!({ "brief": "launch" }),
        &ctx,
    )
    .await
    .expect("workflow runs");

    let stored = store
        .get_run_output(&rec.id, &run_id)
        .await
        .expect("store read")
        .expect("a completed run must persist its output");
    assert_eq!(stored.workflow_id, "greet");
    assert_eq!(stored.run_id, run_id);
    assert!(
        stored.nodes.to_string().contains("hello-marker"),
        "the agent node's produced text must be in the durable snapshot: {}",
        stored.nodes
    );
}

/// A paused (`requires_approval`) run still settles with an outcome and so
/// persists the output of the nodes it reached before the gate.
#[tokio::test]
async fn a_paused_run_persists_the_output_it_reached() {
    let src = r#"
id = "gated"
name = "Gated"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "gate"
kind = "tool_call"
name = "Gate"
requires_approval = true
[node.config]
slug = "csv_export"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "gate"
[[edge]]
from = "gate"
to = "done"
"#;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FsOps::new(dir.path()));
    let mut deps = deps(dir.path());
    deps.run_output_store = Some(store.clone());
    let rec = tools_record();
    let file = parse_workflow(src).expect("parses");
    let ctx = WorkflowRunContext::new(false);
    let run_id = ctx.run_id.clone();

    let run = run_workflow(
        Arc::new(HarnessPool::new()),
        deps,
        &rec,
        &file,
        serde_json::json!({ "seed": 1 }),
        &ctx,
    )
    .await
    .expect("run pauses cleanly");
    assert!(run.pending_approvals.iter().any(|id| id == "gate"));

    assert!(
        store
            .get_run_output(&rec.id, &run_id)
            .await
            .unwrap()
            .is_some(),
        "a paused-with-pending-approvals run must persist its reached output"
    );
}

/// Issue #661 (M5): the **hard-abort** arm lists the board writes its nodes
/// already performed.
///
/// A wedged run's future is dropped, so there is no outcome to read and every
/// other field of `cancelled_run` is empty as a claim about the run's
/// *result*. Its board rows are not a result — they record a card that is
/// already on the operator's board by the time this is reached. Emptying them
/// would leave a card that no run admits to opening.
///
/// Unit-level rather than a wedged end-to-end run on purpose: reaching this
/// arm for real means outlasting `CANCEL_HARD_ABORT_GRACE`, and a five-second
/// sleep in the suite buys nothing this does not pin — the arm's whole
/// behaviour is what it threads through.
///
/// `notices` rides along for the same reason, and that half is a fix: this
/// constructor used to hard-code `Vec::new()` for it, so a wedged run silently
/// dropped notices its completed nodes had raised.
#[test]
fn a_hard_aborted_run_still_lists_its_board_writes() {
    let row = crate::ports::WorkflowRunBoardRow {
        action: crate::ports::WorkflowBoardAction::Spawned,
        task_id: Some("card-1".to_string()),
        title: Some("Reply to the auditor".to_string()),
        assignee: None,
    };
    // Issue #880: a parked approval is threaded in for the same reason the
    // board row is — the card is already on the operator's Approvals page,
    // so a hard abort must not un-say that the run opened it.
    let parked = crate::ports::WorkflowRunApprovalRow {
        node_id: Some("work".to_string()),
        tool: Some("publish_artifact".to_string()),
        outcome: crate::ports::WorkflowApprovalOutcome::Parked,
        approval_id: Some("appr-1".to_string()),
    };
    let run = cancelled_run(
        vec!["something was discarded".to_string()],
        vec![row.clone()],
        vec![parked.clone()],
    );

    assert_eq!(
        run.approvals,
        vec![parked],
        "a run stopped after parking an approval really did park it; zeroing the receipt \
         would leave a card no run admits to opening"
    );

    assert!(run.cancelled);
    assert_eq!(
        run.board,
        vec![row],
        "the card is durable, so the stopped run must still list it"
    );
    assert_eq!(
        run.notices,
        vec!["something was discarded".to_string()],
        "and the notices its nodes raised are not the run's result either"
    );
    // Everything that IS the run's result stays empty, unchanged.
    assert_eq!(run.output, Value::Null);
    assert!(run.deliveries.is_empty());
    assert!(run.pending_approvals.is_empty());
    assert!(run.nodes.is_empty());
}

/// Issue #900's regression: `Iterator::all` is vacuously `true` on an
/// empty iterator, so before this guard required at least one errored row,
/// an engine failure that named no node at all satisfied
/// `only_blocked_nodes_errored` by default and would have been
/// relabelled as a plain block — exactly the "hide a real error behind
/// waiting on approval" lie the function's own doc comment says it exists
/// to prevent.
#[test]
fn no_errored_nodes_never_counts_as_only_blocked_nodes_errored() {
    let blocked = vec![crate::ports::WorkflowBlockedNode {
        node_id: "work".to_string(),
        tools: vec!["shell".to_string()],
        approval_ids: vec!["appr-1".to_string()],
        unparkable: 0,
        stranded: 0,
        blockers: 0,
    }];
    // No node row reported `Error` at all — a setup/validation failure the
    // engine raised before any node ran, for instance.
    let nodes: Vec<crate::ports::WorkflowRunNodeRow> = Vec::new();
    assert!(
        !only_blocked_nodes_errored(&nodes, &blocked),
        "an engine error naming no errored node must never be waved through as \
         a plain block"
    );
}

/// The guard's positive case still holds: when every errored row is one the
/// host blocked, reclassification is safe.
#[test]
fn every_errored_node_blocked_counts_as_only_blocked_nodes_errored() {
    let blocked = vec![crate::ports::WorkflowBlockedNode {
        node_id: "work".to_string(),
        tools: vec!["shell".to_string()],
        approval_ids: vec!["appr-1".to_string()],
        unparkable: 0,
        stranded: 0,
        blockers: 0,
    }];
    let nodes = vec![crate::ports::WorkflowRunNodeRow {
        node_id: "work".to_string(),
        status: WorkflowNodeStatus::Error,
        elapsed_ms: 10,
        diagnostics: Vec::new(),
    }];
    assert!(only_blocked_nodes_errored(&nodes, &blocked));
}
