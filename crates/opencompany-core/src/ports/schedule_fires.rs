//! The [`ScheduleFireStore`] port: a durable, cross-replica claim on one
//! schedule's fire instant.
//!
//! Both schedulers — [`CompanyScheduler`](crate::runtime::CompanyScheduler) for
//! manifest `[[schedule]]` crons and
//! [`WorkflowScheduler`](crate::runtime::WorkflowScheduler) for saved-workflow
//! triggers — used to dedup fires purely in process memory (a
//! `HashMap<_, u64>` of the last minute each schedule fired). That map cannot
//! survive a restart and cannot see a second replica, so a schedule whose fire
//! instant fell during a deploy was silently dropped, and two hosted replicas of
//! one tenant both fired every schedule. This port is the durable record those
//! two failures needed and never had.
//!
//! ## One row per (company, schedule, minute)
//!
//! The unit of the claim is the exact UTC epoch minute both ticks already
//! compute (`minute = now / MINUTE_MS`), so no new time math is introduced and
//! two processes whose clocks agree within a minute compute the *same* key. The
//! composite key is `(company_id, schedule_id, scheduled_for)`; the winning
//! caller is whoever inserts that row first, decided by the backend's own
//! uniqueness primitive:
//!
//! * **sqlite** — `INSERT OR IGNORE` into a table with
//!   `PRIMARY KEY (company_id, schedule_id, scheduled_for)`; rows-affected picks
//!   the winner.
//! * **mongodb** — `insert_one` against a unique compound index; an `E11000`
//!   duplicate-key means lost. This is the arbiter that actually matters, since
//!   hosted replicas share the tenant database.
//! * **fs** — an `OpenOptions::create_new` (`O_EXCL`) marker file. Declared
//!   single-node-only: `O_EXCL` is not trustworthy on NFS, and hosted
//!   deployments run mongodb.
//!
//! A claim is **terminal** — there is no lease and no release. Awaited *before*
//! any side effect, it gives at-most-once firing: the loser skips with zero side
//! effects, and a cycle that fails *after* a won claim burns that instant rather
//! than risking a double external effect (a second email, a second task, more
//! token spend). Because the key is the minute itself, a burned instant cannot
//! wedge a schedule — the next minute is a fresh key.
//!
//! ## Schedule identity is restart-stable and path-safe
//!
//! `schedule_id` is a caller-minted string, never a positional index:
//!
//! * workflow crons use `"workflow-<workflow_id>"`;
//! * manifest schedules use `"manifest-" + hex(sha256(cron + "\n" + prompt))`
//!   truncated to 16 hex chars, so reordering the manifest does not reassign a
//!   schedule's identity and two identical `(cron, prompt)` entries collapse to
//!   one fire.
//!
//! The hyphen prefixes (no colon) keep the id readable in a log line. The fs
//! backend hashes the id again before letting it address the filesystem, so an
//! id the store did not mint can never become a path component (the same rule
//! [`Bundle::runs_jsonl`](crate::store::paths) documents for run ids).
//!
//! ## Retention
//!
//! [`latest_fire`](ScheduleFireStore::latest_fire) is the restart catch-up
//! anchor (the most recent minute this schedule is known to have fired), and
//! [`prune_fires_before`](ScheduleFireStore::prune_fires_before) bounds growth —
//! a `* * * * *` schedule writes 1440 rows a day. The maintenance tick prunes at
//! a cutoff that sits *outside* the catch-up window, so the anchor can never be
//! pruned out from under a catch-up.

use async_trait::async_trait;

use crate::Result;
use crate::ports::types::CompanyId;

/// Durable per-company claims that one schedule fired at one UTC epoch minute.
///
/// Company A's claims MUST be invisible to company B, exactly like every other
/// per-company port. The four methods are storage verbs only: the schedulers
/// own the claim *policy* (claim before side effect, fail closed on error, one
/// bounded catch-up), and a backend only ever sees "insert this key if absent",
/// "what is the max minute for this schedule", "delete minutes below this
/// cutoff", "delete every minute for one schedule".
#[async_trait]
pub trait ScheduleFireStore: Send + Sync {
    /// Atomically records that `schedule_id` fired at epoch `minute` for
    /// `company`, if no peer has already recorded it.
    ///
    /// Returns `Ok(true)` when *this* caller inserted the row (it won the race
    /// and may proceed to fire), `Ok(false)` when the row already existed (a
    /// peer — another replica, or this process before a restart — already
    /// claimed the instant, so the caller must skip with zero side effects).
    ///
    /// The atomicity MUST come from the backend's own uniqueness primitive
    /// (`INSERT OR IGNORE`, a unique index, `O_EXCL`), never from an in-process
    /// lock: an in-process lock cannot see the second replica this port exists
    /// to arbitrate against.
    ///
    /// `claimed_at_ms` (wall-clock at insert) may be stored alongside for
    /// debugging, but it is not part of the key and no caller reads it back.
    async fn claim_fire(&self, company: &CompanyId, schedule_id: &str, minute: u64)
    -> Result<bool>;

    /// The highest `minute` ever claimed for `(company, schedule_id)`, or `None`
    /// when the schedule has never fired.
    ///
    /// This is the restart catch-up anchor. `None` (a fresh install) means "no
    /// anchor", which the schedulers read as "no catch-up" — a company that has
    /// never fired a schedule has nothing to make up.
    async fn latest_fire(&self, company: &CompanyId, schedule_id: &str) -> Result<Option<u64>>;

    /// Removes every claim row for `company` whose `minute` is strictly less
    /// than `cutoff_minute`, returning how many rows were removed.
    ///
    /// Called from the maintenance tick with a cutoff far enough in the past
    /// that a catch-up anchor is never eligible (see
    /// [`PRUNE_CUTOFF_MINUTES`](crate::runtime::PRUNE_CUTOFF_MINUTES) vs
    /// [`CATCHUP_WINDOW_MINUTES`](crate::runtime::CATCHUP_WINDOW_MINUTES)).
    async fn prune_fires_before(&self, company: &CompanyId, cutoff_minute: u64) -> Result<usize>;

    /// Removes *every* claim row for `(company, schedule_id)`, whatever its
    /// minute, returning how many rows were removed.
    ///
    /// Called when a schedule's identity is retired — a workflow is deleted
    /// (issue #708) — so a later schedule that mints the *same* `schedule_id`
    /// (a delete+recreate of a workflow reuses `"workflow-<id>"`, which is
    /// deliberately restart-stable and NOT re-keyed) starts with an empty
    /// ledger: no [`latest_fire`](ScheduleFireStore::latest_fire) anchor to
    /// mis-anchor its catch-up on, and every past minute claimable again so the
    /// first tick after recreation fires instead of losing to a stale claim.
    ///
    /// A `schedule_id` that never fired removes nothing and returns `Ok(0)`;
    /// the call is idempotent (a second delete of the same id also returns
    /// `Ok(0)`). Unlike [`claim_fire`](ScheduleFireStore::claim_fire), this is
    /// not a race arbiter — it is a one-shot purge on the delete path.
    async fn delete_schedule_fires(&self, company: &CompanyId, schedule_id: &str) -> Result<usize>;
}
