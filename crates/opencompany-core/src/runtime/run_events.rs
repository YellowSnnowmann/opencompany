//! The seam that makes every task-attempt status change observable (issue #1015).
//!
//! # Why a decorator, and why on `put_run`
//!
//! [`RunStatus`](crate::ports::runs::RunStatus) transitions were written by
//! three paths and journalled by none, so attempt status was poll-only: the task
//! detail screen re-fetched every four seconds and did not move at all while the
//! tab was hidden. That is the surface #581 spent its effort removing — it
//! replaced a five-second whole-company refetch with Snapshot + Refresh — so a
//! poll-only screen is swimming against the direction the console is going.
//!
//! The obvious seam is the cycle's call sites, and it is wrong.
//! [`reap_orphaned_runs`](crate::ports::runs::reap_orphaned_runs) settles
//! crash-killed runs by calling `finish_run` **directly**, never through
//! `cycle.rs`. Emitting from the cycle would leave exactly those runs — the ones
//! the reaper exists to make visible — moving in silence, which is the partial
//! event surface that is worse than none, because a consumer assumes the frames
//! are complete.
//!
//! So the frame is emitted one level down, at the store. [`put_run`] is the
//! single write primitive: `begin_run` and `finish_run` are **trait defaults**
//! that call `self.put_run`, no backend overrides either (sqlite, MongoDB and
//! the filesystem all implement `put_run` and `create_run` only), and nothing
//! else in the tree calls it. Wrapping it therefore covers every status change
//! that exists, and — because the defaults dispatch through `self` — covers them
//! by construction rather than by anyone remembering a call site.
//!
//! ## `put_run` emits rather than being sealed
//!
//! It is documented as "the backend seam behind the transition methods — **not**
//! the API … calling it directly is how a run ends up in a state the rest of the
//! system does not believe in." It stays that escape hatch, and it emits. A
//! hatch that can skip the legality check but *cannot* skip being observed is
//! the useful shape: the illegal state still lands, and the log still says it
//! did. Sealing it instead would have meant reimplementing both transitions
//! here, which is how the decorator and the port drift apart.
//!
//! ## Ordering
//!
//! The frame is appended **after** the inner write resolves `Ok`, never before,
//! so a consumer reacting to it can never read a row that has not landed. The
//! reverse order would be a worse bug than the polling this replaces: a reader
//! that re-fetches on the frame would see the *old* status and cache it as
//! current, and nothing would fire again to correct it.
//!
//! An append that fails is logged and swallowed — the status write already
//! happened and is the durable fact, and turning a journal hiccup into a failed
//! transition would let an observability concern strand a run. The consumer's
//! fallback poll is what covers that gap, which is why this does not replace it.

use std::sync::Arc;

use async_trait::async_trait;

use crate::Result;
use crate::ports::events::EventLog;
use crate::ports::runs::{NewRun, RunFilter, RunRecord, RunStepRecord, RunStore};
use crate::ports::types::{CompanyEvent, CompanyId};

/// Wraps a [`RunStore`] so every status write journals a
/// [`CompanyEvent::RunStatusChanged`].
pub struct EventingRunStore {
    inner: Arc<dyn RunStore>,
    events: Arc<dyn EventLog>,
}

impl EventingRunStore {
    /// Wraps `inner`, journalling to `events`.
    pub fn new(inner: Arc<dyn RunStore>, events: Arc<dyn EventLog>) -> Self {
        Self { inner, events }
    }

    /// Appends the frame for a settled write, or logs why it could not.
    async fn announce(&self, company: &CompanyId, run: &RunRecord, from: Option<String>) {
        if let Err(err) = self
            .events
            .append(
                company,
                CompanyEvent::RunStatusChanged {
                    run_id: run.id.clone(),
                    task_id: run.task_id.clone(),
                    attempt: run.attempt,
                    from,
                    to: run.status.as_str().to_string(),
                    error: run.error.clone(),
                },
            )
            .await
        {
            // Swallowed on purpose — see the module note on ordering. The status
            // write is the durable fact and it already happened.
            tracing::warn!(
                company = %company,
                run = %run.id,
                status = %run.status,
                error = %err,
                "[runs] could not journal an attempt's status change; the row moved anyway"
            );
        }
    }
}

impl std::fmt::Debug for EventingRunStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventingRunStore").finish_non_exhaustive()
    }
}

#[async_trait]
impl RunStore for EventingRunStore {
    /// Mints the row, then announces it as a move to `pending` with no `from` —
    /// the one write that is a birth rather than a transition.
    async fn create_run(&self, company: &CompanyId, spec: NewRun) -> Result<RunRecord> {
        let run = self.inner.create_run(company, spec).await?;
        self.announce(company, &run, None).await;
        Ok(run)
    }

    /// **The seam.** Reads the prior status, writes, then announces — and only
    /// when the status actually moved, so a write that revises cost or step
    /// count on an already-settled row does not manufacture a transition.
    ///
    /// The extra read is the price of naming both ends of the move. A consumer
    /// applying a frame to a row it already holds needs `from` to tell an
    /// out-of-order or replayed frame from a live one, which a bare `to` cannot.
    async fn put_run(&self, company: &CompanyId, run: &RunRecord) -> Result<()> {
        let before = self
            .inner
            .get_run(company, &run.id)
            .await
            .ok()
            .flatten()
            .map(|prior| prior.status);
        self.inner.put_run(company, run).await?;
        if before != Some(run.status) {
            self.announce(company, run, before.map(|s| s.as_str().to_string()))
                .await;
        }
        Ok(())
    }

    async fn get_run(&self, company: &CompanyId, id: &str) -> Result<Option<RunRecord>> {
        self.inner.get_run(company, id).await
    }

    async fn list_runs(&self, company: &CompanyId, filter: &RunFilter) -> Result<Vec<RunRecord>> {
        self.inner.list_runs(company, filter).await
    }

    async fn append_run_step(&self, company: &CompanyId, step: &RunStepRecord) -> Result<()> {
        self.inner.append_run_step(company, step).await
    }

    async fn list_run_steps(
        &self,
        company: &CompanyId,
        run_id: &str,
    ) -> Result<Vec<RunStepRecord>> {
        self.inner.list_run_steps(company, run_id).await
    }
}

#[cfg(test)]
#[path = "run_events_tests.rs"]
mod tests;
