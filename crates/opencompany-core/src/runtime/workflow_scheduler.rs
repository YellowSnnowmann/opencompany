//! [`WorkflowScheduler`]: fires saved workflows whose trigger carries a cron.
//!
//! A workflow graph's `trigger` node may carry a `schedule` — a standard 5-field
//! cron expression, always UTC (issue #169). Without this scheduler that field
//! would be inert: a workflow would still only run when an operator clicked Run.
//! This module is the half that makes a saved schedule actually fire.
//!
//! It is deliberately shaped like the manifest-cron
//! [`CompanyScheduler`](super::scheduler::CompanyScheduler) — same injectable
//! [`Clock`], same minute-boundary sleep loop, same
//! [`spawn`](WorkflowScheduler::spawn) shape — and reuses the same
//! [`CronExpr`] matcher, so the two schedules speak one dialect and no new
//! dependency is introduced. Three things differ, each for a reason:
//!
//! * **One process-global task, not one per company.** Workflow schedules
//!   mutate at runtime (creating a workflow adds a cron with no reboot) and a
//!   hosted tenant can be provisioned after boot, so the tick re-reads
//!   [`CompanyRegistry::list`] every minute rather than snapshotting companies
//!   at boot.
//! * **Enumeration includes the global baseline after the seed ∪ overlay
//!   union**, through
//!   [`list_workflows_with_globals`](crate::company::list_workflows_with_globals),
//!   never a raw `source_dir` scan and never the manifest's
//!   `[workflows].enabled` list. Company graphs keep precedence, and
//!   `[globals].disable` removes opted-out globals. See [`WorkflowScheduler::tick`]
//!   for why the enabled list is not a filter.
//! * **Each fire runs on its own tokio task**, so one long agent run cannot
//!   starve every other company's schedule, with an in-flight guard so a slow
//!   run is never overlapped by its own next tick.
//!
//! Missed runs are skipped, never caught up — identical to the manifest cron
//! scheduler, so a restart after downtime does not burst.
//!
//! ## Report delivery on a scheduled run
//!
//! An `output` node may route its report to a person or a channel (issue #170).
//! The runner delivers it either way — the scheduler drives the same
//! [`WorkflowRunner`] port the console's Run button does — but only a manual run
//! gets the [`WorkflowRun::deliveries`](crate::ports::WorkflowRun) rows back in
//! an HTTP response for the console to render. A scheduled run has no response
//! and no one watching, so this module logs them instead.
//!
//! **Be clear about the ceiling of that.** A log line reaches whoever can read
//! the host's stdout. On a self-hosted deployment that is the operator; on a
//! hosted tenant it is emphatically not — it is us. So the log makes a failed
//! scheduled delivery *diagnosable*, not *operator-visible*.
//!
//! Issue #228 closes that gap without inventing a subsystem: every finished run
//! — this scheduler's and the console's Run button alike — is journaled as a
//! [`CompanyEvent::WorkflowRunFinished`](crate::ports::types::CompanyEvent)
//! through [`record_run_finished`](crate::runtime::record_run_finished), projected
//! live onto the operator SSE stream,
//! and read back durably from `GET …/workflows/runs`. The log lines below stay
//! exactly as they were: they remain the platform team's diagnostic, and the
//! event is the operator's surface. The two answer to different readers, so
//! neither replaces the other.
//!
//! **That split decides what the log lines may say.** Because host stdout is a
//! platform surface, the undelivered-report warning below carries only fields
//! that are safe for a reader who is not the tenant: the company, the workflow,
//! the node, the destination *kind*, whether a target resolved at all, the
//! status, and a [`DeliveryReason`](crate::ports::DeliveryReason). It does
//! **not** carry the target, and — since issue #248 — it does not carry
//! [`DeliveryReport::detail`](crate::ports::DeliveryReport) either: `detail`
//! interpolates the transport's own text on the failure arms, and a mail
//! transport quotes the mailbox it refused. `DeliveryReason` is the closed set
//! that says the same thing about the failure without the ability to carry the
//! address. The full `detail` still reaches the operator, through the run
//! response and the journaled event above.
//!
//! (A run outcome is deliberately *not* modelled as issue #242's first-class
//! `RunRecord`. That is a task-attempt record minted at the task dispatch choke
//! point and keyed to a board task with an attempt ordinal; a workflow run
//! enters through the [`WorkflowRunner`] port, has no task, and produces
//! host-side delivery rows per output node. The shapes don't meet.)

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::company::{WorkflowFile, list_workflows_with_global_baseline};
use crate::ports::types::CompanyId;
use crate::ports::{DeliveryReport, DeliveryStatus, WorkflowRunContext, is_undelivered};
use crate::runtime::CompanyRegistry;
use crate::runtime::WorkflowSpawn;
use crate::runtime::cron::{CivilTime, CronExpr};
use crate::runtime::run_supervisor::RunGuard;
use crate::runtime::scheduler::{
    CATCHUP_WINDOW_MINUTES, Clock, MINUTE_MS, PRUNE_CUTOFF_MINUTES, millis_to_next_minute,
    missed_instant,
};

/// Identifies one schedulable workflow: which company, which graph.
type WorkflowKey = (CompanyId, String);

/// How a scheduled run's report deliveries came out, folded for one log line.
///
/// A count per status rather than a bare total: `skipped` and `denied` mean
/// policy refused to send (a missing `email` grant, no mailbox) while `failed`
/// means something broke, and an operator reading a scheduled run's outcome
/// needs to tell those apart before deciding whether to act.
///
/// `pending` is the fourth thing entirely, and the reason this line matters
/// most on the scheduled path: the report is not lost and nothing is broken —
/// it is sitting in the approvals queue waiting for a human. A scheduled run
/// is exactly the nobody-is-watching case, so without this count an operator
/// would have no signal that a card is waiting for them.
#[derive(Debug, Default, PartialEq, Eq)]
struct DeliveryCounts {
    sent: usize,
    pending: usize,
    skipped: usize,
    denied: usize,
    failed: usize,
    /// Reports that did NOT reach their destination **and never will without a
    /// change** — the number worth alerting on.
    ///
    /// A field rather than `skipped + denied + failed`, because that sum stopped
    /// being the definition (issue #981): `skipped` also covers a report an
    /// earlier run in the approval lineage already sent and a test run that
    /// deliberately attempted nothing, and neither is a report that went
    /// missing. [`is_undelivered`] is the one rung every surface stands on, so
    /// this counts through it and the four per-status numbers stay exactly what
    /// they are — a breakdown for the log line, not a classification.
    ///
    /// `pending` is excluded there for its own reason: it is awaiting a verdict,
    /// not a fix, and folding it in would page someone for a queue that is
    /// working as designed. It gets its own count on the summary line.
    undelivered: usize,
}

impl DeliveryCounts {
    fn of(reports: &[DeliveryReport]) -> Self {
        let mut counts = Self::default();
        for report in reports {
            match report.status {
                DeliveryStatus::Sent => counts.sent += 1,
                DeliveryStatus::Pending => counts.pending += 1,
                DeliveryStatus::Skipped => counts.skipped += 1,
                DeliveryStatus::Denied => counts.denied += 1,
                DeliveryStatus::Failed => counts.failed += 1,
            }
            if is_undelivered(report) {
                counts.undelivered += 1;
            }
        }
        counts
    }

    /// See [`DeliveryCounts::undelivered`].
    fn undelivered(&self) -> usize {
        self.undelivered
    }
}

/// Drives the cron schedules authored on saved workflow graphs.
pub struct WorkflowScheduler {
    registry: CompanyRegistry,
    clock: Arc<dyn Clock>,
    /// Per-workflow last-fired epoch minute, so a workflow fires at most once
    /// per minute no matter how often [`tick`](Self::tick) is called.
    last_fired: HashMap<WorkflowKey, u64>,
    /// Scheduled runs currently executing. Shared with the spawned run tasks,
    /// which remove their key on completion, so a run that outlives its minute
    /// suppresses its own next fire instead of stacking up.
    in_flight: Arc<Mutex<HashSet<WorkflowKey>>>,
    /// Companies already warned about having scheduled workflows but no
    /// [`WorkflowRunner`](crate::ports::WorkflowRunner) to run them on. The tick
    /// is once a minute forever, so the warning is latched here and re-armed
    /// only when the situation changes — see [`note_unwired`](Self::note_unwired).
    warned_unwired: HashSet<CompanyId>,
    /// Workflows whose one restart catch-up attempt has **completed** (issue
    /// #241). Keyed per `(company, workflow)` and inserted the FIRST time each is
    /// seen — not once globally — so a tenant provisioned after boot and a
    /// workflow created at runtime each get their catch-up when they first
    /// appear, rather than being missed by a boot-only pass. Swept when a company
    /// leaves the registry, like [`warned_unwired`](Self::warned_unwired).
    ///
    /// The invariant is **latched ⇔ the catch-up attempt COMPLETED**, not merely
    /// "was reached once". An attempt completes when it fires the make-up
    /// (`Ok(true)`), finds a peer already claimed the minute (`Ok(false)`), or
    /// finds nothing to make up (`missed_instant == None`) — all terminal. But an
    /// attempt that could not run to a verdict on a *transient* condition —
    /// admission rejected at the #401 cap, the durable anchor read failing, the
    /// catch-up claim failing, or a still-in-flight prior run holding the overlap
    /// slot — DROPS the key again (issue #661 F2), so a later tick re-attempts the
    /// make-up while the missed minute is still inside the catch-up window rather
    /// than the transient forfeiting that workflow's catch-up for the life of the
    /// process. The re-attempt is bounded: at most one [`latest_fire`] read per
    /// key per minute-tick, and it stops the moment an attempt completes.
    ///
    /// [`latest_fire`]: crate::ports::ScheduleFireStore::latest_fire
    caught_up: HashSet<WorkflowKey>,
    /// How many unwired-company warnings have been emitted, so a test can assert
    /// the latch actually suppresses the repeat.
    #[cfg(test)]
    unwired_warnings: usize,
}

impl WorkflowScheduler {
    /// Builds a scheduler over every company in `registry`, driven by `clock`.
    pub fn new(registry: CompanyRegistry, clock: Arc<dyn Clock>) -> Self {
        Self {
            registry,
            clock,
            last_fired: HashMap::new(),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            warned_unwired: HashSet::new(),
            caught_up: HashSet::new(),
            #[cfg(test)]
            unwired_warnings: 0,
        }
    }

    /// Runs one tick: fires every saved workflow whose trigger schedule matches
    /// the current UTC minute. Returns how many runs were started.
    ///
    /// Per company, in order, the tick skips:
    ///
    /// * a company whose `ensure_running` guard rejects (paused or archived) —
    ///   the same guard [`CompanyScheduler::tick`](super::scheduler::CompanyScheduler::tick)
    ///   uses, so schedules resume cleanly on unpause;
    /// * a company with no [`WorkflowRunner`](crate::ports::WorkflowRunner)
    ///   wired — the default build has none, so it stays inert through the same
    ///   port seam the run route reports `not_wired` on. If that company *does*
    ///   have scheduled workflows the skip is announced once (see
    ///   [`note_unwired`](Self::note_unwired)), because a configured build whose
    ///   inference source failed to resolve lands here too and would otherwise
    ///   look identical to a working one;
    /// * a workflow whose graph is malformed (skipped by the union loader with a
    ///   warning, so one bad graph never silences the rest);
    /// * a workflow already fired this minute, or whose previous scheduled run
    ///   is still in flight;
    /// * a workflow the operator has switched **off** (issue #276) — see below.
    ///
    /// # The switch, and which one it is (issue #276)
    ///
    /// The tick gates on
    /// [`CompanyRecord::workflow_enabled`](crate::ports::types::CompanyRecord::workflow_enabled),
    /// **not** on the manifest's `[workflows].enabled` list. Until #276 it gated
    /// on neither, and the argument for that was: a trigger `schedule` is itself
    /// the operator's "run this on a cron" statement, so re-asking a second list
    /// adds a second switch for one decision.
    ///
    /// That argument was right about `[workflows].enabled` and wrong about
    /// having no switch at all. Two things it did not account for:
    ///
    /// * **There was no way to pause.** Stopping a schedule meant deleting the
    ///   workflow, which threw the graph away to silence it for an afternoon.
    /// * **A schedule could arm itself without review.** An edit that added a
    ///   cron to a manual workflow — or an orchestrator-authored create — went
    ///   live on the next tick with nobody having looked at it.
    ///
    /// So the gate is a **dedicated** durable field rather than the manifest
    /// list, which stays exactly what it was: a declaration of which workflows
    /// this company was provisioned with. It could not have become the switch:
    /// `merge_enabled_workflows` (`src/runtime/builder.rs`, issue #208) rebuilds
    /// that list at boot from seed ids ∪ surviving overlay ids, so "off"
    /// expressed as absence from it would re-arm itself on the next restart.
    ///
    /// Enumeration still runs over the union of seed files and overlay graph
    /// bodies, both of which survive a rebuild — the gate filters that set, it
    /// does not replace it, so a paused workflow is still listed and still
    /// runnable by hand from the console's Run button. Pausing stops the
    /// *schedule*, not the workflow.
    ///
    /// The flag is read from the record this tick already loaded for its overlay
    /// bodies, so the gate costs no extra store round-trip.
    pub async fn tick(&mut self) -> usize {
        self.tick_with_globals(crate::globals::workflows()).await
    }

    async fn tick_with_globals(&mut self, global_workflows: &[WorkflowFile]) -> usize {
        let now = self.clock.now_millis();
        let minute = now / MINUTE_MS;
        let civil = CivilTime::from_unix_millis(now);
        // Issue #241: prune stale fire claims once a day, on the 00:00 UTC tick,
        // per visited company. Once daily rather than every tick keeps a
        // `* * * * *` schedule's 1440-rows-a-day log bounded without a delete on
        // every minute.
        let prune_due = civil.hour == 0 && civil.minute == 0;

        let mut fired = 0;
        for company in self.registry.list() {
            let Some(runtime) = self.registry.get(&company) else {
                continue; // removed between listing and lookup (archive)
            };
            // Not accepting work: paused or archived.
            if runtime.ensure_running().await.is_err() {
                continue;
            }
            // Emergency stop is a separate switch from `lifecycle` — a stopped
            // company still reports `running` — so `ensure_running` alone
            // misses it. This tick is the fifth doorway
            // `CompanyRuntime::ensure_not_emergency_stopped`'s own doc did not
            // enumerate: a cron fire starts a new workflow run without ever
            // reaching `run_cycle`, `spawn_follow_up`, or the boot reconciler.
            if runtime.ensure_not_emergency_stopped().is_err() {
                continue;
            }
            // The cutoff sits a full week past the catch-up window
            // (PRUNE_CUTOFF_MINUTES > CATCHUP_WINDOW_MINUTES), so an anchor a
            // booting replica still needs is never eligible. Best-effort — a
            // prune failure must not stop this company's schedules from firing.
            if prune_due {
                let cutoff = minute.saturating_sub(PRUNE_CUTOFF_MINUTES);
                if let Err(err) = runtime
                    .schedule_fires()
                    .prune_fires_before(&company, cutoff)
                    .await
                {
                    tracing::warn!(%company, %err, "workflow scheduler: pruning old fire claims failed");
                }
            }
            // The record's runtime-authored graph bodies, the ids the operator
            // has switched off, and global opt-outs come from the same load, so
            // the gates cannot observe different record versions. A company
            // with no persisted record contributes none; a store failure is
            // logged and skipped rather than aborting every other company's
            // schedules.
            //
            // A load failure skipping the company is what makes the gate
            // fail-safe: an unreadable record fires nothing, rather than firing
            // everything because the disable list came back empty.
            let (overlays, disabled, global_disable) = match runtime.store().load(&company).await {
                Ok(Some(record)) => (
                    record.overlay_workflows,
                    record.disabled_workflows,
                    record.manifest.globals.disable,
                ),
                Ok(None) => (Vec::new(), Vec::new(), Vec::new()),
                Err(err) => {
                    tracing::warn!(%company, %err, "workflow scheduler: cannot read company record");
                    continue;
                }
            };

            // Enumerate the company's scheduled workflows BEFORE checking for a
            // runner: whether any exist is exactly what decides if an unwired
            // company is misconfigured or simply has nothing to run.
            let mut scheduled: Vec<(WorkflowFile, String, CronExpr)> = Vec::new();
            for file in list_workflows_with_global_baseline(
                runtime.source_dir(),
                &overlays,
                &global_disable,
                global_workflows,
            ) {
                let Some(cron) = trigger_schedule(&file) else {
                    continue; // no schedule: manual-run only
                };
                // Issue #276: switched off. Filtered here, above the `scheduled`
                // list, so a paused workflow also does not count toward the
                // unwired-company warning below — a company whose only schedule
                // is paused is not misconfigured, it is switched off, and saying
                // otherwise would train an operator to ignore that warning.
                if disabled.iter().any(|id| id == &file.id) {
                    tracing::trace!(
                        %company,
                        workflow = %file.id,
                        "workflow scheduler: skipping a switched-off workflow"
                    );
                    continue;
                }
                // Validation already accepted this expression; a parse failure
                // here would mean a body written by an older/looser path, so
                // skip that one workflow loudly rather than panicking.
                let Ok(expr) = CronExpr::parse(&cron) else {
                    tracing::warn!(
                        %company,
                        workflow = %file.id,
                        schedule = %cron,
                        "workflow scheduler: skipping an unparsable schedule"
                    );
                    continue;
                };
                scheduled.push((file, cron, expr));
            }

            // No execution wired. This is the default build's inert seam, but it
            // is ALSO what a configured build looks like when its inference
            // source failed to resolve at boot — an operator-visible
            // misconfiguration in which a saved schedule is indistinguishable
            // from a broken one. Say so, once.
            let runner = match runtime.workflow_runner().cloned() {
                Some(runner) => {
                    // A runner appeared: re-arm the warning for this company.
                    self.warned_unwired.remove(&company);
                    runner
                }
                None => {
                    self.note_unwired(&company, scheduled.len());
                    continue;
                }
            };

            // Issue #440: the shared way to start a supervised run. Built once
            // per company (it holds cloned handles, so a per-fire clone is
            // cheap) and cloned into each fire's task below.
            //
            // This scheduler used to mint its own run id through the supervisor
            // and journal its own `WorkflowRunFinished` on both arms — a second
            // copy of the two rules `WorkflowSpawn` exists to own. The copies
            // agreed, which is exactly what made the duplication dangerous: a
            // fix to one would not have reached the other, and nothing would
            // have failed to say so.
            let spawn = crate::runtime::WorkflowSpawn::new(&runtime, runner);

            // The durable fire-claim store for this company (issue #241),
            // reached through the runtime resolved this tick.
            let store = runtime.schedule_fires().clone();

            // Issue #661: the per-company in-flight run cap (issue #401). The
            // scheduler admits a fire against it — `RunSupervisor::begin` —
            // synchronously on THIS tick thread, BEFORE the durable `claim_fire`
            // (see the catch-up and steady-state arms below), and holds the guard
            // across the claim. Admitting before claiming is what keeps a company
            // at its cap from durably claiming — and burning — a minute whose run
            // `begin` would then reject: at the cap `begin` refuses before any
            // claim, so nothing is marked and the guard it would have handed back
            // is simply never taken. There is no advisory `len() >= limit`
            // pre-check any more: `begin` enforces the cap under the same lock it
            // inserts under, so it is the authority and there is nothing to race
            // it against. Doing it on the tick thread — not inside a spawned task
            // — also makes the count EXACT for same-tick sibling schedules: an
            // admitted run's guard is registered before the NEXT schedule's
            // `begin` reads the count, so `cap` of N schedules all due at one
            // minute admit exactly `cap`, never all N.
            let supervisor = runtime.run_supervisor().clone();

            for (file, cron, expr) in scheduled {
                let key = (company.clone(), file.id.clone());
                // The restart-stable durable identity for this workflow's cron.
                let schedule_id = workflow_schedule_id(&file.id);

                // First-sight restart catch-up (issue #241). The FIRST time this
                // scheduler sees a (company, workflow), make up at most one fire
                // that fell during downtime — covering a tenant provisioned after
                // boot and a workflow created at runtime, neither of which a
                // boot-only pass would reach. A disabled workflow never gets here
                // (filtered above), so a paused schedule is never caught up.
                if self.caught_up.insert(key.clone()) {
                    match store.latest_fire(&company, &schedule_id).await {
                        Ok(anchor) => {
                            if let Some(missed) =
                                missed_instant(&expr, anchor, minute, CATCHUP_WINDOW_MINUTES)
                            {
                                // Hold the overlap slot across the catch-up run so
                                // a same-minute steady-state fire below suppresses
                                // WITHOUT claiming (the minute is suppressed, not
                                // burned) rather than running a second copy.
                                if let Some(claim) = self.claim(&key) {
                                    // Issue #661: admit (`RunSupervisor::begin`)
                                    // BEFORE the durable `claim_fire`, holding the
                                    // guard across it. At the #401 in-flight cap
                                    // `begin` refuses here, so `missed` is never
                                    // claimed and the make-up is DEFERRED, not
                                    // burned: drop the first-sight latch so a later
                                    // tick re-attempts catch-up — `missed` is still
                                    // inside the catch-up window — once a slot
                                    // frees, and `continue` so the steady-state arm
                                    // does not also log an at-cap line for the same
                                    // key on this tick.
                                    let (ctx, guard) = match supervisor.begin(&file.id, true) {
                                        Ok(admitted) => admitted,
                                        Err(_) => {
                                            drop(claim);
                                            self.caught_up.remove(&key);
                                            tracing::info!(
                                                %company,
                                                workflow = %file.id,
                                                schedule = %cron,
                                                limit = supervisor.limit(),
                                                missed_minute = missed,
                                                "workflow scheduler: company at its in-flight run cap; deferring restart catch-up, leaving the missed minute unclaimed for a later tick to make up while inside the catch-up window"
                                            );
                                            continue;
                                        }
                                    };
                                    match store.claim_fire(&company, &schedule_id, missed).await {
                                        Ok(true) => {
                                            let input = json!({
                                                "request": format!("Scheduled run (cron `{cron}`)"),
                                                "scheduled": true,
                                                "cron": cron.clone(),
                                                // The ORIGINAL missed minute, not
                                                // now — the run is a make-up of it.
                                                "firedAtMs": missed * MINUTE_MS,
                                                "catchUp": true,
                                            });
                                            tracing::info!(
                                                %company,
                                                workflow = %file.id,
                                                schedule = %cron,
                                                missed_minute = missed,
                                                "workflow scheduler: firing one catch-up for a schedule missed during downtime"
                                            );
                                            spawn_scheduled_run(
                                                &spawn,
                                                ctx,
                                                guard,
                                                claim,
                                                company.clone(),
                                                file.clone(),
                                                input,
                                            );
                                            fired += 1;
                                        }
                                        // A simultaneously-booting replica claimed
                                        // the catch-up first: release the slot and
                                        // the admission.
                                        Ok(false) => {
                                            drop(guard);
                                            drop(claim);
                                        }
                                        Err(err) => {
                                            drop(guard);
                                            drop(claim);
                                            // Issue #661 (F2): a transient claim
                                            // failure defers, it does not forfeit.
                                            // Drop the first-sight latch (the #676
                                            // at-cap style, above) so a later tick
                                            // re-attempts the make-up while `missed`
                                            // is still inside the catch-up window.
                                            self.caught_up.remove(&key);
                                            tracing::warn!(%company, workflow = %file.id, %err, "workflow scheduler: could not claim catch-up fire; skipping (fail closed)");
                                        }
                                    }
                                } else {
                                    // Issue #661 (F2): the overlap slot is held by
                                    // a prior run still in flight, so the make-up
                                    // could not even be attempted this tick. Drop
                                    // the first-sight latch so a later tick — once
                                    // that run finishes and frees the slot —
                                    // re-attempts it while `missed` is still inside
                                    // the catch-up window, rather than the overlap
                                    // forfeiting catch-up for this key entirely.
                                    self.caught_up.remove(&key);
                                }
                            }
                        }
                        // Fail closed: without a trustworthy anchor we cannot tell
                        // a missed fire from an already-made one.
                        Err(err) => {
                            // Issue #661 (F2): the anchor read failed transiently,
                            // so this attempt reached no verdict. Drop the
                            // first-sight latch so a later tick re-reads the anchor
                            // and re-attempts catch-up — without this, one flaky
                            // read permanently forfeits this workflow's make-up for
                            // the life of the process.
                            self.caught_up.remove(&key);
                            tracing::warn!(%company, workflow = %file.id, %err, "workflow scheduler: could not read catch-up anchor; skipping catch-up (fail closed)");
                        }
                    }
                }

                if !expr.matches(&civil) {
                    continue;
                }

                if self.last_fired.get(&key) == Some(&minute) {
                    continue; // already fired this minute (in-process first pass)
                }

                // Overlap guard, checked BEFORE both admission and the durable
                // claim: a previous scheduled run still executing suppresses this
                // fire WITHOUT claiming the minute or touching the cap, so a slow
                // run's own next tick is suppressed rather than burned. Manual
                // runs go through the run route and are unaffected.
                //
                // Issue #661 (ordering): this sits ABOVE the cap decision so a
                // minute suppressed by overlap — or already fired this minute
                // (the `last_fired` guard above) — is never mislabelled as
                // skipped for cap reasons in the operator log.
                let Some(claim) = self.claim(&key) else {
                    tracing::info!(
                        %company,
                        workflow = %file.id,
                        schedule = %cron,
                        "workflow scheduler: previous scheduled run still in flight, skipping"
                    );
                    continue;
                };

                // Issue #661: admit against the #401 in-flight run cap BEFORE
                // claiming the minute, holding the guard across `claim_fire`.
                // `begin` is the authoritative admission (it enforces the cap
                // under the same lock it registers under), so there is no advisory
                // pre-check to race: at the cap it refuses HERE, before any
                // durable claim, and the minute is left UNCLAIMED — nothing in
                // `last_fired`, the overlap claim dropped, no durable claim.
                //
                // What recovers the minute is NOT the next expression match: for
                // any schedule coarser than `* * * * *` that minute has already
                // passed and the expression will not match again until its next
                // occurrence. It is the restart-style catch-up path — re-armed for
                // this key by dropping the first-sight latch below — which a later
                // tick re-attempts for the missed minute while it is still inside
                // the catch-up window, without needing a process restart.
                //
                // The guard is held across `claim_fire().await` below, so for that
                // window a cap slot is occupied by a run that may never start — the
                // case where a peer replica wins the durable claim (`Ok(false)`) and
                // this replica drops the guard unused. A sibling schedule admitting
                // in that window can therefore be refused at `begin` for capacity
                // that is about to be released. That is the conservative direction
                // (defer, never burn) and it is self-correcting: the refused sibling
                // drops its own first-sight latch and is made up by catch-up.
                let (ctx, guard) = match supervisor.begin(&file.id, true) {
                    Ok(admitted) => admitted,
                    Err(_) => {
                        drop(claim);
                        self.caught_up.remove(&key);
                        tracing::info!(
                            %company,
                            workflow = %file.id,
                            schedule = %cron,
                            limit = supervisor.limit(),
                            "workflow scheduler: company at its in-flight run cap; leaving this minute unclaimed and unfired, to be made up by catch-up on a later tick within the catch-up window (not the next expression match, for any schedule coarser than every-minute)"
                        );
                        continue;
                    }
                };

                // Admitted: mark the in-process dedup only now. A minute deferred
                // at the cap above never reaches here, so it stays eligible for
                // the catch-up re-attempt rather than being recorded as fired.
                self.last_fired.insert(key.clone(), minute);

                // Durable cross-replica claim (issue #241): the authority the
                // in-process `last_fired` map only approximates. Awaited before
                // the run spawns, so a loser produces zero side effects.
                match store.claim_fire(&company, &schedule_id, minute).await {
                    // Won: this replica fires.
                    Ok(true) => {}
                    // A peer already fired this minute: release the overlap slot
                    // and the admission, and skip with zero side effects.
                    Ok(false) => {
                        drop(guard);
                        drop(claim);
                        continue;
                    }
                    // Fail closed: never fire unclaimed, or the cross-replica
                    // double-fire this claim exists to prevent comes back.
                    Err(err) => {
                        drop(guard);
                        drop(claim);
                        tracing::warn!(%company, workflow = %file.id, %err, "workflow scheduler: could not claim a fire; skipping this minute (fail closed)");
                        continue;
                    }
                }

                let input = json!({
                    // `request` is what `run_request_text` reads, so every agent
                    // turn in the run knows it was started by a schedule rather
                    // than by an operator typing a topic.
                    "request": format!("Scheduled run (cron `{cron}`)"),
                    "scheduled": true,
                    "cron": cron.clone(),
                    "firedAtMs": now,
                });
                spawn_scheduled_run(&spawn, ctx, guard, claim, company.clone(), file, input);
                fired += 1;
            }
        }

        // --- sweep stale per-company/per-workflow state -------------------
        // Both maps are keyed on things that can disappear (a workflow is
        // deleted, a company is archived out of the registry), so both need a
        // sweep or they grow for the life of the process. One place, so there
        // is a single obvious point where this happens.

        // Only the CURRENT minute can dedupe a fire, so every older entry is
        // dead weight.
        self.last_fired.retain(|_, fired_at| *fired_at >= minute);

        // A company removed from the registry is never visited again, so
        // NEITHER re-arm path in `note_unwired` (a runner appearing, or the
        // scheduled count dropping to zero) can ever clear its latch — the
        // entry would be orphaned forever.
        let registry = &self.registry;
        self.warned_unwired
            .retain(|company| registry.get(company).is_some());
        // The catch-up latch is keyed per (company, workflow); a company archived
        // out of the registry is never visited again, so its entries would be
        // orphaned forever. Swept here beside the others. (A deleted-but-company-
        // still-present workflow leaves one stale entry, harmless: it is only a
        // "have I run catch-up for this" bit, cleared on the next sight. A
        // re-created workflow of the same id is a genuine first sight against an
        // EMPTY ledger, because the delete path purges the fire rows under
        // `workflow_schedule_id` (issue #708) — so there is no inherited anchor
        // for this re-armed latch to catch up against.)
        self.caught_up
            .retain(|(company, _)| registry.get(company).is_some());

        fired
    }

    /// Records that `company` has `scheduled` workflows but no runner to fire
    /// them on, warning at most once per company.
    ///
    /// Two rules, both deliberate:
    ///
    /// * **Silence when `scheduled == 0`.** A company with no scheduled
    ///   workflows and no runner is not misconfigured — it simply has nothing to
    ///   run. The latch is cleared in that case, so if a schedule is saved later
    ///   while the company is still unwired, the operator does get told.
    /// * **Once per company, not once per tick.** The tick is every minute
    ///   forever; logging there would be ~1440 lines a day per tenant, which
    ///   buries the signal it is trying to raise. The latch is re-armed in
    ///   [`tick`](Self::tick) as soon as a runner appears, and swept there when
    ///   the company leaves the registry — neither re-arm path can reach a
    ///   company that is no longer visited.
    fn note_unwired(&mut self, company: &CompanyId, scheduled: usize) {
        if scheduled == 0 {
            self.warned_unwired.remove(company);
            return;
        }
        if !self.warned_unwired.insert(company.clone()) {
            return; // already warned for this company
        }
        #[cfg(test)]
        {
            self.unwired_warnings += 1;
        }
        tracing::warn!(
            %company,
            scheduled_workflows = scheduled,
            "workflow scheduler: {scheduled} scheduled workflow(s) will not fire — no workflow \
             runner is wired for this company (inference source unresolved?)"
        );
    }

    /// Claims `key` for a run, or `None` when a run already holds it.
    ///
    /// The returned [`Claim`] IS the hold: dropping it releases the slot, so a
    /// caller that takes a claim and then bails out cannot strand it.
    fn claim(&self, key: &WorkflowKey) -> Option<Claim> {
        if lock_in_flight(&self.in_flight).insert(key.clone()) {
            Some(Claim {
                in_flight: self.in_flight.clone(),
                key: key.clone(),
            })
        } else {
            None
        }
    }

    /// Whether any scheduled run is currently executing.
    #[cfg(test)]
    fn is_running_any(&self) -> bool {
        !lock_in_flight(&self.in_flight).is_empty()
    }

    /// Spawns a background task that ticks on every minute boundary until
    /// `shutdown` is notified. Boot holds the join handle and the shared
    /// `shutdown` so the scheduler stops cleanly when the server does.
    pub fn spawn(mut self, shutdown: Arc<Notify>) -> JoinHandle<()> {
        tokio::spawn(async move {
            // The `Notified` future is built ONCE and pinned across iterations,
            // not rebuilt inside the `select!`. Boot signals with
            // `notify_waiters()`, which wakes only the waiters registered at
            // that instant — a future created fresh each iteration is not
            // registered while `tick` is running, so a shutdown arriving
            // mid-tick would be dropped and the scheduler would sleep another
            // full minute before noticing. Polled once here, this one stays
            // registered, and a notification delivered during `tick` is
            // latched: the next `select!` sees it immediately.
            let notified = shutdown.notified();
            tokio::pin!(notified);
            loop {
                let sleep_ms = millis_to_next_minute(self.clock.now_millis());
                tokio::select! {
                    _ = &mut notified => break,
                    _ = tokio::time::sleep(Duration::from_millis(sleep_ms)) => {
                        self.tick().await;
                    }
                }
            }
        })
    }
}

/// An RAII hold on one workflow's in-flight slot, released on drop.
///
/// A hold released by a statement after the `await` would survive a panic in
/// the run: the task unwinds, the statement never executes, and the key stays
/// in the set forever — permanently retiring that schedule. `Drop` runs while
/// unwinding, so tying the release to a guard closes that path.
struct Claim {
    in_flight: Arc<Mutex<HashSet<WorkflowKey>>>,
    key: WorkflowKey,
}

impl Drop for Claim {
    fn drop(&mut self) {
        lock_in_flight(&self.in_flight).remove(&self.key);
    }
}

/// Locks the in-flight set, recovering rather than panicking on a poisoned
/// mutex.
///
/// Two reasons this never unwraps. First, [`Claim::drop`] can run while
/// unwinding from the very panic that poisoned the lock, and a panic inside a
/// `Drop` during an unwind aborts the process. Second, the alternative — a
/// tolerant `if let Ok(..)` — would *skip* the release on a poisoned lock and
/// reintroduce the leak this guard exists to prevent. Recovering is safe here
/// because every critical section is a single `insert` / `remove` / `is_empty`
/// on a `HashSet`, none of which can leave it half-updated.
fn lock_in_flight(
    in_flight: &Mutex<HashSet<WorkflowKey>>,
) -> std::sync::MutexGuard<'_, HashSet<WorkflowKey>> {
    in_flight
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The durable [`ScheduleFireStore`](crate::ports::ScheduleFireStore) identity
/// for a saved workflow's cron trigger: `"workflow-<workflow_id>"` (issue #241).
///
/// The natural `(company, workflow)` identity, so it survives a restart and does
/// not depend on any positional index. The `workflow-` prefix (a hyphen, never a
/// colon) keeps it readable in a log line; the fs backend hashes it before it
/// can address the filesystem, so a console-chosen `<workflow_id>` is safe.
///
/// # Identity reuse across delete+recreate (issue #708)
///
/// Because this key is the workflow id alone, deleting a workflow and recreating
/// one with the **same id** would make the new workflow inherit the old one's
/// fire ledger — the durable `claim_fire` / [`latest_fire`] rows outlive the
/// graph. So the delete path purges those rows: `delete_company_workflow`
/// (`src/company/workflow_create.rs`) calls
/// [`delete_schedule_fires`](crate::ports::ScheduleFireStore::delete_schedule_fires)
/// under this exact key after the graph, revisions, and events are gone, so a
/// recreated same-id workflow starts against an empty ledger — no stale anchor
/// to mis-anchor a catch-up on, and every past minute claimable again.
///
/// The key is **deliberately not re-keyed** to force that separation — the
/// re-key was considered for #661 and rejected for three reasons, which is why
/// the fix lives on the delete path, not in this id:
///
/// 1. **Per-deploy catch-up loss** — a new key orphans every deployed tenant's
///    existing anchors, so the first boot after the change treats every armed
///    schedule as a fresh install and forfeits its legitimate restart catch-up.
/// 2. **Rolling-deploy double-fire (a #241 regression)** — during a rolling
///    deploy, mixed-version replicas would key the same minute under two ids and
///    both could win a claim: the exact cross-replica double-fire the #241
///    durable claim exists to prevent.
/// 3. It is the natural, restart-stable `(company, workflow)` identity that
///    survives a restart without depending on a positional index.
///
/// Minted here at the one authoritative site and re-exported (`pub(crate)`) so
/// the delete path forms the same key from the same code, never a duplicated
/// format string.
///
/// [`latest_fire`]: crate::ports::ScheduleFireStore::latest_fire
pub(crate) fn workflow_schedule_id(workflow_id: &str) -> String {
    format!("workflow-{workflow_id}")
}

/// Starts one already-admitted scheduled run and, on a background task, awaits
/// it while holding `claim` for the run's whole lifetime and logging its
/// delivery outcome.
///
/// Shared by the steady-state fire path and the restart catch-up (issue #241),
/// so both start a run and record it the one way [`WorkflowSpawn`] owns (issues
/// #228 / #383 / #440); the callers differ only in the `firedAtMs` / `catchUp`
/// they stamp into `input`.
///
/// Issue #661: admission is no longer this function's job. The caller has
/// already run `RunSupervisor::begin` on the tick thread — ordered before its
/// durable `claim_fire` — and hands the admitted `(ctx, guard)` in. So starting
/// the run is now infallible: [`WorkflowSpawn::spawn_admitted`] cannot refuse,
/// and there is no at-cap arm here any more (a company at its cap never reaches
/// this call — the caller left the minute unclaimed instead). `spawn_admitted`
/// is called synchronously on the tick thread so the guard's slot is registered
/// before the loop moves to the next schedule; only the await + delivery-log
/// sweep is pushed onto a background task.
///
/// A FRESH TASK PER FIRE IS CORRECT HERE, and is not a hole in the
/// `WORKFLOW_DEPTH` re-entry guard (`crate::workflows::runner`). That guard is a
/// task-local counting one *causal chain*: a run, its agent turns, and the tools
/// those turns call all stay on one task, so a workflow that reaches back into
/// itself is bounded. A scheduled fire starts no such chain — it is a new root,
/// at depth 0, exactly like an operator clicking Run. The engine runs on
/// `spawn_admitted`'s own task, and awaiting it on a second task here keeps one
/// slow agent run from starving every other company's schedule on the tick loop.
fn spawn_scheduled_run(
    spawn: &WorkflowSpawn,
    ctx: WorkflowRunContext,
    guard: RunGuard,
    claim: Claim,
    company: CompanyId,
    workflow: WorkflowFile,
    input: serde_json::Value,
) {
    let workflow_id = workflow.id.clone();
    // Issue #661: start the run HERE, on the tick thread, through the shared
    // primitive (issue #440 — supervisor-minted run id #383/#371 and
    // `record_run_finished` on BOTH arms #228, with the run guard held across the
    // journal write). Synchronous `spawn_admitted` runs the engine on its own
    // task and hands back the join handle; the admission (`begin`) already
    // happened in `tick`, so this cannot fail. Issue #542: a scheduled run is
    // always for real — `dry_run = false`.
    //
    // Cloned before the spawn: the run task and the awaiting task both outlive
    // the tick's borrow of `runtime`, so everything they need is moved in. The
    // clone carries the company id, the event log (issue #228), the run
    // supervisor (issue #383 — the Cancel button on a cron fire) and the runner.
    let (_run_id, handle) = spawn
        .clone()
        .spawn_admitted(ctx, guard, workflow, input, false);
    tokio::spawn(async move {
        // Held for the whole run so the overlap slot is released on EVERY exit
        // path, including an unwind. Releasing after the `await` instead would
        // leak the claim when the runner panics — and a leaked claim is
        // permanent, because nothing else ever removes it, so one panic would
        // retire that schedule for the life of the process with no log line.
        //
        // The handle is **awaited**, not dropped: the claim is this scheduler's
        // own overlap guard and has to outlive the run, and the delivery-log
        // sweep below needs the outcome.
        let _claim = claim;
        match handle.await {
            Ok(Ok(run)) => {
                // A manual run hands `deliveries` back in the HTTP response and
                // the console renders it. A scheduled run has no response and
                // nobody watching, so without this the exact case the operator
                // most needs to know about — the owner summary that did NOT go
                // out — would be the quietest thing the system does.
                let counts = DeliveryCounts::of(&run.deliveries);
                for report in &run.deliveries {
                    if report.status == DeliveryStatus::Sent {
                        continue;
                    }
                    // A parked report is not a failure and must not be logged as
                    // one — it is a card waiting in the approvals queue. Say where
                    // to go, at info, and move on.
                    if report.status == DeliveryStatus::Pending {
                        tracing::info!(
                            %company,
                            workflow = %workflow_id,
                            node = %report.node,
                            kind = %report.kind,
                            // Same reason as the warn below: never the recipient's
                            // address in a host log.
                            target_configured = report.target.is_some(),
                            "workflow scheduler: a scheduled run's report is parked for operator approval — see the Approvals view"
                        );
                        continue;
                    }
                    // Issue #981: a row whose fate is accounted for is not a
                    // problem to warn about. An approval-gate continuation
                    // deliberately does not re-send a report an earlier run in
                    // its lineage already delivered (issue #438), and warning
                    // once a minute about a graph behaving exactly as designed
                    // is how a real refusal gets scrolled past. The `skipped=`
                    // number on the summary line below still carries it.
                    if !is_undelivered(report) {
                        continue;
                    }
                    tracing::warn!(
                        %company,
                        workflow = %workflow_id,
                        node = %report.node,
                        kind = %report.kind,
                        // NOT the target itself: for an `email` destination that
                        // is the recipient's address, and this line goes to host
                        // stdout — which on a hosted tenant is us, not the
                        // operator. Whether one resolved is the part with
                        // diagnostic value anyway.
                        target_configured = report.target.is_some(),
                        status = ?report.status,
                        // The classification, NOT `report.detail`. `detail`
                        // interpolates the transport's own words on the failure
                        // arms, and a mail transport quotes the mailbox it refused
                        // — so logging it walks the recipient address onto host
                        // stdout through the back door that scrubbing `target`
                        // left open (issue #248). `reason` says the same thing
                        // about what failed, out of a closed set that cannot carry
                        // transport text; the operator still gets the full
                        // `detail` on the run response and in the run history
                        // their own console reads back.
                        reason = %report.reason,
                        "workflow scheduler: a scheduled run's report was NOT delivered"
                    );
                }
                tracing::info!(
                    %company,
                    workflow = %workflow_id,
                    pending_approvals = run.pending_approvals.len(),
                    sent = counts.sent,
                    // Awaiting a human, not a fix — counted apart from
                    // `undelivered` for exactly that reason.
                    pending_approval = counts.pending,
                    skipped = counts.skipped,
                    denied = counts.denied,
                    failed = counts.failed,
                    // The one number worth alerting on, so a log query need not
                    // sum the three refusal kinds.
                    undelivered = counts.undelivered(),
                    "workflow scheduler: scheduled run finished"
                );
                // The operator-facing half — the journaled `WorkflowRunFinished`
                // the tenant's own console reads back — was written by `spawn`
                // before this handle resolved (issue #228). The log lines above
                // stay exactly as they are: they are the platform team's
                // diagnostic on host stdout, which on a hosted tenant is
                // emphatically not the operator.
            }
            Ok(Err(err)) => {
                tracing::warn!(
                    %company,
                    workflow = %workflow_id,
                    %err,
                    "workflow scheduler: scheduled run failed"
                );
            }
            // The run task itself came apart — a panic inside the runner, which
            // unwinds in the spawned task rather than here. Nothing was journaled
            // for it (the outcome write lives in that task), so this line is the
            // only trace there is, and it must not be silent. The claim still
            // releases: it is held by this task's guard.
            Err(err) => {
                tracing::error!(
                    %company,
                    workflow = %workflow_id,
                    %err,
                    "workflow scheduler: a scheduled run's task did not complete; its outcome was \
                     never recorded"
                );
            }
        }
    });
}

/// The cron a graph's trigger schedules itself on, if any.
///
/// Validation allows `schedule` only on a `trigger` node and permits at most one
/// scheduled trigger per graph, so the node this finds is the only one there is
/// — a graph with two schedules is rejected at parse rather than silently
/// resolving to whichever came first.
///
/// Delegates to [`WorkflowFile::trigger_schedule`] rather than re-deriving the
/// predicate: the disarm rule in `workflow_create.rs` reads the same one, and a
/// second copy here is how "the host thinks this is manual, the scheduler thinks
/// it is armed" would get in.
fn trigger_schedule(file: &WorkflowFile) -> Option<String> {
    file.trigger_schedule().map(str::to_string)
}

#[cfg(test)]
#[path = "workflow_scheduler_tests_core.rs"]
mod tests_core;
#[cfg(test)]
#[path = "workflow_scheduler_tests_part1.rs"]
mod tests_part1;
#[cfg(test)]
#[path = "workflow_scheduler_tests_part3.rs"]
mod tests_part3;
#[cfg(test)]
#[path = "workflow_scheduler_tests_part4.rs"]
mod tests_part4;
#[cfg(test)]
#[path = "workflow_scheduler_tests_path_1_the_cron_fire.rs"]
mod tests_path_1_the_cron_fire;
