use super::*;

#[tokio::test]
async fn query_company_board_section_reports_unavailable_on_a_read_failure_not_empty() {
    let tasks: Arc<dyn TaskStore> = Arc::new(BrokenTaskStore);
    let tool = QueryCompanyTool::new(CompanyId::new("acme"), None, None, None, None, Some(tasks));
    let result = tool.execute(json!({})).await.unwrap();
    assert!(!result.is_error, "the whole tool must still answer");
    let text = result.output_for_llm(true);
    assert!(
        !text.contains("No open cards"),
        "must not claim the board is empty when it could not be read: {text}"
    );
    assert!(text.contains("Board unavailable"), "{text}");

    let payload = match &result.content[0] {
        openhuman_core::skills::types::ToolContent::Json { data } => data.clone(),
        other => panic!("expected a JSON content block, got {other:?}"),
    };
    assert_eq!(
        payload["board_open"], 0,
        "board_open must stay at zero on a read failure, not report a fabricated count: \
         {payload}"
    );
}

#[tokio::test]
async fn read_task_falls_back_to_the_output_stamp_when_the_artifact_store_is_wired_but_empty() {
    let dir = tempfile::tempdir().unwrap();
    let fs = Arc::new(crate::store::FsOps::new(dir.path()));
    let tasks: Arc<dyn TaskStore> = fs.clone();
    let artifacts: Arc<dyn ArtifactStore> = fs;
    let company = CompanyId::new("acme");

    let mut card = task_card(
        "t-1",
        "Reply to the customer",
        crate::ports::tasks::COLUMN_IN_REVIEW,
        "engineer",
    );
    card.output = Some(TaskOutput {
        source: crate::ports::tasks::TaskOutputSource::Run {
            run_id: "r-9".to_string(),
            attempt: Some(3),
        },
        at_millis: 5,
        artifacts: Vec::new(),
        workflows: Vec::new(),
    });
    tasks.upsert(&company, &card).await.unwrap();

    let tool = ReadTaskTool::new(company, Some(tasks), None, Some(artifacts));
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(
        out.contains("run `r-9`"),
        "an artifact store wired but empty must still surface the card's own output \
         stamp instead of claiming nothing published: {out}"
    );
    assert!(out.contains("attempt 3)"), "{out}");
    assert!(
        !out.contains("Nothing published yet"),
        "must not claim nothing happened when the card recorded an attempt: {out}"
    );
}

#[tokio::test]
async fn read_task_reports_run_history_unavailable_instead_of_no_attempts_on_a_read_failure() {
    let dir = tempfile::tempdir().unwrap();
    let tasks: Arc<dyn TaskStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    let company = CompanyId::new("acme");
    tasks
        .upsert(
            &company,
            &task_card(
                "t-1",
                "Investigate the outage",
                crate::ports::tasks::COLUMN_IN_REVIEW,
                "engineer",
            ),
        )
        .await
        .unwrap();

    let runs: Arc<dyn RunStore> = Arc::new(BrokenRunStore);
    let tool = ReadTaskTool::new(company, Some(tasks), Some(runs), None);
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(
        !out.contains("No attempts yet"),
        "a run-history read failure must not look like a card nobody attempted: {out}"
    );
    assert!(out.contains("Run history unavailable"), "{out}");
}

#[tokio::test]
async fn list_tasks_reports_attempt_status_unavailable_on_a_run_history_read_failure() {
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

    let runs: Arc<dyn RunStore> = Arc::new(BrokenRunStore);
    let tool = ListTasksTool::new(company, Some(tasks), Some(runs));
    let out = tool.execute(json!({})).await.unwrap().output_for_llm(true);

    assert!(
        out.contains("attempt status unavailable"),
        "a per-card run-history read failure must not render identically to a card with \
         no attempt clause at all: {out}"
    );
}

#[tokio::test]
async fn read_run_reports_a_run_store_failure_instead_of_a_missing_run() {
    let runs: Arc<dyn RunStore> = Arc::new(FailingGetRun);
    let tool = ReadRunTool::new(CompanyId::new("acme"), Some(runs), None);
    let result = tool.execute(json!({ "run_id": "r-1" })).await.unwrap();
    assert!(
        result.is_error,
        "a run-store read failure must be a refusal, not a fabricated miss"
    );
    let text = result.output_for_llm(true);
    assert!(
        !text.contains("No run"),
        "must not claim the run doesn't exist when the run store couldn't be read: {text}"
    );
}

#[tokio::test]
async fn read_run_reports_an_event_log_failure_instead_of_a_missing_run() {
    let events: Arc<dyn EventLog> = Arc::new(BrokenEventLog);
    let tool = ReadRunTool::new(CompanyId::new("acme"), None, Some(events));
    let result = tool.execute(json!({ "run_id": "wf-1" })).await.unwrap();
    assert!(
        result.is_error,
        "an event-log read failure must be a refusal, not a fabricated miss"
    );
    let text = result.output_for_llm(true);
    assert!(
        !text.contains("not an agent attempt and not a workflow run"),
        "must not claim the run doesn't exist when the event log couldn't be read: {text}"
    );
}

#[tokio::test]
async fn read_task_reports_output_unavailable_on_an_artifact_read_failure() {
    let dir = tempfile::tempdir().unwrap();
    let tasks: Arc<dyn TaskStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    let company = CompanyId::new("acme");
    tasks
        .upsert(
            &company,
            &task_card(
                "t-1",
                "Reply to the customer",
                crate::ports::tasks::COLUMN_IN_REVIEW,
                "engineer",
            ),
        )
        .await
        .unwrap();

    let artifacts: Arc<dyn ArtifactStore> = Arc::new(BrokenArtifactStore);
    let tool = ReadTaskTool::new(company, Some(tasks), None, Some(artifacts));
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(
        !out.contains("Nothing published yet"),
        "an artifact-store read failure must not look like a genuinely empty store: {out}"
    );
}

#[tokio::test]
async fn read_task_includes_each_attempts_run_id_so_read_run_is_reachable() {
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
                "Investigate the outage",
                crate::ports::tasks::COLUMN_IN_REVIEW,
                "engineer",
            ),
        )
        .await
        .unwrap();
    let mut run = runs
        .create_run(
            &company,
            crate::ports::runs::NewRun::for_task("r-1", "t-1", "engineer"),
        )
        .await
        .unwrap();
    run.status = RunStatus::Failed;
    run.error = Some("timed out".to_string());
    runs.put_run(&company, &run).await.unwrap();

    let tool = ReadTaskTool::new(company, Some(tasks), Some(runs), None);
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(
        out.contains("r-1"),
        "an attempt's run id must be discoverable from read_task, since read_run requires \
         it: {out}"
    );
}

#[tokio::test]
async fn read_task_bounds_rendered_attempts_so_output_cannot_be_pushed_out_of_budget() {
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
                "Flaky deploy",
                crate::ports::tasks::COLUMN_IN_REVIEW,
                "engineer",
            ),
        )
        .await
        .unwrap();
    for n in 1..=(READ_TASK_ATTEMPTS_LIMIT + 3) {
        let mut run = runs
            .create_run(
                &company,
                crate::ports::runs::NewRun::for_task(format!("r-{n}"), "t-1", "engineer"),
            )
            .await
            .unwrap();
        run.status = RunStatus::Failed;
        run.error = Some("boom".to_string());
        runs.put_run(&company, &run).await.unwrap();
    }

    let tool = ReadTaskTool::new(company, Some(tasks), Some(runs), None);
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(
        out.contains("3 earlier attempt(s) omitted"),
        "must report how many older attempts were cut: {out}"
    );
    let output_at = out.find("## Output").expect("Output section present");
    let attempts_at = out.find("## Attempts").expect("Attempts section present");
    assert!(
        output_at > attempts_at,
        "the Output section must still be reachable after a long attempt history: {out}"
    );
}

/// LIMIT-axis (HT-072): `ReadTaskTool::description()` used to promise the
/// model "every attempt's status" while the render silently truncates to
/// `READ_TASK_ATTEMPTS_LIMIT` rows — a false completeness claim the model
/// reads before ever calling the tool, independent of the honest
/// `_N earlier attempt(s) omitted_` notice the render itself carries (see
/// `read_task_bounds_rendered_attempts_...` above). The description must
/// name the same cap the render enforces, and the two are pinned against
/// the same literal so a change to one is forced to touch the other
/// instead of silently drifting out of step the way they did to get here.
#[test]
fn read_task_description_names_the_same_cap_the_render_enforces() {
    let tool = ReadTaskTool::new(CompanyId::new("acme"), None, None, None);
    assert!(
        !tool.description().contains("every attempt's status"),
        "the description must not promise completeness the render does not keep: {}",
        tool.description()
    );
    assert!(
        tool.description().contains("10 most recent"),
        "the description should name the cap the render enforces: {}",
        tool.description()
    );
    assert_eq!(
        READ_TASK_ATTEMPTS_LIMIT, 10,
        "pinned against the same literal the description names, so a cap change cannot \
         drift silently out of step with the sentence the model reads"
    );
}

#[tokio::test]
async fn read_task_bounds_the_rendered_title_so_attempts_and_output_stay_reachable() {
    let dir = tempfile::tempdir().unwrap();
    let tasks: Arc<dyn TaskStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    let company = CompanyId::new("acme");
    let long_title = "x".repeat(5_000);
    tasks
        .upsert(
            &company,
            &task_card(
                "t-1",
                &long_title,
                crate::ports::tasks::COLUMN_IN_REVIEW,
                "engineer",
            ),
        )
        .await
        .unwrap();

    let tool = ReadTaskTool::new(company, Some(tasks), None, None);
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    let header_line = out.lines().next().expect("header line present");
    assert!(
        header_line.chars().count() <= READ_TASK_TITLE_LIMIT + 2,
        "an operator-pasted title must not render verbatim and unbounded, or it can \
         consume the whole tool-result budget before later sections: {} chars",
        header_line.chars().count()
    );
    let output_at = out.find("## Output").expect("Output section present");
    let attempts_at = out.find("## Attempts").expect("Attempts section present");
    assert!(
        output_at > attempts_at,
        "the Output section must stay reachable behind a very long card title: {} bytes total",
        out.len()
    );
}

#[tokio::test]
async fn read_task_resolves_the_pinned_artifact_version_not_a_later_operator_edit() {
    let dir = tempfile::tempdir().unwrap();
    let fs = Arc::new(crate::store::FsOps::new(dir.path()));
    let tasks: Arc<dyn TaskStore> = fs.clone();
    let artifacts: Arc<dyn ArtifactStore> = fs;
    let company = CompanyId::new("acme");

    let mut card = task_card(
        "t-1",
        "Draft the memo",
        crate::ports::tasks::COLUMN_IN_REVIEW,
        "engineer",
    );
    card.output = Some(TaskOutput {
        source: crate::ports::tasks::TaskOutputSource::Run {
            run_id: "r-1".to_string(),
            attempt: Some(1),
        },
        at_millis: 5,
        artifacts: vec![crate::ports::tasks::TaskOutputArtifact {
            artifact_id: "art-1".to_string(),
            version: 1,
            title: "Memo".to_string(),
            kind: crate::ports::artifacts::ArtifactKind::Markdown,
        }],
        workflows: Vec::new(),
    });
    tasks.upsert(&company, &card).await.unwrap();

    let mut record = crate::ports::artifacts::ArtifactRecord::new(
        "art-1",
        "t-1",
        "Memo",
        crate::ports::artifacts::ArtifactKind::Markdown,
        "the agent's draft body",
        "engineer",
        5,
    );
    record.push_version(
        "an operator edited this after the attempt settled",
        crate::ports::artifacts::ArtifactAuthor::Operator,
        "operator",
        10,
        None,
    );
    artifacts.upsert(&company, &record).await.unwrap();

    let tool = ReadTaskTool::new(company, Some(tasks), None, Some(artifacts));
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(
        out.contains("the agent's draft body"),
        "must render the version the task's output pinned, not the latest: {out}"
    );
    assert!(
        !out.contains("an operator edited this"),
        "a later operator edit must not render as what the task produced: {out}"
    );
}

#[tokio::test]
async fn read_task_only_renders_artifacts_pinned_by_the_current_output_stamp() {
    let dir = tempfile::tempdir().unwrap();
    let fs = Arc::new(crate::store::FsOps::new(dir.path()));
    let tasks: Arc<dyn TaskStore> = fs.clone();
    let artifacts: Arc<dyn ArtifactStore> = fs;
    let company = CompanyId::new("acme");

    let mut card = task_card(
        "t-1",
        "Draft the memo",
        crate::ports::tasks::COLUMN_IN_REVIEW,
        "engineer",
    );
    card.output = Some(TaskOutput {
        source: crate::ports::tasks::TaskOutputSource::Run {
            run_id: "r-2".to_string(),
            attempt: Some(2),
        },
        at_millis: 10,
        artifacts: vec![crate::ports::tasks::TaskOutputArtifact {
            artifact_id: "art-b".to_string(),
            version: 1,
            title: "Follow-up".to_string(),
            kind: crate::ports::artifacts::ArtifactKind::Markdown,
        }],
        workflows: Vec::new(),
    });
    tasks.upsert(&company, &card).await.unwrap();

    let record_a = crate::ports::artifacts::ArtifactRecord::new(
        "art-a",
        "t-1",
        "First draft",
        crate::ports::artifacts::ArtifactKind::Markdown,
        "attempt 1's body — superseded, no longer part of the latest output",
        "engineer",
        5,
    );
    artifacts.upsert(&company, &record_a).await.unwrap();
    let record_b = crate::ports::artifacts::ArtifactRecord::new(
        "art-b",
        "t-1",
        "Follow-up",
        crate::ports::artifacts::ArtifactKind::Markdown,
        "attempt 2's body",
        "engineer",
        10,
    );
    artifacts.upsert(&company, &record_b).await.unwrap();

    let tool = ReadTaskTool::new(company, Some(tasks), None, Some(artifacts));
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(
        out.contains("attempt 2's body"),
        "the artifact pinned by the current output stamp must render: {out}"
    );
    assert!(
        !out.contains("First draft") && !out.contains("superseded"),
        "an artifact from an earlier attempt that the current output stamp does not pin \
         must not render as part of the latest output: {out}"
    );
}

#[tokio::test]
async fn read_task_treats_an_empty_output_stamp_as_the_latest_attempt_publishing_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let fs = Arc::new(crate::store::FsOps::new(dir.path()));
    let tasks: Arc<dyn TaskStore> = fs.clone();
    let artifacts: Arc<dyn ArtifactStore> = fs;
    let company = CompanyId::new("acme");

    let mut card = task_card(
        "t-1",
        "Draft the memo",
        crate::ports::tasks::COLUMN_IN_REVIEW,
        "engineer",
    );
    card.output = Some(TaskOutput {
        source: crate::ports::tasks::TaskOutputSource::Run {
            run_id: "r-2".to_string(),
            attempt: Some(2),
        },
        at_millis: 10,
        artifacts: Vec::new(),
        workflows: Vec::new(),
    });
    tasks.upsert(&company, &card).await.unwrap();

    let record_a = crate::ports::artifacts::ArtifactRecord::new(
        "art-a",
        "t-1",
        "First draft",
        crate::ports::artifacts::ArtifactKind::Markdown,
        "attempt 1's body — attempt 2 published nothing",
        "engineer",
        5,
    );
    artifacts.upsert(&company, &record_a).await.unwrap();

    let tool = ReadTaskTool::new(company, Some(tasks), None, Some(artifacts));
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(
        !out.contains("First draft") && !out.contains("attempt 1's body"),
        "an earlier attempt's artifact must not render as the latest attempt's output when \
         the current output stamp pins an empty (non-absent) artifact list: {out}"
    );
    assert!(
        out.contains("No artifacts published"),
        "an empty-but-present output stamp must render as the latest attempt publishing \
         nothing, not fall through to the all-artifacts legacy fallback: {out}"
    );
}

#[tokio::test]
async fn read_task_renders_workflows_recorded_in_the_output() {
    let dir = tempfile::tempdir().unwrap();
    let tasks: Arc<dyn TaskStore> = Arc::new(crate::store::FsOps::new(dir.path()));
    let company = CompanyId::new("acme");

    let mut card = task_card(
        "t-1",
        "Automate the weekly report",
        crate::ports::tasks::COLUMN_IN_REVIEW,
        "orchestrator",
    );
    card.output = Some(TaskOutput {
        source: crate::ports::tasks::TaskOutputSource::Run {
            run_id: "r-1".to_string(),
            attempt: Some(1),
        },
        at_millis: 5,
        artifacts: Vec::new(),
        workflows: vec![TaskOutputWorkflow {
            workflow_id: "wf-weekly-report".to_string(),
            run_id: Some("wf-run-1".to_string()),
            action: TaskOutputAction::Ran,
        }],
    });
    tasks.upsert(&company, &card).await.unwrap();

    let tool = ReadTaskTool::new(company, Some(tasks), None, None);
    let out = tool
        .execute(json!({ "task_id": "t-1" }))
        .await
        .unwrap()
        .output_for_llm(true);

    assert!(out.contains("### Workflows"), "{out}");
    assert!(out.contains("wf-weekly-report"), "{out}");
    assert!(
        out.contains("wf-run-1"),
        "the workflow's run id must be surfaced for read_run: {out}"
    );
}

/// FAIL-axis (HT-073): `ReadRunTool` is company-scoped only by
/// construction — `self.company` is the sole company argument it ever
/// passes to the run store, never anything derived from the `run_id`
/// argument. This pins that structural argument against a store that
/// genuinely partitions by company: a `run_id` that exists, but filed
/// under a DIFFERENT company, must read as not found, never leak.
#[tokio::test]
async fn read_run_never_leaks_a_run_id_belonging_to_another_company() {
    let runs: Arc<dyn RunStore> = Arc::new(TenantScopedRunStore {
        rows: std::sync::Mutex::new(vec![tenant_run("beta", "r-secret")]),
    });
    let tool = ReadRunTool::new(CompanyId::new("acme"), Some(runs), None);
    let out = tool
        .execute(json!({ "run_id": "r-secret" }))
        .await
        .unwrap()
        .output_for_llm(true);
    assert!(
        out.contains("No run"),
        "acme asking about beta's run_id must read as not-found: {out}"
    );
    assert!(
        !out.contains("Attempt"),
        "must not render beta's run: {out}"
    );
}
