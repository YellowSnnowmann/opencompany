use super::tests_recovery_evidence::EscalatingJudgeProvider;
use super::tests_turn_dispatch::{CappedWorkflowTurn, RecordingWorkflowTurn};
use super::*;

/// PR #1883 review (Codex #3874941288): the sibling of
/// `a_capped_turn_settles_failed_and_feeds_run_capped_nodes` for the OTHER
/// signal that settles this attempt row `Failed` — `outcome.budget_paused`.
/// Before this fix, only `hit_iteration_cap` fed `RunCappedNodes`, so
/// `reclassify_capped_nodes` never saw a budget-paused node's id and its
/// row stayed `Ok` even though the attempt was `Failed` — the exact
/// disagreement #1865 exists to close, just via the other cap.
#[tokio::test]
async fn a_budget_paused_turn_settles_failed_and_feeds_run_capped_nodes() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1883-budget-paused-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "paused — out of budget".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: Some(crate::harness::BudgetPause {
            agent: "researcher".to_string(),
            summary: "acme is out of inference credits".to_string(),
        }),
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1883"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1883"));
    let capped = RunCappedNodes::default();
    let runs: Arc<dyn crate::ports::RunStore> =
        Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1883".to_string(),
        "run-1883".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        capped.clone(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    )
    .with_runs(Some(runs.clone()), None, RunAttempts::default());

    let (_, outcome) = runner
        .run_turn(
            "researcher",
            json!({ "node_id": "spend_step", "prompt": "keep going" }),
        )
        .await
        .expect("a budget-paused turn is still Ok — the reply is a real, partial checkpoint");
    assert!(outcome.budget_paused.is_some());

    // Half 1: the sideways channel `reclassify_capped_nodes` reads. This
    // is the assertion that failed before the fix — `capped.take()` came
    // back empty because only `hit_iteration_cap` pushed to it.
    assert_eq!(
        capped.take(),
        vec!["spend_step".to_string()],
        "the budget-paused node's id must reach the channel the runner reconciles \
         against, the same as a capped node's"
    );

    // Half 2: the attempt row this run's Observatory/task-detail surfaces
    // read, pinned here so it cannot drift from the #1865 signal above.
    let attempts = runs
        .list_runs(
            &CompanyId::new("acme"),
            &crate::ports::RunFilter::for_workflow_run("run-1883".to_string()),
        )
        .await
        .expect("list attempts");
    assert_eq!(attempts.len(), 1, "one attempt for one node turn");
    assert_eq!(attempts[0].status, crate::ports::RunStatus::Failed);
    assert_eq!(
        attempts[0].error.as_deref(),
        Some("agent paused for lack of inference budget/credits: acme is out of inference credits")
    );
}

#[tokio::test]
async fn a_spend_halted_turn_skips_the_judge_and_settles_failed() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-spend-halted-")
        .tempdir()
        .expect("tempdir");
    let (mut deps, _journal) =
        crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let provider = Arc::new(EscalatingJudgeProvider::default());
    deps.provider = provider.clone();
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "here is what I found so far".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: Some(crate::harness::SpendHalt {
            agent: "researcher".to_string(),
            spent_usd: 5.25,
            cap_usd: 5.0,
        }),
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1990-spend"));
    let publish_refusal_claim = Arc::new(
        deps.pending_publishes
            .claim_refusals_for_run("run-1990-spend"),
    );
    let capped = RunCappedNodes::default();
    let runs: Arc<dyn crate::ports::RunStore> =
        Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1990-spend".to_string(),
        "run-1990-spend".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        capped.clone(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    )
    .with_runs(Some(runs.clone()), None, RunAttempts::default());

    let (_, outcome) = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "spend_step",
                "prompt": "keep going",
                "verify": { "criteria": "the report must be sent" },
            }),
        )
        .await
        .expect("a spend-halted turn is still Ok — the reply is a real, partial checkpoint");

    assert_eq!(
        provider.calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a spend-halted turn must not pay for a sufficiency judge"
    );
    assert!(outcome.halted_for_spend.is_some());

    assert_eq!(
        capped.take(),
        vec!["spend_step".to_string()],
        "the spend-halted node's id must reach the channel the runner reconciles against"
    );

    let attempts = runs
        .list_runs(
            &CompanyId::new("acme"),
            &crate::ports::RunFilter::for_workflow_run("run-1990-spend".to_string()),
        )
        .await
        .expect("list attempts");
    assert_eq!(attempts.len(), 1, "one attempt for one node turn");
    assert_eq!(attempts[0].status, crate::ports::RunStatus::Failed);
    assert!(
        attempts[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("spend cap partway through")),
        "the spend-halt diagnosis must survive to the attempt row, got {:?}",
        attempts[0].error
    );
}

/// A [`RunTurn`] that always answers with a scripted outcome — standing in
/// for an ACP-backed harness whose turn stopped abnormally, without
/// needing a real ACP subprocess to produce one.
pub(super) struct ScriptedTurn(pub(super) crate::harness::TurnOutcome);

#[async_trait]
impl RunTurn for ScriptedTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        Ok(self.0.clone())
    }

    async fn run_steered(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        Ok(self.0.clone())
    }

    async fn run_steered_background(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        Ok(self.0.clone())
    }
}

/// PR #1880 review: "Propagate abnormal ACP stops beyond step notes." The
/// gap was that `HarnessAgentRunner::run_turn` read only
/// `hit_iteration_cap`, which stays `false` on an ACP `refusal`,
/// `cancelled`, or unrecognized `stopReason` — so the node settled
/// `Succeeded` here and `run` (the `AgentRunner` impl below) reported
/// `StopReason::Finished`, indistinguishable from the agent having
/// actually answered.
///
/// Asserted on the **outcome**, not on whether a `Note` step exists —
/// `harness::acp::run_turn::fold` already put a note on the timeline
/// before this fix, and the finding was explicitly that the note alone
/// does not stop the workflow graph from advancing as if the turn
/// succeeded. This is that stronger claim: the node call itself must
/// fail.
#[tokio::test]
async fn an_abnormal_acp_stop_fails_the_workflow_node() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1880-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "I can't help with that.".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: Some("[stopped: the agent declined to continue]".to_string()),
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1880"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1880"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1".to_string(),
        "run-1880".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let result = runner
        .run_turn("responder", json!({ "prompt": "do the thing" }))
        .await;

    let err = result.expect_err(
        "a refused/cancelled/unrecognized ACP stop must fail the node, \
         not settle it Succeeded/Finished",
    );
    let message = err.to_string();
    assert!(
        message.contains("the agent declined to continue"),
        "the error must carry the abnormal-stop reason, not a generic failure: {message}"
    );
}

/// Issue #1866 (deterministic tier) — the RED-on-old proof. A capped
/// turn's partial reply already settles the attempt row `Failed` (issue
/// #1865, pinned above), but on the pre-#1866 `run_turn` it still returns
/// `Ok` and flows the truncated text downstream via `=items` — nothing
/// stops it. Declaring a `postcondition` this same output fails must
/// ALSO turn the return into `Err`, so nothing downstream ever binds it.
///
/// Reuses [`CappedWorkflowTurn`] — its `{ "text": "partial answer, still
/// going", "agent_ref": ... }` envelope has no `items` field, so
/// `field_present` on `items` is exactly the gap this node's truncated
/// output represents. On the code as it stood before this issue, this
/// assertion fails: `run_turn` returns `Ok` here (see the sibling test
/// above, which asserts `.expect(...)` on the identical outcome).
#[tokio::test]
async fn a_node_whose_postcondition_fails_halts_before_returning_ok() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1866-postcondition-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(CappedWorkflowTurn);
    let board_claim = Arc::new(deps.delegations.claim_board("run-1866"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1866"));
    let runs: Arc<dyn crate::ports::RunStore> =
        Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1866".to_string(),
        "run-1866".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    )
    .with_runs(Some(runs.clone()), None, RunAttempts::default());

    let result = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "loop_step",
                "prompt": "keep going",
                "postcondition": { "require": "field_present", "field": "items" }
            }),
        )
        .await;

    let err = result.expect_err(
        "a truncated reply that also fails its declared postcondition must halt — \
         this is the RED-on-old assertion: pre-#1866 code returns Ok here",
    );
    let EngineError::Capability(message) = err else {
        panic!("expected a capability error");
    };
    assert!(
        message.contains("items"),
        "the halting message should name what the output was missing: {message}"
    );

    // The ordinary failure bucket, not `WaitingApproval` — nobody has to
    // approve a bad output the way they approve a gated tool call.
    let attempts = runs
        .list_runs(
            &CompanyId::new("acme"),
            &crate::ports::RunFilter::for_workflow_run("run-1866".to_string()),
        )
        .await
        .expect("list attempts");
    assert_eq!(attempts.len(), 1, "one attempt for one node turn");
    assert_eq!(attempts[0].status, crate::ports::RunStatus::Failed);
}

/// Companion GREEN: a node with no `postcondition` declared is completely
/// unaffected — the exact back-compat contract every other first-class
/// field on this call site keeps (`on_error`, `retry`,
/// `requires_approval`). Reuses the ordinary `RecordingWorkflowTurn` /
/// `ok_outcome` fixture the #1702 dispatch test above already trusts.
#[tokio::test]
async fn a_node_with_no_postcondition_is_unaffected() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1866-no-postcondition-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(RecordingWorkflowTurn::new());
    let board_claim = Arc::new(deps.delegations.claim_board("run-1866b"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1866b"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1866b".to_string(),
        "run-1866b".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let (value, outcome) = runner
        .run_turn("researcher", json!({ "node_id": "plain", "prompt": "go" }))
        .await
        .expect("a node with no postcondition must not be gated at all");
    assert_eq!(outcome.reply, "ok");
    assert_eq!(value["text"], "ok");
}

/// Companion GREEN: an output that DOES satisfy its declared
/// postcondition returns `Ok` exactly as an ungated node would — the gate
/// only ever removes a path, never adds one for output that clears it.
#[tokio::test]
async fn a_satisfying_output_still_returns_ok() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1866-satisfying-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(RecordingWorkflowTurn::new());
    let board_claim = Arc::new(deps.delegations.claim_board("run-1866c"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1866c"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1866c".to_string(),
        "run-1866c".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    // `RecordingWorkflowTurn::ok_outcome` replies "ok" — non-empty, so
    // `non_empty` is satisfied and the turn proceeds exactly as if no
    // postcondition were declared at all.
    let (value, outcome) = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "plain",
                "prompt": "go",
                "postcondition": { "require": "non_empty" }
            }),
        )
        .await
        .expect("an output that satisfies its postcondition must not be halted");
    assert_eq!(outcome.reply, "ok");
    assert_eq!(value["text"], "ok");
}

/// Codex review on #1937 (issue #1866) — the RED-on-old proof for
/// `non_empty_list`. The postcondition envelope this call site built was
/// always `{ "text": <reply>, "agent_ref": <ref> }`: an object, never a
/// `Value::Array`, so a `require = "non_empty_list"` declaration with no
/// `field` could never be satisfied by ANY agent reply — including a
/// reply that is itself the literal JSON text of a non-empty list, which
/// is exactly what this test sends. On the code as it stood before this
/// fix, this assertion fails: `run_turn` returns `Err` here because the
/// envelope's `json` never carried the agent's parsed reply.
///
/// Updated for Codex #3893541856 (bare-array emission): the emitted
/// `value` is now the array itself, not an object with a `text` key — see
/// `a_bare_array_reply_replaces_the_emitted_value_wholesale` below for the
/// dedicated coverage of that shape. `outcome.reply` (a separate field,
/// untouched by any of this) still carries the raw string regardless.
#[tokio::test]
async fn a_reply_that_is_a_json_list_satisfies_non_empty_list_with_no_field() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1937-postcondition-list-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "[\"x\", \"y\"]".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1937"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1937"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1937".to_string(),
        "run-1937".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let (value, outcome) = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "lister",
                "prompt": "list two things",
                "postcondition": { "require": "non_empty_list" }
            }),
        )
        .await
        .expect(
            "a reply that IS the JSON text of a non-empty list must satisfy \
             `non_empty_list` with no `field` — this is the RED-on-old assertion: \
             pre-fix code always built a `{text, agent_ref}` envelope that could \
             never be seen as a `Value::Array`",
        );
    assert_eq!(outcome.reply, "[\"x\", \"y\"]");
    assert_eq!(value, json!(["x", "y"]));
}

/// Companion: a plain-prose reply (the common case — agent nodes are not
/// asked for structured output by default) still fails `non_empty_list`
/// honestly, rather than the fix silently passing everything through
/// once a `json` key exists on the envelope.
#[tokio::test]
async fn a_prose_reply_still_fails_non_empty_list_with_no_field() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1937-postcondition-prose-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "here is a summary, not a list".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1937b"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1937b"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1937b".to_string(),
        "run-1937b".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let result = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "lister",
                "prompt": "list two things",
                "postcondition": { "require": "non_empty_list" }
            }),
        )
        .await;

    let err = result.expect_err(
        "a plain-prose reply must still fail `non_empty_list` — the fix must not \
         silently pass every reply once the envelope carries a `json` key",
    );
    let EngineError::Capability(message) = err else {
        panic!("expected a capability error");
    };
    assert!(
        message.contains("not a list"),
        "the halting message should say the shape did not match: {message}"
    );
}
