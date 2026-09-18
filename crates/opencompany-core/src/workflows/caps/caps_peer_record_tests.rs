use super::tests_turn_dispatch::ok_outcome;
use super::tests_turn_dispatch::single_turn;
use super::*;

/// Issue #2005: the engine-side trigger reader — what an answered blocker
/// riding the continuation's trigger input actually does to the node it
/// names.
mod node_blocker_answer {
    use super::*;
    use crate::ports::blockers::{BlockerKind, BlockerSource, BlockerVerdict};
    use crate::runtime::workflow_resume::{
        BlockerAnswer, CONTINUATION_BLOCKER_KEY, with_blocker_answer, workflow_node_turn_key,
    };

    const RUN_ID: &str = "run-2005";

    /// A turn double that records the message it was handed, so an amend's
    /// injection is provable and a skip's non-execution is too.
    struct MessageRecordingTurn {
        messages: std::sync::Mutex<Vec<String>>,
    }

    impl MessageRecordingTurn {
        fn new() -> Self {
            Self {
                messages: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn messages(&self) -> Vec<String> {
            self.messages.lock().expect("messages").clone()
        }
    }

    #[async_trait]
    impl RunTurn for MessageRecordingTurn {
        async fn run(
            &self,
            _company: &CompanyId,
            _agent_id: &str,
            message: &str,
            _chat_id: crate::runtime::delegation::ChatTarget<'_>,
        ) -> crate::Result<crate::harness::TurnOutcome> {
            self.messages
                .lock()
                .expect("messages")
                .push(message.to_string());
            Ok(ok_outcome())
        }

        async fn run_steered(
            &self,
            _company: &CompanyId,
            _agent_id: &str,
            message: &str,
            _control: &crate::company::steer::SteerControl,
            _chat_id: crate::runtime::delegation::ChatTarget<'_>,
            _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
        ) -> crate::Result<crate::harness::TurnOutcome> {
            self.messages
                .lock()
                .expect("messages")
                .push(message.to_string());
            Ok(ok_outcome())
        }

        async fn run_steered_background(
            &self,
            _company: &CompanyId,
            _agent_id: &str,
            message: &str,
            _control: &crate::company::steer::SteerControl,
            _chat: crate::runtime::delegation::ChatTarget<'_>,
            _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
        ) -> crate::Result<crate::harness::TurnOutcome> {
            self.messages
                .lock()
                .expect("messages")
                .push(message.to_string());
            Ok(ok_outcome())
        }

        async fn run_background_workflow(
            &self,
            _company: &CompanyId,
            _agent_id: &str,
            message: &str,
            _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
            _workflow_run_id: &str,
            _node_id: &str,
        ) -> crate::Result<crate::harness::TurnOutcome> {
            self.messages
                .lock()
                .expect("messages")
                .push(message.to_string());
            Ok(ok_outcome())
        }
    }

    fn answered(node: &str, verdict: BlockerVerdict, answer: &str) -> Value {
        with_blocker_answer(
            json!({ "topic": "quarterly numbers" }),
            &BlockerAnswer {
                node: node.to_string(),
                verdict,
                answer: answer.to_string(),
            },
        )
    }

    async fn runner_with(
        dir: &std::path::Path,
        turn: Arc<MessageRecordingTurn>,
        trigger_input: Value,
    ) -> HarnessAgentRunner {
        let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir);
        let board_claim = Arc::new(deps.delegations.claim_board(RUN_ID));
        let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run(RUN_ID));
        HarnessAgentRunner::new(
            turn,
            deps,
            crate::workflows::gated_tool_turn_tests::record(),
            CompanyId::new("acme"),
            "reporting".to_string(),
            RUN_ID.to_string(),
            None,
            trigger_input,
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
    }

    fn tmp(prefix: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("tempdir")
    }

    /// A skip proceeds past the node without spending a turn on the
    /// question the operator just waived.
    #[tokio::test]
    async fn a_skipped_node_does_not_run_its_turn() {
        let dir = tmp("oc-2005-skip-");
        let turn = Arc::new(MessageRecordingTurn::new());
        let runner = runner_with(
            dir.path(),
            turn.clone(),
            answered("gather", BlockerVerdict::Skip, ""),
        )
        .await;

        let (value, outcome) = runner
            .run_turn("researcher", json!({ "node_id": "gather", "prompt": "go" }))
            .await
            .expect("a skipped node still produces an output the branch can bind");

        assert!(
            turn.messages().is_empty(),
            "a waived node must not spend a turn: {:?}",
            turn.messages()
        );
        assert!(outcome.reply.contains("skipped"), "{}", outcome.reply);
        assert_eq!(value["agent_ref"], "researcher");
    }

    /// An amend re-runs the node carrying the operator's correction — the
    /// workflow twin of the card path's note append.
    #[tokio::test]
    async fn an_amended_node_re_runs_carrying_the_operators_words() {
        let dir = tmp("oc-2005-amend-");
        let turn = Arc::new(MessageRecordingTurn::new());
        let runner = runner_with(
            dir.path(),
            turn.clone(),
            answered("gather", BlockerVerdict::Amend, "use gpt-4o-mini instead"),
        )
        .await;

        runner
            .run_turn("researcher", json!({ "node_id": "gather", "prompt": "go" }))
            .await
            .expect("an amended node runs");

        let messages = turn.messages();
        assert_eq!(messages.len(), 1, "the node runs exactly once");
        assert!(
            messages[0].contains("use gpt-4o-mini instead"),
            "the correction has to reach the turn, or the re-run repeats the failure: {}",
            messages[0]
        );
    }

    /// A retry runs the step again as it was — no correction to inject.
    #[tokio::test]
    async fn a_retried_node_runs_again_as_it_was() {
        let dir = tmp("oc-2005-retry-");
        let turn = Arc::new(MessageRecordingTurn::new());
        let runner = runner_with(
            dir.path(),
            turn.clone(),
            answered("gather", BlockerVerdict::Retry, ""),
        )
        .await;

        runner
            .run_turn("researcher", json!({ "node_id": "gather", "prompt": "go" }))
            .await
            .expect("a retried node runs");

        let messages = turn.messages();
        assert_eq!(messages.len(), 1);
        assert!(
            !messages[0].contains("Answer from the operator"),
            "a bare retry carries no words: {}",
            messages[0]
        );
    }

    /// One node's answer is not the graph's: every other node runs as it
    /// always did.
    #[tokio::test]
    async fn an_answer_for_another_node_leaves_this_one_alone() {
        let dir = tmp("oc-2005-other-");
        let turn = Arc::new(MessageRecordingTurn::new());
        let runner = runner_with(
            dir.path(),
            turn.clone(),
            answered("review", BlockerVerdict::Skip, ""),
        )
        .await;

        runner
            .run_turn("researcher", json!({ "node_id": "gather", "prompt": "go" }))
            .await
            .expect("an unanswered node runs");

        assert_eq!(turn.messages().len(), 1);
    }

    /// An unreadable answer fails the node rather than degrading to
    /// "nobody answered" — the degrade would spend a turn on the identical
    /// failure with the operator's decision gone.
    #[tokio::test]
    async fn an_unreadable_answer_fails_the_node_rather_than_running_it() {
        let dir = tmp("oc-2005-garbled-");
        let turn = Arc::new(MessageRecordingTurn::new());
        let runner = runner_with(
            dir.path(),
            turn.clone(),
            json!({
                CONTINUATION_BLOCKER_KEY: [{ "node": "gather", "verdict": "shrug" }]
            }),
        )
        .await;

        let outcome = runner
            .run_turn("researcher", json!({ "node_id": "gather", "prompt": "go" }))
            .await;

        assert!(outcome.is_err(), "a garbled answer must stop the node");
        assert!(turn.messages().is_empty(), "and must not spend a turn");
    }

    /// A cancel starts no run at all, so a node reached carrying one is a
    /// host bug — and stops loudly rather than carrying on as if the
    /// operator had said yes.
    #[tokio::test]
    async fn a_cancelled_answer_stops_the_node() {
        let dir = tmp("oc-2005-cancel-");
        let turn = Arc::new(MessageRecordingTurn::new());
        let runner = runner_with(
            dir.path(),
            turn.clone(),
            json!({
                CONTINUATION_BLOCKER_KEY: [{ "node": "gather", "verdict": "cancel" }]
            }),
        )
        .await;

        let outcome = runner
            .run_turn("researcher", json!({ "node_id": "gather", "prompt": "go" }))
            .await;

        assert!(outcome.is_err());
        assert!(turn.messages().is_empty());
    }

    /// The other half of the thread: a blocker's park has to stash what the
    /// answer's re-entry will need. The gated-call arm cannot cover it — a
    /// turn that parked no gated call returns before reaching that arm, and
    /// the runner's settle-time pass refuses to arm a turn that is not
    /// already armed — so without this the answer reaches a resume with no
    /// run to continue.
    #[tokio::test]
    async fn parking_a_node_blocker_stashes_the_run_its_answer_re_enters() {
        let dir = tmp("oc-2005-stash-");
        let (deps, _journal) =
            crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
        let parking = deps
            .delivery
            .clone()
            .expect("delivery")
            .parking
            .clone()
            .expect("parking");
        let trigger_input = json!({ "topic": "quarterly numbers" });
        let board_claim = Arc::new(deps.delegations.claim_board(RUN_ID));
        let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run(RUN_ID));
        let runner = HarnessAgentRunner::new(
            single_turn(&deps),
            deps,
            crate::workflows::gated_tool_turn_tests::record(),
            CompanyId::new("acme"),
            "reporting".to_string(),
            RUN_ID.to_string(),
            None,
            trigger_input.clone(),
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

        let parked = runner
            .park_node_blocker_as(
                "gather",
                "the model id `gpt-nope` was rejected",
                BlockerKind::Infrastructure,
                BlockerSource::Provider,
                "a model id this provider serves",
            )
            .await;
        assert!(parked.is_some(), "the blocker parks");

        let stashed = parking
            .blocked_nodes
            .peek(&workflow_node_turn_key(RUN_ID, "gather"))
            .expect("a parked blocker must stash the run its answer re-enters");
        assert_eq!(stashed.workflow_id, "reporting");
        assert_eq!(stashed.input, trigger_input);
    }

    /// A resolver racing in against a live blocker park must always find
    /// the stash already armed. This spies on the approval gate's own
    /// `park` call and captures whether the stash is armed at that exact
    /// point — deterministic, no wall-clock race needed, on the same
    /// principle as
    /// `park_and_journal_arms_the_continuation_slot_before_the_card_is_parkable`
    /// in `workflows::delivery`.
    #[tokio::test]
    async fn park_node_blocker_as_arms_the_stash_before_the_card_is_parkable() {
        use crate::ports::ApprovalGate;
        use crate::ports::types::{Actor, ApprovalId, Effect, PolicyDecision, Verdict};

        struct Spy {
            inner: Arc<dyn ApprovalGate>,
            blocked_nodes: crate::runtime::blocked_nodes::BlockedNodeQueue,
            turn: String,
            armed_at_park: std::sync::Mutex<Option<bool>>,
        }

        #[async_trait]
        impl ApprovalGate for Spy {
            async fn evaluate(
                &self,
                company: &CompanyId,
                effect: &Effect,
            ) -> crate::Result<PolicyDecision> {
                self.inner.evaluate(company, effect).await
            }

            async fn park(&self, company: &CompanyId, effect: Effect) -> crate::Result<ApprovalId> {
                *self.armed_at_park.lock().expect("spy lock") =
                    Some(self.blocked_nodes.is_armed(&self.turn));
                self.inner.park(company, effect).await
            }

            async fn resolve(
                &self,
                id: &ApprovalId,
                verdict: Verdict,
                by: Actor,
            ) -> crate::Result<Option<Effect>> {
                self.inner.resolve(id, verdict, by).await
            }
        }

        let dir = tmp("oc-2005-race-a-");
        let (mut deps, _journal) =
            crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
        let parking = deps
            .delivery
            .clone()
            .expect("delivery")
            .parking
            .clone()
            .expect("parking");

        let run_id = "run-2005-race-a".to_string();
        let turn = workflow_node_turn_key(&run_id, "gather");
        let spy = Arc::new(Spy {
            inner: parking.approvals.clone(),
            blocked_nodes: parking.blocked_nodes.clone(),
            turn: turn.clone(),
            armed_at_park: std::sync::Mutex::new(None),
        });
        let mut spied_parking = parking.clone();
        spied_parking.approvals = spy.clone();
        deps.delivery.as_mut().expect("delivery").parking = Some(spied_parking);

        let trigger_input = json!({ "topic": "quarterly numbers" });
        let board_claim = Arc::new(deps.delegations.claim_board(&run_id));
        let publish_refusal_claim =
            Arc::new(deps.pending_publishes.claim_refusals_for_run(&run_id));
        let runner = HarnessAgentRunner::new(
            single_turn(&deps),
            deps,
            crate::workflows::gated_tool_turn_tests::record(),
            CompanyId::new("acme"),
            "reporting".to_string(),
            run_id,
            None,
            trigger_input,
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

        let parked = runner
            .park_node_blocker_as(
                "gather",
                "the model id `gpt-nope` was rejected",
                BlockerKind::Infrastructure,
                BlockerSource::Provider,
                "a model id this provider serves",
            )
            .await;
        assert!(parked.is_some(), "the blocker parks");

        let captured = spy
            .armed_at_park
            .lock()
            .expect("spy lock")
            .expect("park was called");
        assert!(
            captured,
            "the blocked-node stash must already be armed by the time the approval gate's \
             park() runs, before the card becomes resolvable to a concurrent operator"
        );
    }

    /// The same proof as above, for the sibling site: a blocker card
    /// extracted from a node's gated-call batch inside `park_gated_calls`.
    #[tokio::test]
    async fn park_gated_calls_blocker_extraction_arms_the_stash_before_the_first_card_is_parkable()
    {
        use crate::harness::policy::{ApprovalRequest, ApprovalScope};
        use crate::ports::ApprovalGate;
        use crate::ports::blockers::BlockerPayload;
        use crate::ports::types::{
            Actor, ApprovalId, Effect, EffectGroup, PolicyDecision, Verdict,
        };

        struct Spy {
            inner: Arc<dyn ApprovalGate>,
            blocked_nodes: crate::runtime::blocked_nodes::BlockedNodeQueue,
            turn: String,
            armed_at_park: std::sync::Mutex<Option<bool>>,
        }

        #[async_trait]
        impl ApprovalGate for Spy {
            async fn evaluate(
                &self,
                company: &CompanyId,
                effect: &Effect,
            ) -> crate::Result<PolicyDecision> {
                self.inner.evaluate(company, effect).await
            }

            async fn park(&self, company: &CompanyId, effect: Effect) -> crate::Result<ApprovalId> {
                *self.armed_at_park.lock().expect("spy lock") =
                    Some(self.blocked_nodes.is_armed(&self.turn));
                self.inner.park(company, effect).await
            }

            async fn resolve(
                &self,
                id: &ApprovalId,
                verdict: Verdict,
                by: Actor,
            ) -> crate::Result<Option<Effect>> {
                self.inner.resolve(id, verdict, by).await
            }
        }

        let dir = tmp("oc-2005-race-b-");
        let (mut deps, _journal) =
            crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
        let parking = deps
            .delivery
            .clone()
            .expect("delivery")
            .parking
            .clone()
            .expect("parking");

        let run_id = "run-2005-race-b".to_string();
        let turn = workflow_node_turn_key(&run_id, "work");
        let spy = Arc::new(Spy {
            inner: parking.approvals.clone(),
            blocked_nodes: parking.blocked_nodes.clone(),
            turn: turn.clone(),
            armed_at_park: std::sync::Mutex::new(None),
        });
        let mut spied_parking = parking.clone();
        spied_parking.approvals = spy.clone();
        deps.delivery.as_mut().expect("delivery").parking = Some(spied_parking);

        let queue = deps.approval_requests.clone();
        let trigger_input = json!({ "topic": "quarterly numbers" });
        let board_claim = Arc::new(deps.delegations.claim_board(&run_id));
        let publish_refusal_claim =
            Arc::new(deps.pending_publishes.claim_refusals_for_run(&run_id));
        let runner = HarnessAgentRunner::new(
            single_turn(&deps),
            deps,
            crate::workflows::gated_tool_turn_tests::record(),
            CompanyId::new("acme"),
            "reporting".to_string(),
            run_id.clone(),
            None,
            trigger_input,
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

        let payload = BlockerPayload {
            kind: BlockerKind::Information,
            source: BlockerSource::AgentQuestion,
            step: None,
            reason: "which quarter should I report?".to_string(),
            needed: "the quarter to report on".to_string(),
            group_key: None,
        };
        let effect_kind = payload.effect_kind();
        let reason = payload.reason.clone();
        let payload_value = serde_json::to_value(&payload).expect("payload serializes");

        let claim = queue.claim(ApprovalScope::Run(run_id.clone()));
        claim
            .scoped(async {
                queue.push(ApprovalRequest {
                    tool: "escalate_to_human".to_string(),
                    reason,
                    effect: Effect {
                        kind: effect_kind,
                        group: EffectGroup::Other,
                        amount_usd: None,
                        established_thread: false,
                        first_time_counterparty: false,
                        payload: payload_value,
                        agent: None,
                        run_id: None,
                    },
                });
            })
            .await;

        claim
            .scoped(runner.park_gated_calls(Some("work"), "work", &turn))
            .await;

        let captured = spy
            .armed_at_park
            .lock()
            .expect("spy lock")
            .expect("park was called");
        assert!(
            captured,
            "a blocker card extracted from a node's gated-call batch must find the stash \
             already armed by the time the approval gate's park() runs"
        );
    }
}

// ── the recovery ladder's peer rung ──────────────────────────────────────

/// The fixture roster plus one teammate whose role and description match a
/// question about a customer's renewal, so [`pick_peer`] has somebody to
/// choose. The node itself runs as `researcher`, which is deliberately not
/// on the roster.
pub(super) fn record_with_peer() -> CompanyRecord {
    let mut record = crate::workflows::gated_tool_turn_tests::record();
    record.manifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\n[[agent]]\nid = \"cfo\"\nrole = \"Chief Financial \
         Officer\"\ndescription = \"Owns renewal contracts and customer pricing\"\n",
    )
    .expect("manifest parses");
    record
}

/// A [`RunTurn`] whose node turn refuses for want of a fact, and whose peer
/// consultation answers with `peer_reply` — optionally staging board work
/// on the way, standing in for a consulted teammate whose tools wrote.
pub(super) struct ConsultedPeerTurn {
    pub(super) peer_reply: &'static str,
    pub(super) consults: std::sync::atomic::AtomicUsize,
    pub(super) node_turns: std::sync::atomic::AtomicUsize,
    pub(super) stage: Option<HarnessDeps>,
}
