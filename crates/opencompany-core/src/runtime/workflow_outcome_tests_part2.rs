use super::tests_core::*;

/// A cancelled run carries `error: None`, so it clears the ledger — and that
/// is correct: a cancelled run returns before `deliver_outputs`, so it
/// dispatched nothing here, and clearing simply resets to owed-nothing.
#[tokio::test]
async fn a_cancelled_finish_clears_the_ledger() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    // A prior crashed run stranded a delivery…
    start(&events, &company, "run-1", true).await;
    deliver(
        &events,
        &company,
        "digest",
        "run-1",
        "owner_summary",
        "owner",
    )
    .await;
    // …then a later run of the same workflow was stopped by an operator.
    let mut cancelled = run_with(Vec::new(), Vec::new());
    cancelled.cancelled = true;
    record_run_finished(&events, &company, "digest", true, "run-2", Ok(&cancelled)).await;

    assert!(
        stranded(&events, &company, "digest").await.is_empty(),
        "a cancelled run has no error, so it clears like any clean finish"
    );
}

/// The multi-crash union: run 1 sends A then crashes, run 2 skips A, sends B,
/// then crashes. The ledger accumulates {A, B} across both unclean runs.
#[tokio::test]
async fn deliveries_union_across_multiple_crashes() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    start(&events, &company, "run-1", true).await;
    deliver(&events, &company, "digest", "run-1", "a", "owner").await;
    record_run_finished(
        &events,
        &company,
        "digest",
        true,
        "run-1",
        Err("crash 1".into()),
    )
    .await;

    start(&events, &company, "run-2", true).await;
    // run 2 skipped A (its own already_delivered saw it) and sent B.
    deliver(&events, &company, "digest", "run-2", "b", "owner").await;
    record_run_finished(
        &events,
        &company,
        "digest",
        true,
        "run-2",
        Err("crash 2".into()),
    )
    .await;

    assert_eq!(
        stranded(&events, &company, "digest").await,
        vec!["a".to_string(), "b".to_string()],
        "two unclean runs accumulate rather than the second forgetting the first"
    );
}

/// The same node delivered twice (an `owner` fan-out writes one line per
/// admin) collapses to a single ledger entry, keyed on the node.
#[tokio::test]
async fn a_fanned_out_node_collapses_to_one_entry() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    start(&events, &company, "run-1", true).await;
    deliver(
        &events,
        &company,
        "digest",
        "run-1",
        "owner_summary",
        "owner",
    )
    .await;
    deliver(
        &events,
        &company,
        "digest",
        "run-1",
        "owner_summary",
        "owner",
    )
    .await;

    assert_eq!(
        stranded(&events, &company, "digest").await,
        vec!["owner_summary".to_string()],
        "per-recipient lines dedupe to one node in the ledger"
    );
}

/// The fold is per-workflow: a crash in workflow A does not strand a
/// delivery against workflow B, and a clean finish of A does not clear B.
#[tokio::test]
async fn the_fold_is_isolated_per_workflow() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    deliver(&events, &company, "digest", "run-1", "a", "owner").await;
    deliver(&events, &company, "weekly", "run-2", "b", "owner").await;
    // A clean finish of `digest` clears only `digest`.
    record_run_finished(
        &events,
        &company,
        "digest",
        true,
        "run-1",
        Ok(&run_with(Vec::new(), Vec::new())),
    )
    .await;

    assert!(
        stranded(&events, &company, "digest").await.is_empty(),
        "digest's own clean finish cleared it"
    );
    assert_eq!(
        stranded(&events, &company, "weekly").await,
        vec!["b".to_string()],
        "weekly's stranded delivery is untouched by digest's finish"
    );
}

/// Events the fold does not care about — starts, node finishes, unrelated
/// records — are ignored rather than disturbing the ledger.
#[tokio::test]
async fn unrelated_events_are_ignored() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    start(&events, &company, "run-1", true).await;
    events
        .append(
            &company,
            CompanyEvent::WorkflowNodeFinished {
                workflow_id: "digest".to_string(),
                run_id: "run-1".to_string(),
                node_id: "ceo".to_string(),
                status: crate::ports::types::WorkflowNodeStatus::Ok,
                elapsed_ms: 3,
                diagnostics: Vec::new(),
                agent_run_id: None,
            },
        )
        .await
        .expect("append");
    deliver(
        &events,
        &company,
        "digest",
        "run-1",
        "owner_summary",
        "owner",
    )
    .await;

    assert_eq!(
        stranded(&events, &company, "digest").await,
        vec!["owner_summary".to_string()],
        "only delivered/finished events for this workflow move the ledger"
    );
}

/// An empty or delivery-free journal folds to nothing rather than erroring.
#[tokio::test]
async fn an_empty_journal_folds_to_nothing() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    assert!(stranded(&events, &company, "digest").await.is_empty());
}

/// Issue #1009 (path B): a finish whose append is swallowed reports
/// `journaled == false` rather than the silent `()` it used to. The run is
/// otherwise unaffected — `record_run_finished` must not panic and must not
/// propagate the append error — and a working log reports `true`, so the
/// signal the caller escalates on is real both ways.
#[tokio::test]
async fn a_swallowed_append_reports_it_was_not_journaled() {
    let company = CompanyId::new("acme");
    let run = run_with(Vec::new(), Vec::new());

    let failing: Arc<dyn EventLog> = Arc::new(FailingAppendLog);
    let journaled =
        record_run_finished(&failing, &company, "digest", false, "run-1", Ok(&run)).await;
    assert!(
        !journaled,
        "a swallowed append reports the finish did not land"
    );

    let (_home, working) = log();
    let journaled =
        record_run_finished(&working, &company, "digest", false, "run-1", Ok(&run)).await;
    assert!(journaled, "a working log reports the finish landed");
}
