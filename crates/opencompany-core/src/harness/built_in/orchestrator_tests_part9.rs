use super::*;

/// The [`DrainClaim::Board`] permit matrix: both kinds a run may perform
/// stage, and both it may not are refused — each for its own reason.
#[tokio::test]
async fn a_board_claim_permits_cards_and_refuses_review_and_hand_off() {
    let queue = DelegationQueue::default();
    let run = queue.claim_board("run-1");

    run.scoped(async {
        assert_eq!(stage(&queue, card("open a card")), Staged::Queued);
        assert_eq!(
            stage(
                &queue,
                Delegation::AssignTask {
                    task_id: "t1".to_string(),
                    assignee: "design".to_string(),
                    note: None,
                }
            ),
            Staged::Queued
        );

        // Lifecycle is the operator's lane.
        assert_eq!(
            stage(
                &queue,
                Delegation::ReviewTask {
                    task_id: "t1".to_string(),
                    decision: ReviewDecision::Approve,
                    note: None,
                }
            ),
            Staged::NoDrain(NoDrainReason::WorkflowLifecycle)
        );
        // A hand-off has nowhere to put the reply it exists for.
        assert_eq!(
            stage(&queue, hand_off()),
            Staged::NoDrain(NoDrainReason::WorkflowHandOff)
        );
    })
    .await;
}

/// The refusal text is what a model reads and reacts to, so both wordings
/// have to name the real cause and what the run *can* do instead — and must
/// not be each other's.
#[test]
fn the_two_workflow_refusals_say_what_the_run_can_do_instead() {
    let lifecycle = no_drain(
        REVIEW_TASK_TOOL,
        "the card was NOT reviewed",
        NoDrainReason::WorkflowLifecycle,
    );
    assert!(
        lifecycle.contains("running inside a workflow"),
        "{lifecycle}"
    );
    assert!(lifecycle.contains("operator's call"), "{lifecycle}");
    assert!(
        lifecycle.contains("`spawn_task`") && lifecycle.contains("`assign_task`"),
        "it must name what the run can do instead: {lifecycle}"
    );
    assert!(
        !lifecycle.contains("no conversation"),
        "the lifecycle refusal must not borrow the hand-off's cause: {lifecycle}"
    );

    let hand_off = no_drain(
        DELEGATE_TO_DESK_TOOL,
        "nothing was handed to the design desk",
        NoDrainReason::WorkflowHandOff,
    );
    assert!(hand_off.contains("running inside a workflow"), "{hand_off}");
    assert!(hand_off.contains("no conversation"), "{hand_off}");
    assert!(
        hand_off.contains("`spawn_task`"),
        "it must name the durable alternative: {hand_off}"
    );
    assert!(
        !hand_off.contains("operator's call"),
        "the hand-off refusal must not borrow the lifecycle's cause: {hand_off}"
    );

    // Both keep the do-not-report-it-as-done half every refusal here needs.
    for text in [&lifecycle, &hand_off] {
        assert!(text.contains("Do not retry this call"), "{text}");
        assert!(text.contains("do NOT report"), "{text}");
    }

    // …and they stay countable apart in the logs, from each other and from
    // the three that came before.
    let labels = [
        NoDrainReason::Unwired,
        NoDrainReason::Triage,
        NoDrainReason::Depth,
        NoDrainReason::WorkflowLifecycle,
        NoDrainReason::WorkflowHandOff,
    ]
    .map(|r| r.as_str());
    let unique: std::collections::BTreeSet<_> = labels.iter().collect();
    assert_eq!(unique.len(), labels.len(), "{labels:?}");
}

#[tokio::test]
async fn list_tasks_filters_by_column_and_assignee_and_excludes_done_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let fs = Arc::new(crate::store::FsOps::new(dir.path()));
    let tasks: Arc<dyn TaskStore> = fs;
    let company = CompanyId::new("acme");
    tasks
        .upsert(
            &company,
            &task_card(
                "t-1",
                "Draft the memo",
                crate::ports::tasks::COLUMN_TODO,
                "maya",
            ),
        )
        .await
        .unwrap();
    tasks
        .upsert(
            &company,
            &task_card(
                "t-2",
                "Fix the flaky test",
                crate::ports::tasks::COLUMN_PAUSED,
                "engineer",
            ),
        )
        .await
        .unwrap();
    tasks
        .upsert(
            &company,
            &task_card("t-3", "Ship the release", COLUMN_DONE, "maya"),
        )
        .await
        .unwrap();

    let tool = ListTasksTool::new(company, Some(tasks), None);

    let default_view = tool.execute(json!({})).await.unwrap().output_for_llm(true);
    assert!(default_view.contains("Draft the memo"), "{default_view}");
    assert!(
        default_view.contains("Fix the flaky test"),
        "{default_view}"
    );
    assert!(
        !default_view.contains("Ship the release"),
        "done cards must be excluded by default: {default_view}"
    );

    let by_column = tool
        .execute(json!({ "column": "paused" }))
        .await
        .unwrap()
        .output_for_llm(true);
    assert!(by_column.contains("Fix the flaky test"), "{by_column}");
    assert!(!by_column.contains("Draft the memo"), "{by_column}");

    let by_assignee = tool
        .execute(json!({ "assignee": "MAYA" }))
        .await
        .unwrap()
        .output_for_llm(true);
    assert!(
        by_assignee.contains("Draft the memo"),
        "case-insensitive assignee match: {by_assignee}"
    );
    assert!(!by_assignee.contains("Fix the flaky test"), "{by_assignee}");

    let done_explicit = tool
        .execute(json!({ "column": "done" }))
        .await
        .unwrap()
        .output_for_llm(true);
    assert!(
        done_explicit.contains("Ship the release"),
        "an explicit `column: done` must still answer: {done_explicit}"
    );
}

#[tokio::test]
async fn list_tasks_truncates_with_an_honest_marker() {
    let dir = tempfile::tempdir().unwrap();
    let tasks: Arc<dyn TaskStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    let company = CompanyId::new("acme");
    for n in 0..(LIST_TASKS_LIMIT + 5) {
        tasks
            .upsert(
                &company,
                &task_card(
                    &format!("t-{n}"),
                    &format!("Card {n}"),
                    crate::ports::tasks::COLUMN_TODO,
                    "maya",
                ),
            )
            .await
            .unwrap();
    }

    let tool = ListTasksTool::new(company, Some(tasks), None);
    let out = tool.execute(json!({})).await.unwrap().output_for_llm(true);
    assert!(out.contains("TRUNCATED"), "{out}");
    assert!(out.contains("5 more card"), "{out}");
}

#[tokio::test]
async fn list_tasks_reports_unavailable_when_the_board_is_unwired() {
    let tool = ListTasksTool::new(CompanyId::new("acme"), None, None);
    let result = tool.execute(json!({})).await.unwrap();
    assert!(
        result.is_error,
        "no task board wired must be a refusal, not an empty board"
    );
    assert!(
        result.output_for_llm(true).contains("No task board wired"),
        "{:?}",
        result.output_for_llm(true)
    );
}

#[tokio::test]
async fn read_task_renders_header_every_attempt_and_falls_back_to_the_cards_output_stamp() {
    let dir = tempfile::tempdir().unwrap();
    let fs = Arc::new(crate::store::FsOps::new(dir.path()));
    let tasks: Arc<dyn TaskStore> = fs.clone();
    let runs: Arc<dyn RunStore> = fs;
    let company = CompanyId::new("acme");

    let mut card = task_card(
        "t-1",
        "Investigate the outage",
        crate::ports::tasks::COLUMN_IN_REVIEW,
        "engineer",
    );
    card.note = Some("check the load balancer first".to_string());
    card.output = Some(crate::ports::tasks::TaskOutput {
        source: crate::ports::tasks::TaskOutputSource::Run {
            run_id: "r-2".to_string(),
            attempt: Some(2),
        },
        at_millis: 5,
        artifacts: Vec::new(),
        workflows: Vec::new(),
    });
    tasks.upsert(&company, &card).await.unwrap();

    let mut r1 = runs
        .create_run(
            &company,
            crate::ports::runs::NewRun::for_task("r-1", "t-1", "engineer"),
        )
        .await
        .unwrap();
    r1.status = RunStatus::Failed;
    r1.error = Some("timed out".to_string());
    runs.put_run(&company, &r1).await.unwrap();
    let mut r2 = runs
        .create_run(
            &company,
            crate::ports::runs::NewRun::for_task("r-2", "t-1", "engineer"),
        )
        .await
        .unwrap();
    r2.status = RunStatus::Succeeded;
    runs.put_run(&company, &r2).await.unwrap();

    let tool = ReadTaskTool::new(company, Some(tasks), Some(runs), None);
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(out.contains("Investigate the outage"), "{out}");
    assert!(out.contains("check the load balancer first"), "{out}");
    assert!(out.contains("attempt 1"), "{out}");
    assert!(out.contains("attempt 2"), "{out}");
    assert!(out.contains("timed out"), "{out}");
    // No artifact store wired: falls back to the card's own recorded
    // output stamp rather than fabricating anything.
    assert!(out.contains("run `r-2`"), "{out}");
    assert!(out.contains("attempt 2)"), "{out}");
}

#[tokio::test]
async fn read_task_errors_on_an_unknown_id_instead_of_fabricating_a_card() {
    let dir = tempfile::tempdir().unwrap();
    let tasks: Arc<dyn TaskStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    let tool = ReadTaskTool::new(CompanyId::new("acme"), Some(tasks), None, None);
    let result = tool.execute(json!({ "task_id": "nope" })).await.unwrap();
    assert!(result.is_error, "an unknown task_id must error");
    let text = result.output_for_llm(true);
    assert!(text.contains("nope"), "{text}");
    assert!(text.contains("list_tasks"), "{text}");
}

/// AUTH-axis (HT-072): `read_task` is company-scoped only through
/// `tasks.list(&self.company)` — a real backend (here `FsOps`, the
/// production store, not a hand-rolled fake) partitions its data by
/// company on disk, so a `task_id` that exists but is filed under a
/// DIFFERENT company must read as not-found, never leak.
#[tokio::test]
async fn read_task_never_leaks_a_task_id_belonging_to_another_company() {
    let dir = tempfile::tempdir().unwrap();
    let tasks: Arc<dyn TaskStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    tasks
        .upsert(
            &CompanyId::new("beta"),
            &task_card(
                "t-secret",
                "beta's confidential rollout plan",
                crate::ports::tasks::COLUMN_TODO,
                "",
            ),
        )
        .await
        .unwrap();

    let tool = ReadTaskTool::new(CompanyId::new("acme"), Some(tasks), None, None);
    let result = tool
        .execute(json!({ "task_id": "t-secret" }))
        .await
        .unwrap();
    assert!(
        result.is_error,
        "acme asking about beta's task_id must read as not-found: {}",
        result.text()
    );
    let text = result.output_for_llm(true);
    assert!(
        !text.contains("confidential rollout"),
        "must not render beta's card: {text}"
    );
}

/// Fail-closed by construction (issue #1859's approved redaction posture):
/// `read_task` never reads [`RunRecord::usage`], so a run's USD cost cannot
/// reach its rendering no matter what that run cost.
#[tokio::test]
async fn read_task_never_renders_a_runs_usd_cost() {
    let dir = tempfile::tempdir().unwrap();
    let fs = Arc::new(crate::store::FsOps::new(dir.path()));
    let tasks: Arc<dyn TaskStore> = fs.clone();
    let runs: Arc<dyn RunStore> = fs;
    let company = CompanyId::new("acme");
    tasks
        .upsert(
            &company,
            &task_card(
                "t-1",
                "Send the invoice",
                crate::ports::tasks::COLUMN_IN_PROGRESS,
                "finance",
            ),
        )
        .await
        .unwrap();
    let mut run = runs
        .create_run(
            &company,
            crate::ports::runs::NewRun::for_task("r-1", "t-1", "finance"),
        )
        .await
        .unwrap();
    run.status = RunStatus::Succeeded;
    run.usage = crate::ports::types::TokenUsage {
        input: 500,
        output: 200,
        cached_input: 0,
        cost_usd: 4.20,
    };
    runs.put_run(&company, &run).await.unwrap();

    let tool = ReadTaskTool::new(company, Some(tasks), Some(runs), None);
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(
        !out.contains("4.2") && !out.to_lowercase().contains("cost") && !out.contains("usd"),
        "a run's USD cost must never reach read_task: {out}"
    );
}

#[tokio::test]
async fn read_run_reads_an_agent_attempt_row() {
    let dir = tempfile::tempdir().unwrap();
    let runs: Arc<dyn RunStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    let company = CompanyId::new("acme");
    let mut run = runs
        .create_run(
            &company,
            crate::ports::runs::NewRun::for_task("r-1", "t-1", "engineer"),
        )
        .await
        .unwrap();
    run.status = RunStatus::Failed;
    run.error = Some("connection refused".to_string());
    runs.put_run(&company, &run).await.unwrap();

    let tool = ReadRunTool::new(company, Some(runs), None);
    let out = tool
        .execute(json!({ "run_id": "r-1" }))
        .await
        .unwrap()
        .output_for_llm(true);
    assert!(out.contains("failed"), "{out}");
    assert!(out.contains("connection refused"), "{out}");
    assert!(out.contains("t-1"), "{out}");
}

/// The dual-source lookup's second half: no [`RunStore`] row named
/// `run_id`, so `read_run` folds it out of the journal via
/// [`crate::server::ops::workflows::fold_run_events`] instead — the same
/// fold the console's run-history route reads.
#[tokio::test]
async fn read_run_folds_a_workflow_run_out_of_the_journal_when_no_attempt_row_exists() {
    use crate::ports::types::{StoredEvent, WorkflowNodeStatus};
    use futures::stream::{self, BoxStream};

    struct FixedLog(Vec<StoredEvent>);

    #[async_trait]
    impl EventLog for FixedLog {
        async fn append(&self, _id: &CompanyId, _event: CompanyEvent) -> crate::Result<EventSeq> {
            unreachable!("read_run only reads")
        }
        async fn read_from(
            &self,
            _id: &CompanyId,
            seq: EventSeq,
            limit: usize,
        ) -> crate::Result<Vec<StoredEvent>> {
            Ok(self
                .0
                .iter()
                .filter(|e| e.seq.value() >= seq.value())
                .take(limit)
                .cloned()
                .collect())
        }
        fn subscribe(
            &self,
            _id: &CompanyId,
        ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
            Box::pin(stream::empty())
        }
    }

    let company = CompanyId::new("acme");
    let history = vec![
        StoredEvent {
            seq: EventSeq::new(0),
            company: company.clone(),
            event: CompanyEvent::WorkflowRunStarted {
                workflow_id: "demo".to_string(),
                run_id: "wf-run-1".to_string(),
                scheduled: false,
                started_by: None,
                resume_semantic: None,
            },
            at_millis: 1,
        },
        StoredEvent {
            seq: EventSeq::new(1),
            company: company.clone(),
            event: CompanyEvent::WorkflowNodeFinished {
                workflow_id: "demo".to_string(),
                run_id: "wf-run-1".to_string(),
                node_id: "fetch".to_string(),
                status: WorkflowNodeStatus::Ok,
                elapsed_ms: 10,
                diagnostics: Vec::new(),
                agent_run_id: None,
            },
            at_millis: 2,
        },
        StoredEvent {
            seq: EventSeq::new(2),
            company: company.clone(),
            event: CompanyEvent::WorkflowRunFinished {
                workflow_id: "demo".to_string(),
                scheduled: false,
                run_id: Some("wf-run-1".to_string()),
                deliveries: Vec::new(),
                pending_approvals: vec!["gate-1".to_string()],
                error: None,
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                blocked_nodes: Vec::new(),
                approvals: Vec::new(),
            },
            at_millis: 3,
        },
    ];
    let events: Arc<dyn EventLog> = Arc::new(FixedLog(history));

    let tool = ReadRunTool::new(company, None, Some(events));
    let out = tool
        .execute(json!({ "run_id": "wf-run-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(out.contains("demo"), "{out}");
    assert!(out.contains("fetch"), "{out}");
    assert!(out.contains("1 pending approval"), "{out}");
    // Summarized, never dumped: no step trace, no node output/argument text
    // rides this fold in the first place (see `WorkflowNodeFinished`'s own
    // doc comment), so there is nothing here to assert absent beyond what
    // the fixture itself never supplied.
}

#[tokio::test]
async fn read_run_errors_on_an_id_that_is_neither_an_attempt_nor_a_workflow_run() {
    let dir = tempfile::tempdir().unwrap();
    let runs: Arc<dyn RunStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    let tool = ReadRunTool::new(CompanyId::new("acme"), Some(runs), None);
    let result = tool.execute(json!({ "run_id": "nope" })).await.unwrap();
    assert!(result.is_error, "an unknown run_id must error");
    assert!(result.output_for_llm(true).contains("nope"));
}

#[tokio::test]
async fn query_company_board_section_groups_open_cards_by_column_and_omits_done() {
    let dir = tempfile::tempdir().unwrap();
    let tasks: Arc<dyn TaskStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    let company = CompanyId::new("acme");
    tasks
        .upsert(
            &company,
            &task_card(
                "t-1",
                "Draft the memo",
                crate::ports::tasks::COLUMN_TODO,
                "maya",
            ),
        )
        .await
        .unwrap();
    tasks
        .upsert(
            &company,
            &task_card("t-2", "Ship the release", COLUMN_DONE, "maya"),
        )
        .await
        .unwrap();

    let tool = QueryCompanyTool::new(company, None, None, None, None, Some(tasks));
    let out = tool.execute(json!({})).await.unwrap().output_for_llm(true);

    assert!(out.contains("## Board"), "{out}");
    assert!(out.contains("Draft the memo"), "{out}");
    assert!(
        !out.contains("Ship the release"),
        "the Board section must exclude Done, like `list_tasks`: {out}"
    );
    // Desks stays present AND after Board never gets to run — Board is the
    // LAST section, so this just pins Desks is still there at all.
    assert!(out.contains("## Desks"), "{out}");
}

#[tokio::test]
async fn query_company_board_section_is_unavailable_when_the_board_is_unwired() {
    let tool = QueryCompanyTool::new(CompanyId::new("acme"), None, None, None, None, None);
    let out = tool.execute(json!({})).await.unwrap().output_for_llm(true);
    assert!(out.contains("## Board"), "{out}");
    assert!(out.contains("Board unavailable"), "{out}");
}

/// The ordering guarantee the byte-budget reasoning depends on: Board is
/// the LAST section, so a company with an oversized board can never push
/// the Desks list — which `delegate_to_desk` needs to ground a hand-off —
/// out of the tool result ahead of it.
#[tokio::test]
async fn query_company_desks_section_still_renders_after_the_board_section() {
    let tool = QueryCompanyTool::new(CompanyId::new("acme"), None, None, None, None, None);
    let out = tool.execute(json!({})).await.unwrap().output_for_llm(true);
    let desks_at = out.find("## Desks").expect("Desks section present");
    let board_at = out.find("## Board").expect("Board section present");
    assert!(
        board_at > desks_at,
        "Board must render after Desks, never before: {out}"
    );
}

#[tokio::test]
async fn list_tasks_reports_a_read_failure_instead_of_an_empty_board() {
    let tasks: Arc<dyn TaskStore> = Arc::new(BrokenTaskStore);
    let tool = ListTasksTool::new(CompanyId::new("acme"), Some(tasks), None);
    let result = tool.execute(json!({})).await.unwrap();
    assert!(
        result.is_error,
        "a board read failure must be a refusal, not a silently empty board"
    );
    let text = result.output_for_llm(true);
    assert!(
        !text.contains("No matching cards"),
        "must not claim the board is simply empty: {text}"
    );
    assert!(text.contains("Couldn't read the task board"), "{text}");
}

#[tokio::test]
async fn read_task_reports_a_read_failure_instead_of_a_missing_card() {
    let tasks: Arc<dyn TaskStore> = Arc::new(BrokenTaskStore);
    let tool = ReadTaskTool::new(CompanyId::new("acme"), Some(tasks), None, None);
    let result = tool.execute(json!({ "task_id": "t-1" })).await.unwrap();
    assert!(
        result.is_error,
        "a board read failure must be a refusal, not a fabricated missing-card error"
    );
    let text = result.output_for_llm(true);
    assert!(
        !text.contains("No card `t-1`"),
        "must not claim the card doesn't exist when the board couldn't be read: {text}"
    );
    assert!(text.contains("Couldn't read the task board"), "{text}");
}
