use super::tests_core::*;

/// **Issue #440: the cron path and the shared path record the same thing.**
///
/// The scheduler used to keep its own copy of "mint the id through the
/// supervisor, journal the outcome on both arms" — the two rules
/// [`WorkflowSpawn`](crate::runtime::WorkflowSpawn) owns. The copies agreed,
/// which is what made the duplication dangerous rather than harmless: a fix
/// to one would silently miss the other and no test would notice.
///
/// So this runs the same graph through both entry points over one company
/// (one event log, one runner, one supervisor) and asserts the journaled
/// records differ in exactly two places: the `scheduled` flag, which is the
/// one thing the two paths genuinely mean differently, and the run id, which
/// is fresh per run by construction. Everything else — the workflow id, the
/// delivery rows, the pending approvals, the error and the cancelled flag —
/// must match, and a divergence introduced on either side fails here.
#[tokio::test]
async fn a_cron_fire_and_a_direct_spawn_journal_the_same_record() {
    use crate::company::parse_workflow;

    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let company = "spawn-parity-co";
    // Non-empty delivery rows, so the comparison covers the part of the
    // record most likely to be dropped by one path and not the other.
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

    // --- path 1: the cron fire.
    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| completed.load(Ordering::SeqCst) == 1).await;
    wait_for_outcomes(&registry, company, 1).await;

    // --- path 2: the shared primitive, driven directly over the SAME
    // runtime — same event log, same runner, same supervisor.
    let runtime = registry
        .get(&CompanyId::new(company))
        .expect("the company is registered");
    let runner = runtime
        .workflow_runner()
        .cloned()
        .expect("the same runner the scheduler used");
    let workflow = parse_workflow(&body("digest", Some("* * * * *"))).expect("parses");
    let (_run_id, handle) = crate::runtime::WorkflowSpawn::new(&runtime, runner)
        .spawn(workflow, json!({}), false, false)
        .expect("under the run cap");
    handle.await.expect("the run task completes").expect("runs");
    let outcomes = wait_for_outcomes(&registry, company, 2).await;

    let [cron, direct] = [&outcomes[0], &outcomes[1]].map(|event| {
        let CompanyEvent::WorkflowRunFinished {
            workflow_id,
            scheduled,
            run_id,
            deliveries,
            pending_approvals,
            error,
            cancelled,
            notices: _,
            board: _,
            blocked_nodes: _,
            approvals: _,
        } = event
        else {
            unreachable!("filtered above")
        };
        (
            workflow_id,
            scheduled,
            run_id,
            deliveries,
            pending_approvals,
            error,
            cancelled,
        )
    });

    // The two legitimate differences.
    assert!(*cron.1, "a cron fire is scheduled");
    assert!(!*direct.1, "a directly spawned run is not");
    assert!(cron.2.is_some() && direct.2.is_some(), "both carry an id");
    assert_ne!(cron.2, direct.2, "each run is its own causal root");

    // Everything else is the same record, written by the same code.
    assert_eq!(cron.0, direct.0, "workflow id");
    assert_eq!(cron.3, direct.3, "delivery rows");
    assert_eq!(cron.4, direct.4, "pending approvals");
    assert_eq!(cron.5, direct.5, "error");
    assert_eq!(cron.6, direct.6, "cancelled");
    assert_eq!(cron.3.len(), 2, "the fixture's rows really did survive");
}

/// The arm that was quietest of all: a scheduled run that failed outright
/// produced one host-stdout warning and nothing durable. Now it records the
/// error where an operator can find it.
#[tokio::test]
async fn a_failed_scheduled_run_journals_the_error() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let company = "journal-failure-co";
    let (runner, attempts) = FailingRunner::new("no inference source for agent node `worker`");
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
    wait_for(|| attempts.load(Ordering::SeqCst) == 1).await;
    let outcomes = loop {
        let outcomes = run_outcomes(&registry, company).await;
        if !outcomes.is_empty() {
            break outcomes;
        }
        tokio::task::yield_now().await;
    };

    assert_eq!(outcomes.len(), 1);
    let CompanyEvent::WorkflowRunFinished {
        scheduled,
        deliveries,
        error,
        ..
    } = &outcomes[0]
    else {
        unreachable!("filtered above")
    };
    assert!(*scheduled);
    assert!(deliveries.is_empty(), "a run that died routed nothing");
    assert!(
        error
            .as_deref()
            .is_some_and(|e| e.contains("no inference source")),
        "the failure reason must survive: {error:?}"
    );
}

/// A workflow with no schedule is never fired by the scheduler — it stays
/// manual-run only.
#[tokio::test]
async fn an_unscheduled_workflow_never_fires() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("manual", None)],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 0);
    assert!(started.lock().unwrap().is_empty());
}

/// A paused company fires nothing — the same `ensure_running` guard the
/// manifest cron scheduler uses, so schedules resume on unpause.
#[tokio::test]
async fn a_paused_company_is_skipped() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("* * * * *"))],
        Some(runner),
        "paused",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 0);
    assert!(started.lock().unwrap().is_empty());
}

/// Codex review finding on PR #2140 (`3952230576`): the emergency stop is a
/// separate switch from `lifecycle` — a stopped company still reports
/// `running` — so this tick must check it independently of the ordinary
/// pause skip proven above, or a cron fire starts a new, billed run while
/// the company reports itself stopped.
#[tokio::test]
async fn an_emergency_stopped_company_is_skipped() {
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
    let runtime = registry
        .get(&CompanyId::new("acme"))
        .expect("registered above");
    runtime
        .emergency_pause(
            crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::Operator,
                id: "owner".into(),
            },
            None,
        )
        .await
        .expect("pause");
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 0);
    assert!(started.lock().unwrap().is_empty());
}

/// No runner wired (the default build) is a clean no-op, not an error — the
/// same port seam the run route reports `not_wired` on. Because the company
/// *does* have a scheduled workflow, the skip is announced — exactly once,
/// no matter how many minutes pass, so a once-a-minute tick cannot bury the
/// signal it is raising.
#[tokio::test]
async fn no_runner_wired_is_a_noop_and_warns_once() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("* * * * *"))],
        None,
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock.clone());

    assert_eq!(scheduler.tick().await, 0);
    assert_eq!(scheduler.unwired_warnings, 1, "the first skip must be said");

    // Several more matching minutes: still skipped, still silent.
    for minute in 1..5 {
        clock.set(millis_at(2026, 7, 13, 9, minute));
        assert_eq!(scheduler.tick().await, 0);
    }
    assert_eq!(
        scheduler.unwired_warnings, 1,
        "the warning must not repeat every tick"
    );
}

/// A company with no runner AND no scheduled workflows is not
/// misconfigured — it simply has nothing to run, so it stays silent.
#[tokio::test]
async fn no_runner_and_no_schedules_does_not_warn() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("manual", None)],
        None,
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 0);
    assert_eq!(
        scheduler.unwired_warnings, 0,
        "a company with nothing scheduled deserves silence"
    );
}

/// The latch is re-armed when the situation changes: a schedule saved onto a
/// still-unwired company warns, even though an earlier tick already found
/// that company unwired (with nothing scheduled) and said nothing.
#[tokio::test]
async fn a_schedule_added_later_still_warns() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let company = CompanyId::new("acme");
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("manual", None)],
        None,
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock.clone());

    assert_eq!(scheduler.tick().await, 0);
    assert_eq!(scheduler.unwired_warnings, 0);

    // The operator saves a schedule on the (still unwired) company.
    let runtime = registry.get(&company).expect("registered");
    let store = runtime.store().clone();
    let mut record = store.load(&company).await.unwrap().unwrap();
    record.overlay_workflows = vec![overlay("digest", Some("* * * * *"))];
    store.save(&record).await.unwrap();

    clock.set(millis_at(2026, 7, 13, 9, 1));
    assert_eq!(scheduler.tick().await, 0);
    assert_eq!(
        scheduler.unwired_warnings, 1,
        "a schedule saved onto an unwired company must be reported"
    );
}

/// A malformed graph body skips only itself: the healthy scheduled workflow
/// beside it still fires.
#[tokio::test]
async fn a_malformed_graph_skips_only_itself() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (runner, started, _completed) = RecordingRunner::new();
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![
            OverlayWorkflow {
                id: "broken".to_string(),
                toml: "id = \"broken\"\nname =".to_string(),
            },
            overlay("healthy", Some("* * * * *")),
        ],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock);

    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    assert_eq!(started.lock().unwrap()[0].workflow, "healthy");
}

/// **Issue #383: a cron fire is cancellable from the console.**
///
/// This is the claim the scheduler's comment makes and nothing pinned. It
/// is worth pinning because the two things that make it true are both easy
/// to undo without breaking anything else: the run id has to be minted
/// through the supervisor **inside** the spawned task, and its guard has to
/// be held across `record_run_finished` rather than dropped after the run.
/// Starting the run unregistered, or binding the guard to `_`, would still
/// pass every other test in this module — the run would fire, complete and
/// journal exactly as before, and simply stop being stoppable.
///
/// Since issue #440 both are `WorkflowSpawn`'s to keep rather than this
/// module's, which is the point of routing through it — but the claim is
/// the scheduler's to make, so the test stays here.
///
/// The cron case matters more than the manual one: nobody chose the timing,
/// and a wedged nightly run holds its overlap claim, suppressing every later
/// fire of that schedule until it ends.
#[tokio::test]
async fn a_scheduled_run_can_be_cancelled_while_it_is_in_flight() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let company = "cancel-cron-co";
    let gate = Arc::new(Semaphore::new(0));
    let (runner, started, _completed) = RecordingRunner::gated(gate.clone());
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
    wait_for(|| started.lock().unwrap().len() == 1).await;

    // Discover the run the way the cancel route does — off the company's
    // own supervisor. Nobody handed this id to anybody: the scheduler minted
    // it inside a spawned task, which is exactly why registration is what
    // makes a cron fire reachable at all.
    let runtime = registry
        .get(&CompanyId::new(company))
        .expect("the company is registered");
    wait_for(|| !runtime.run_supervisor().is_empty()).await;
    let live = runtime.run_supervisor().live();
    assert_eq!(live.len(), 1, "the in-flight cron run is registered");
    let (run_id, workflow_id) = live.into_iter().next().unwrap();
    assert_eq!(workflow_id, "digest");

    assert!(
        runtime.run_supervisor().cancel(&run_id),
        "an in-flight cron run is cancellable"
    );

    // It settles as stopped — never released through the gate, so the only
    // way it could have finished is the cancel.
    let outcomes = loop {
        let outcomes = run_outcomes(&registry, company).await;
        if !outcomes.is_empty() {
            break outcomes;
        }
        tokio::task::yield_now().await;
    };
    assert_eq!(outcomes.len(), 1);
    let CompanyEvent::WorkflowRunFinished {
        scheduled,
        cancelled,
        error,
        run_id: journaled_id,
        ..
    } = &outcomes[0]
    else {
        unreachable!("filtered above")
    };
    assert!(
        *scheduled,
        "a cron fire stays flagged scheduled when stopped"
    );
    assert!(*cancelled, "the outcome must record the stop");
    assert!(error.is_none(), "a stop is not a failure: {error:?}");
    assert_eq!(
        journaled_id.as_deref(),
        Some(run_id.as_str()),
        "the outcome carries the id the supervisor registered"
    );

    // And the guard let go — held across the journal write above, released
    // once the task ended.
    wait_for(|| runtime.run_supervisor().is_empty()).await;
}

/// A scheduled run still executing suppresses the next fire; once it
/// completes, the workflow becomes schedulable again.
#[tokio::test]
async fn an_in_flight_run_suppresses_the_next_fire() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let gate = Arc::new(Semaphore::new(0));
    let (runner, started, completed) = RecordingRunner::gated(gate.clone());
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("slow", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock.clone());

    // Minute 1: fires, and the run parks on the gate.
    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| started.lock().unwrap().len() == 1).await;
    assert!(scheduler.is_running_any());

    // Minute 2: matches, but the previous run is still in flight.
    clock.set(millis_at(2026, 7, 13, 9, 1));
    assert_eq!(scheduler.tick().await, 0);
    assert_eq!(started.lock().unwrap().len(), 1);

    // Let the first run finish, then wait for its task to release the claim.
    gate.add_permits(1);
    wait_for(|| completed.load(Ordering::SeqCst) == 1).await;
    drain(&scheduler).await;

    // Minute 3: the workflow is schedulable again.
    gate.add_permits(1);
    clock.set(millis_at(2026, 7, 13, 9, 2));
    assert_eq!(
        scheduler.tick().await,
        1,
        "the workflow must be schedulable once its run completed"
    );
    wait_for(|| started.lock().unwrap().len() == 2).await;
}

/// A company removed from the registry (archived) is swept out of the
/// warning latch. Neither re-arm path in `note_unwired` can reach it — both
/// require the company to still be visited by a tick — so without the sweep
/// its entry would be orphaned for the life of the process.
#[tokio::test]
async fn a_removed_company_is_swept_from_the_warning_latch() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let company = CompanyId::new("acme");
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("digest", Some("* * * * *"))],
        None, // no runner, so the company latches a warning
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock.clone());

    assert_eq!(scheduler.tick().await, 0);
    assert_eq!(scheduler.unwired_warnings, 1);
    assert!(scheduler.warned_unwired.contains(&company));

    // The company is archived out of the registry.
    assert!(registry.remove(&company).is_some());

    clock.set(millis_at(2026, 7, 13, 9, 1));
    assert_eq!(scheduler.tick().await, 0);
    assert!(
        scheduler.warned_unwired.is_empty(),
        "a company that no longer exists must not be held in the latch"
    );
}

/// The dedupe map keeps only the current minute. Anything older can never
/// dedupe again, and retaining it would grow the map forever — one entry per
/// (company, workflow) ever fired, including deleted ones.
#[tokio::test]
async fn the_dedupe_map_does_not_grow_without_bound() {
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
    let mut scheduler = WorkflowScheduler::new(registry, clock.clone());

    assert_eq!(scheduler.tick().await, 1);
    // The entry for the minute just fired is kept — that is what dedupes a
    // second tick inside the same minute.
    assert_eq!(scheduler.last_fired.len(), 1);
    clock.advance(30_000);
    assert_eq!(scheduler.tick().await, 0, "same minute: deduped");

    // Many minutes later the map still holds exactly one entry, not one per
    // minute elapsed. Each iteration drains first: without that, the
    // previous run may still hold its claim and the overlap guard would
    // (correctly) refuse the fire.
    for minute in 1..10 {
        drain(&scheduler).await;
        clock.set(millis_at(2026, 7, 13, 9, minute));
        assert_eq!(scheduler.tick().await, 1, "minute {minute}");
        assert_eq!(scheduler.last_fired.len(), 1, "minute {minute}");
    }
    wait_on_time(|| started.lock().unwrap().len() == 10).await;
}

/// A runner that panics must not strand the in-flight claim. Releasing the
/// slot after the `await` would leave the key set forever, silently retiring
/// that schedule for the life of the process; the RAII [`Claim`] releases it
/// on the unwind instead.
#[tokio::test]
async fn a_panicking_run_releases_its_claim() {
    struct PanickingRunner {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl WorkflowRunner for PanickingRunner {
        async fn run(
            &self,
            _company: &CompanyId,
            _workflow: &WorkflowFile,
            _input: Value,
            _ctx: &crate::ports::WorkflowRunContext,
        ) -> crate::Result<WorkflowRun> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            panic!("the runner blew up mid-run");
        }
    }

    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let calls = Arc::new(AtomicUsize::new(0));
    let runner = Arc::new(PanickingRunner {
        calls: calls.clone(),
    });
    let registry = company_with_overlays(
        &home,
        "acme",
        vec![overlay("boom", Some("* * * * *"))],
        Some(runner),
        "running",
    )
    .await;
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 13, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry, clock.clone());

    // Minute 1: fires, and the run panics inside its task.
    assert_eq!(scheduler.tick().await, 1);
    wait_for(|| calls.load(Ordering::SeqCst) == 1).await;
    // The unwind must have released the slot.
    drain(&scheduler).await;

    // Minute 2: the schedule is still alive. Before the RAII guard this
    // fired 0 forever.
    clock.set(millis_at(2026, 7, 13, 9, 1));
    assert_eq!(
        scheduler.tick().await,
        1,
        "a panicking run must not retire the schedule"
    );
    wait_for(|| calls.load(Ordering::SeqCst) == 2).await;
}
