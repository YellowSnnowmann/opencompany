//! The week-1 "did this user save a workflow" query (issue #1845).
//!
//! The retention strategy's fix for the "never saved a workflow" churn cause:
//! a user who signs up and never saves a workflow has nothing to come back
//! to. [`user_saved_workflow_in_week1`] is the one place that answers
//! *"has this particular person cleared that bar"*, so
//! [`LifecycleScheduler`](crate::runtime::LifecycleScheduler) — the scheduler
//! that decides whether to nudge — never re-derives it.
//!
//! # Per-user, not per-company
//!
//! The obvious-looking shortcut — "does the company have any attributed
//! `WorkflowCreated`" — is wrong: a five-person company where only the
//! founder has ever saved a workflow would silently excuse the other four
//! from ever being nudged. [`crate::company::activation`]'s module docs raise
//! the same concern one layer up (company-level activation), and this module
//! makes the same choice at the person level: `by == user_id`, always.
//!
//! # Why this is not `activation::any_workflow_run_succeeded`
//!
//! Company activation's workflow step requires a *real run that succeeded* —
//! deliberately a high bar, because activation is "did this operator clear
//! onboarding". The churn cause this nudge targets is narrower and earlier:
//! "never saved a workflow at all". A graph saved but never run still means
//! there is something to come back to, so the bar here is `WorkflowCreated`,
//! not `WorkflowRunFinished`.
//!
//! # The historical attribution gap
//!
//! `WorkflowCreated.by` was `None` on every create path before issue #1843
//! wired the signed-in actor through. A `None` here is therefore ambiguous —
//! nobody-in-particular authored it, or it *was* this user and the surface
//! just did not say so — and this module treats it as neither: the `by ==
//! user_id` match can never be true for an unattributed row, so a user whose
//! only save happened through an old, unattributed path reads as "never
//! saved one". [`LifecycleScheduler`](crate::runtime::LifecycleScheduler)
//! is what actually protects against the false-positive nag this would
//! otherwise cause — it never evaluates this query for a user who signed up
//! before its own cutoff, i.e. before every create path was attributed.

use std::sync::Arc;

use crate::Result;
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId};

/// Milliseconds in seven days — the width of the week-1 window a signup gets
/// to save their first workflow before
/// [`LifecycleScheduler`](crate::runtime::LifecycleScheduler) nudges them.
pub(crate) const SEVEN_DAYS_MILLIS: u64 = 7 * 24 * 60 * 60 * 1000;

/// The [`Notification::kind`](crate::ports::notifications::Notification::kind)
/// wire token the week-1 nudge is filed under.
///
/// Authored here — beside the query that decides whether to file one — rather
/// than in `lifecycle_scheduler`, so the scheduler and any future reader (a
/// digest, a settings toggle) import one constant instead of duplicating the
/// string. **Stable**: [`NotificationStore`](crate::ports::notifications::NotificationStore)
/// backends persist `kind` verbatim, and this row is also the idempotency
/// ledger the scheduler re-reads on every tick — renaming it is a data
/// migration that would make every already-nudged user look never-nudged.
pub(crate) const NUDGE_KIND: &str = "workflow_nudge";

/// Whether `user_id` has at least one workflow attributed to them
/// (`WorkflowCreated { by: Some(Actor { id: user_id, .. }), .. }`) journaled
/// at or before `evaluated_at_millis` — the instant the scheduler is
/// actually deciding whether to nudge, not the nominal week-1 boundary.
///
/// # Why `evaluated_at_millis`, not a fixed `signup_millis + 7d` cutoff
///
/// [`LifecycleScheduler::tick`](crate::runtime::LifecycleScheduler::tick)
/// runs at most once a day, so the tick that first finds a user past their
/// day-7 boundary can land anywhere up to ~24h after it (the scheduler's own
/// `TICK_INTERVAL`). A save that lands in that gap — after the
/// nominal week-1 window closes but before the scheduler actually looks — is
/// a save all the same, and the module docs above already frame the bar as
/// "never saved a workflow **at all**", not "saved one in exactly the first
/// 168 hours". Stopping the count at a fixed `signup_millis + 7d` regardless
/// of when the scheduler is asking would nudge (and email) a user who, by
/// the time the message lands, has already done the thing being asked of
/// them. The caller always passes its tick's own `now`, so this is
/// equivalent to "has the user saved one by the moment we're about to nudge
/// them" — never wider than that, since a save has to exist to be counted at
/// all.
///
/// Reads the whole journal — the same cost
/// [`activation::any_workflow_run_succeeded`](crate::company::activation)
/// accepts and for a similar reason: no indexed "workflow creates by actor"
/// query exists, and this is called at most once per user per scheduler tick,
/// bounded to users still inside
/// [`LifecycleScheduler`](crate::runtime::LifecycleScheduler)'s lookback
/// window rather than for every user forever.
pub(crate) async fn user_saved_workflow_in_week1(
    company: &CompanyId,
    events: &Arc<dyn EventLog>,
    user_id: &str,
    signup_millis: u64,
    evaluated_at_millis: u64,
) -> Result<bool> {
    let stored = events
        .read_from(company, crate::ports::types::EventSeq::new(0), usize::MAX)
        .await?;
    Ok(stored.iter().any(|entry| {
        entry.at_millis >= signup_millis
            && entry.at_millis <= evaluated_at_millis
            && matches!(
                &entry.event,
                CompanyEvent::WorkflowCreated { by: Some(actor), .. }
                    if actor.id == user_id
            )
    }))
}

#[cfg(test)]
#[path = "week1_nudge_tests.rs"]
mod tests;
