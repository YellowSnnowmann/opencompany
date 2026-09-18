//! Issue #464: the board announces its own writes.
//!
//! A card can come into being on several paths — chat intake's deterministic
//! triage (`company::task_intent::triage_message`, which opens a card only for
//! the messages it classes as work), a delegation opening a card for a desk,
//! the publish drain, the console's `POST …/tasks` — and before this nothing on the company
//! feed said so. The durable task events all describe a card that *already*
//! exists ([`TaskDispatched`](CompanyEvent::TaskDispatched) fires on the
//! `in_progress` edge, [`DeskTaskCompleted`](CompanyEvent::DeskTaskCompleted) on
//! the settle), so a console watching the board had nothing to react to and no
//! way to learn a card had appeared.
//!
//! ## Why the store, and not the callers
//!
//! [`BoardAnnouncer`] is a decorator over the company's
//! [`TaskStore`](crate::ports::tasks::TaskStore), wrapped in once by
//! [`RuntimeBuilder`](crate::runtime::RuntimeBuilder). Every writer — REST,
//! cycle, delegation, the settle mover — already goes through that port, so the
//! announcement is emitted **once, where cards are actually written**, rather
//! than once per call site that happens to write one.
//!
//! That distinction is the fix, not an implementation detail. The alternative
//! (an emit at each creation site) is the same shape as the bug: it is correct
//! only for the paths somebody remembered, and the next path added silently
//! announces nothing. Here a new writer cannot forget, because it does not get
//! a say.
//!
//! ## What it is not
//!
//! * **Not a stimulus.** The event is appended after the write and never fed
//!   into a cycle. A card that started work merely by existing would re-enter
//!   this store and announce again.
//! * **Not the board's truth.** The store is. The frame says *something on the
//!   board changed*; a console reacts by re-reading the board it already knows
//!   how to read. That is why a lost frame degrades to "one stale render" and
//!   never to wrong state.
//! * **Not a failure path.** Record-keeping never fails the work it records: an
//!   event log that refuses is logged and swallowed, exactly as the other
//!   best-effort journalling sites do. A card that was written stays written.
//!
//! The same shape fits any other store whose surface is watched and whose
//! writers are plural — the workspace tree (issue #327) is the obvious next one.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::ports::events::EventLog;
use crate::ports::tasks::{TaskRecord, TaskStore};
use crate::ports::types::{CompanyEvent, CompanyId};

/// The wire word for a write that brought a card into existence.
pub const CHANGE_OPENED: &str = "opened";
/// The wire word for a write that changed a card already on the board.
pub const CHANGE_UPDATED: &str = "updated";
/// The wire word for a card that was deleted.
pub const CHANGE_REMOVED: &str = "removed";

/// A [`TaskStore`] that appends a
/// [`TaskCardChanged`](CompanyEvent::TaskCardChanged) to the company's event log
/// after any write that actually changed the board.
///
/// Reads pass straight through. See the module docs for why this lives at the
/// store rather than at the callers.
pub struct BoardAnnouncer {
    inner: Arc<dyn TaskStore>,
    events: Arc<dyn EventLog>,
}

impl BoardAnnouncer {
    /// Wraps `inner`, announcing onto `events`.
    pub fn new(inner: Arc<dyn TaskStore>, events: Arc<dyn EventLog>) -> Self {
        Self { inner, events }
    }

    /// Appends one announcement, best-effort.
    ///
    /// A refusing event log is warned about and swallowed: the board write it
    /// describes has already succeeded and must not be undone (or reported as
    /// failed) because the *record* of it could not be filed.
    async fn announce(
        &self,
        company: &CompanyId,
        task_id: &str,
        change: &str,
        column: Option<String>,
    ) {
        let event = CompanyEvent::TaskCardChanged {
            task_id: task_id.to_string(),
            change: change.to_string(),
            column,
        };
        if let Err(err) = self.events.append(company, event).await {
            tracing::warn!(
                company = %company,
                task = %task_id,
                change = %change,
                error = %err,
                "could not announce a board write"
            );
        }
    }

    /// The card with `id` as the board holds it now, or `None`.
    ///
    /// A read that fails yields `None` rather than propagating: the caller uses
    /// it only to choose the wire word, and refusing a write because the *prior*
    /// state could not be read would fail board writes for a diagnostic.
    async fn current(&self, company: &CompanyId, id: &str) -> Option<TaskRecord> {
        self.inner
            .list(company)
            .await
            .ok()?
            .into_iter()
            .find(|t| t.id == id)
    }
}

#[async_trait]
impl TaskStore for BoardAnnouncer {
    async fn list(&self, company: &CompanyId) -> Result<Vec<TaskRecord>> {
        self.inner.list(company).await
    }

    /// Writes through, then announces — `opened` when the card was not there
    /// before, `updated` when it was and the write changed it.
    ///
    /// **A re-save that changes nothing says nothing.** Idempotent writes are
    /// ordinary here (a settle that re-persists an unchanged card, a retry), and
    /// announcing one would put a frame on the feed for a board that did not
    /// move — noise a console cannot distinguish from a real change.
    async fn upsert(&self, company: &CompanyId, task: &TaskRecord) -> Result<()> {
        // Read before the write, so `previous` is genuinely the prior state.
        // Two concurrent writers can both read "absent" and both announce
        // `opened`; that costs a duplicate re-read on the console and is
        // deliberately not locked against, because the board write itself is
        // where ordering is owned.
        let previous = self.current(company, &task.id).await;
        self.inner.upsert(company, task).await?;
        if previous.as_ref() == Some(task) {
            return Ok(());
        }
        let change = if previous.is_none() {
            CHANGE_OPENED
        } else {
            CHANGE_UPDATED
        };
        self.announce(company, &task.id, change, Some(task.column.clone()))
            .await;
        Ok(())
    }

    async fn update_if_column(
        &self,
        company: &CompanyId,
        task: &TaskRecord,
        observed: &TaskRecord,
        expected_column: &str,
    ) -> Result<bool> {
        let updated = self
            .inner
            .update_if_column(company, task, observed, expected_column)
            .await?;
        if updated {
            self.announce(company, &task.id, CHANGE_UPDATED, Some(task.column.clone()))
                .await;
        }
        Ok(updated)
    }

    /// Deletes through, and announces only when a card was actually removed —
    /// a delete of an id the board never held changed nothing.
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        let removed = self.inner.delete(company, id).await?;
        if removed {
            self.announce(company, id, CHANGE_REMOVED, None).await;
        }
        Ok(removed)
    }
}

#[cfg(test)]
#[path = "board_events_tests.rs"]
mod tests;
