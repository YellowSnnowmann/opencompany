use super::tests_peer_record::{ConsultedPeerTurn, record_with_peer};
use super::*;

impl ConsultedPeerTurn {
    fn new(peer_reply: &'static str) -> Self {
        Self {
            peer_reply,
            consults: std::sync::atomic::AtomicUsize::new(0),
            node_turns: std::sync::atomic::AtomicUsize::new(0),
            stage: None,
        }
    }

    fn staging(peer_reply: &'static str, deps: HarnessDeps) -> Self {
        Self {
            stage: Some(deps),
            ..Self::new(peer_reply)
        }
    }

    fn consults(&self) -> usize {
        self.consults.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl RunTurn for ConsultedPeerTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        unreachable!("workflow agent nodes route through run_background_workflow")
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

    async fn run_background(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        self.consults
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(deps) = self.stage.as_ref() {
            deps.delegations
                .push(crate::harness::orchestrator::Delegation::SpawnTask {
                    title: "Chase the renewal paperwork".to_string(),
                    note: None,
                    assignee: None,
                });
            deps.pending_publishes
                .push_refusal("consultation-note.md".to_string());
            deps.approval_requests
                .push(crate::harness::policy::ApprovalRequest {
                    tool: "send_email".to_string(),
                    reason: "the consulted peer tried a gated tool".to_string(),
                    effect: crate::ports::types::Effect {
                        kind: "email.send".to_string(),
                        group: crate::ports::types::EffectGroup::Other,
                        amount_usd: None,
                        established_thread: false,
                        first_time_counterparty: false,
                        payload: json!({ "to": "someone@example.com" }),
                        agent: Some("cfo".to_string()),
                        run_id: None,
                    },
                });
        }
        Ok(crate::harness::TurnOutcome {
            reply: self.peer_reply.to_string(),
            steps: Vec::new(),
            hit_iteration_cap: false,
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        })
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
        self.node_turns
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(crate::harness::TurnOutcome {
            reply: "I cannot draft the email without the customer's renewal date.".to_string(),
            steps: Vec::new(),
            hit_iteration_cap: false,
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        })
    }
}

/// The verify node every peer-rung test below drives.
fn verify_node() -> Value {
    json!({
        "node_id": "draft",
        "prompt": "Draft the customer email.",
        "verify": { "criteria": "must include the customer's renewal date" }
    })
}

/// Builds a runner over `turn` with the peer-bearing roster, handing back
/// the run-scoped collectors a test asserts on.
#[allow(clippy::type_complexity)]
fn peer_runner(
    turn: Arc<dyn RunTurn>,
    deps: HarnessDeps,
    run_id: &str,
    runs: Option<Arc<dyn crate::ports::RunStore>>,
) -> (
    HarnessAgentRunner,
    RunBlocks,
    RunBoard,
    RunNotices,
    RunArtifacts,
) {
    let board_claim = Arc::new(deps.delegations.claim_board(run_id.to_string()));
    let publish_refusal_claim = Arc::new(
        deps.pending_publishes
            .claim_refusals_for_run(run_id.to_string()),
    );
    let blocks = RunBlocks::default();
    let board = RunBoard::default();
    let notices = RunNotices::default();
    let artifacts = RunArtifacts::default();
    let mut runner = HarnessAgentRunner::new(
        turn,
        deps,
        record_with_peer(),
        CompanyId::new("acme"),
        format!("wf-{run_id}"),
        run_id.to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        notices.clone(),
        board.clone(),
        blocks.clone(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        artifacts.clone(),
        board_claim,
        publish_refusal_claim,
    );
    if let Some(runs) = runs {
        runner = runner.with_runs(Some(runs), None, RunAttempts::default());
    }
    (runner, blocks, board, notices, artifacts)
}

/// The rung's reason to exist: an information gap the fact store and the
/// workspace cannot close is put to one roster peer, and an answer the
/// re-verification judge accepts ships as the node's output carrying its
/// provenance.
#[tokio::test]
async fn a_peer_answer_the_judge_accepts_ships_the_recovered_context_block() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1866-peer-accepted-")
        .tempdir()
        .expect("tempdir");
    let (base_url, script) = crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"continue\"}"),
    ])
    .await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let turn = Arc::new(ConsultedPeerTurn::new("The renewal date is March 1st."));
    let (runner, blocks, _board, _notices, _artifacts) =
        peer_runner(turn.clone(), deps, "run-1866-peer-ok", None);

    let (_value, outcome) = runner
        .run_turn("researcher", verify_node())
        .await
        .expect("an answered consultation the judge accepts must let the node ship");

    assert_eq!(turn.consults(), 1, "exactly one peer turn is spent");
    assert!(
        outcome
            .reply
            .contains("peer cfo (Chief Financial Officer): The renewal date is March 1st."),
        "the shipped reply must carry the peer's answer and its provenance: {}",
        outcome.reply
    );
    assert!(
        outcome.reply.contains("Recovered company context:"),
        "the shipped reply must be the augmented text: {}",
        outcome.reply
    );
    let seen = script.seen.lock().expect("seen");
    assert_eq!(seen.len(), 2, "one judge call, then one re-verification");
    assert!(
        seen[1].to_string().contains("peer cfo"),
        "the re-verification judge must see the peer's answer, not the bare refusal"
    );
    assert!(
        blocks.take().is_empty(),
        "an accepted recovery blocks nobody"
    );
}

/// A peer answer is not privileged: the second judge still gets to refuse
/// it, and when it does the operator gets an Information blocker whose
/// recovery log names the peer that was asked.
#[tokio::test]
async fn a_peer_answer_the_judge_rejects_parks_an_information_blocker() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1866-peer-rejected-")
        .tempdir()
        .expect("tempdir");
    let (base_url, _script) =
        crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"retry\"}"),
        ])
        .await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let runs: Arc<dyn crate::ports::RunStore> =
        Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let turn = Arc::new(ConsultedPeerTurn::new("I do not have that date either."));
    let (runner, blocks, _board, _notices, _artifacts) = peer_runner(
        turn.clone(),
        deps,
        "run-1866-peer-blocked",
        Some(runs.clone()),
    );

    let err = runner
        .run_turn("researcher", verify_node())
        .await
        .expect_err("an unclosed information gap must not advance downstream");
    let EngineError::Capability(message) = err else {
        panic!("expected a capability error");
    };
    assert!(
        message.contains("peer: cfo answered"),
        "the operator's blocker must say the peer was asked and answered: {message}"
    );
    assert_eq!(turn.consults(), 1);
    assert_eq!(
        blocks.take().len(),
        1,
        "the gap is parked as one blocked node"
    );
    let attempts = runs
        .list_runs(
            &CompanyId::new("acme"),
            &crate::ports::RunFilter::for_workflow_run("run-1866-peer-blocked".to_string()),
        )
        .await
        .expect("list attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].status,
        crate::ports::RunStatus::Blocked,
        "an information gap is blocked on a person, not failed"
    );
}

/// The deterministic check outranks the peer. An answer the judge accepts
/// still has to satisfy the node's declared postcondition, and when it does
/// not the attempt fails rather than shipping.
#[tokio::test]
async fn a_peer_answer_the_judge_accepts_still_fails_its_postcondition() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1866-peer-postcondition-")
        .tempdir()
        .expect("tempdir");
    let (base_url, _script) =
        crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"continue\"}"),
        ])
        .await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let runs: Arc<dyn crate::ports::RunStore> =
        Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let turn = Arc::new(ConsultedPeerTurn::new("The renewal date is March 1st."));
    let (runner, blocks, _board, _notices, _artifacts) = peer_runner(
        turn.clone(),
        deps,
        "run-1866-peer-postcondition",
        Some(runs.clone()),
    );

    let err = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "draft",
                "prompt": "Draft the customer email.",
                "verify": { "criteria": "must include the customer's renewal date" },
                "postcondition": { "require": "field_present", "field": "items" }
            }),
        )
        .await
        .expect_err("a recovered reply must still clear the deterministic check");
    let EngineError::Capability(message) = err else {
        panic!("expected a capability error");
    };
    assert!(
        message.contains("items"),
        "the halt must name what the recovered output was missing: {message}"
    );
    assert!(
        blocks.take().is_empty(),
        "a failed postcondition asks nobody for anything"
    );
    let attempts = runs
        .list_runs(
            &CompanyId::new("acme"),
            &crate::ports::RunFilter::for_workflow_run("run-1866-peer-postcondition".to_string()),
        )
        .await
        .expect("list attempts");
    assert_eq!(attempts[0].status, crate::ports::RunStatus::Failed);
}

/// A consultation was asked a question, not given authority. Whatever the
/// consulted peer staged on the shared board and publish queues is thrown
/// away, so the *next* node's drain — the path that would otherwise execute
/// it and attribute it to this run — finds nothing.
#[tokio::test]
async fn a_consultation_that_stages_board_work_leaves_nothing_behind() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1866-peer-no-authority-")
        .tempdir()
        .expect("tempdir");
    let (base_url, _script) =
        crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"continue\"}"),
        ])
        .await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let queues = deps.clone();
    let turn = Arc::new(ConsultedPeerTurn::staging(
        "The renewal date is March 1st.",
        deps.clone(),
    ));
    let (runner, blocks, board, notices, _artifacts) =
        peer_runner(turn.clone(), deps, "run-1866-peer-authority", None);

    runner
        .run_turn("researcher", verify_node())
        .await
        .expect("the consultation answered, so the node ships");
    assert_eq!(turn.consults(), 1);

    // The node that runs next is what would execute a leaked staging: its
    // own post-turn drain reads the same queues.
    runner
        .run_turn(
            "researcher",
            json!({ "node_id": "next", "prompt": "carry on" }),
        )
        .await
        .expect("a plain node runs");

    assert!(
        board.take().is_empty(),
        "a consultation must open no card on the run's board"
    );
    assert!(
        blocks.take().is_empty(),
        "a consultation settles nothing and blocks nobody"
    );
    assert!(
        notices.take().is_empty(),
        "no operator notice may be raised for work a consultation only staged"
    );
    assert_eq!(
        queues.approval_requests.queued(),
        0,
        "a consultation must not leave an approval card for the operator's next chat cycle \
         to drain as if the operator had asked for it"
    );
}

/// A consulted peer may be the orchestrator, which can run a whole workflow
/// — whose nodes reach this same recover path. The rung must fire once down
/// the whole stack, not once per nested node.
#[tokio::test]
async fn a_consultation_cannot_re_enter_the_recovery_ladder() {
    struct ReentrantPeerTurn {
        consults: std::sync::atomic::AtomicUsize,
        node_turns: std::sync::atomic::AtomicUsize,
        runner: std::sync::OnceLock<Arc<HarnessAgentRunner>>,
    }

    #[async_trait]
    impl RunTurn for ReentrantPeerTurn {
        async fn run(
            &self,
            _company: &CompanyId,
            _agent_id: &str,
            _message: &str,
            _chat: crate::runtime::delegation::ChatTarget<'_>,
        ) -> crate::Result<crate::harness::TurnOutcome> {
            unreachable!("nodes route through run_background_workflow")
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
            unreachable!("nodes route through run_background_workflow")
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
            unreachable!("nodes route through run_background_workflow")
        }

        async fn run_background(
            &self,
            _company: &CompanyId,
            _agent_id: &str,
            _message: &str,
            _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
        ) -> crate::Result<crate::harness::TurnOutcome> {
            self.consults
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // The consulted peer runs a nested graph node, which reaches
            // the same recover path.
            let runner = self.runner.get().expect("runner wired").clone();
            let _ = Box::pin(runner.run_turn("researcher", verify_node())).await;
            Ok(crate::harness::TurnOutcome {
                reply: String::new(),
                steps: Vec::new(),
                hit_iteration_cap: false,
                abnormal_stop: None,
                halted_for_spend: None,
                budget_paused: None,
            })
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
            self.node_turns
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::harness::TurnOutcome {
                reply: "I cannot draft the email without the customer's renewal date.".to_string(),
                steps: Vec::new(),
                hit_iteration_cap: false,
                abnormal_stop: None,
                halted_for_spend: None,
                budget_paused: None,
            })
        }
    }

    let dir = tempfile::Builder::new()
        .prefix("oc-1866-peer-reentrant-")
        .tempdir()
        .expect("tempdir");
    let (base_url, _script) =
        crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
        ])
        .await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let turn = Arc::new(ReentrantPeerTurn {
        consults: std::sync::atomic::AtomicUsize::new(0),
        node_turns: std::sync::atomic::AtomicUsize::new(0),
        runner: std::sync::OnceLock::new(),
    });
    let (runner, _blocks, _board, _notices, _artifacts) =
        peer_runner(turn.clone(), deps, "run-1866-peer-reentrant", None);
    let runner = Arc::new(runner);
    turn.runner.set(runner.clone()).ok().expect("wire runner");

    let _ = runner.run_turn("researcher", verify_node()).await;

    assert_eq!(
        turn.node_turns.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the nested node did run, so the re-entrancy this guards against was reached"
    );
    assert_eq!(
        turn.consults.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the peer rung must fire once down the whole stack, not once per nested node"
    );
}

/// The whole gate is opt-in: a node with no `verify` spends no judge call
/// and asks no peer, exactly as it did before any of this existed.
#[tokio::test]
async fn a_node_with_no_verify_calls_neither_judge_nor_peer() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1866-peer-optin-")
        .tempdir()
        .expect("tempdir");
    let (base_url, script) =
        crate::workflows::gated_tool_turn_tests::spawn_script_recording(Vec::new()).await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let turn = Arc::new(ConsultedPeerTurn::new("should never be reached"));
    let (runner, _blocks, _board, _notices, _artifacts) =
        peer_runner(turn.clone(), deps, "run-1866-peer-optin", None);

    runner
        .run_turn("researcher", json!({ "node_id": "plain", "prompt": "go" }))
        .await
        .expect("an unverified node runs as it always did");

    assert!(
        script.seen.lock().expect("seen").is_empty(),
        "no judge call for a node that declared no criteria"
    );
    assert_eq!(turn.consults(), 0, "and therefore no peer turn either");
}

/// `escalate` is a different arm from `recover` and must never touch the
/// ladder: an infrastructure or human gap is not something a teammate can
/// answer, so spending a peer turn on it would be pure cost. Held by
/// construction; pinned so a refactor that folded the arms together fails.
#[tokio::test]
async fn an_escalate_verdict_never_enters_the_recovery_ladder() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1866-peer-escalate-")
        .tempdir()
        .expect("tempdir");
    let (base_url, script) = crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
        crate::workflows::gated_tool_turn_tests::Turn::Say(
            "{\"verdict\":\"escalate\",\"gap\":\"infrastructure\"}",
        ),
    ])
    .await;
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    let turn = Arc::new(ConsultedPeerTurn::new("should never be reached"));
    let (runner, _blocks, _board, _notices, _artifacts) =
        peer_runner(turn.clone(), deps, "run-1866-peer-escalate", None);

    let err = runner
        .run_turn("researcher", verify_node())
        .await
        .expect_err("an escalated gap halts the node");
    let EngineError::Capability(message) = err else {
        panic!("expected a capability error");
    };
    assert!(
        message.contains("after semantic verification"),
        "the escalate arm's own message, not the recovery arm's: {message}"
    );
    assert!(
        !message.contains("recovery tried"),
        "an escalated gap must not report a recovery ladder it never ran: {message}"
    );
    assert_eq!(turn.consults(), 0, "no peer turn on the escalate arm");
    assert_eq!(
        script.seen.lock().expect("seen").len(),
        1,
        "one judge call and no re-verification"
    );
}
