//! One turn, one continuation (issue #469).
//!
//! A single agent turn can park several approvals — four `composio_execute`
//! calls with different arguments is a real, reported case. Before this, each
//! resolve spawned its own follow-up cycle, so approving all four re-ran the
//! same turn four times. The re-runs did not race (the per-company serial lock
//! in [`CycleRunner::run`](crate::runtime::CycleRunner::run) holds for a whole
//! cycle, so they queued), but they were four full agent turns over one turn's
//! worth of work, each told about one decision and blind to the other three.
//!
//! The missing fact was that the four belonged together. Nothing on a parked
//! approval said so: the thread key is shared by every turn in a conversation,
//! and the task and run keys are absent for exactly the case that matters most,
//! a chat turn. So [`ApprovalOrigin::cycle`](crate::runtime::journal::ApprovalOrigin::cycle)
//! now records the parking cycle, and this queue counts, per turn, how many of
//! its approvals are still undecided.
//!
//! The rule it enforces: **a turn is continued once, when the last decision it
//! was blocked on lands**, carrying every `ApprovalResolved` event the turn
//! accumulated on the way. That is the same continuation whether the operator
//! decides the four together or one at a time over a minute — the trigger is
//! the last decision, not the clock, so the two orders cannot diverge.
//!
//! ## Why counting, and not asking the journal
//!
//! "Is anything from this turn still parked?" is answerable from the journal,
//! and answering it there is racy. Resolves arrive over HTTP concurrently even
//! though the cycles they spawn do not: two requests can each un-park their own
//! approval and then each read a queue that looks empty, and both would run a
//! continuation. Counting here closes that by construction — the decrement and
//! the "was that the last one" test happen under one lock, so exactly one
//! caller is ever handed the batch.
//!
//! ## What is deliberately not gated
//!
//! An approval with no turn key — a journal line written before #469 — is never
//! armed, and [`decide`](ContinuationQueue::decide) hands its event straight
//! back. Those continue on their own exactly as they always did.
//!
//! ## The cost, stated plainly
//!
//! A turn whose approvals are only *partly* decided now waits, where before it
//! re-dispatched the approved ones immediately. That is the honest state — the
//! turn really is still blocked — and the alternative is what this issue
//! reports: a re-run that re-parks everything still undecided, which compounds
//! with every decision. The wait is bounded: an undecided approval expires to a
//! default-deny on the gate's TTL, and that expiry is a decision like any other
//! (see [`CompanyRuntime::sweep_expired_approvals`](crate::company::runtime::CompanyRuntime::sweep_expired_approvals)),
//! so the turn is always eventually continued rather than stranded.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::ports::types::CompanyEvent;

/// What a turn is still waiting on, and what it has collected so far.
#[derive(Default)]
struct PendingTurn {
    /// How many of this turn's approvals are still undecided.
    outstanding: usize,
    /// The `ApprovalResolved` events its decided approvals owe the brain, in
    /// decision order. Handed to the continuation cycle as one batch.
    decided: Vec<CompanyEvent>,
}

/// Per-turn continuation state: how many approvals each turn is still blocked
/// on, and the decisions it has banked (issue #469).
///
/// Cheap to [`Clone`] — a shared handle, like every other queue in the runtime —
/// so the parking side (the cycle host) and the resolving side (the runtime)
/// see one set of counters.
#[derive(Clone, Default)]
pub struct ContinuationQueue {
    inner: Arc<Mutex<HashMap<String, PendingTurn>>>,
}

impl ContinuationQueue {
    /// Records that `turn` just parked one more approval.
    ///
    /// Called from the one place approvals are parked, so the count cannot
    /// drift from the queue it describes.
    pub fn arm(&self, turn: &str) {
        self.inner
            .lock()
            .expect("continuation queue poisoned")
            .entry(turn.to_string())
            .or_default()
            .outstanding += 1;
    }

    /// Records one decision on `turn`, banking `event` for the continuation.
    ///
    /// Returns `Some(batch)` **exactly when this was the last decision the turn
    /// was blocked on** — the caller then owes one continuation cycle over the
    /// whole batch, and the turn's state is dropped. `None` means the turn is
    /// still waiting on another decision and the caller owes nothing.
    ///
    /// `event` is `None` for a decision that carries no event for the brain —
    /// an expiry, whose `ApprovalResolved` the sweep has already appended
    /// itself. It still counts as a decision, because it still unblocks.
    ///
    /// A turn this queue never armed is not gated: the event comes straight
    /// back as a batch of one. That covers a pre-#469 journal line and a
    /// restart that lost the counter, and in both cases it is the pre-#469
    /// behaviour rather than a turn that never continues.
    pub fn decide(&self, turn: &str, event: Option<CompanyEvent>) -> Option<Vec<CompanyEvent>> {
        let mut guard = self.inner.lock().expect("continuation queue poisoned");
        let Some(pending) = guard.get_mut(turn) else {
            return Some(event.into_iter().collect());
        };
        if let Some(event) = event {
            pending.decided.push(event);
        }
        pending.outstanding = pending.outstanding.saturating_sub(1);
        if pending.outstanding > 0 {
            return None;
        }
        guard.remove(turn).map(|pending| pending.decided)
    }

    /// Re-arms the counters after a journal replay, from the turn key of every
    /// approval that is still parked — one entry per approval.
    ///
    /// **Idempotent**: it *sets* each turn's count to what the journal says
    /// rather than adding to it, so replaying twice (boot loads the journal, and
    /// [`CompanyRuntime::recover`](crate::company::runtime::CompanyRuntime::recover)
    /// can be driven again) leaves a turn blocked on the number of decisions it
    /// is actually blocked on. Any decisions already banked for a turn are kept:
    /// the count is what replay knows, the bank is what this process has seen.
    ///
    /// A handed-over runtime inherits the live queue instead and never comes
    /// through here — the counters there include decisions the journal has
    /// already recorded as resolved.
    pub fn rearm(&self, turns: impl IntoIterator<Item = String>) {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for turn in turns {
            *counts.entry(turn).or_default() += 1;
        }
        let mut guard = self.inner.lock().expect("continuation queue poisoned");
        for (turn, count) in counts {
            guard.entry(turn).or_default().outstanding = count;
        }
    }

    /// How many turns are still waiting on a decision.
    pub fn waiting(&self) -> usize {
        self.inner
            .lock()
            .expect("continuation queue poisoned")
            .len()
    }

    /// How many decisions `turn` is still blocked on. `0` for a turn this queue
    /// is not tracking.
    pub fn outstanding(&self, turn: &str) -> usize {
        self.inner
            .lock()
            .expect("continuation queue poisoned")
            .get(turn)
            .map(|p| p.outstanding)
            .unwrap_or(0)
    }
}

#[cfg(test)]
#[path = "continuation_tests.rs"]
mod tests;
