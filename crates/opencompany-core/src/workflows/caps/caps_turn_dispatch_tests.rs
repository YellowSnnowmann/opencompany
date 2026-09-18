use super::*;

/// The single-harness turn over a fresh pool, as the non-lane entrypoint
/// wraps — what a workflow agent node runs on when no lanes are declared.
pub(super) fn single_turn(deps: &HarnessDeps) -> Arc<dyn RunTurn> {
    Arc::new(crate::harness::built_in::run_turn::HarnessRunTurn::new(
        Arc::new(crate::harness::HarnessPool::new()),
        Arc::new(deps.clone()),
    ))
}

/// A [`RunTurn`] that records the workflow-route ids each
/// `run_background_workflow` call receives, standing in for the harness pool
/// so the #1702 dispatch test can assert the run and node ids actually reach
/// the turn rather than being silently dropped by a fallback to the
/// un-streamed `run_background`.
pub(super) struct RecordingWorkflowTurn {
    /// `(agent_ref, workflow_run_id, node_id)` per call, in order.
    calls: std::sync::Mutex<Vec<(String, String, String)>>,
}

impl RecordingWorkflowTurn {
    pub(super) fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
}

/// The shape every recorded turn answers with — the dispatch under test only
/// cares about the ids it is handed, not what the (absent) agent did.
pub(super) fn ok_outcome() -> crate::harness::TurnOutcome {
    crate::harness::TurnOutcome {
        reply: "ok".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }
}

#[async_trait]
impl RunTurn for RecordingWorkflowTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        Ok(ok_outcome())
    }

    async fn run_steered(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        Ok(ok_outcome())
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
        Ok(ok_outcome())
    }

    async fn run_background_workflow(
        &self,
        _company: &CompanyId,
        agent_id: &str,
        _message: &str,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
        workflow_run_id: &str,
        node_id: &str,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        self.calls.lock().expect("calls").push((
            agent_id.to_string(),
            workflow_run_id.to_string(),
            node_id.to_string(),
        ));
        Ok(ok_outcome())
    }
}

/// Issue #1702: the workflow agent-node dispatch routes through
/// `run_background_workflow`, not the un-streamed `run_background`, so the
/// node's live tool frames stream tagged with the run and node ids. This
/// pins the forward: a regression that swapped the arguments or fell back
/// to `run_background` would leave the node functional but its live
/// activity silently gone.
#[tokio::test]
async fn an_agent_node_dispatches_through_run_background_workflow_with_run_and_node_ids() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1702-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(RecordingWorkflowTurn::new());
    let board_claim = Arc::new(deps.delegations.claim_board("run-1702"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1702"));
    let runner = HarnessAgentRunner::new(
        turn.clone(),
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1".to_string(),
        "run-1702".to_string(),
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

    // A node resolved from the graph: `node_id` present, so the resolved
    // `lineage_node` is that id, and the turn must receive the runner's OWN
    // run id.
    let (_, outcome) = runner
        .run_turn(
            "researcher",
            json!({ "node_id": "gather", "prompt": "collect the numbers" }),
        )
        .await
        .expect("agent node turn");
    assert_eq!(outcome.reply, "ok");
    assert_eq!(
        turn.calls.lock().expect("calls").as_slice(),
        &[(
            "researcher".to_string(),
            "run-1702".to_string(),
            "gather".to_string(),
        )],
        "the node's live frames must be tagged with the runner's run id and the resolved node id"
    );

    // A node with no graph id (a hand-built request, or a graph compiled
    // before #881) resolves lineage to the agent ref — and the ids still
    // route through, tagged with that fallback.
    runner
        .run_turn("researcher", json!({ "prompt": "no node id" }))
        .await
        .expect("agent node turn without a node id");
    assert_eq!(
        turn.calls.lock().expect("calls").as_slice(),
        &[
            (
                "researcher".to_string(),
                "run-1702".to_string(),
                "gather".to_string(),
            ),
            (
                "researcher".to_string(),
                "run-1702".to_string(),
                "researcher".to_string(),
            ),
        ],
        "a node with no graph id resolves lineage to the agent ref"
    );
}

/// A turn double that answers every call by reporting it truncated at the
/// iteration cap (issue #1865) — the one signal `reclassify_capped_nodes`
/// keys off, so a fake this narrow is enough to drive the arm under test
/// without a scripted model.
pub(super) struct CappedWorkflowTurn;

#[async_trait]
impl RunTurn for CappedWorkflowTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        unreachable!("workflow agent nodes route through run_background_workflow")
    }

    async fn run_steered(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        unreachable!("workflow agent nodes route through run_background_workflow")
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
        unreachable!("workflow agent nodes route through run_background_workflow")
    }

    async fn run_background_workflow(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
        _workflow_run_id: &str,
        _node_id: &str,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        Ok(crate::harness::TurnOutcome {
            reply: "partial answer, still going".to_string(),
            steps: Vec::new(),
            hit_iteration_cap: true,
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        })
    }
}

/// Issue #1865: the two halves of the disagreement the issue reports —
/// closed at their source. `run_turn` settles the attempt row `Failed` for
/// a capped turn (issue #926); this pins that the SAME turn also feeds
/// `RunCappedNodes`, the one channel `reclassify_capped_nodes` reads to
/// bring the run-level node row into agreement.
///
/// Not an end-to-end `run_workflow` proof (that would need the scripted
/// HTTP model `iteration_cap_turn_test` documents as the only way to
/// genuinely spend `max_tool_iterations`) — this pins the host-side HALF
/// of the mechanism this module owns: given the engine already told the
/// host "this turn was capped", both the attempt row and the sideways
/// channel agree about it. `runner::reclassify_capped_nodes`'s own test
/// pins the other half — that the channel's contents actually flip a
/// node's row from `Ok` to `Error`.
#[tokio::test]
async fn a_capped_turn_settles_failed_and_feeds_run_capped_nodes() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1865-capped-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(CappedWorkflowTurn);
    let board_claim = Arc::new(deps.delegations.claim_board("run-1865"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1865"));
    let capped = RunCappedNodes::default();
    let runs: Arc<dyn crate::ports::RunStore> =
        Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1865".to_string(),
        "run-1865".to_string(),
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
            json!({ "node_id": "loop_step", "prompt": "keep going" }),
        )
        .await
        .expect("a capped turn is still Ok — the reply is a real, partial checkpoint");
    assert!(outcome.hit_iteration_cap);

    // Half 1: the sideways channel `reclassify_capped_nodes` reads.
    assert_eq!(
        capped.take(),
        vec!["loop_step".to_string()],
        "the capped node's id must reach the channel the runner reconciles against"
    );

    // Half 2: the attempt row this run's Observatory/task-detail surfaces
    // read — issue #926's pre-existing settle, pinned here so a future
    // change cannot decouple it from the #1865 signal above without a
    // test noticing.
    let attempts = runs
        .list_runs(
            &CompanyId::new("acme"),
            &crate::ports::RunFilter::for_workflow_run("run-1865".to_string()),
        )
        .await
        .expect("list attempts");
    assert_eq!(attempts.len(), 1, "one attempt for one node turn");
    assert_eq!(attempts[0].status, crate::ports::RunStatus::Failed);
    assert_eq!(
        attempts[0].error.as_deref(),
        Some("agent stopped at the max_tool_iterations cap before finishing")
    );
}

/// A turn double that reports truncation at the iteration cap, the same
/// shape as [`CappedWorkflowTurn`], for a node that also declares `verify`.
struct CappedVerifiedWorkflowTurn;

#[async_trait]
impl RunTurn for CappedVerifiedWorkflowTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        unreachable!("workflow agent nodes route through run_background_workflow")
    }

    async fn run_steered(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        unreachable!("workflow agent nodes route through run_background_workflow")
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
        unreachable!("workflow agent nodes route through run_background_workflow")
    }

    async fn run_background_workflow(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
        _workflow_run_id: &str,
        _node_id: &str,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        Ok(crate::harness::TurnOutcome {
            reply: "partial answer, still going".to_string(),
            steps: Vec::new(),
            hit_iteration_cap: true,
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        })
    }
}

/// Codex review on #1990 (issue #1866): a node whose turn truncated at the
/// iteration cap is still handed to the semantic judge when `verify` is
/// declared — and the judge is told `execution_failed: false` regardless,
/// so a judge that answers `halt_benign` for the truncated partial reply
/// was never caught by `enforce_anti_suppression`'s blank/failed guard.
/// Before the fix, a capped turn could be recorded as an intentional
/// benign stop instead of the truncated failure it actually is.
#[tokio::test]
async fn a_capped_turn_with_verify_is_never_recorded_as_a_benign_halt() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-capped-verify-")
        .tempdir()
        .expect("tempdir");
    let base_url = crate::workflows::gated_tool_turn_tests::spawn_script(vec![
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"halt_benign\"}"),
    ])
    .await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(CappedVerifiedWorkflowTurn);
    let board_claim = Arc::new(deps.delegations.claim_board("run-1990v"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1990v"));
    let halted = RunHaltedNodes::default();
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1990v".to_string(),
        "run-1990v".to_string(),
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
    .with_halted(halted.clone());

    let result = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "loop_step",
                "prompt": "keep going",
                "verify": { "criteria": "must finish the report" }
            }),
        )
        .await;
    assert!(
        result.is_err(),
        "a truncated turn must never be accepted as semantically sufficient"
    );
    assert!(
        halted.take().is_empty(),
        "a capped/truncated turn must never be recorded as an intentional benign halt, \
         regardless of what the judge answers"
    );
}

/// Codex review on #1990 (issue #1866): `RunHaltedNodes` is shared across
/// every attempt `HarnessAgentRunner` makes for a run, and tinyflows
/// re-runs a node's whole turn when `retry.max_attempts > 1`. This drives
/// the exact sequence: attempt 1's judge answers `halt_benign` (pushing the
/// node id), attempt 2 (the retry) succeeds outright. Without retracting
/// the stale entry, `reclassify_halted_nodes` would relabel attempt 2's
/// genuinely successful row `Declined` using a marker left over from the
/// attempt that failed.
#[tokio::test]
async fn a_later_successful_attempt_is_not_shadowed_by_an_earlier_benign_halt() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-retry-halt-")
        .tempdir()
        .expect("tempdir");
    let base_url = crate::workflows::gated_tool_turn_tests::spawn_script(vec![
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"halt_benign\"}"),
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"continue\"}"),
    ])
    .await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(RecordingWorkflowTurn::new());
    let board_claim = Arc::new(deps.delegations.claim_board("run-1990h"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1990h"));
    let halted = RunHaltedNodes::default();
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1990h".to_string(),
        "run-1990h".to_string(),
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
    .with_halted(halted.clone());
    let node = json!({
        "node_id": "flaky",
        "prompt": "go",
        "verify": { "criteria": "must finish" }
    });

    let first = runner.run_turn("researcher", node.clone()).await;
    assert!(
        first.is_err(),
        "attempt 1's halt_benign verdict must gate the node"
    );
    assert!(
        halted.contains("flaky"),
        "attempt 1 must record the benign halt while it is the node's only outcome"
    );

    let second = runner.run_turn("researcher", node).await;
    assert!(
        second.is_ok(),
        "attempt 2's continue verdict must let the node through"
    );
    assert!(
        !halted.contains("flaky"),
        "attempt 2 succeeded outright; attempt 1's stale benign-halt marker must not survive \
         to shadow it"
    );
}

/// Codex review on #1990 (issue #1866): the agent's turn is composed from
/// the node's static instruction AND the operator's run-specific request
/// (`compose_turn_message`, issue #154) — but the judge was handed only the
/// static instruction. A reusable node's `verify.criteria` can only be
/// checked against what was actually asked this run; passing the judge the
/// pre-compose instruction meant it evaluated a different, narrower prompt
/// than the one the agent answered.
#[tokio::test]
async fn the_judge_sees_the_operators_run_request_not_just_the_static_instruction() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-run-request-")
        .tempdir()
        .expect("tempdir");
    let (base_url, script) = crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"continue\"}"),
    ])
    .await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(RecordingWorkflowTurn::new());
    let board_claim = Arc::new(deps.delegations.claim_board("run-1990r"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1990r"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1990r".to_string(),
        "run-1990r".to_string(),
        Some("check tuesday's numbers".to_string()),
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

    runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "reusable",
                "prompt": "Summarize the report.",
                "verify": { "criteria": "must call out tuesday's numbers specifically" }
            }),
        )
        .await
        .expect("a `continue` verdict must not gate the node");

    let seen = script.seen.lock().expect("seen");
    assert_eq!(
        seen.len(),
        1,
        "only the judge calls the scripted model here"
    );
    let sent = seen[0].to_string();
    assert!(
        sent.contains("check tuesday's numbers"),
        "the judge's prompt must carry the operator's run-specific request, not just the \
         node's static instruction: {sent}"
    );
    assert!(
        sent.contains("Request for this run:"),
        "the judge's prompt must use the same composed shape the agent's own turn ran on: {sent}"
    );
}

/// A turn double that always answers with a fixed refusal reply — a node
/// whose agent could not complete the ask, the shape a `recover` verdict is
/// meant to rescue.
pub(super) struct RefusalWorkflowTurn;
