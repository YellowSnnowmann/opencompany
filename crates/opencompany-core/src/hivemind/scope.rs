//! Which journal rows one running episode may fold as its own.
//!
//! `EpisodeState::watermark` (in `tinyhivemind_hive`) is a **lower** bound
//! only: everything at or below it is context, and everything above it folds
//! as this episode's traces. That is exactly right for the one episode a desk
//! usually has running, and exactly wrong the moment a second one opens in the
//! same conversation — a follow-up accepted before the first episode's turns
//! finish shares its `(desk_id, thread_root)` with the first, so the second
//! episode's watermark check sees the first episode's turns sitting above its
//! own trigger and folds them as its own live traces. A desk can converge on
//! the answer to a question nobody in that episode ever asked.
//!
//! `tinyhivemind_hive` cannot fix this itself: `step` folds a transcript slice
//! the host already narrowed, and has no idea two hosts are narrowing the same
//! journal at once. The narrowing has to happen here, before the transcript
//! ever reaches the fold. [`EpisodeScope`] is that narrowing: an upper
//! boundary companion to the watermark, scoped to one [`EpisodeDriver`]
//! instance for the lifetime of one `run` call.
//!
//! A row is admitted by a scope when it is at or below the trigger (shared,
//! readable context — unchanged from before this existed) **or** when this
//! specific episode instance is the one that appended it. The second half is
//! knowable without a second journal read: every row this episode ever writes
//! goes through [`EpisodeScope::record`] at the point it is appended, whether
//! that append is a turn, a failed-turn note, the closing report, or an answer
//! a referral carried back onto this desk. Nothing about a *concurrent*
//! episode's own turns can ever appear in this set, because they are recorded
//! against a different `EpisodeScope` instance entirely.
//!
//! [`EpisodeDriver`]: super::episode::EpisodeDriver

use std::collections::HashSet;
use std::sync::Mutex;

use crate::ports::types::EventSeq;

/// The fold boundary for one running episode.
///
/// Cheap to construct and to check: the common case is one lookup in a small
/// set, guarded by a lock nothing ever contends, because a scope is only ever
/// touched by the single task driving its episode.
#[derive(Debug)]
pub struct EpisodeScope {
    /// Exclusive lower bound shared with `EpisodeState::watermark`: at or
    /// below this, every episode reads the same shared context.
    trigger: EventSeq,
    /// Sequences this specific episode instance has appended above its
    /// trigger. The only sequences above `trigger` this scope admits.
    mine: Mutex<HashSet<EventSeq>>,
}

impl EpisodeScope {
    /// Open a scope at `trigger`, with nothing recorded as this episode's own
    /// yet.
    #[must_use]
    pub fn new(trigger: EventSeq) -> Self {
        Self {
            trigger,
            mine: Mutex::new(HashSet::new()),
        }
    }

    /// Mark `seq` as a row this episode instance itself appended.
    ///
    /// Idempotent, and cheap to call for every append this driver makes
    /// regardless of whether the row could ever matter to the fold (the
    /// closing report never does) — a scope that is occasionally over-eager
    /// about what counts as "mine" costs nothing, since it can only ever
    /// *widen* what this episode's own reads admit, never narrow what a
    /// concurrent episode's separate scope does.
    pub fn record(&self, seq: EventSeq) {
        self.lock().insert(seq);
    }

    /// Whether `seq` is context this scope's episode may read: shared history
    /// at or below the trigger, or a row this same episode instance appended.
    #[must_use]
    pub fn admits(&self, seq: EventSeq) -> bool {
        seq <= self.trigger || self.lock().contains(&seq)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<EventSeq>> {
        self.mine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
#[path = "scope_tests.rs"]
mod tests;
