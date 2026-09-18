//! The one-slot outbox holding the company's Agent Card while tiny.place is
//! unreachable — and nothing else (issue #454).
//!
//! When tiny.place cannot be reached, the [`TinyplaceEconomy`](super::adapter::
//! TinyplaceEconomy) parks the card here instead of failing boot or a cycle, and
//! the replayer attached by
//! [`spawn_outbox_replayer`](super::adapter::spawn_outbox_replayer) publishes it
//! once the network comes back. **Queuing is only ever done by a code path that
//! has a replayer attached** — see [`TinyplaceEconomy::publish_card`](super::
//! adapter::TinyplaceEconomy::publish_card). A queue nothing drains is not a
//! degrade, it is a silent drop reported as success.
//!
//! # Why one slot, and why only the card
//!
//! **One slot, newest wins.** Replay only ever needs the *newest* card: the
//! directory holds one record per agent and a `put_agent` overwrites it, so
//! publishing an older card after a newer one is strictly wrong, and publishing
//! both is one wasted round-trip. Collapsing to a single slot therefore bounds
//! the queue (an outage cannot grow it) and de-duplicates it, in one move.
//!
//! **Only the card.** The two other actions this queue used to carry —
//! registering a `@handle` and sending an A2A task — had no production
//! enqueuer left once #454 removed the ghost copy `send_a2a_task` was pushing
//! alongside the error it already returned. A paid outbound task must not be
//! replayed in the background either: the caller owns that retry, because at
//! flush time there is no budget scope to charge it against and a double-send is
//! a double-spend. See [`TinyplaceEconomy::send_a2a_task`](super::adapter::
//! TinyplaceEconomy::send_a2a_task).
//!
//! # Why in-memory is enough now
//!
//! This is memory-only and a restart drops whatever is queued — which used to be
//! a documented follow-up and, with a card-only outbox, is no longer a loss that
//! needs one. Boot republishes the card from the manifest
//! ([`maybe_go_public`](crate::runtime::builder)), so a restart during an outage
//! self-heals: the queued card is exactly the card the next boot would send.

use std::sync::Mutex;

use crate::ports::types::AgentCard;

/// One deferred outbound action.
///
/// Exactly one variant, and that is the invariant rather than an accident: an
/// action belongs here only if replaying it later, unattended and without the
/// caller, is *safe*. Publishing a card is idempotent and carries no money, so
/// it qualifies; a paid task send does not.
#[derive(Clone, Debug, PartialEq)]
pub enum OutboxAction {
    /// Publish (or refresh) the company's Agent Card.
    PublishCard(AgentCard),
}

/// A thread-safe single-slot outbox.
#[derive(Default)]
pub struct Outbox {
    slot: Mutex<Option<OutboxAction>>,
}

impl Outbox {
    /// Creates an empty outbox.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues an action, **replacing** whatever was queued before it.
    ///
    /// Newest wins: a card published while an older one is still waiting makes
    /// the older one obsolete, so keeping it would only publish a stale record
    /// on the way to the current one.
    pub fn enqueue(&self, action: OutboxAction) {
        *self.lock() = Some(action);
    }

    /// Removes and returns the queued action, leaving the outbox empty.
    pub fn take(&self) -> Option<OutboxAction> {
        self.lock().take()
    }

    /// Puts an action back after a replay attempt failed — but only if the slot
    /// is still empty.
    ///
    /// The guard is the whole point. A replay takes the card out, awaits a
    /// network call, and may come back to find that `publish_card` queued a
    /// *newer* card in the meantime; restoring the old one unconditionally would
    /// silently undo that write and leave the directory one revision behind for
    /// good. Newest still wins, even against the replayer.
    pub fn requeue(&self, action: OutboxAction) {
        let mut slot = self.lock();
        if slot.is_none() {
            *slot = Some(action);
        }
    }

    /// The number of actions currently queued — `0` or `1`.
    pub fn len(&self) -> usize {
        usize::from(self.lock().is_some())
    }

    /// Whether the outbox holds no action.
    pub fn is_empty(&self) -> bool {
        self.lock().is_none()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<OutboxAction>> {
        self.slot.lock().expect("outbox poisoned")
    }
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
