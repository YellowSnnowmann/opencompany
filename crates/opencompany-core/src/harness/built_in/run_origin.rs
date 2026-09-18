//! Which run dispatched the current agent turn (issue #2150, Rung 3 of epic
//! #1817).
//!
//! Vendored OpenHuman answers the same question for its own turns with
//! `AgentTurnOrigin`
//! (`vendor/openhuman/crates/openhuman-core/src/agent/turn_origin.rs`), read by its
//! approval gate at `vendor/openhuman/crates/openhuman-core/src/security/approval/gate.rs`.
//! This is the same shape at OpenCompany's own dispatch layer — a company can
//! run a task card, a workflow node or a scheduled job with nobody watching,
//! and [`ApprovalPolicy::check`](crate::harness::built_in::policy::ApprovalPolicy::check)
//! needs to tell that apart from a model improvising inside a live operator
//! chat.
//!
//! An unlabelled turn reads as [`RunOrigin::Unknown`] and stays untrusted —
//! [`current`]'s `unwrap_or` is that fail-closed default, not a convenience.

use std::future::Future;

/// Who dispatched the current agent turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOrigin {
    /// No entry point scoped a real origin.
    Unknown,
    /// A live operator chat turn.
    Operator,
    /// A turn this company dispatched itself, with no operator watching it
    /// happen.
    Dispatched {
        /// The roster agent this run was dispatched to.
        ///
        /// Compared against the checking [`ApprovalPolicy`]'s own agent, so an
        /// origin carried across a same-turn delegation hand-off cannot admit
        /// a call the delegate itself was never dispatched to make —
        /// delegation inherits this origin unchanged rather than re-deriving
        /// it, so the mismatch is exactly what stops trust from widening at
        /// the hand-off.
        ///
        /// [`ApprovalPolicy`]: crate::harness::built_in::policy::ApprovalPolicy
        agent: String,
        /// Which kind of dispatch this was.
        source: DispatchSource,
        /// The scope this trust is confined to, when the dispatch declared
        /// one.
        ///
        /// `None` admits only a call whose
        /// [`Standing`](crate::policy::Standing) needs no scope at all
        /// (`Standing::Grantable`) — never a
        /// [`Standing::ScopedGrantable`](crate::policy::Standing::ScopedGrantable)
        /// call, whatever scope that call itself resolves to. An unscoped
        /// grant admitting everything is exactly what
        /// [`crate::runtime::grants`] calls catastrophic; an unscoped origin
        /// must not reproduce that shape for the trust this module hands out
        /// instead of a grant.
        scope: Option<String>,
    },
}

/// Which kind of dispatch produced a [`RunOrigin::Dispatched`] turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchSource {
    /// A task card the board dispatched to its assignee.
    Task,
    /// A saved workflow's agent node, or the peer consultation a recovery
    /// rung raises on its behalf.
    Workflow {
        /// The workflow this run belongs to.
        workflow_id: String,
    },
    /// A scheduled run with no operator watching.
    Schedule,
}

tokio::task_local! {
    static RUN_ORIGIN: RunOrigin;
}

/// The ambient origin, or [`RunOrigin::Unknown`] when nothing scoped one.
///
/// The `unwrap_or` is the fail-closed requirement this whole module rests on:
/// a caller that forgot to scope an origin — or one running detached from any
/// scope at all, such as a `tokio::spawn`ed task nothing re-scoped — reads as
/// unlabelled rather than as whatever the last scope happened to leave
/// behind.
pub fn current() -> RunOrigin {
    RUN_ORIGIN
        .try_with(Clone::clone)
        .unwrap_or(RunOrigin::Unknown)
}

/// One turn's exclusive hold on a [`RunOrigin`] (issue #2150).
///
/// Modelled on [`ApprovalClaim`](crate::harness::built_in::policy::ApprovalClaim):
/// obtained once via [`claim`] and scoped around the turn with
/// [`scoped`](Self::scoped). `tokio::task_local`'s own `scope` already
/// installs and removes its value around each individual poll rather than for
/// the future's whole lifetime, so an early return, a panic unwinding through
/// the scoped future, or the future being dropped mid-poll by a cancellation
/// all leave no origin installed for whatever runs next on this task — a
/// hand-dropped guard is not what makes that safe. This type exists so the
/// open/close of a trust window reads as the same claim-and-scope pair as
/// every other one in this file, and so its `Drop` gives the open/close a
/// symmetric debug-log trace.
pub struct RunOriginClaim {
    origin: RunOrigin,
}

impl RunOriginClaim {
    /// The origin this claim carries.
    pub fn origin(&self) -> &RunOrigin {
        &self.origin
    }

    /// Runs `fut` with this claim's origin installed as the ambient one.
    ///
    /// Mirrors [`ApprovalClaim::scoped`](crate::harness::built_in::policy::ApprovalClaim::scoped):
    /// this does not box `fut` itself — the caller does, same as every other
    /// claim in this file. A dispatch site is typically already nested inside
    /// an [`ApprovalScope`](crate::harness::built_in::policy::ApprovalScope)
    /// claim (and, at the workflow agent node, a `DelegationScope` claim
    /// beneath that too) — three task-local scopes wrapping one very large
    /// openhuman agent turn future have overflowed the worker thread's stack
    /// before (`.cargo/config.toml`, issue #895), so a caller nesting this
    /// scope inside others must box its future before passing it here.
    pub async fn scoped<F: Future>(&self, fut: F) -> F::Output {
        RUN_ORIGIN.scope(self.origin.clone(), fut).await
    }
}

impl Drop for RunOriginClaim {
    fn drop(&mut self) {
        tracing::trace!(origin = ?self.origin, "[run_origin] dispatch origin scope closed");
    }
}

/// Claims `origin` for the span of one turn.
pub fn claim(origin: RunOrigin) -> RunOriginClaim {
    tracing::trace!(?origin, "[run_origin] dispatch origin scope opened");
    RunOriginClaim { origin }
}

#[cfg(test)]
#[path = "run_origin_tests.rs"]
mod tests;
