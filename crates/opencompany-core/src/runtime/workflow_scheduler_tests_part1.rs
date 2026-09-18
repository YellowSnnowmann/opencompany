use super::tests_core::*;

/// The headline: a workflow that exists ONLY as a record overlay (no source
/// file — the console-created shape #168 introduced) is picked up and fired
/// on a matching minute, once. It does not re-fire in the same minute, stays
/// silent on a non-matching minute, and fires again on the next match.
#[tokio::test]
async fn fires_an_overlay_only_workflow_once_per_matching_minute() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("0 9 * * MON"))],
        Some(runner),
        "running",
    )
    .await;

    // Monday 2026-07-13 09:00 UTC — the schedule matches.
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock.clone());

    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    let run = started.lock().unwrap()[0].clone();
    assert_eq!(run.company, "acme");
    assert_eq!(run.workflow, "digest");

    // Same minute again: deduped.
    clock.advance(30_000);
    assert_eq!(scheduler.tick().await, 0);

    // A non-matching minute (09:01) is silent.
    clock.set(millis_at(2026, 7, 13, 9, 1));
    assert_eq!(scheduler.tick().await, 0);

    // The following Monday fires again — after the first run has let go of
    // its claim, or the overlap guard would rightly refuse.
    drain(&scheduler).await;
    clock.set(millis_at(2026, 7, 20, 9, 0));
    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 2).await;
}

#[tokio::test]
async fn fires_a_global_only_workflow() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(&home, "acme", Vec::new(), Some(runner), "running").await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);
    let workflows = [global("global_digest", "* * * * *")];

    assert_eq!(scheduler.tick_with_globals(&workflows).await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    let run = started.lock().unwrap()[0].clone();
    assert_eq!(run.workflow, "global_digest");
    assert!(run.global);
}

#[tokio::test]
async fn a_disabled_global_workflow_does_not_fire() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(&home, "acme", Vec::new(), Some(runner), "running").await;
    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();
    let store = runtime.store().clone();
    let mut record = store.load(&company).await.unwrap().unwrap();
    record.manifest.globals.disable = vec!["workflow:global_digest".to_string()];
    store.save(&record).await.unwrap();
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);
    let workflows = [global("global_digest", "* * * * *")];

    assert_eq!(scheduler.tick_with_globals(&workflows).await, 0);
    assert!(started.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_company_workflow_shadows_a_scheduled_global() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);
    let workflows = [global("digest", "* * * * *")];

    assert_eq!(scheduler.tick_with_globals(&workflows).await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    let run = started.lock().unwrap()[0].clone();
    assert_eq!(run.workflow, "digest");
    assert!(!run.global);
}

/// The seeded input tells the run — and every agent turn inside it — that a
/// schedule started it. `request` is the key `run_request_text` reads.
#[tokio::test]
async fn the_seeded_input_marks_the_run_as_scheduled() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let fired_at = millis_at(2026, 7, 13, 9, 0);
    let clock = Arc::new(FakeClock::new(fired_at));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;

    let input = started.lock().unwrap()[0].input.clone();
    assert_eq!(input["scheduled"], true);
    assert_eq!(input["cron"], "* * * * *");
    assert_eq!(input["firedAtMs"], fired_at);
    // `request` is exactly the key `workflows::caps::run_request_text`
    // reads, so this string reaches every agent turn in the run.
    let request = input["request"].as_str().expect("a request string");
    assert!(request.contains("Scheduled run"), "{request}");
    assert!(request.contains("* * * * *"), "{request}");
    assert!(!request.trim().is_empty());
}

/// Issue #981: `skipped` is no longer the same question as "did not go
/// out". Two of its reasons describe a report whose fate is accounted for —
/// an earlier run in the approval lineage sent it (issue #438), or a test
/// run attempted nothing on purpose (issue #542) — so they sit in the
/// `skipped` breakdown and out of the number an operator alerts on.
#[test]
fn an_accounted_for_skip_is_counted_but_not_alerted_on() {
    let counts = DeliveryCounts::of(&[
        reported(
            "a",
            DeliveryStatus::Skipped,
            DeliveryReason::AlreadyDelivered,
            "",
        ),
        reported("b", DeliveryStatus::Skipped, DeliveryReason::DryRun, ""),
    ]);
    assert_eq!(counts.skipped, 2, "the breakdown still sees them");
    assert_eq!(counts.undelivered(), 0, "but nothing here needs a fix");

    // The deliberate non-move: an `output` node with nowhere to send
    // produced a report and lost it, which is what issue #925 added the row
    // to make visible.
    let nowhere = DeliveryCounts::of(&[reported(
        "c",
        DeliveryStatus::Skipped,
        DeliveryReason::NoDestinationConfigured,
        "",
    )]);
    assert_eq!(nowhere.skipped, 1);
    assert_eq!(nowhere.undelivered(), 1);
}

/// The fold behind the summary line counts each status separately, because
/// "policy refused to send" and "something broke" are different problems.
#[test]
fn delivery_counts_separate_the_five_outcomes() {
    let counts = DeliveryCounts::of(&[
        report("a", DeliveryStatus::Sent, ""),
        report("b", DeliveryStatus::Sent, ""),
        report("c", DeliveryStatus::Skipped, ""),
        report("d", DeliveryStatus::Denied, ""),
        report("e", DeliveryStatus::Failed, ""),
        report("f", DeliveryStatus::Pending, ""),
    ]);
    assert_eq!(
        counts,
        DeliveryCounts {
            sent: 2,
            pending: 1,
            skipped: 1,
            denied: 1,
            failed: 1,
            undelivered: 3,
        }
    );
    // A parked report awaits a verdict, not a fix: it must NOT inflate the
    // number an operator alerts on, or a working approvals queue would page
    // someone every scheduled minute.
    assert_eq!(counts.undelivered(), 3);
    // The common case: nothing routed anywhere.
    assert_eq!(DeliveryCounts::of(&[]), DeliveryCounts::default());
    assert_eq!(DeliveryCounts::of(&[]).undelivered(), 0);
}

/// **The finding CodeRabbit raised.** A scheduled run whose report did not
/// go out must not be silent. There is no HTTP response and no drawer, so
/// the log line is the whole channel — this reads what the scheduler
/// actually emitted and asserts the operator-actionable parts are in it:
/// which company, which workflow, which node, and the `reason` that says
/// what to do about it.
#[tokio::test]
async fn a_scheduled_run_reports_an_undelivered_report() {
    let sink = captured_logs();
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    // A company id unique to this test: the capture buffer is shared with
    // every other test in the binary, so this is what makes the assertions
    // about *this* run rather than about whatever else logged.
    let company = "undelivered-co";
    let (runner, completed) = RecordingRunner::with_deliveries(vec![
        report(
            "owner_summary",
            DeliveryStatus::Skipped,
            "this recipient has never written to the company",
        ),
        report("also_sent", DeliveryStatus::Sent, "emailed the recipient"),
    ]);
    let registry = company_with_overlays(
        &home,
        company,
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| completed.load(Ordering::SeqCst) == 1).await;
    // The run task logs after `completed` is bumped, so wait for the line
    // itself rather than racing it.
    //
    // Qualified by BOTH this company and the marker, and scanned per line:
    // `CAPTURE` is shared across every test in the binary, and
    // `owner_summary` is the node name three tests in this module use — an
    // unqualified `contains` could be satisfied by a sibling's line before
    // this run has logged anything, and the lookup below would then fail
    // intermittently.
    wait_for(|| {
        captured_text(&sink)
            .lines()
            .any(|l| l.contains(company) && l.contains("was NOT delivered"))
    })
    .await;

    let logs = captured_text(&sink);
    let line = logs
        .lines()
        .find(|l| l.contains(company) && l.contains("was NOT delivered"))
        .unwrap_or_else(|| panic!("no undelivered-report line for {company}: {logs}"));
    assert!(line.contains("digest"), "names the workflow: {line}");
    assert!(line.contains("owner_summary"), "names the node: {line}");
    assert!(line.contains("Skipped"), "names the status: {line}");
    assert!(
        line.contains("never written to the company"),
        "carries the reason, which is the part that says what to fix: {line}"
    );
    // …but NOT the recipient's address. This line lands on host stdout,
    // which on a hosted tenant is us rather than the operator, so the
    // address must never ride it — only whether one resolved at all.
    assert!(
        !line.contains(RECIPIENT),
        "must not leak the recipient address to host stdout: {line}"
    );
    assert!(
        line.contains("target_configured=true"),
        "keeps the non-sensitive half of the diagnostic: {line}"
    );

    // The delivery that DID land gets no warning of its own — otherwise a
    // healthy run would look broken.
    assert_eq!(
        logs.lines()
            .filter(|l| l.contains(company) && l.contains("was NOT delivered"))
            .count(),
        1,
        "{logs}"
    );
    // …and the run's own summary still reports the split.
    let summary = logs
        .lines()
        .find(|l| l.contains(company) && l.contains("scheduled run finished"))
        .unwrap_or_else(|| panic!("no summary line for {company}: {logs}"));
    // Every count, including the zeroes: asserting only the non-zero ones
    // lets a regression that stops emitting `denied` or `failed` pass.
    assert!(summary.contains("sent=1"), "{summary}");
    assert!(summary.contains("skipped=1"), "{summary}");
    assert!(summary.contains("denied=0"), "{summary}");
    assert!(summary.contains("failed=0"), "{summary}");
    assert!(summary.contains("pending_approval=0"), "{summary}");
    assert!(summary.contains("undelivered=1"), "{summary}");
}

/// **Issue #248 — the indirect path.** Scrubbing `report.target` from the
/// warning above closed the direct route to host stdout. This is the other
/// one: on the transport-failure arms `report.detail` interpolates the
/// transport's own words, and a mail transport quotes the mailbox it
/// refused — an SMTP `550`/`553` reply is routinely of the form
/// `<recipient@…>: Recipient address rejected`. So a row whose `target` is
/// scrubbed can still walk the address in through `detail`.
///
/// The `detail` here is verbatim what
/// [`crate::workflows::delivery`] builds for a refused send, wrapped around
/// a realistic SMTP reply, so the fixture fails the way production does
/// rather than the way a mock does.
///
/// **The assertion is over the emitted event**, not over a string on the way
/// to it: it scans the captured `tracing` output for the line the scheduler
/// actually wrote. Asserting on `report.reason` alone would prove nothing
/// about the log — the whole bug was a field being logged that nobody meant
/// to log.
#[tokio::test]
async fn a_transport_failure_does_not_log_the_address_the_transport_quoted() {
    let sink = captured_logs();
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let company = "transport-refusal-co";
    // What `delivery::deliver_one`'s `Err(err)` arm produces, with a reply
    // shaped like a real one.
    let detail = format!(
        "the mail transport refused the message: 550 5.1.1 <{RECIPIENT}>: Recipient address \
         rejected: User unknown in local recipient table"
    );
    let (runner, completed) = RecordingRunner::with_deliveries(vec![reported(
        "owner_summary",
        DeliveryStatus::Failed,
        DeliveryReason::MailTransportRefused,
        &detail,
    )]);
    let registry = company_with_overlays(
        &home,
        company,
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| completed.load(Ordering::SeqCst) == 1).await;
    wait_for(|| {
        captured_text(&sink)
            .lines()
            .any(|l| l.contains(company) && l.contains("was NOT delivered"))
    })
    .await;

    let logs = captured_text(&sink);
    let line = logs
        .lines()
        .find(|l| l.contains(company) && l.contains("was NOT delivered"))
        .unwrap_or_else(|| panic!("no undelivered-report line for {company}: {logs}"));

    // The point of the issue: not through `target`, and not through the
    // transport's reply either.
    assert!(
        !line.contains(RECIPIENT),
        "the transport's reply must not walk the recipient address onto host stdout: {line}"
    );
    // Belt and braces — the address's local part alone is enough to
    // identify a person, so an over-eager "strip the domain" fix must fail
    // this too.
    assert!(
        !line.contains("recipient@"),
        "not even a partial address: {line}"
    );
    // The whole reply is absent, not merely masked mid-string: nothing of
    // the transport's own text reaches the line.
    assert!(
        !line.contains("Recipient address rejected"),
        "no transport-supplied text at all: {line}"
    );

    // …and the line is still worth reading. An operator paged by this needs
    // to know where to look and what class of thing broke.
    assert!(line.contains("digest"), "names the workflow: {line}");
    assert!(line.contains("owner_summary"), "names the node: {line}");
    assert!(
        line.contains("kind=email"),
        "names the destination kind: {line}"
    );
    assert!(line.contains("Failed"), "names the status: {line}");
    assert!(
        line.contains("target_configured=true"),
        "says a target resolved, without saying which: {line}"
    );
    assert!(
        line.contains("the mail transport refused the message"),
        "names the failure class: {line}"
    );
}

/// The operator's half is deliberately NOT scrubbed. `detail` is what makes
/// a refused send fixable, the run response and the journaled
/// `WorkflowRunFinished` event are tenant-scoped surfaces, and an operator
/// is entitled to their own recipient's address. Pinned so a later "scrub
/// it everywhere" sweep has to argue with a test.
#[test]
fn the_operator_facing_detail_keeps_the_transport_text() {
    let detail =
        format!("the mail transport refused the message: 550 5.1.1 <{RECIPIENT}>: rejected");
    let row = reported(
        "owner_summary",
        DeliveryStatus::Failed,
        DeliveryReason::MailTransportRefused,
        &detail,
    );
    assert!(row.detail.contains(RECIPIENT), "{row:?}");
    assert_eq!(row.target.as_deref(), Some(RECIPIENT));
    // The two halves disagree on purpose — that IS the fix.
    assert!(!row.reason.to_string().contains(RECIPIENT), "{row:?}");
    assert!(!row.reason.to_string().contains('@'), "{row:?}");
}

/// Issue #227: a scheduled run whose report was parked for approval is the
/// nobody-is-watching case squared — there is no drawer AND the operator
/// has to be told a card is waiting for them. It must be said, but not as a
/// failure: no `was NOT delivered` warning, and it must not inflate the
/// `undelivered` number an alert keys on.
#[tokio::test]
async fn a_scheduled_run_reports_a_parked_report_without_crying_wolf() {
    let sink = captured_logs();
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let company = "parked-delivery-co";
    let (runner, completed) = RecordingRunner::with_deliveries(vec![report(
        "owner_summary",
        DeliveryStatus::Pending,
        "parked for operator approval",
    )]);
    let registry = company_with_overlays(
        &home,
        company,
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| completed.load(Ordering::SeqCst) == 1).await;
    // Qualified by BOTH this company and the marker, and scanned per line:
    // `CAPTURE` is shared across every test in the binary, so an
    // unqualified `contains("scheduled run finished")` is satisfied by any
    // sibling test's summary line — including one logged before this run
    // finished — and the lookups below would then fail intermittently.
    wait_for(|| {
        captured_text(&sink)
            .lines()
            .any(|l| l.contains(company) && l.contains("scheduled run finished"))
    })
    .await;

    let logs = captured_text(&sink);
    // Said, and pointed at the place the operator has to go.
    let line = logs
        .lines()
        .find(|l| l.contains(company) && l.contains("parked for operator approval"))
        .unwrap_or_else(|| panic!("no parked-report line for {company}: {logs}"));
    assert!(line.contains("owner_summary"), "names the node: {line}");
    assert!(line.contains("Approvals view"), "says where to go: {line}");
    // Never the recipient's address on host stdout, same as the warn path.
    assert!(
        !line.contains(RECIPIENT),
        "must not leak the recipient address to host stdout: {line}"
    );
    // Not cried wolf about.
    assert!(
        !logs
            .lines()
            .any(|l| l.contains(company) && l.contains("was NOT delivered")),
        "a parked report is not a failed delivery: {logs}"
    );
    let summary = logs
        .lines()
        .find(|l| l.contains(company) && l.contains("scheduled run finished"))
        .unwrap_or_else(|| panic!("no summary line for {company}: {logs}"));
    assert!(summary.contains("pending_approval=1"), "{summary}");
    assert!(summary.contains("undelivered=0"), "{summary}");
    assert!(summary.contains("sent=0"), "{summary}");
}

/// A scheduled run that delivered everything says so without crying wolf:
/// no warning line, and a summary that still accounts for what went out.
#[tokio::test]
async fn a_clean_scheduled_run_logs_no_delivery_warning() {
    let sink = captured_logs();
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let company = "clean-delivery-co";
    let (runner, completed) = RecordingRunner::with_deliveries(vec![report(
        "owner_summary",
        DeliveryStatus::Sent,
        "emailed the company's admin",
    )]);
    let registry = company_with_overlays(
        &home,
        company,
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| completed.load(Ordering::SeqCst) == 1).await;
    wait_for(|| {
        captured_text(&sink)
            .lines()
            .any(|l| l.contains(company) && l.contains("scheduled run finished"))
    })
    .await;

    let logs = captured_text(&sink);
    assert!(
        !logs
            .lines()
            .any(|l| l.contains(company) && l.contains("was NOT delivered")),
        "a fully delivered run must not warn: {logs}"
    );
    let summary = logs
        .lines()
        .find(|l| l.contains(company) && l.contains("scheduled run finished"))
        .expect("a summary line");
    assert!(summary.contains("sent=1"), "{summary}");
    assert!(summary.contains("skipped=0"), "{summary}");
    assert!(summary.contains("denied=0"), "{summary}");
    assert!(summary.contains("failed=0"), "{summary}");
    assert!(summary.contains("undelivered=0"), "{summary}");
}

/// **The issue.** A scheduled run's delivery rows must land somewhere the
/// tenant's own console can read back — the log line only reaches whoever
/// reads host stdout, which on a hosted tenant is not the operator.
#[tokio::test]
async fn a_scheduled_run_journals_its_delivery_rows_and_approvals() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let company = "journal-delivery-co";
    let (runner, completed) = RecordingRunner::with_deliveries(vec![
        report(
            "owner_summary",
            DeliveryStatus::Skipped,
            "this recipient has never written to the company",
        ),
        report("also_sent", DeliveryStatus::Sent, "emailed the recipient"),
    ]);
    let registry = company_with_overlays(
        &home,
        company,
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock);

    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| completed.load(Ordering::SeqCst) == 1).await;
    // The append happens after the run completes, so wait on the journal
    // rather than racing it.
    let outcomes = loop {
        let outcomes = run_outcomes(&registry, company).await;
        if !outcomes.is_empty() {
            break outcomes;
        }
        tokio::task::yield_now().await;
    };

    assert_eq!(outcomes.len(), 1);
    let CompanyEvent::WorkflowRunFinished {
        workflow_id,
        scheduled,
        deliveries,
        error,
        ..
    } = &outcomes[0]
    else {
        unreachable!("filtered above")
    };
    assert_eq!(workflow_id, "digest");
    assert!(*scheduled, "a cron fire records itself as scheduled");
    assert!(error.is_none());
    assert_eq!(deliveries.len(), 2, "both rows, not just the failed one");
    let skipped = deliveries
        .iter()
        .find(|d| d.node == "owner_summary")
        .expect("the undelivered row");
    assert_eq!(skipped.status, DeliveryStatus::Skipped);
    // The `detail` is the part that says what to fix. The log line carries
    // it too, but the log is not where the operator can look.
    assert!(
        skipped.detail.contains("never written to the company"),
        "{skipped:?}"
    );
    // The journal is operator-scoped — unlike host stdout — so the resolved
    // target rides it. This is the same field the manual run's HTTP response
    // already ships to the console today.
    assert_eq!(skipped.target.as_deref(), Some(RECIPIENT));
}
