//! Graceful shutdown for the host (issue #986).
//!
//! A hosted tenant is a pod. Every deploy, every `refresh_all_tenants` run and
//! every eviction sends it `SIGTERM`, and until this module existed nothing in
//! the process handled that signal — so the default disposition applied and the
//! host died, mid-turn. Turns are long (measured well past fifteen minutes on
//! staging), so that reliably meant "in the middle of one". In a container the
//! entrypoint `exec`s `opencompany`, making it PID 1 — where the kernel *drops*
//! a default-disposition `SIGTERM` rather than delivering it — so the pod sat
//! idle until the kubelet's `SIGKILL` at the end of its grace period, and even
//! a turn that finished in the remaining seconds of that window was cut off
//! before it could be saved. The journal was left holding a question with no
//! answer.
//!
//! The two halves here are the ones the signal needs and neither is sufficient
//! alone:
//!
//! 1. [`signal`] — a future that resolves on `SIGTERM` (and `SIGINT`, so a local
//!    `Ctrl-C` takes the same path a rollout does). Registering a handler at all
//!    is what stops the kernel's default kill, which is why an unset
//!    `terminationGracePeriodSeconds` bought nothing before: a grace period is a
//!    window to *handle* the signal in, and nothing was handling it.
//! 2. [`drain`] — quiesce every registered company, then wait for the cycle each
//!    one has in flight, bounded.
//!
//! ## Why the drain cannot be axum's connection drain
//!
//! `with_graceful_shutdown` waits for in-flight *connections*. That would have
//! been enough if a turn lived inside its request future — but issue #383
//! deliberately moved it off, precisely so a client walking away could not take
//! the agent's continuation with it. Dispatches, scheduled workflow runs and
//! approval follow-ups all run on detached `tokio::spawn`s. So at the moment
//! `SIGTERM` arrives the connection set can be empty while several turns are
//! very much running, and a connection drain would report "nothing in flight"
//! and exit.
//!
//! [`CompanyRuntime::quiesce`](crate::company::runtime::CompanyRuntime::quiesce)
//! is the primitive that does see them. It was built for the rebuild swap
//! (issue #290) and answers exactly the question shutdown asks: stop accepting
//! new cycles, then prove the one in flight has finished by acquiring the
//! per-company `serial` lock every cycle holds for its whole duration. Detached
//! work is covered because it takes that same lock — it either completed before
//! the flag was set or it is the turn being waited on.
//!
//! ## Why the bound is honest rather than generous
//!
//! A turn can outlast any grace period a rollout is willing to wait through, so
//! this reduces how often work is killed; it does not eliminate it. The
//! failed-run record from issue #983 stage 1 stays the backstop for the turns
//! that still get cut off.
//!
//! ## What this must not touch
//!
//! `/healthz`. The manager's wake-on-request proxy blocks on that endpoint and
//! gives up after its startup timeout, so nothing here may slow boot. Nothing
//! here runs before the signal, and the signal only ever arrives at the end of a
//! pod's life.

use std::time::Duration;

use crate::AppState;

/// Overrides how long [`drain`] waits for in-flight turns, in seconds.
///
/// `0` disables the wait entirely (quiesce, then exit immediately), which is a
/// deliberate escape hatch for an operator who wants the old behaviour back
/// without reverting.
pub const GRACE_ENV: &str = "OPENCOMPANY_SHUTDOWN_GRACE_SECONDS";

/// How long [`drain`] waits for in-flight turns when [`GRACE_ENV`] is unset.
///
/// Sized to fit inside Kubernetes' *default* 30s `terminationGracePeriodSeconds`
/// together with [`CONNECTION_GRACE`], so a tenant running this build gets a
/// real, complete drain even on a pod spec that has not been updated to name a
/// grace period of its own. Raising this past the pod's grace period does not
/// buy a longer drain — it buys a `SIGKILL` in the middle of one — so the two
/// are meant to move together.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(25);

/// The extra window the server gets, after the drain, to finish writing
/// responses on connections that are still open.
///
/// Small on purpose. By the time the drain returns, the work those connections
/// were waiting on is either done or already past its bound; this is for the
/// bytes, not for the turn. It is also what keeps a long-lived stream — the
/// console's event stream never ends on its own — from holding the process open
/// past the pod's grace period and turning a clean exit into a `SIGKILL`.
pub const CONNECTION_GRACE: Duration = Duration::from_secs(2);

/// The last window, after the server has stopped serving, for analytics to get
/// its final batch out (issue #1739).
///
/// **Bounded because the budget above is the whole point.** [`DEFAULT_GRACE`]
/// and [`CONNECTION_GRACE`] are sized to land at 27s, deliberately under
/// Kubernetes' default 30s `terminationGracePeriodSeconds`. The flush is a
/// network call to a collector this process does not control, and its client
/// timeout is 5s — so an unbounded flush during a rollout with a slow collector
/// took the worst case to 32s and invited the `SIGKILL` the 27s exists to
/// avoid, losing the drain rather than the telemetry.
///
/// 2s keeps the total at 29s. Telemetry is the right thing to give up here: a
/// dropped batch costs a boot line in a dashboard, while an overrun costs a
/// half-finished turn. An operator who raises [`GRACE_ENV`] past the pod's
/// grace period has already left this budget behind, and this bound does not
/// grow with it.
pub const FLUSH_BUDGET: Duration = Duration::from_secs(2);

/// Kubernetes' default `terminationGracePeriodSeconds`, which every budget in
/// this module is sized against. Named rather than left in prose, so the
/// arithmetic can be asserted.
pub const POD_DEFAULT_GRACE: Duration = Duration::from_secs(30);

/// How long the analytics flush actually gets, given the configured drain.
///
/// [`FLUSH_BUDGET`] is a **ceiling, not an allowance**. Added flat to a
/// configurable drain it re-created the problem it was added to fix: with
/// `OPENCOMPANY_SHUTDOWN_GRACE_SECONDS=28`, drain plus connection grace fit in
/// 30s exactly, and a flat two seconds on top took it to 32 — the same
/// mid-shutdown `SIGKILL`, for a value the operator had every reason to think
/// was safe.
///
/// So it is derived from what is left: whatever remains of the pod's default
/// grace after the drain and the connection window, capped at [`FLUSH_BUDGET`].
/// A drain that already fills the budget leaves zero, and the flush is skipped.
///
/// Telemetry is the right thing to give way — a dropped batch costs a line in a
/// dashboard, an overrun costs a half-finished turn — and that applies just as
/// much to an operator who raised the drain deliberately: this cannot know what
/// their pod's grace period actually is, so it declines to spend seconds it
/// cannot prove are there.
pub fn flush_budget(drain: Duration) -> Duration {
    POD_DEFAULT_GRACE
        .saturating_sub(drain.saturating_add(CONNECTION_GRACE))
        .min(FLUSH_BUDGET)
}

/// The drain bound, read from [`GRACE_ENV`] and falling back to
/// [`DEFAULT_GRACE`].
///
/// A malformed value falls back rather than failing: this is read while the
/// process is already on its way out, and refusing to shut down over a typo in
/// an environment variable would be a worse outcome than the wrong bound.
pub fn grace_from_env() -> Duration {
    parse_grace(std::env::var(GRACE_ENV).ok().as_deref())
}

/// The parse behind [`grace_from_env`], split out so it can be tested without
/// mutating process environment — which no test can do safely in a binary whose
/// other tests run on the same process.
fn parse_grace(raw: Option<&str>) -> Duration {
    let Some(raw) = raw else {
        return DEFAULT_GRACE;
    };
    match raw.trim().parse::<u64>() {
        Ok(secs) => Duration::from_secs(secs),
        Err(_) => {
            tracing::warn!(
                "{GRACE_ENV}=`{raw}` is not a whole number of seconds; using the {}s default",
                DEFAULT_GRACE.as_secs()
            );
            DEFAULT_GRACE
        }
    }
}

/// Resolves on the first termination signal.
///
/// `SIGTERM` is the one a rollout, a refresh and an eviction all send.
/// `SIGINT` is included so a developer's `Ctrl-C` exercises the same path in
/// development that production takes — a shutdown path that only ever runs in
/// production is a shutdown path nobody has watched work.
#[cfg(unix)]
pub async fn signal() {
    use tokio::signal::unix::{SignalKind, signal};

    // Registering both is what displaces the kernel's default kill. If either
    // registration fails we still want the other, and if both fail we fall back
    // to never resolving: an unhandled signal then kills the process exactly as
    // it did before this module, which is the honest degradation.
    let mut term = signal(SignalKind::terminate()).ok();
    let mut interrupt = signal(SignalKind::interrupt()).ok();
    if term.is_none() && interrupt.is_none() {
        tracing::error!("could not install signal handlers; shutdown will not be graceful");
        std::future::pending::<()>().await;
    }
    let terminated = async {
        match term.as_mut() {
            Some(s) => {
                s.recv().await;
            }
            None => std::future::pending().await,
        }
    };
    let interrupted = async {
        match interrupt.as_mut() {
            Some(s) => {
                s.recv().await;
            }
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        () = terminated => tracing::info!("received SIGTERM"),
        () = interrupted => tracing::info!("received SIGINT"),
    }
}

/// Resolves on the first termination signal.
///
/// Off Unix there is no `SIGTERM`; `Ctrl-C` is the whole of it.
#[cfg(not(unix))]
pub async fn signal() {
    if tokio::signal::ctrl_c().await.is_ok() {
        tracing::info!("received Ctrl-C");
    } else {
        tracing::error!("could not install a Ctrl-C handler; shutdown will not be graceful");
        std::future::pending::<()>().await;
    }
}

/// Arms a listener for a *second* termination signal and hard-exits on it.
///
/// The first signal replaces the kernel's default disposition and resolves
/// [`signal`], so a `SIGTERM`/`SIGINT` that arrives after that (while the drain
/// is running) is otherwise swallowed for the rest of the process's life. On an
/// idle host the process is gone a couple of seconds after the first signal and
/// this never fires; it exists for the other half — a long drain, where the
/// developer who pressed Ctrl-C once and watched the 25s ceiling start has no
/// way to change their mind. A second press is the escape hatch. `130` is the
/// conventional "terminated by Ctrl-C" exit code.
///
/// Deliberately a one-way, process-level action: there is no graceful recovery
/// from having been told to stop twice.
#[cfg(unix)]
pub fn arm_force_exit_on_second_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    tokio::spawn(async move {
        let mut term = signal(SignalKind::terminate()).ok();
        let mut interrupt = signal(SignalKind::interrupt()).ok();
        let terminated = async {
            match term.as_mut() {
                Some(s) => {
                    let _ = s.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        let interrupted = async {
            match interrupt.as_mut() {
                Some(s) => {
                    let _ = s.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            () = terminated => {}
            () = interrupted => {}
        }
        tracing::warn!("received a second termination signal; exiting immediately");
        std::process::exit(130);
    });
}

/// Arms a listener for a second `Ctrl-C` and hard-exits on it.
#[cfg(not(unix))]
pub fn arm_force_exit_on_second_signal() {
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::warn!("received a second Ctrl-C; exiting immediately");
        std::process::exit(130);
    });
}

/// Stops every registered company accepting new cycles, then waits for the ones
/// in flight to settle, for at most `grace`.
///
/// Returns whether the wait completed — `false` means at least one turn was
/// still running when the bound expired and is about to be cut off. That is a
/// real outcome, not an error: see the module docs on why the bound is shorter
/// than the longest turn.
///
/// The companies are drained concurrently. Serially, a single busy company would
/// spend the whole bound and leave every other company's turn to be killed for
/// no reason — and a tenant can hold more than one.
pub async fn drain(state: &AppState, grace: Duration) -> bool {
    let registry = state.registry();
    // Before the snapshot, deliberately. The host keeps serving through the
    // drain, so `POST /api/v1/companies` can register a company while this
    // runs; without this it would land after the snapshot, never be quiesced,
    // and be free to start a turn nothing is waiting for. Setting the flag
    // first makes every later registration born quiesced, so a company is
    // either in the snapshot below or unable to run a cycle at all.
    registry.begin_shutdown();
    let runtimes: Vec<_> = registry
        .list()
        .iter()
        .filter_map(|id| registry.get(id))
        .collect();
    if runtimes.is_empty() {
        return true;
    }

    let count = runtimes.len();
    tracing::info!(
        "draining {count} compan{}",
        if count == 1 { "y" } else { "ies" }
    );
    let drained = futures::future::join_all(runtimes.iter().map(|runtime| runtime.quiesce()));
    match tokio::time::timeout(grace, drained).await {
        Ok(_) => {
            tracing::info!("all in-flight turns settled; shutting down");
            true
        }
        Err(_) => {
            // Named so the operator reading the pod's last lines can tell this
            // apart from the silent kill it replaces, and can tell that raising
            // the bound is the knob that would have helped.
            tracing::warn!(
                "a turn was still running after {}s ({GRACE_ENV}); \
                 shutting down anyway — it will be reaped as failed on the next boot",
                grace.as_secs()
            );
            false
        }
    }
}

#[cfg(test)]
#[path = "shutdown_tests.rs"]
mod tests;
