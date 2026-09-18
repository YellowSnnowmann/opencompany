use super::*;

/// A poisoned supervisor must report **busy**, not panic.
///
/// `is_empty` is read by `CompanyRuntime::is_busy` behind
/// `GET /healthz/busy`, whose router has no `CatchPanicLayer`. A panic there
/// resets the connection, the manager reads that as "cannot tell", and its
/// default is to park — destroying the in-flight work the endpoint exists to
/// protect, at the one moment the registry's state is unknown (issue #1239).
///
/// `len` is the other half of the contract: it is test-and-diagnostics only,
/// so it keeps its `expect`, and `is_empty` therefore cannot be written as
/// `self.len() == 0` — which is precisely how the panic got onto the
/// production path in the first place.
#[test]
fn a_poisoned_supervisor_reports_busy_instead_of_panicking() {
    let supervisor = RunSupervisor::new();
    assert!(supervisor.is_empty(), "an untouched supervisor is empty");

    supervisor.poison_for_test();

    assert!(
        !supervisor.is_empty(),
        "a poisoned supervisor must read as not-empty, so is_busy reports busy \
         and the manager does not park a company mid-run"
    );
}

/// The core loop: a begun run is registered under the id its context
/// carries, cancelling fires the signal that context holds, and the guard
/// takes the entry away.
#[test]
fn begin_registers_cancel_fires_and_the_guard_deregisters() {
    let supervisor = RunSupervisor::new();
    assert!(supervisor.is_empty());

    let (ctx, guard) = supervisor
        .begin("digest", false)
        .expect("the first run is under any cap");
    assert_eq!(supervisor.len(), 1);
    assert_eq!(guard.run_id(), ctx.run_id);
    assert!(!ctx.cancel.is_cancelled());

    assert_eq!(
        supervisor.live(),
        vec![(ctx.run_id.clone(), "digest".to_string())],
        "a registered run is discoverable by a caller that did not start it"
    );

    assert!(supervisor.cancel(&ctx.run_id), "a live run cancels");
    assert!(
        ctx.cancel.is_cancelled(),
        "the signal the runner is selecting on is the one the supervisor fired"
    );

    drop(guard);
    assert!(supervisor.is_empty());
}

/// The two 404 cases, which are one case: nothing to stop. A settled run is
/// indistinguishable from one that never existed, and deliberately so —
/// keeping a tombstone would mean deciding when to expire it.
#[test]
fn cancelling_an_unknown_or_settled_run_reports_false() {
    let supervisor = RunSupervisor::new();
    assert!(!supervisor.cancel("never-existed"));

    let (ctx, guard) = supervisor
        .begin("digest", false)
        .expect("under the default cap");
    drop(guard);
    assert!(
        !supervisor.cancel(&ctx.run_id),
        "a settled run is no longer cancellable"
    );
}

/// The guard deregisters on an **unwind**, not just on a clean return. This
/// is the case a manual `deregister()` call at the end of the run body would
/// get wrong, and it is why this is a `Drop` type: a panicking run task must
/// not leave a permanently-cancellable ghost behind.
#[test]
fn the_guard_deregisters_when_the_run_panics() {
    let supervisor = RunSupervisor::new();
    let outer = supervisor.clone();
    let result = std::panic::catch_unwind(move || {
        let (_ctx, _guard) = outer.begin("digest", false).expect("under the default cap");
        assert_eq!(outer.len(), 1);
        panic!("the run blew up");
    });
    assert!(result.is_err(), "the panic really happened");
    assert!(
        supervisor.is_empty(),
        "the guard unwound and took the entry with it"
    );
}

/// Two runs of the same graph coexist and are cancelled independently. The
/// host places no cap on concurrent runs of one workflow (the console keeps
/// its own per-workflow guard), so the map must key on the run rather than
/// on the graph.
#[test]
fn concurrent_runs_of_one_workflow_cancel_independently() {
    let supervisor = RunSupervisor::new();
    let (first, _first_guard) = supervisor
        .begin("digest", false)
        .expect("under the default cap");
    let (second, _second_guard) = supervisor
        .begin("digest", true)
        .expect("under the default cap");
    assert_eq!(supervisor.len(), 2);
    assert_ne!(first.run_id, second.run_id);

    assert!(supervisor.cancel(&second.run_id));
    assert!(second.cancel.is_cancelled());
    assert!(
        !first.cancel.is_cancelled(),
        "cancelling one run leaves the other alone"
    );
}

/// A cancel that lands *before* anything awaits the signal is still
/// observed. This is the property the watch channel buys over a `Notify`,
/// and losing it would make a cancel racing a slow node hang until the run
/// finished on its own.
#[tokio::test]
async fn a_cancel_before_the_await_is_still_seen() {
    let supervisor = RunSupervisor::new();
    let (ctx, _guard) = supervisor
        .begin("digest", false)
        .expect("under the default cap");
    supervisor.cancel(&ctx.run_id);

    tokio::time::timeout(std::time::Duration::from_secs(1), ctx.cancel.cancelled())
        .await
        .expect("an already-fired signal resolves immediately");
}

/// And a cancel that lands *after* the await started wakes it.
#[tokio::test]
async fn a_cancel_after_the_await_wakes_it() {
    let supervisor = RunSupervisor::new();
    let (ctx, _guard) = supervisor
        .begin("digest", false)
        .expect("under the default cap");
    let waiter = tokio::spawn({
        let cancel = ctx.cancel.clone();
        async move { cancel.cancelled().await }
    });
    // Yield so the waiter is definitely parked before the signal fires.
    tokio::task::yield_now().await;
    supervisor.cancel(&ctx.run_id);

    tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("the waiter woke")
        .expect("the waiter did not panic");
}

/// Issue #401: the ceiling admits exactly `limit` runs and refuses the next,
/// naming the limit it enforced. Held guards stand in for in-flight runs, so
/// the property is proven without a single spawned task or wall-clock wait.
#[test]
fn the_cap_admits_up_to_the_limit_then_refuses() {
    let supervisor = RunSupervisor::with_limit(2);
    assert_eq!(supervisor.limit(), 2);

    let (_first, _g1) = supervisor
        .begin("digest", false)
        .expect("first run is under the cap of 2");
    let (_second, _g2) = supervisor
        .begin("digest", false)
        .expect("second run reaches the cap of 2");
    assert_eq!(supervisor.len(), 2);

    match supervisor.begin("digest", false) {
        Err(OpenCompanyError::WorkflowRunLimit { limit }) => assert_eq!(limit, 2),
        Ok(_) => panic!("a third run must be refused, not admitted, at the cap of 2"),
        Err(other) => panic!("expected a run-limit refusal, got {other:?}"),
    }
    assert_eq!(
        supervisor.len(),
        2,
        "a refused run registers nothing — the map is untouched"
    );
}

/// **Codex review findings on PR #2140 (`3952368160`, `3952368162`,
/// `3951723397`).** This is the choke point every workflow-run entry point
/// funnels through — the manual run route, the cron scheduler, an approved
/// gate's resume, a reconciled blocked-node dispatch, an expiry that
/// releases a workflow run, and the orchestrator's `run_workflow` tool.
/// Each already asks `CompanyRuntime::ensure_not_emergency_stopped` early,
/// but that ask sits behind at least one `.await` before the run is
/// actually admitted — this proves the recheck under `begin`'s own lock
/// closes that window, and that it correctly leaves every other supervisor
/// (the ones with no company to ask) admitting exactly as before.
#[test]
fn begin_refuses_once_the_installed_emergency_gate_engages() {
    let gate = std::sync::Arc::new(crate::policy::gate::ManifestApprovalGate::new(
        crate::company::Policy {
            mode: "full".to_string(),
            always_approve: Vec::new(),
            auto_approve_under_usd: None,
            approval_ttl_hours: None,
        },
    ));
    let supervisor = RunSupervisor::with_limit(2).with_emergency_gate(gate.clone());

    let (_ctx, _guard) = supervisor
        .begin("digest", false)
        .expect("not stopped yet, so the first run is admitted");

    gate.set_emergency(true);
    match supervisor.begin("digest", false) {
        Err(OpenCompanyError::EmergencyStop(_)) => {}
        Ok(_) => panic!("a run must be refused once the installed gate is stopped"),
        Err(other) => panic!("expected an emergency-stop refusal, got {other:?}"),
    }
    assert_eq!(
        supervisor.len(),
        1,
        "the refused run registers nothing — only the pre-stop run is in the map"
    );

    gate.set_emergency(false);
    let (_ctx2, _guard2) = supervisor
        .begin("digest", false)
        .expect("releasing the stop restores ordinary admission");
}

/// A supervisor built with no [`with_emergency_gate`](RunSupervisor::with_emergency_gate)
/// call — every construction site with no company to ask — admits
/// regardless of any flag, exactly as before this recheck existed.
#[test]
fn begin_ignores_emergency_state_with_no_gate_installed() {
    let supervisor = RunSupervisor::with_limit(1);
    supervisor
        .begin("digest", false)
        .expect("no gate installed, so nothing here can refuse on that basis");
}

/// Issue #401: dropping a guard frees the slot it held, so a run refused at
/// the ceiling succeeds once an in-flight run settles. This is the RAII
/// release the whole design leans on — no second ledger to keep in step.
#[test]
fn dropping_a_guard_frees_a_slot_for_a_refused_run() {
    let supervisor = RunSupervisor::with_limit(1);
    let (_first, guard) = supervisor
        .begin("digest", false)
        .expect("first run fills the cap of 1");

    assert!(
        matches!(
            supervisor.begin("digest", false),
            Err(OpenCompanyError::WorkflowRunLimit { limit: 1 })
        ),
        "the second run is refused while the first holds the only slot"
    );

    drop(guard);
    let (_second, _g2) = supervisor
        .begin("digest", false)
        .expect("the freed slot admits a new run");
    assert_eq!(supervisor.len(), 1);
}

/// Issue #401: a **panicking** run frees its capped slot on the unwind, not
/// just on a clean return. Extends the existing panic test to prove the
/// ceiling recovers — a run that blew up must not permanently retire a slot.
#[test]
fn a_panicking_run_frees_its_capped_slot() {
    let supervisor = RunSupervisor::with_limit(1);
    let outer = supervisor.clone();
    let result = std::panic::catch_unwind(move || {
        let (_ctx, _guard) = outer.begin("digest", false).expect("fills the cap of 1");
        assert_eq!(outer.len(), 1);
        panic!("the run blew up while holding the only slot");
    });
    assert!(result.is_err(), "the panic really happened");
    assert_eq!(
        supervisor.len(),
        0,
        "the guard unwound and freed the slot it held against the cap"
    );
    supervisor
        .begin("digest", false)
        .expect("the recovered slot admits a fresh run");
}
