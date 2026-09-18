use super::*;

// ---- the transcript fold (the record a workflow node now leaves) -------
mod transcript_fold {
    use super::super::transcript_from_steps;
    use crate::ports::types::{TurnStep, TurnStepFailure, TurnStepKind, TurnStepStatus};

    fn step(kind: TurnStepKind, status: TurnStepStatus, label: &str) -> TurnStep {
        TurnStep {
            kind,
            status,
            label: label.to_string(),
            ..TurnStep::default()
        }
    }

    #[test]
    fn a_tool_less_turn_folds_to_nothing() {
        // The zero-steps tell: a memory-served answer genuinely did nothing
        // worth recording, and an empty transcript says exactly that.
        assert!(transcript_from_steps(&[]).is_empty());
    }

    #[test]
    fn each_step_kind_maps_to_an_engine_word() {
        let steps = vec![
            step(TurnStepKind::Thinking, TurnStepStatus::Ok, "Thinking"),
            step(TurnStepKind::Note, TurnStepStatus::Ok, "note"),
            step(TurnStepKind::ToolCall, TurnStepStatus::Ok, "shell"),
            step(TurnStepKind::ToolCall, TurnStepStatus::Error, "shell"),
            step(TurnStepKind::ToolCall, TurnStepStatus::Running, "shell"),
            step(
                TurnStepKind::ToolCall,
                TurnStepStatus::AwaitingApproval,
                "shell",
            ),
        ];
        assert_eq!(
            transcript_from_steps(&steps)
                .iter()
                .map(|e| e.kind.clone())
                .collect::<Vec<_>>(),
            [
                "agent_thinking",
                "agent_message",
                "tool_result",
                "error",
                "tool_call",
                "tool_awaiting_approval",
            ]
        );
    }

    #[test]
    fn a_parked_call_is_not_folded_as_a_failure() {
        // The #411 distinction, preserved through the fold: the one step an
        // operator can act on must not read as a crash.
        let parked = transcript_from_steps(&[step(
            TurnStepKind::ToolCall,
            TurnStepStatus::AwaitingApproval,
            "shell",
        )]);
        assert_eq!(parked[0].kind, "tool_awaiting_approval");
        assert_ne!(parked[0].kind, "error");
    }

    #[test]
    fn the_line_carries_what_the_step_knows() {
        let entry = transcript_from_steps(&[TurnStep {
            kind: TurnStepKind::ToolCall,
            status: TurnStepStatus::Ok,
            label: "shell".to_string(),
            detail: Some("python3 solve.py".to_string()),
            result: Some("3 lines".to_string()),
            truncated: true,
            elapsed_ms: Some(1200),
            failure: None,
        }]);
        assert_eq!(
            entry[0].text,
            "shell: python3 solve.py → 3 lines [truncated] (1200ms)"
        );
    }

    #[test]
    fn a_failure_class_rides_the_line_in_snake_case() {
        let entry = transcript_from_steps(&[TurnStep {
            kind: TurnStepKind::ToolCall,
            status: TurnStepStatus::Error,
            label: "github.merge".to_string(),
            failure: Some(TurnStepFailure::BlockedByPolicy),
            ..TurnStep::default()
        }]);
        assert!(
            entry[0].text.contains("[blocked_by_policy]"),
            "got {:?}",
            entry[0].text
        );
    }

    #[test]
    fn empty_detail_and_result_add_no_punctuation() {
        // A bare label must not fold to "label: " or "label → ".
        let entry = transcript_from_steps(&[TurnStep {
            kind: TurnStepKind::ToolCall,
            status: TurnStepStatus::Ok,
            label: "workspace_read".to_string(),
            detail: Some(String::new()),
            result: Some(String::new()),
            ..TurnStep::default()
        }]);
        assert_eq!(entry[0].text, "workspace_read");
    }

    #[test]
    fn one_long_step_cannot_eat_the_records_budget() {
        // `TranscriptEntry::bounded` is the crate's own ceiling; the fold
        // must go through it rather than around it.
        let entry = transcript_from_steps(&[TurnStep {
            kind: TurnStepKind::ToolCall,
            status: TurnStepStatus::Ok,
            label: "shell".to_string(),
            result: Some("x".repeat(64 * 1024)),
            ..TurnStep::default()
        }]);
        assert!(
            entry[0].text.len() < 8 * 1024,
            "entry was {} bytes — bounded() was bypassed",
            entry[0].text.len()
        );
        assert!(entry[0].text.ends_with("…[truncated]"));
    }

    #[test]
    fn order_is_preserved() {
        // A transcript read out of order is not a transcript.
        let steps: Vec<TurnStep> = (0..5)
            .map(|i| {
                step(
                    TurnStepKind::ToolCall,
                    TurnStepStatus::Ok,
                    &format!("step{i}"),
                )
            })
            .collect();
        assert_eq!(
            transcript_from_steps(&steps)
                .iter()
                .map(|e| e.text.clone())
                .collect::<Vec<_>>(),
            ["step0", "step1", "step2", "step3", "step4"]
        );
    }

    #[test]
    fn every_failure_class_has_a_stable_snake_case_wire_word() {
        for (failure, expected) in [
            (TurnStepFailure::Declined, "declined"),
            (TurnStepFailure::BlockedByPolicy, "blocked_by_policy"),
            (TurnStepFailure::Unauthorized, "unauthorized"),
            (TurnStepFailure::MissingPermission, "missing_permission"),
            (TurnStepFailure::MissingApp, "missing_app"),
            (TurnStepFailure::NotFound, "not_found"),
            (TurnStepFailure::Unsupported, "unsupported"),
            (TurnStepFailure::Timeout, "timeout"),
            (TurnStepFailure::Unavailable, "unavailable"),
            (TurnStepFailure::Failed, "failed"),
        ] {
            assert_eq!(failure.wire_word(), expected);
        }
    }
}

// ---- the attempt row a workflow node now opens ------------------------

mod attempt {
    use super::*;
    use crate::ports::{NewRun, RunFilter, RunStatus, RunStore};

    fn store() -> Arc<dyn RunStore> {
        let dir = tempfile::Builder::new()
            .prefix("oc-attempt-")
            .tempdir()
            .expect("tempdir");
        let path = dir.path().to_path_buf();
        // The tempdir must outlive the store; leak it, this is a test.
        std::mem::forget(dir);
        Arc::new(crate::store::fs_ops::FsOps::new(&path))
    }

    #[tokio::test]
    async fn a_node_run_is_addressable_by_its_workflow_run() {
        // The join, end to end at the port: this is the query that had no
        // answer before, because a node's attempt had neither a card nor a
        // conversation to be found by.
        let runs = store();
        let company = CompanyId::new("acme");
        for (id, node) in [("a", "solve"), ("b", "check")] {
            let row = runs
                .create_run(
                    &company,
                    NewRun::for_workflow_node(id, "run-1", node, "programmer"),
                )
                .await
                .expect("create");
            runs.begin_run_untriggered(&company, &row.id)
                .await
                .expect("begin");
        }

        let found = runs
            .list_runs(&company, &RunFilter::for_workflow_run("run-1"))
            .await
            .expect("list");
        assert_eq!(found.len(), 2);
        assert!(
            found.iter().all(|r| r.status == RunStatus::Running),
            "an untriggered begin still moves the row to Running"
        );
        assert!(
            found.iter().all(|r| r.trigger_event_seq.is_none()),
            "a workflow node has no driving journal event, and says so"
        );
        let mut nodes: Vec<&str> = found.iter().filter_map(|r| r.node_id.as_deref()).collect();
        nodes.sort_unstable();
        assert_eq!(nodes, ["check", "solve"]);
    }

    #[tokio::test]
    async fn an_untriggered_begin_refuses_an_illegal_transition() {
        // The transition legality that lives on the port must not be
        // bypassed by the sibling entry point.
        let runs = store();
        let company = CompanyId::new("acme");
        let row = runs
            .create_run(
                &company,
                NewRun::for_workflow_node("a", "run-1", "solve", "p"),
            )
            .await
            .expect("create");
        runs.begin_run_untriggered(&company, &row.id)
            .await
            .expect("first begin");
        assert!(
            runs.begin_run_untriggered(&company, &row.id).await.is_err(),
            "Running -> Running is not a legal transition"
        );
    }
}

// ── Issue #1861: a node blocked on something a person can answer ────────

/// A turn double that fails with an arbitrary message, so the classifier
/// sees a real error chain rather than a hand-built string.
struct FailingTurn(String);

#[async_trait]
impl RunTurn for FailingTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        Err(crate::error::OpenCompanyError::Harness(self.0.clone()))
    }

    async fn run_steered(
        &self,
        company: &CompanyId,
        agent_id: &str,
        message: &str,
        _control: &crate::company::steer::SteerControl,
        chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        self.run(company, agent_id, message, chat).await
    }

    async fn run_steered_background(
        &self,
        company: &CompanyId,
        agent_id: &str,
        message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        self.run(
            company,
            agent_id,
            message,
            crate::runtime::delegation::ChatTarget::channel(None),
        )
        .await
    }
}

async fn run_failing_node(
    dir: &std::path::Path,
    error: &str,
) -> (RunBlocks, Arc<crate::runtime::journal::RuntimeJournal>) {
    let (deps, journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir);
    let record = crate::workflows::gated_tool_turn_tests::record();
    let board_claim = Arc::new(deps.delegations.claim_board("run-1861"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1861"));
    let blocks = RunBlocks::default();
    let runner = HarnessAgentRunner::new(
        Arc::new(FailingTurn(error.to_string())),
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1".to_string(),
        "run-1861".to_string(),
        None,
        json!({}),
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        blocks.clone(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );
    let outcome = runner
        .run_turn("researcher", json!({ "node_id": "gather", "prompt": "go" }))
        .await;
    assert!(outcome.is_err(), "a failed node must not advance the graph");
    (blocks, journal)
}

/// The workflow half of #1861. A node that died on a model id the provider
/// rejects is answerable, so it reaches the operator as a parked question
/// and the node holds open — through the same #881 machinery an agent's own
/// blocked tool call already uses, which is what makes the two arrive as
/// one shape.
#[tokio::test]
async fn a_node_that_fails_on_a_rejected_model_parks_a_blocker() {
    use crate::ports::blockers::{BlockerKind, BlockerPayload, BlockerSource, BlockerStep};

    let dir = tempfile::Builder::new()
        .prefix("oc-1861-")
        .tempdir()
        .expect("tempdir");
    let (blocks, journal) = run_failing_node(
        dir.path(),
        "the model `gpt-nonexistent` does not exist or you do not have access to it",
    )
    .await;

    let blocked = blocks.take();
    assert_eq!(blocked.len(), 1, "the node is held open, not failed");
    assert_eq!(blocked[0].node_id, "gather");
    assert!(
        blocked[0].tools.is_empty(),
        "nothing the agent called was gated; the node itself stopped"
    );
    assert_eq!(
        blocked[0].approval_ids.len(),
        1,
        "the block must name the approval it is decidable through"
    );

    let parked = journal
        .pending()
        .into_iter()
        .find(|p| p.effect.kind.starts_with("blocker."))
        .expect("a blocker is parked");
    assert_eq!(parked.effect.kind, "blocker.infrastructure");
    assert_eq!(parked.effect.run_id.as_deref(), Some("run-1861"));

    let payload: BlockerPayload =
        serde_json::from_value(parked.effect.payload.clone()).expect("payload round-trips");
    assert_eq!(payload.kind, BlockerKind::Infrastructure);
    assert_eq!(payload.source, BlockerSource::Provider);
    assert_eq!(
        payload.step,
        Some(BlockerStep::Node {
            run_id: "run-1861".to_string(),
            node_id: "gather".to_string()
        }),
        "a run has no card to name instead, and #1864 restarts the node"
    );
}

/// The conservative default holds here too: an error the classifier does
/// not recognise fails the node exactly as it did before, and holds nothing
/// open on a question nobody was asked.
#[tokio::test]
async fn an_unrecognised_node_failure_still_fails_and_parks_nothing() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1861b-")
        .tempdir()
        .expect("tempdir");
    let (blocks, journal) = run_failing_node(dir.path(), "index out of bounds").await;

    assert!(
        blocks.take().is_empty(),
        "an unrecognised failure is a failure, and the node must settle as one"
    );
    assert!(
        journal
            .pending()
            .into_iter()
            .all(|p| !p.effect.kind.starts_with("blocker.")),
        "nothing was parked"
    );
}

/// A transient stop is recognised precisely so it does **not** hold the run
/// open: a rate limit resolves itself and asking about it wastes the ask.
#[tokio::test]
async fn a_transient_node_failure_does_not_hold_the_run_open() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1861c-")
        .tempdir()
        .expect("tempdir");
    let (blocks, _journal) = run_failing_node(
        dir.path(),
        "hosted inference returned 429: rate limit exceeded",
    )
    .await;
    assert!(blocks.take().is_empty());
}
