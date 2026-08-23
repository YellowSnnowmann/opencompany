//! Issue #383: the live set of workflow runs an operator can still stop.
//!
//! A run used to be reachable only as an in-flight HTTP request. Nothing held a
//! handle to it, so there was nowhere to send "stop" — which is why the issue is
//! *both* halves at once: a run has to be addressable while it is running before
//! it can be cancellable at all.
//!
//! [`RunSupervisor`] is that address book. [`begin`](RunSupervisor::begin) mints
//! the run's [`WorkflowRunContext`] — the same context, carrying the same
//! `run_id` issue #371 already correlates progress events on, so **no second
//! identifier is introduced** — and registers its [`RunCancel`] handle;
//! [`cancel`](RunSupervisor::cancel) fires that handle;
//! [`RunGuard`]'s `Drop` deregisters the entry on every exit path.
//!
//! ## What it holds, and what it deliberately does not
//!
//! It holds **cancel handles, not [`JoinHandle`](tokio::task::JoinHandle)s**. A
//! run settles itself and its guard reaps the entry; the supervisor never needs
//! to abort a task, and holding join handles would mean deciding who reaps a run
//! nobody is waiting on. The shape is the one
//! [`InflightRegistry`](crate::company::steer::InflightRegistry) already uses
//! for steerable turns, for the same RAII reason.
//!
//! It is also **not** how a host that dies mid-run is cleaned up. That is
//! already solved: a dead host's runs are settled at the next boot by
//! [`sweep_interrupted_runs`](super::sweep_interrupted_runs), which needs no
//! help from an in-memory map (and could not get any — the map dies with the
//! process). This module adds no sweep of its own.
//!
//! ## Two known gaps, both inherited rather than introduced
//!
//! * A **panicking** run task unwinds, so its guard drops and the entry goes —
//!   but nothing journals a `WorkflowRunFinished`, so the run reads
//!   `running: true` in `GET …/workflows/runs` until the next restart sweeps it.
//!   That is the same exposure #371 accepted for a run whose journal append
//!   failed, and the same remedy settles both.
//! * A live runtime swap ([`rebuild_company`](super::rebuild_company)) gives the
//!   successor runtime a fresh supervisor, so a run registered on the old one
//!   can no longer be cancelled (it still finishes and still journals). This
//!   matches how the steer registry behaves across a rebuild — see
//!   [`RuntimeHandover`](super::RuntimeHandover) — and cancelling is a
//!   best-effort operator convenience, not a correctness guarantee.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::company::DEFAULT_MAX_IN_FLIGHT_RUNS;
use crate::error::OpenCompanyError;
use crate::ports::{RunCancel, WorkflowRunContext};

/// One registered run: its stop signal, plus the graph it belongs to for the log
/// line the cancel route emits.
struct Slot {
    workflow_id: String,
    cancel: RunCancel,
}

/// The live set of cancellable workflow runs, keyed by run id.
///
/// One per [`CompanyRuntime`](crate::company::runtime::CompanyRuntime), so the
/// key needs no company component — every reader already resolved a company
/// before it got here. Cheap to [`Clone`] (a shared handle): the same map is
/// seen by the HTTP run route that registers runs, the cancel route that fires
/// them, the cron scheduler, and the orchestrator's `run_workflow` tool.
///
/// # The concurrency cap lives here (issue #401)
///
/// Every entry point that starts a run — the manual run route, the cron
/// scheduler, an approved gate's continuation, and the orchestrator's
/// `run_workflow` tool — reaches a run through [`begin`](Self::begin), so this
/// map is the one choke point where a per-company ceiling can be enforced
/// without trusting each caller to remember it. `begin` is fallible for exactly
/// that reason: the check and the insert happen under the same lock, so the
/// count can never be raced past the limit, and the compiler forces every
/// present and future caller to handle a refusal rather than silently
/// overshoot. A run over the ceiling is refused, never queued.
#[derive(Clone)]
pub struct RunSupervisor {
    inner: Arc<Mutex<HashMap<String, Slot>>>,
    /// The most runs that may be registered at once. Enforced by
    /// [`begin`](Self::begin) under the map lock.
    limit: usize,
}

impl Default for RunSupervisor {
    /// A supervisor with the default ceiling
    /// ([`DEFAULT_MAX_IN_FLIGHT_RUNS`]). The builder overrides it per company
    /// from the manifest via [`with_limit`](Self::with_limit); this default is
    /// what the default build's runtime (which can start no run at all) keeps.
    fn default() -> Self {
        Self::with_limit(DEFAULT_MAX_IN_FLIGHT_RUNS)
    }
}

impl RunSupervisor {
    /// An empty supervisor with the default concurrency ceiling.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty supervisor that admits at most `limit` concurrent runs
    /// (issue #401).
    ///
    /// The builder constructs one per company from
    /// `manifest.workflows.max_in_flight_runs`, which validation has already
    /// held at `>= 1`.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            limit,
        }
    }

    /// This supervisor's concurrency ceiling. For diagnostics and tests.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Mints a run context and registers its stop signal, or refuses when the
    /// company is already at its concurrency ceiling (issue #401).
    ///
    /// `workflow_id` is the graph about to run and `scheduled` says whether a
    /// cron started it — both ride the context exactly as they did before, so
    /// on the success path this is a drop-in for [`WorkflowRunContext::new`]
    /// that additionally makes the run reachable.
    ///
    /// The ceiling check and the registration happen under **one** hold of the
    /// map lock, so two callers racing at the boundary cannot both see room and
    /// both insert — the count is authoritative, not a separate counter that
    /// could drift from the map [`RunGuard`] drops entries from. On refusal no
    /// context is minted: there is deliberately no run id to hand back, because
    /// nothing started.
    ///
    /// The returned [`RunGuard`] MUST be held for the duration of the run.
    /// Dropping it deregisters the entry, which is what keeps a settled run from
    /// lingering as a cancellable one *and* frees the slot it held against the
    /// ceiling — and because it is a `Drop`, that holds on the error and panic
    /// paths too.
    pub fn begin(
        &self,
        workflow_id: &str,
        scheduled: bool,
    ) -> Result<(WorkflowRunContext, RunGuard)> {
        let mut map = self.inner.lock().expect("run supervisor poisoned");
        if map.len() >= self.limit {
            return Err(OpenCompanyError::WorkflowRunLimit { limit: self.limit });
        }
        let ctx = WorkflowRunContext::new(scheduled);
        map.insert(
            ctx.run_id.clone(),
            Slot {
                workflow_id: workflow_id.to_string(),
                cancel: ctx.cancel.clone(),
            },
        );
        let guard = RunGuard {
            supervisor: self.clone(),
            run_id: ctx.run_id.clone(),
        };
        Ok((ctx, guard))
    }

    /// Fires a registered run's stop signal.
    ///
    /// Returns `false` when no run with `run_id` is registered — which covers
    /// both "never existed" and "already settled", and the route answers `404`
    /// for both. They are genuinely the same answer to the operator: there is
    /// nothing here to stop.
    pub fn cancel(&self, run_id: &str) -> bool {
        let guard = self.inner.lock().expect("run supervisor poisoned");
        let Some(slot) = guard.get(run_id) else {
            return false;
        };
        tracing::info!(
            workflow = %slot.workflow_id,
            %run_id,
            "workflow run: an operator asked to stop this run"
        );
        slot.cancel.cancel();
        true
    }

    /// Removes a run's slot (called by [`RunGuard`]'s `Drop`).
    fn deregister(&self, run_id: &str) {
        self.inner
            .lock()
            .expect("run supervisor poisoned")
            .remove(run_id);
    }

    /// The runs currently registered, as `(run_id, workflow_id)` pairs.
    ///
    /// Order is unspecified — a `HashMap`'s, so callers that care must sort.
    /// Mirrors [`InflightRegistry::list`](crate::company::steer::InflightRegistry::list),
    /// and exists for the same two reasons: it is the read a "what is running
    /// right now" surface would need, and without it a test cannot discover the
    /// id of a run it did not start itself — which is exactly the position the
    /// cron scheduler's tests are in, since the scheduler mints the id inside a
    /// spawned task.
    pub fn live(&self) -> Vec<(String, String)> {
        self.inner
            .lock()
            .expect("run supervisor poisoned")
            .iter()
            .map(|(run_id, slot)| (run_id.clone(), slot.workflow_id.clone()))
            .collect()
    }

    /// How many runs are currently registered.
    ///
    /// For tests and diagnostics, and it stays that way: it keeps the `expect`,
    /// so [`is_empty`](Self::is_empty) — which *is* on a production path —
    /// deliberately does not route through it.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("run supervisor poisoned").len()
    }

    /// Whether no run is registered.
    ///
    /// Poison-tolerant, and it reports **not empty** — that is, busy — rather
    /// than panicking. [`CompanyRuntime::is_busy`] reads this for
    /// `GET /healthz/busy`, and a panic there reaches an axum handler with no
    /// `CatchPanicLayer`: the connection resets, the manager reads that as
    /// "cannot tell", and its default is to park — losing the very work this
    /// exists to protect, at the one moment the registry is in an unknown state.
    ///
    /// Reporting busy on poison is bounded: the manager parks anyway once a
    /// tenant exceeds its idle timeout plus the busy extension, so a permanently
    /// poisoned supervisor delays a park rather than preventing one.
    ///
    /// This mirrors [`InflightRegistry::any_inflight`], the sibling arm of the
    /// same predicate, which was given this treatment in #1133 while this one
    /// was missed (issue #1239).
    ///
    /// [`CompanyRuntime::is_busy`]: crate::company::runtime::CompanyRuntime::is_busy
    /// [`InflightRegistry::any_inflight`]: crate::company::steer::InflightRegistry::any_inflight
    pub fn is_empty(&self) -> bool {
        let Ok(runs) = self.inner.lock() else {
            return false;
        };
        runs.is_empty()
    }

    /// Poisons the inner mutex so a test can exercise the fail-closed read.
    ///
    /// Test-only: nothing in production has a reason to poison a lock. The
    /// panic it provokes is caught by the joined thread, but the default panic
    /// hook still prints one line to the test output — that line is expected.
    #[cfg(test)]
    pub(crate) fn poison_for_test(&self) {
        let inner = Arc::clone(&self.inner);
        let _ = std::thread::spawn(move || {
            let _guard = inner.lock().expect("run supervisor poisoned");
            panic!("poisoning the run supervisor for a test");
        })
        .join();
        assert!(
            self.inner.lock().is_err(),
            "the helper must actually leave the mutex poisoned"
        );
    }
}

/// An RAII guard that deregisters a run when dropped.
///
/// Held for the whole run — through the runner call *and* the
/// [`record_run_finished`](super::record_run_finished) that follows it — so a
/// run stays cancellable right up to the moment it settles, and stops being
/// cancellable the moment it does.
pub struct RunGuard {
    supervisor: RunSupervisor,
    run_id: String,
}

impl RunGuard {
    /// The run this guard keeps registered.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.supervisor.deregister(&self.run_id);
    }
}

#[cfg(test)]
mod test {
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
}
