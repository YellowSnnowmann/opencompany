use super::tests_core::*;

/// A workflow committed as a seed file (not an overlay) is scheduled too, so
/// the union really is the read path.
#[tokio::test]
async fn a_seed_file_workflow_is_scheduled() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source.path().join("workflows")).unwrap();
    std::fs::write(
        source.path().join("workflows").join("seeded.toml"),
        body("seeded", Some("* * * * *")),
    )
    .unwrap();

    let company = CompanyId::new("acme");
    let (runner, started, _completed) = RecordingRunner::new();
    let mut runtime = RuntimeBuilder::new(home.clone(), manifest())
        .with_id(company.clone())
        .build()
        .await
        .unwrap();
    runtime.set_source_dir(Some(source.path().to_path_buf()));
    runtime.set_workflow_runner(runner);
    let registry = CompanyRegistry::new();
    registry.insert(company, Arc::new(runtime));

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);
    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    assert_eq!(started.lock().unwrap()[0].workflow, "seeded");
}

/// The helper reads the first scheduled trigger and ignores everything else.
#[test]
fn trigger_schedule_reads_the_trigger_only() {
    let scheduled = crate::company::parse_workflow(&body("wf", Some("0 * * * *"))).unwrap();
    assert_eq!(trigger_schedule(&scheduled).as_deref(), Some("0 * * * *"));

    let bare = crate::company::parse_workflow(&body("wf", None)).unwrap();
    assert!(trigger_schedule(&bare).is_none());
}

/// An empty registry ticks cleanly (the shape a server with no companies
/// boots into).
#[tokio::test]
async fn an_empty_registry_ticks_to_zero() {
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(CompanyRegistry::new(), clock);
    assert_eq!(scheduler.tick().await, 0);
}

/// **Delete needs no scheduler teardown.** The workflow fires, is removed
/// from the record, and never fires again — no restart, no unbind call.
#[tokio::test]
async fn a_deleted_workflow_stops_firing_on_the_very_next_tick() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("0 9 * * *"))],
        Some(runner),
        "running",
    )
    .await;

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock.clone());

    // It fires while it exists.
    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    drain(&scheduler).await;

    // The operator deletes it (the route drops the overlay body).
    rewrite_overlays(&registry, "acme", Vec::new()).await;

    // Next matching minute: nothing. No process restart in between.
    clock.set(millis_at(2026, 7, 14, 9, 0));
    assert_eq!(
        scheduler.tick().await,
        0,
        "a deleted workflow must not keep firing"
    );
    assert_eq!(started.lock().unwrap().len(), 1);
}

/// **Edit needs no rebinding.** A corrected cron takes effect on the next
/// tick: the old expression stops matching and the new one starts. This is
/// the issue's own example — a typo'd schedule that used to be permanent.
#[tokio::test]
async fn an_edited_schedule_takes_effect_without_rebinding() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("0 9 * * *"))],
        Some(runner),
        "running",
    )
    .await;

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock.clone());

    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    drain(&scheduler).await;

    // The operator corrects 09:00 → 10:00.
    rewrite_overlays(
        &registry,
        "acme",
        vec![overlay("digest", Some("0 10 * * *"))],
    )
    .await;

    // The OLD cadence is dead…
    clock.set(millis_at(2026, 7, 14, 9, 0));
    assert_eq!(
        scheduler.tick().await,
        0,
        "the replaced schedule must stop firing"
    );

    // …and the NEW one is live, with no restart and no rebind call.
    clock.set(millis_at(2026, 7, 14, 10, 0));
    assert_eq!(
        scheduler.tick().await,
        1,
        "the corrected schedule must start firing"
    );
    wait_for(|| started.lock().unwrap().len() == 2).await;
    drain(&scheduler).await;
}

/// A schedule appearing on a stored graph is picked up on the next tick with
/// no re-registration — the reconcile property, still true after #276.
///
/// **This is not "an edit arms a workflow", and it used to be.** The test it
/// replaces was named `adding_a_schedule_by_edit_arms_the_workflow_on_the_next_tick`
/// and pinned exactly that, on the pre-#276 argument that the scheduler gated
/// on nothing. It does now, so arming is a separate decision made by
/// `set_company_workflow_enabled` (or refused by the disarm rule), and this
/// test writes the overlay body **directly** to isolate the reconcile from
/// that decision.
///
/// What the disarm rule does to a real edit is pinned where the rule lives —
/// `company::workflow_create`'s
/// `an_edit_that_adds_a_schedule_switches_the_workflow_off`. Keep the two
/// together in your head: this one says the tick sees graph changes, that one
/// says a person still has to arm them.
#[tokio::test]
async fn a_schedule_added_to_the_stored_graph_is_picked_up_without_re_registration() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", None)], // manual-run only
        Some(runner),
        "running",
    )
    .await;

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock.clone());
    assert_eq!(scheduler.tick().await, 0, "no schedule, no fire");

    rewrite_overlays(
        &registry,
        "acme",
        vec![overlay("digest", Some("0 9 * * *"))],
    )
    .await;

    clock.set(millis_at(2026, 7, 14, 9, 0));
    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    drain(&scheduler).await;
}

// ── Issue #276: the pause switch ────────────────────────────────────────

/// A switched-off workflow does not fire, however well its cron matches.
///
/// The core of issue #276(a): before this, silencing a schedule meant
/// deleting the workflow and losing the graph.
#[tokio::test]
async fn a_switched_off_workflow_does_not_fire() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("0 9 * * *"))],
        Some(runner),
        "running",
    )
    .await;
    set_enabled(&registry, "acme", "digest", false).await;

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock.clone());

    assert_eq!(
        scheduler.tick().await,
        0,
        "a switched-off workflow must not fire on a matching minute"
    );
    assert!(
        started.lock().unwrap().is_empty(),
        "nothing may reach the runner"
    );
}

/// Switching a workflow back on resumes it on the very next tick — no
/// restart, no rebind, the same reconcile the graph edits rely on.
///
/// Asserted **after** a suppressed minute, so the test proves the pause was
/// real rather than that the cron never matched.
#[tokio::test]
async fn switching_a_workflow_back_on_resumes_it_on_the_next_tick() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("0 9 * * *"))],
        Some(runner),
        "running",
    )
    .await;
    set_enabled(&registry, "acme", "digest", false).await;

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock.clone());
    assert_eq!(scheduler.tick().await, 0, "paused");

    set_enabled(&registry, "acme", "digest", true).await;
    clock.set(millis_at(2026, 7, 14, 9, 0));
    assert_eq!(scheduler.tick().await, 1, "armed again");
    wait_for(|| started.lock().unwrap().len() == 1).await;
    drain(&scheduler).await;
}

/// Pausing one workflow leaves its siblings firing. The gate is per-workflow,
/// not per-company — a company-wide pause is `lifecycle`, and conflating the
/// two would make the switch far blunter than the console shows it as.
#[tokio::test]
async fn pausing_one_workflow_does_not_silence_the_others() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![
            overlay("digest", Some("0 9 * * *")),
            overlay("standup", Some("0 9 * * *")),
        ],
        Some(runner),
        "running",
    )
    .await;
    set_enabled(&registry, "acme", "digest", false).await;

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock.clone());

    assert_eq!(scheduler.tick().await, 1, "only the armed sibling fires");
    wait_on_time(|| started.lock().unwrap().len() == 1).await;
    assert_eq!(
        started.lock().unwrap()[0].workflow,
        "standup",
        "the paused workflow must be the one that did not run"
    );
    drain(&scheduler).await;
}

/// Two independent schedulers over one durable store — a second replica —
/// fire a matching minute exactly once between them.
#[tokio::test]
async fn two_schedulers_over_one_store_fire_once() {
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
    // Monday 2026-07-13 09:00 — the schedule matches.
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut a = WorkflowScheduler::new(registry.clone(), clock.clone());
    let mut b = WorkflowScheduler::new(registry, clock);

    assert_eq!(a.tick().await, 1, "the first replica wins the claim");
    assert_eq!(
        b.tick().await,
        0,
        "the second replica loses and fires nothing"
    );
    wait_for(|| started.lock().unwrap().len() == 1).await;
    assert_eq!(started.lock().unwrap().len(), 1, "one run in total");
}

/// The FIRST time a scheduler sees a scheduled workflow, it makes up one fire
/// missed during downtime — at the original missed minute, marked `catchUp`.
#[tokio::test]
async fn first_sight_catch_up_fires_one_missed_run() {
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
    // Anchor two Mondays back; the most recent missed Monday is 2026-07-13.
    let anchor = minute_at(2026, 7, 6, 9, 0);
    runtime
        .schedule_fires()
        .claim_fire(&company, "workflow-digest", anchor)
        .await
        .unwrap();

    // "Now" is a Tuesday, so the CURRENT minute never matches — isolating the
    // catch-up as the only possible fire.
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);
    assert_eq!(scheduler.tick().await, 1, "one catch-up fires");
    wait_for(|| started.lock().unwrap().len() == 1).await;

    let run = started.lock().unwrap()[0].clone();
    assert_eq!(run.workflow, "digest");
    assert_eq!(run.input["catchUp"], true);
    assert_eq!(
        run.input["firedAtMs"],
        minute_at(2026, 7, 13, 9, 0) * MINUTE_MS,
        "the make-up run carries the ORIGINAL missed minute, not now"
    );
    // The catch-up claimed that minute, so the anchor advanced to it.
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        Some(minute_at(2026, 7, 13, 9, 0))
    );
}

/// Issue #661: a steady-state fire that would be rejected by the #401
/// in-flight run cap must leave its minute UNCLAIMED, so a later tick with
/// freed capacity still fires it. Before the fix the scheduler claimed the
/// minute and only THEN had the run rejected inside its spawned task, so
/// catch-up read the minute as already fired and the occurrence was lost for
/// good — durably claimed, never run, and invisible to the operator.
#[tokio::test]
async fn a_fire_at_the_in_flight_cap_leaves_the_minute_unclaimed_to_retry() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays_capped(
        &home,
        "acme",
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "running",
        Some(1),
    )
    .await;
    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();

    // Fill the single slot with a stand-in in-flight run, so the company is
    // at its cap exactly as it would be with a real run underway.
    let supervisor = runtime.run_supervisor().clone();
    let (_ctx, filler) = supervisor
        .begin("filler", false)
        .expect("fills the cap of 1");
    assert_eq!(
        supervisor.len(),
        supervisor.limit(),
        "the company is at its cap"
    );

    // Every minute matches `* * * * *`.
    let fired_at = millis_at(2026, 7, 13, 9, 0);
    let clock = Arc::new(FakeClock::new(fired_at));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock);

    // At cap: nothing fires, and — the point of the fix — the minute is NOT
    // durably claimed, so catch-up cannot later mistake it for fired.
    assert_eq!(scheduler.tick().await, 0, "a capped fire starts no run");
    assert!(started.lock().unwrap().is_empty(), "no run was recorded");
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        None,
        "a fire rejected by the cap must NOT claim the minute"
    );

    // Free the slot; the SAME minute now fires, proving the occurrence was
    // only deferred, never lost.
    drop(filler);
    assert_eq!(
        scheduler.tick().await,
        1,
        "the freed slot lets the deferred fire land"
    );
    wait_for(|| started.lock().unwrap().len() == 1).await;
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        Some(fired_at / MINUTE_MS),
        "the admitted fire now claims the minute exactly once"
    );
}

/// Issue #661: the restart catch-up make-up is subject to the same cap. At
/// the #401 in-flight cap on first sight it must DEFER — dropping the
/// first-sight latch and leaving the missed minute unclaimed — so a later
/// tick re-attempts it once a slot frees, rather than claiming the missed
/// minute and losing the make-up when `begin` rejects the run.
#[tokio::test]
async fn a_catch_up_at_the_in_flight_cap_is_deferred_not_burned() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays_capped(
        &home,
        "acme",
        vec![overlay("digest", Some("0 9 * * MON"))],
        Some(runner),
        "running",
        Some(1),
    )
    .await;
    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();
    // Anchor two Mondays back; the most recent missed Monday is 2026-07-13.
    let anchor = minute_at(2026, 7, 6, 9, 0);
    runtime
        .schedule_fires()
        .claim_fire(&company, "workflow-digest", anchor)
        .await
        .unwrap();

    // Fill the single slot so the company is at its cap.
    let supervisor = runtime.run_supervisor().clone();
    let (_ctx, filler) = supervisor
        .begin("filler", false)
        .expect("fills the cap of 1");

    // "Now" is a Tuesday: the current minute never matches, isolating the
    // catch-up as the only possible fire.
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    // At cap on first sight: catch-up is deferred, nothing fires, and the
    // missed minute is NOT claimed (the anchor stays where it was).
    assert_eq!(scheduler.tick().await, 0, "a capped catch-up starts no run");
    assert!(started.lock().unwrap().is_empty());
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        Some(anchor),
        "the missed minute must not be claimed while deferred"
    );

    // Free the slot; because the first-sight latch was dropped, the next tick
    // re-attempts the catch-up and it now lands.
    drop(filler);
    assert_eq!(
        scheduler.tick().await,
        1,
        "the deferred catch-up fires once a slot frees"
    );
    wait_for(|| started.lock().unwrap().len() == 1).await;
    assert_eq!(started.lock().unwrap()[0].input["catchUp"], true);
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        Some(minute_at(2026, 7, 13, 9, 0)),
        "the made-up minute is claimed only once it actually runs"
    );
}

/// Issue #661 (reviewer): the "retries on the next tick" story is FALSE for
/// any schedule coarser than `* * * * *`. A steady-state fire deferred at the
/// in-flight cap leaves its minute unclaimed and unfired; the schedule's next
/// expression match is a whole day away, so recovery is the restart-style
/// catch-up path — which a LATER tick re-attempts while the missed minute is
/// still inside the catch-up window, without a process restart. This proves
/// both halves on a genuinely coarse `0 9 * * *`: the make-up lands at 09:01,
/// a minute the expression does NOT match.
#[tokio::test]
async fn a_coarse_steady_state_fire_at_cap_is_made_up_by_catch_up_not_the_next_match() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays_capped(
        &home,
        "acme",
        vec![overlay("digest", Some("0 9 * * *"))], // daily at 09:00 — coarse
        Some(runner),
        "running",
        Some(1),
    )
    .await;
    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();

    // Anchor at YESTERDAY's 09:00 — the most recent prior occurrence — so the
    // first-sight catch-up finds nothing to make up on the fire tick, and the
    // STEADY-STATE arm is the one that defers today's 09:00. Without this the
    // catch-up would own the deferral and we'd be re-testing that path.
    let yesterday_nine = minute_at(2026, 7, 13, 9, 0);
    runtime
        .schedule_fires()
        .claim_fire(&company, "workflow-digest", yesterday_nine)
        .await
        .unwrap();

    // Fill the only slot so the company is at its cap.
    let supervisor = runtime.run_supervisor().clone();
    let (_ctx, filler) = supervisor
        .begin("filler", false)
        .expect("fills the cap of 1");

    // Today 09:00 matches; at cap the steady-state fire is deferred, unclaimed.
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock.clone());
    assert_eq!(
        scheduler.tick().await,
        0,
        "a capped coarse fire starts no run"
    );
    assert!(started.lock().unwrap().is_empty());
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        Some(yesterday_nine),
        "the deferred coarse minute is NOT claimed — the anchor stays at yesterday's fire"
    );

    // A minute later the slot frees. 09:01 does NOT match `0 9 * * *`, so if
    // the only recovery were the next expression match nothing would fire
    // until tomorrow. Instead the catch-up re-attempt makes up today's 09:00.
    drop(filler);
    clock.set(millis_at(2026, 7, 14, 9, 1));
    assert_eq!(
        scheduler.tick().await,
        1,
        "the deferred coarse minute is made up by catch-up once a slot frees — at a NON-matching minute"
    );
    wait_for(|| started.lock().unwrap().len() == 1).await;
    let today_nine = minute_at(2026, 7, 14, 9, 0);
    assert_eq!(started.lock().unwrap()[0].input["catchUp"], true);
    assert_eq!(
        started.lock().unwrap()[0].input["firedAtMs"],
        today_nine * MINUTE_MS,
        "the make-up carries today's 09:00, the minute that was deferred — not 09:01"
    );
    assert_eq!(
        runtime
            .schedule_fires()
            .latest_fire(&company, "workflow-digest")
            .await
            .unwrap(),
        Some(today_nine),
        "the made-up minute is claimed exactly once, only when it actually runs"
    );
}

/// The exact-count property on the default current-thread runtime.
#[tokio::test]
async fn siblings_due_the_same_minute_admit_exactly_the_cap_current_thread() {
    assert_siblings_admit_exactly_the_cap_and_burn_no_minute(1).await;
    assert_siblings_admit_exactly_the_cap_and_burn_no_minute(2).await;
}

/// And on a multi-thread runtime, where the spawned run tasks really do run
/// on other worker threads while the tick loop keeps walking its siblings —
/// the case a stale, off-thread cap read would get wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn siblings_due_the_same_minute_admit_exactly_the_cap_multi_thread() {
    assert_siblings_admit_exactly_the_cap_and_burn_no_minute(1).await;
    assert_siblings_admit_exactly_the_cap_and_burn_no_minute(2).await;
}
