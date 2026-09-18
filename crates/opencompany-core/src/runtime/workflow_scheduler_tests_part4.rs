use super::tests_core::*;

/// A switched-off workflow (#276) gets NO catch-up: it is filtered before the
/// catch-up check, so its anchor is never touched.
#[tokio::test]
async fn a_disabled_workflow_gets_no_catch_up() {
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
    set_enabled(&registry, "acme", "digest", false).await;
    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();
    let anchor = minute_at(2026, 7, 6, 9, 0);
    runtime
        .schedule_fires()
        .claim_fire(&company, "workflow-digest", anchor)
        .await
        .unwrap();

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);
    assert_eq!(
        scheduler.tick().await,
        0,
        "a paused schedule makes up nothing"
    );
    // The anchor is untouched: no catch-up claim was written.
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        Some(anchor)
    );
    assert!(started.lock().unwrap().is_empty());
}

/// A previous scheduled run still in flight suppresses the next minute's fire
/// WITHOUT claiming it — the minute is suppressed (a slow run's own next
/// tick), not burned, so a peer could still fire it.
#[tokio::test]
async fn an_in_flight_run_suppresses_the_next_minute_without_claiming_it() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let gate = Arc::new(Semaphore::new(0));
    let (runner, started, _completed) = RecordingRunner::gated(gate.clone());
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock.clone());

    // Minute M fires and the run parks (gated), holding the in-flight slot.
    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    let m = minute_at(2026, 7, 13, 9, 0);
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        Some(m)
    );

    // Minute M+1: the still-in-flight run suppresses this fire. Crucially the
    // durable claim is NOT taken, so the anchor stays at M — the minute is
    // suppressed, not burned.
    clock.set(millis_at(2026, 7, 13, 9, 1));
    assert_eq!(scheduler.tick().await, 0);
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        Some(m),
        "a suppressed minute must NOT be claimed"
    );

    // Let the parked run finish so the test tears down cleanly.
    gate.add_permits(1);
    drain(&scheduler).await;
}

/// A company provisioned AFTER boot still gets its catch-up: the check is
/// per-(company, workflow) first-sight, not a one-shot boot pass.
#[tokio::test]
async fn a_late_provisioned_company_gets_its_catch_up_on_first_sight() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();

    // The scheduler starts over an EMPTY registry — nothing to do yet.
    let registry = CompanyRegistry::new();
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock);
    assert_eq!(scheduler.tick().await, 0, "empty registry fires nothing");

    // Provision a company with a missed schedule, after boot.
    let late = company_with_overlays(
        &home,
        "late",
        vec![overlay("digest", Some("0 9 * * MON"))],
        Some(runner),
        "running",
    )
    .await;
    let company = CompanyId::new("late");
    let runtime = late.get(&company).unwrap();
    runtime
        .schedule_fires()
        .claim_fire(&company, "workflow-digest", minute_at(2026, 7, 6, 9, 0))
        .await
        .unwrap();
    registry.insert(company, runtime);

    // Next tick sees the company for the first time and makes up its fire.
    assert_eq!(
        scheduler.tick().await,
        1,
        "the late company gets its catch-up"
    );
    wait_for(|| started.lock().unwrap().len() == 1).await;
    assert_eq!(started.lock().unwrap()[0].input["catchUp"], true);
}

/// #708: a workflow deleted and recreated with the SAME id must not inherit
/// the old one's fire ledger. Driven against the real per-company store (not
/// a double), in two phases in one test so the stale-fixture guard holds:
///
/// * Phase 1 — with the inherited claim still present, minute M is suppressed
///   (`claim_fire` loses to the stale row). This is exactly the bug a
///   delete+recreate exhibited before this fix.
/// * Phase 2 — after `delete_schedule_fires` (what `delete_company_workflow`
///   now calls on delete) purges the ledger, the SAME minute is claimable
///   again and the recreated workflow fires.
///
/// Phase 1 proves the seeded claim genuinely suppresses, so phase 2's fire
/// can only come from the purge actually removing the row — a no-op purge
/// leaves phase 2 asserting `0` and fails the test.
#[tokio::test]
async fn a_recreated_workflow_does_not_inherit_the_deleted_fire_ledger() {
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
    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();
    let schedule_id = workflow_schedule_id("digest");

    // The OLD workflow already fired minute M; its durable claim survives the
    // delete because the key is the restart-stable `workflow-<id>`.
    let m = minute_at(2026, 7, 14, 9, 0);
    runtime
        .schedule_fires()
        .claim_fire(&company, &schedule_id, m)
        .await
        .unwrap();

    // A recreated workflow is, to the durable ledger, a fresh scheduler view
    // over the same store (the in-process minute dedup is empty on the new
    // sighting — most starkly across a restart). So each phase uses its own
    // scheduler instance, exactly like `two_schedulers_over_one_store_fire_once`,
    // isolating the DURABLE ledger as the only variable between them.
    let clock = || Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));

    // Phase 1 — WITHOUT the purge: the inherited claim suppresses minute M.
    // This is exactly the #708 bug a delete+recreate exhibited.
    let mut before = WorkflowScheduler::new(registry.clone(), clock());
    assert_eq!(
        before.tick().await,
        0,
        "an inherited claim suppresses the recreated workflow's fire (the #708 bug)"
    );
    assert!(started.lock().unwrap().is_empty());

    // Phase 2 — WITH the purge (what `delete_company_workflow` now does): the
    // ledger is cleared, so the SAME minute is claimable again and the
    // recreated workflow fires. A no-op purge would leave this asserting 0.
    let removed = runtime
        .schedule_fires()
        .delete_schedule_fires(&company, &schedule_id)
        .await
        .unwrap();
    assert_eq!(removed, 1, "the purge removes exactly the inherited claim");

    let mut after = WorkflowScheduler::new(registry.clone(), clock());
    assert_eq!(
        after.tick().await,
        1,
        "after the purge the recreated workflow fires the same minute"
    );
    wait_for(|| started.lock().unwrap().len() == 1).await;

    drain(&after).await;
}

/// A transient anchor-read failure on first sight DEFERS the catch-up: the
/// latch is dropped so the NEXT tick re-reads the anchor and makes up the
/// missed fire, instead of one flaky `latest_fire` forfeiting it forever.
#[tokio::test]
async fn a_transient_anchor_read_error_defers_first_sight_catch_up() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let fires = Arc::new(FlakyFires::new());
    let registry = company_with_overlays_and_fires(
        &home,
        "acme",
        vec![overlay("digest", Some("0 9 * * MON"))],
        Some(runner),
        "running",
        fires.clone(),
    )
    .await;
    // Anchor two Mondays back; the most recent missed Monday is 2026-07-13.
    fires.seed("acme", "workflow-digest", minute_at(2026, 7, 6, 9, 0));
    // The FIRST anchor read fails; the second (next tick) succeeds.
    fires.arm_latest_failures(1);

    // "Now" is a Tuesday, so the current minute never matches — isolating the
    // catch-up as the only possible fire.
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock);

    // First tick: the anchor read errors, so catch-up is deferred and the
    // latch dropped — nothing fires, nothing is claimed.
    assert_eq!(
        scheduler.tick().await,
        0,
        "a flaky anchor read defers the catch-up on first sight"
    );
    assert!(started.lock().unwrap().is_empty());

    // Second tick: the anchor read now succeeds, so the deferred make-up lands
    // at the ORIGINAL missed minute, proving the latch did not forfeit it.
    assert_eq!(
        scheduler.tick().await,
        1,
        "the next tick re-attempts and fires the deferred catch-up"
    );
    wait_for(|| started.lock().unwrap().len() == 1).await;
    let run = started.lock().unwrap()[0].clone();
    assert_eq!(run.input["catchUp"], true);
    assert_eq!(
        run.input["firedAtMs"],
        minute_at(2026, 7, 13, 9, 0) * MINUTE_MS,
        "the make-up carries the original missed minute"
    );
}

/// A transient claim failure on the first-sight catch-up DEFERS it and — the
/// part that would otherwise be a silent leak — releases the admission guard,
/// so the in-flight slot is not permanently occupied by a run that never
/// started. A later tick re-attempts and fires it.
#[tokio::test]
async fn a_transient_claim_error_on_catch_up_defers_not_forfeits() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let fires = Arc::new(FlakyFires::new());
    let registry = company_with_overlays_and_fires(
        &home,
        "acme",
        vec![overlay("digest", Some("0 9 * * MON"))],
        Some(runner),
        "running",
        fires.clone(),
    )
    .await;
    // Anchor two Mondays back; the most recent missed Monday is 2026-07-13.
    fires.seed("acme", "workflow-digest", minute_at(2026, 7, 6, 9, 0));
    // The FIRST claim (the catch-up claim) fails; the next succeeds.
    fires.arm_claim_failures(1);

    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();
    // "Now" is a Tuesday: the current minute never matches, isolating catch-up.
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock);

    // First tick: the catch-up claim errors after admission, so the guard is
    // released on the fail-closed arm — no run starts, no guard leaks.
    assert_eq!(
        scheduler.tick().await,
        0,
        "a flaky catch-up claim defers, firing nothing"
    );
    assert!(started.lock().unwrap().is_empty());
    assert_eq!(
        runtime.run_supervisor().len(),
        0,
        "the admission guard was released — no in-flight slot leaked"
    );

    // Second tick: the claim now succeeds, so the deferred make-up fires.
    assert_eq!(
        scheduler.tick().await,
        1,
        "the next tick re-attempts and fires the deferred catch-up"
    );
    wait_for(|| started.lock().unwrap().len() == 1).await;
    assert_eq!(started.lock().unwrap()[0].input["catchUp"], true);
}

/// The overlap arm: when a prior run still holds the in-flight slot on first
/// sight, the catch-up cannot even be attempted — so the latch is dropped
/// (not left set), letting a later tick re-attempt once the slot frees. Driven
/// directly on the private latch, since the overlap-on-first-sight race is not
/// cheap to stage through the public tick alone.
#[tokio::test]
async fn an_overlap_on_first_sight_drops_the_catch_up_latch() {
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
    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();
    // A genuine missed fire is available (anchor two Mondays back).
    runtime
        .schedule_fires()
        .claim_fire(&company, "workflow-digest", minute_at(2026, 7, 6, 9, 0))
        .await
        .unwrap();

    // "Now" is a Tuesday, so nothing matches the current minute — the only
    // path that could touch the latch this tick is the first-sight catch-up.
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock);

    // Occupy the overlap slot for this key BEFORE the tick, standing in for a
    // prior scheduled run still executing when first sight happens.
    let key = (company.clone(), "digest".to_string());
    let _held = scheduler.claim(&key).expect("take the overlap slot");

    assert_eq!(
        scheduler.tick().await,
        0,
        "the overlap suppresses the catch-up attempt this tick"
    );
    assert!(started.lock().unwrap().is_empty());
    assert!(
        !scheduler.caught_up.contains(&key),
        "the latch was dropped, so a later tick will re-attempt the catch-up"
    );
}
