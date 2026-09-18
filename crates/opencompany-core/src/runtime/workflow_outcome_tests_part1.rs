use super::tests_core::*;

/// Issue #638: a run's notices survive onto the journaled outcome, which is
/// the row the console's history panel reads.
///
/// The point of the assertion is the **round trip**, not the assignment: the
/// event is written to a real JSONL log and read back, so a field that
/// serialized but did not deserialize — or one `skip_serializing_if` dropped
/// on the way out — fails here rather than in front of an operator.
#[tokio::test]
async fn a_runs_notices_reach_the_journaled_outcome() {
    let (_dir, events) = log();
    let company = CompanyId::new("acme");
    let notice = "Heads up: 3 further gated tool calls were not raised for approval.";
    let run = WorkflowRun {
        notices: vec![notice.to_string()],
        ..run_with(Vec::new(), Vec::new())
    };

    record_run_finished(&events, &company, "wf", false, "run-1", Ok(&run)).await;

    let journaled = journaled(&events, &company).await;
    let CompanyEvent::WorkflowRunFinished { notices, error, .. } = journaled
        .iter()
        .find(|e| matches!(e, CompanyEvent::WorkflowRunFinished { .. }))
        .expect("the outcome was journaled")
    else {
        unreachable!("matched above")
    };
    assert_eq!(notices, &vec![notice.to_string()]);
    assert!(
        error.is_none(),
        "a run that raised a notice did not fail — putting this in `error` \
         would inflate the failure count and hide a real failure among them",
    );
}

/// The other direction, and the one that keeps the field honest: a run with
/// nothing to say carries an empty list, and a failure the caller knows
/// nothing else about carries none either — rather than inheriting whatever
/// the Ok arm would have had.
///
/// Since issue #1008 that second clause is conditional on the *caller*: a
/// failure that arrives with a partial run does carry its notices, which
/// `a_run_that_delivered_and_then_failed_keeps_its_rows` pins. This one pins
/// the arm where there is genuinely nothing to read.
#[tokio::test]
async fn an_ordinary_run_and_a_failure_with_nothing_known_carry_no_notices() {
    let (_dir, events) = log();
    let company = CompanyId::new("acme");

    let ok = run_with(Vec::new(), Vec::new());
    record_run_finished(&events, &company, "wf", false, "run-ok", Ok(&ok)).await;
    record_run_finished(
        &events,
        &company,
        "wf",
        false,
        "run-bad",
        Err("boom".into()),
    )
    .await;

    for event in journaled(&events, &company).await {
        let CompanyEvent::WorkflowRunFinished { notices, .. } = event else {
            continue;
        };
        assert!(notices.is_empty(), "nothing to say means an empty list");
    }
}

/// Issue #1008's second half: a run that **delivered two reports and then
/// failed** still lists those deliveries.
///
/// This is the arm that was hard-coded empty. The reports were already in
/// the recipients' inboxes by the time the later node broke, so journaling
/// zero delivery rows did not describe a run that sent nothing — it
/// described a run whose record disagreed with the world. The same holds for
/// the board card it opened, the approval it parked and the notice it
/// raised, so all four ride the failure arm together.
#[tokio::test]
async fn a_run_that_delivered_and_then_failed_keeps_its_rows() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");

    let partial = WorkflowRun {
        notices: vec!["a call was refused".to_string()],
        board: vec![crate::ports::WorkflowRunBoardRow {
            action: crate::ports::WorkflowBoardAction::Spawned,
            task_id: Some("task-7".to_string()),
            title: Some("Draft the summary".to_string()),
            assignee: None,
        }],
        approvals: vec![crate::ports::WorkflowRunApprovalRow {
            node_id: Some("work".to_string()),
            tool: Some("shell".to_string()),
            outcome: crate::ports::WorkflowApprovalOutcome::Parked,
            approval_id: Some("appr-1".to_string()),
        }],
        ..run_with(
            vec![
                report("owner-summary", DeliveryStatus::Sent),
                report("board-summary", DeliveryStatus::Sent),
            ],
            Vec::new(),
        )
    };

    record_run_finished(
        &events,
        &company,
        "digest",
        false,
        "run-bad",
        Err(FailedRun {
            error: "the third node died",
            partial: Some(&partial),
        }),
    )
    .await;

    let CompanyEvent::WorkflowRunFinished {
        deliveries,
        error,
        board,
        approvals,
        notices,
        pending_approvals,
        ..
    } = journaled(&events, &company)
        .await
        .into_iter()
        .find(|e| matches!(e, CompanyEvent::WorkflowRunFinished { .. }))
        .expect("the failure was journaled")
    else {
        unreachable!("matched above")
    };

    assert_eq!(
        deliveries.len(),
        2,
        "two reports were sent before the run broke; zeroing them tells the operator \
         nothing went out: {deliveries:?}"
    );
    assert_eq!(
        error.as_deref(),
        Some("the third node died"),
        "carrying the rows must not stop this reading as a failure"
    );
    assert_eq!(board.len(), 1, "the card is on the board: {board:?}");
    assert_eq!(
        approvals.len(),
        1,
        "the card is on the Approvals page: {approvals:?}"
    );
    assert_eq!(notices, vec!["a call was refused".to_string()]);
    assert!(
        pending_approvals.is_empty(),
        "the one deliberate exception: a failed run is not waiting on anybody, however \
         many gates it parked on the way down"
    );
}

/// The other half of the same claim: a failure with **nothing known** about
/// what the run did still journals honestly empty rows rather than inventing
/// any. The boot sweep's synthetic finish is exactly this shape.
#[tokio::test]
async fn a_failure_with_no_partial_run_journals_empty_rows() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");

    record_run_finished(
        &events,
        &company,
        "digest",
        false,
        "run-bad",
        Err("boom".into()),
    )
    .await;

    let CompanyEvent::WorkflowRunFinished {
        deliveries,
        board,
        approvals,
        blocked_nodes,
        ..
    } = journaled(&events, &company)
        .await
        .into_iter()
        .find(|e| matches!(e, CompanyEvent::WorkflowRunFinished { .. }))
        .expect("the failure was journaled")
    else {
        unreachable!("matched above")
    };
    assert!(deliveries.is_empty());
    assert!(board.is_empty());
    assert!(approvals.is_empty());
    assert!(blocked_nodes.is_empty());
}

/// A completed run records its delivery rows and pending approvals verbatim
/// — the rows are the whole reason the record exists.
#[tokio::test]
async fn a_completed_run_records_its_rows_and_approvals() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    let run = run_with(
        vec![
            report("owner_summary", DeliveryStatus::Skipped),
            report("also_sent", DeliveryStatus::Sent),
        ],
        vec!["review".to_string()],
    );

    record_run_finished(&events, &company, "digest", true, "run-1", Ok(&run)).await;

    let events = journaled(&events, &company).await;
    assert_eq!(events.len(), 1);
    let CompanyEvent::WorkflowRunFinished {
        workflow_id,
        scheduled,
        deliveries,
        pending_approvals,
        error,
        ..
    } = &events[0]
    else {
        panic!("expected a WorkflowRunFinished, got {:?}", events[0]);
    };
    assert_eq!(workflow_id, "digest");
    assert!(*scheduled);
    assert_eq!(deliveries.len(), 2);
    assert_eq!(deliveries[0].node, "owner_summary");
    assert_eq!(deliveries[0].status, DeliveryStatus::Skipped);
    // The `detail` is the part that says what to fix, so it must survive.
    assert!(deliveries[0].detail.contains("never written"));
    assert_eq!(pending_approvals, &vec!["review".to_string()]);
    assert!(error.is_none(), "a completed run carries no error");
}

/// The arm that matters most: a run that failed outright is recorded, with
/// the reason. Before this it only warned to host stdout.
#[tokio::test]
async fn a_failed_run_records_the_error() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");

    record_run_finished(
        &events,
        &company,
        "digest",
        true,
        "run-1",
        Err("agent node `worker` had no inference source".into()),
    )
    .await;

    let events = journaled(&events, &company).await;
    let CompanyEvent::WorkflowRunFinished {
        deliveries,
        pending_approvals,
        error,
        ..
    } = &events[0]
    else {
        panic!("expected a WorkflowRunFinished");
    };
    assert!(deliveries.is_empty());
    assert!(pending_approvals.is_empty());
    assert_eq!(
        error.as_deref(),
        Some("agent node `worker` had no inference source")
    );
}

/// A manual run is recorded the same way, flagged as not scheduled — that
/// flag is what lets the console tell a cron run from a Run-button one.
#[tokio::test]
async fn a_manual_run_is_recorded_as_unscheduled() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    let run = run_with(Vec::new(), Vec::new());

    record_run_finished(&events, &company, "digest", false, "run-1", Ok(&run)).await;

    let events = journaled(&events, &company).await;
    let CompanyEvent::WorkflowRunFinished { scheduled, .. } = &events[0] else {
        panic!("expected a WorkflowRunFinished");
    };
    assert!(!*scheduled);
}

/// Issue #371: the outcome now carries the caller's run id, which is the
/// only thing tying it to the run's start and per-node events.
#[tokio::test]
async fn the_outcome_carries_the_callers_run_id() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    let run = run_with(Vec::new(), Vec::new());

    record_run_finished(&events, &company, "digest", false, "run-42", Ok(&run)).await;

    let events = journaled(&events, &company).await;
    let CompanyEvent::WorkflowRunFinished { run_id, .. } = &events[0] else {
        panic!("expected a WorkflowRunFinished");
    };
    assert_eq!(run_id.as_deref(), Some("run-42"));
}

/// Issue #383: a run an operator stopped records `cancelled` and **no
/// error**, and the boot sweep leaves it alone because it settled properly.
///
/// The three terminal readings are asserted against each other on purpose:
/// a cancelled run must not be confusable with a failed one (which carries
/// an error) or with an interrupted one (which carries the sweep's
/// synthetic error). Collapsing any pair would put a deliberate stop in the
/// failure count.
#[tokio::test]
async fn a_cancelled_run_records_cancelled_with_no_error() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    let mut run = run_with(Vec::new(), Vec::new());
    run.cancelled = true;
    start(&events, &company, "run-stopped", false).await;

    record_run_finished(&events, &company, "digest", false, "run-stopped", Ok(&run)).await;
    // The sweep must find nothing: the run is settled, not open.
    sweep_interrupted_runs(&events, &company).await;

    let journal = journaled(&events, &company).await;
    assert_eq!(
        journal.len(),
        2,
        "the sweep appended nothing to an already-settled run"
    );
    let CompanyEvent::WorkflowRunFinished {
        cancelled, error, ..
    } = &journal[1]
    else {
        panic!("expected a WorkflowRunFinished, got {:?}", journal[1]);
    };
    assert!(cancelled);
    assert!(
        error.is_none(),
        "a stop is not a failure, so it carries no error: {error:?}"
    );
}

/// The other two readings, for contrast: a failure carries an error and is
/// not cancelled, and the sweep's interrupted row is likewise not cancelled
/// — nobody stopped it, the host went away.
#[tokio::test]
async fn failed_and_interrupted_runs_are_never_flagged_cancelled() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");

    record_run_finished(
        &events,
        &company,
        "digest",
        false,
        "run-bad",
        Err("it broke".into()),
    )
    .await;
    start(&events, &company, "run-dead", false).await;
    sweep_interrupted_runs(&events, &company).await;

    let settled: Vec<(Option<String>, bool)> = journaled(&events, &company)
        .await
        .into_iter()
        .filter_map(|e| match e {
            CompanyEvent::WorkflowRunFinished {
                error, cancelled, ..
            } => Some((error, cancelled)),
            _ => None,
        })
        .collect();
    assert_eq!(
        settled,
        vec![
            (Some("it broke".to_string()), false),
            (Some(INTERRUPTED_BY_RESTART.to_string()), false),
        ],
        "neither a failure nor a host restart may read as an operator stop"
    );
}

/// The case the sweep exists for: a host died mid-run, leaving a start with
/// no finish. Boot settles it — with the *start's* own workflow id and
/// scheduled flag, and the same run id, so the console can still group the
/// nodes that did complete under it.
#[tokio::test]
async fn an_interrupted_run_is_settled_at_boot() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    start(&events, &company, "run-dead", true).await;
    // One node got through before the host went away — the whole point of
    // the record is that this survives.
    events
        .append(
            &company,
            CompanyEvent::WorkflowNodeFinished {
                workflow_id: "digest".to_string(),
                run_id: "run-dead".to_string(),
                node_id: "ceo".to_string(),
                status: crate::ports::types::WorkflowNodeStatus::Ok,
                elapsed_ms: 12,
                diagnostics: Vec::new(),
                agent_run_id: None,
            },
        )
        .await
        .expect("append");

    sweep_interrupted_runs(&events, &company).await;

    let events = journaled(&events, &company).await;
    assert_eq!(events.len(), 3, "the sweep appends exactly one row");
    let CompanyEvent::WorkflowRunFinished {
        workflow_id,
        scheduled,
        run_id,
        error,
        ..
    } = &events[2]
    else {
        panic!("expected a WorkflowRunFinished, got {:?}", events[2]);
    };
    assert_eq!(workflow_id, "digest");
    assert!(*scheduled, "the flag is carried from the start event");
    assert_eq!(run_id.as_deref(), Some("run-dead"));
    assert_eq!(error.as_deref(), Some(INTERRUPTED_BY_RESTART));
}

/// A run that started and finished normally is left alone — otherwise every
/// boot would append a duplicate, contradictory outcome to healthy history.
#[tokio::test]
async fn a_completed_run_is_left_alone() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    let run = run_with(Vec::new(), Vec::new());
    start(&events, &company, "run-ok", false).await;
    record_run_finished(&events, &company, "digest", false, "run-ok", Ok(&run)).await;

    sweep_interrupted_runs(&events, &company).await;

    assert_eq!(
        journaled(&events, &company).await.len(),
        2,
        "the sweep appended nothing"
    );
}

/// A journal written before #371 carries finished rows with no run id and no
/// starts at all. It must sweep to a no-op: those runs are history, not
/// in-flight work, and stamping them "interrupted" would rewrite the past.
#[tokio::test]
async fn a_pre_371_journal_sweeps_to_nothing() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    events
        .append(
            &company,
            CompanyEvent::WorkflowRunFinished {
                workflow_id: "digest".to_string(),
                scheduled: true,
                run_id: None,
                deliveries: Vec::new(),
                pending_approvals: Vec::new(),
                error: None,
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                blocked_nodes: Vec::new(),
                approvals: Vec::new(),
            },
        )
        .await
        .expect("append");

    sweep_interrupted_runs(&events, &company).await;

    assert_eq!(journaled(&events, &company).await.len(), 1);
}

/// Several open runs are all settled, and each keeps its own identity — a
/// scheduled one stays scheduled, a manual one stays manual.
#[tokio::test]
async fn every_open_run_is_settled_with_its_own_flags() {
    let (_home, events) = log();
    let company = CompanyId::new("acme");
    start(&events, &company, "run-a", true).await;
    start(&events, &company, "run-b", false).await;

    sweep_interrupted_runs(&events, &company).await;

    let settled: Vec<(String, bool)> = journaled(&events, &company)
        .await
        .into_iter()
        .filter_map(|e| match e {
            CompanyEvent::WorkflowRunFinished {
                run_id, scheduled, ..
            } => Some((run_id.unwrap_or_default(), scheduled)),
            _ => None,
        })
        .collect();
    assert_eq!(
        settled,
        vec![("run-a".to_string(), true), ("run-b".to_string(), false)]
    );
}

/// A report delivered by a run that never journaled a finish (a crash) is
/// owed-nothing to the next run — it is in the ledger.
#[tokio::test]
async fn a_delivery_without_a_finish_is_stranded() {
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

    let ledger = delivered_by_unsettled_runs(&events, &company, "digest").await;
    assert_eq!(
        ledger,
        vec![DeliveredReport {
            node: "owner_summary".to_string(),
            kind: "owner".to_string(),
        }],
        "a crashed run's delivery must be owed-nothing to the next run"
    );
}

/// The cadence guarantee: a run that delivered AND finished cleanly clears
/// the ledger, so the next scheduled run of the same workflow delivers
/// again rather than being suppressed forever.
#[tokio::test]
async fn a_clean_finish_clears_the_ledger() {
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
        "a clean finish absorbs what the run delivered"
    );
}

/// A failed finish carries an error and does NOT clear — its run's stranded
/// delivery stays owed-nothing to the next run.
#[tokio::test]
async fn a_failed_finish_keeps_the_ledger() {
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
    record_run_finished(
        &events,
        &company,
        "digest",
        true,
        "run-1",
        Err("it broke".into()),
    )
    .await;

    assert_eq!(
        stranded(&events, &company, "digest").await,
        vec!["owner_summary".to_string()],
        "a finish that failed must not clear the delivered ledger"
    );
}

/// The boot sweep's synthetic INTERRUPTED finish is a failed finish
/// (`error: Some`), so it likewise keeps the ledger — a host that died
/// mid-run still owes the next run nothing for what it managed to send.
#[tokio::test]
async fn a_swept_interrupted_finish_keeps_the_ledger() {
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
    // The host went away before finishing; boot settles it synthetically.
    sweep_interrupted_runs(&events, &company).await;

    assert_eq!(
        stranded(&events, &company, "digest").await,
        vec!["owner_summary".to_string()],
        "the sweep's synthetic error must not read as a clean finish"
    );
}
