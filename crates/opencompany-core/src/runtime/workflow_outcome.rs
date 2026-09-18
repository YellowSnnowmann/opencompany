//! Issue #228: journal what a workflow run actually did, from every entry point.
//!
//! A workflow run's outcome used to exist only in the moment. A **manual** run's
//! [`DeliveryReport`] rows lived in the console's run drawer until it was
//! dismissed; a **scheduled** run's reached only host stdout, which on a hosted
//! tenant is the platform team rather than the tenant's operator. Nothing wrote
//! a run outcome anywhere the console could read back afterwards — so the exact
//! thing an operator most needs to find later ("did last night's owner summary
//! actually go out?") was unfindable an hour after the run.
//!
//! This module is the one place that writes
//! [`CompanyEvent::WorkflowRunFinished`]. Both entry points — the console's
//! `POST …/workflows/{wid}/run` route and the cron
//! [`WorkflowScheduler`](super::WorkflowScheduler) — call
//! [`record_run_finished`], so a run's history is uniform no matter what started
//! it and the two call sites cannot drift apart in what they record.
//!
//! **Best-effort by construction.** The append happens *after* the run returns,
//! so it always records a finished run, and a failure to append is logged and
//! swallowed: journalling an outcome must never disturb the run path or fail a
//! run whose work already happened.
//!
//! It deliberately does **not** replace the scheduler's log lines. Those remain
//! the platform team's diagnostic on host stdout; this event is the *operator's*
//! surface, read back through `GET …/workflows/runs`.
//!
//! **Issue #371** added the other end of the same record. A run now also
//! journals a [`CompanyEvent::WorkflowRunStarted`] before the engine call and a
//! [`CompanyEvent::WorkflowNodeFinished`] per node as the graph is walked (both
//! written by the workflow runner), all sharing the `run_id` the entry point
//! mints and hands to [`record_run_finished`]. That correlation is what lets the
//! read side group a run's nodes with its outcome — and it is why a start with
//! no finish is meaningful, which [`sweep_interrupted_runs`] settles at boot.
//!
//! **Issue #383** added a third terminal reading. A run can now be *stopped by
//! an operator*, which is neither a failure nor a host restart, so it lands as
//! `cancelled: true` with **no error at all** rather than as an error string. A
//! cancelled run journals a real finish through this same helper, which is what
//! keeps [`sweep_interrupted_runs`] out of it: there is nothing left open to
//! sweep.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ports::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId, EventSeq};
use crate::ports::workflow_runner::{DeliveryReport, WorkflowRun, WorkflowRunBoardRow};
use crate::runtime::workflow_resume::DeliveredReport;

/// The error stamped on a run the host never got to finish (issue #371).
///
/// Phrased as a host fact rather than a workflow fault: nothing about the graph
/// went wrong, the process holding it went away. An operator reading this in the
/// history should go looking at the deployment, not at their nodes.
pub const INTERRUPTED_BY_RESTART: &str = concat!(
    "this run was interrupted by a host restart and never finished; ",
    "the nodes recorded against it are the ones that completed before it stopped"
);

/// Journals a finished workflow run, best-effort.
///
/// `scheduled` says whether a cron started the run rather than an operator —
/// the distinction is the point, since a scheduled run is the
/// nobody-was-watching case this record exists for.
///
/// `run_id` correlates this outcome with the run's
/// [`WorkflowRunStarted`](CompanyEvent::WorkflowRunStarted) and every
/// [`WorkflowNodeFinished`](CompanyEvent::WorkflowNodeFinished) between them
/// (issue #371). The caller mints it, because on the error arm the runner
/// returns nothing that could carry one — and a failed run's per-node trail is
/// exactly the one worth correlating.
///
/// `outcome` is what the [`WorkflowRunner`](crate::ports::WorkflowRunner)
/// returned, error included: a run that failed outright is recorded too, and is
/// in fact the most important thing here — today's `Err` arm on the scheduled
/// path only warns to host stdout, so **the worst outcome is currently the
/// quietest**.
/// The six fields a settled run contributes to its journal row, split out of
/// [`record_run_finished`] so the Ok/Err fold is one named thing rather than a
/// six-wide tuple.
///
/// Which fields ride which arm is the whole content of this type — see
/// [`Settled::from`].
struct Settled {
    deliveries: Vec<DeliveryReport>,
    pending_approvals: Vec<String>,
    error: Option<String>,
    cancelled: bool,
    notices: Vec<String>,
    board: Vec<WorkflowRunBoardRow>,
    blocked_nodes: Vec<crate::ports::WorkflowBlockedNode>,
    approvals: Vec<crate::ports::WorkflowRunApprovalRow>,
}

/// A run that ended in an error, as [`record_run_finished`] takes it (issue
/// #1008).
///
/// # Why this is not just a `&str`
///
/// It was, and that was the bug. A failed run's journal row listed no
/// deliveries, no board rows, no blocked nodes and no approvals — not because
/// the run had none, but because the *error arm had nowhere to read them from*.
/// A run that mailed two owner summaries and then broke at a later node showed
/// zero delivery rows, which reads as "it sent nothing" and is the opposite of
/// true; a run that opened a card and then failed left the card on the board
/// with no run admitting to it.
///
/// `partial` is what the run had done by the time it broke, threaded up from
/// the runner on
/// [`OpenCompanyError::WorkflowRunFailed`](crate::OpenCompanyError::WorkflowRunFailed).
/// `None` is still correct and still common: the boot sweep's synthetic finish
/// describes a run this process never saw, and a run refused before the engine
/// started has genuinely done nothing.
///
/// The `From<&str>` conversion keeps every call site that has only a message
/// reading as it did.
pub struct FailedRun<'a> {
    /// The failure, as the journal's `error` string.
    pub error: &'a str,
    /// What the run had already done, when the caller can say.
    pub partial: Option<&'a WorkflowRun>,
}

impl<'a> From<&'a str> for FailedRun<'a> {
    /// A failure with nothing known about what the run had done.
    fn from(error: &'a str) -> Self {
        Self {
            error,
            partial: None,
        }
    }
}

impl From<Result<&WorkflowRun, FailedRun<'_>>> for Settled {
    /// # Which fields ride which arm
    ///
    /// Issue #383: `cancelled` rides the `Ok` arm only, and that is not an
    /// oversight. A run the runner never returned from cannot have been *stopped
    /// by an operator* — the stop signal resolves into an `Ok(cancelled)`, never
    /// into an `Err` — so the error arm is unambiguously a failure or the boot
    /// sweep's synthetic one.
    ///
    /// # The other five ride both arms now (issue #1008)
    ///
    /// `deliveries`, `notices`, `board`, `blocked_nodes` and `approvals` used to
    /// be hard-coded empty on the `Err` arm, on the argument that a failure
    /// "returns no `WorkflowRun` to read rows off". That was true of the
    /// signature, not of the world: every one of those rows records something
    /// the run **already did** — a report that was mailed, a card that is on the
    /// board, an approval sitting on the operator's Approvals page — and all of
    /// them are durable by the time the run breaks. Zeroing them made the
    /// journal disagree with what the operator could see in front of them.
    ///
    /// So the failure arm now carries a [`FailedRun::partial`] when the caller
    /// has one, and reads exactly the same fields off it that the `Ok` arm
    /// reads. `pending_approvals` is the one deliberate exception: it describes
    /// what the run is *still waiting on*, and a run that failed is waiting on
    /// nothing.
    fn from(outcome: Result<&WorkflowRun, FailedRun<'_>>) -> Self {
        match outcome {
            Ok(run) => Self {
                deliveries: run.deliveries.clone(),
                pending_approvals: run.pending_approvals.clone(),
                error: None,
                cancelled: run.cancelled,
                notices: run.notices.clone(),
                board: run.board.clone(),
                blocked_nodes: run.blocked_nodes.clone(),
                approvals: run.approvals.clone(),
            },
            Err(failed) => {
                let partial = failed.partial;
                Self {
                    deliveries: partial.map(|p| p.deliveries.clone()).unwrap_or_default(),
                    // The exception, and it is a claim: a failed run is not
                    // waiting on anybody. Any gate it parked before it broke is
                    // still decidable from the Approvals page — `approvals`
                    // below is the receipt for that — but listing it here would
                    // say this run intends to continue, which it does not.
                    pending_approvals: Vec::new(),
                    error: Some(failed.error.to_string()),
                    cancelled: false,
                    notices: partial.map(|p| p.notices.clone()).unwrap_or_default(),
                    board: partial.map(|p| p.board.clone()).unwrap_or_default(),
                    blocked_nodes: partial.map(|p| p.blocked_nodes.clone()).unwrap_or_default(),
                    approvals: partial.map(|p| p.approvals.clone()).unwrap_or_default(),
                }
            }
        }
    }
}

/// Returns whether the finish was durably appended. `false` means the append
/// failed and was swallowed (the run itself is unaffected, exactly as before) —
/// so a caller that knows the run is otherwise about to disappear from the live
/// set can escalate the lost record from this helper's `warn` to an `error`
/// naming the run. Issue #1009 added the return; callers that do not care about
/// the outcome (the boot sweep, the read-side cross-check) simply ignore it.
pub async fn record_run_finished(
    events: &Arc<dyn EventLog>,
    company: &CompanyId,
    workflow_id: &str,
    scheduled: bool,
    run_id: &str,
    outcome: Result<&WorkflowRun, FailedRun<'_>>,
) -> bool {
    // Which fields ride which arm, and why, is documented on `Settled::from`.
    let settled = Settled::from(outcome);

    let event = CompanyEvent::WorkflowRunFinished {
        workflow_id: workflow_id.to_string(),
        scheduled,
        // Issue #371 started populating this reserved field. The event's own
        // wire shape is unchanged — it has always carried an optional `run_id` —
        // so a reader predating #371 still decodes every line it could before.
        run_id: Some(run_id.to_string()),
        deliveries: settled.deliveries,
        pending_approvals: settled.pending_approvals,
        error: settled.error,
        cancelled: settled.cancelled,
        notices: settled.notices,
        board: settled.board,
        blocked_nodes: settled.blocked_nodes,
        approvals: settled.approvals,
    };

    if let Err(err) = events.append(company, event).await {
        // Swallowed on purpose: the run already happened and its work is valid.
        // Losing the record is worth a loud line, never a failed run. The caller
        // learns of the loss through the `false` return (issue #1009) and may
        // escalate it — this helper stays best-effort and never fails the run.
        tracing::warn!(
            %company,
            workflow = %workflow_id,
            scheduled,
            %err,
            "workflow run outcome could not be journaled; the run itself was unaffected"
        );
        return false;
    }
    true
}

/// Terminates workflow runs a previous host process left open (issue #371).
///
/// # Why an unterminated start is provably dead
///
/// Issue #371 made a run journal a
/// [`WorkflowRunStarted`](CompanyEvent::WorkflowRunStarted) *before* the engine
/// call, so a host that dies mid-run leaves a start with no matching finish.
/// Every entry point — the console's run route, the cron scheduler, the
/// orchestrator's `run_workflow` tool — drives the run future **inside this
/// process**, and exactly one process owns a company's journal (it is a
/// single-writer log). So at boot, before any of those entry points can have
/// started anything, an unmatched start cannot belong to a live run: there are
/// no live runs. No timeout heuristic is needed, for the same reason
/// [`reap_orphaned_runs`](crate::ports::runs::reap_orphaned_runs) needs none.
///
/// This is what keeps the read side honest. `GET …/workflows/runs` folds a
/// start without a finish as `running: true`, and that claim is only true
/// because this sweep settles the ones that will never finish — otherwise a run
/// killed last week would show a spinner forever.
///
/// # It must NOT run on a rebuild
///
/// The argument above holds at boot and is false the moment a company has been
/// serving. A scheduler-spawned run survives a live runtime swap
/// ([`rebuild_company`](crate::runtime::rebuild_company)), so sweeping mid-life
/// would stamp "interrupted by a host restart" on a run that is still walking
/// its graph — and then its real finish would land afterwards, leaving two
/// contradictory outcomes for one run id. The caller gates on the handover being
/// absent; see the call site in the runtime builder. Same lesson as #290.
///
/// Best-effort throughout: a read or append failure is logged and swallowed,
/// because record-keeping must never stop a company from booting.
pub async fn sweep_interrupted_runs(events: &Arc<dyn EventLog>, company: &CompanyId) {
    let stored = match events
        .read_from(company, EventSeq::new(0), usize::MAX)
        .await
    {
        Ok(stored) => stored,
        Err(err) => {
            tracing::warn!(
                %company,
                %err,
                "could not read the journal to sweep interrupted workflow runs"
            );
            return;
        }
    };

    // One pass, keyed on run id: a start inserts, a finish removes. Whatever is
    // left started and never settled. `HashMap` rather than two sets because the
    // synthetic finish needs the start's `workflow_id` and `scheduled` flag, and
    // those live only on the start.
    let mut open: HashMap<String, (String, bool)> = HashMap::new();
    for stored in stored {
        match stored.event {
            CompanyEvent::WorkflowRunStarted {
                workflow_id,
                run_id,
                scheduled,
                // This fold groups a run's nodes with its outcome; who started
                // it (issue #1862 prerequisite) is not part of that grouping.
                started_by: _,
                resume_semantic: _,
            } => {
                open.insert(run_id, (workflow_id, scheduled));
            }
            CompanyEvent::WorkflowRunFinished {
                run_id: Some(run_id),
                ..
            } => {
                open.remove(&run_id);
            }
            // A pre-#371 finished row carries no run id and therefore closes
            // nothing. That is correct rather than a gap: it also had no start
            // to be matched against, so no such run can be sitting in `open`.
            _ => {}
        }
    }

    if open.is_empty() {
        return;
    }

    // Sorted so the appended order is deterministic — a `HashMap` iteration
    // order would make the journal's tail differ run to run for no reason, and
    // tests would have to sort around it.
    let mut interrupted: Vec<(String, (String, bool))> = open.into_iter().collect();
    interrupted.sort_by(|a, b| a.0.cmp(&b.0));

    for (run_id, (workflow_id, scheduled)) in interrupted {
        tracing::info!(
            %company,
            workflow = %workflow_id,
            %run_id,
            scheduled,
            "settling a workflow run left open by a previous host process"
        );
        record_run_finished(
            events,
            company,
            &workflow_id,
            scheduled,
            &run_id,
            Err(INTERRUPTED_BY_RESTART.into()),
        )
        .await;
    }
}

/// What the trailing suffix of **unclean** runs of one workflow already
/// delivered, folded from the journal (issue #529).
///
/// # The hole this closes
///
/// A run's deliveries live only on
/// [`WorkflowRunFinished::deliveries`](CompanyEvent::WorkflowRunFinished), which
/// is journaled *after* the run returns. A crash, a mid-graph failure, or a
/// panic therefore orphans the side effect: the report left the process, but the
/// boot sweep settles the run `FAILED` (or the run simply has no finish at all),
/// so no delivery record exists — and an operator's re-run re-mails every
/// already-sent report to real people. Issue #438's ledger does not cover this:
/// it rides one approval lineage's trigger input and is never persisted, so an
/// *independently* re-run or re-triggered workflow re-delivers.
///
/// This fold is the durable half. It replays the journal and returns the reports
/// dispatched by the **uncleanly-finished trailing runs** — the ones that owe
/// the next run nothing, because their work is stranded.
///
/// # Semantics: a clean finish absorbs and clears
///
/// One pass in journal order, holding a set of delivered nodes for `workflow_id`:
///
/// * a [`WorkflowReportDelivered`](CompanyEvent::WorkflowReportDelivered) for
///   this workflow **inserts** its node (write-behind at dispatch, so a
///   crashed run's sends are here even though its finish is not);
/// * a [`WorkflowRunFinished`](CompanyEvent::WorkflowRunFinished) for this
///   workflow with **no error clears** the set.
///
/// The clear is what keeps a daily scheduled digest delivering every day: a run
/// that finishes cleanly (a normal completion, or a
/// [`cancelled`](CompanyEvent::WorkflowRunFinished) one — which returns *before*
/// `deliver_outputs` and so dispatched nothing here, and carries no error)
/// absorbs everything delivered up to it, so the next run starts owed nothing.
/// Only deliveries by the *unclean* trailing suffix — a crash whose boot-sweep
/// finish carries the synthetic `error: Some(..)`, a mid-graph failure, or a
/// panic that never journaled a finish at all — survive the fold, and those are
/// exactly the ones a re-run must not repeat.
///
/// Identity is the **node**, so this unions cleanly with issue #438's
/// [`DeliveredReport`](crate::runtime::workflow_resume::DeliveredReport): the
/// caller concatenates the two and dedupes. An `owner` fan-out that wrote one
/// line per admin collapses to a single node entry here.
///
/// Best-effort read, the same posture as
/// [`sweep_interrupted_runs`] (which is left untouched): a journal read failure
/// yields an empty ledger and a warn, because a delivery guard that cannot read
/// the journal must fail *open* to the pre-#529 behaviour (deliver) rather than
/// silently suppress a report.
pub async fn delivered_by_unsettled_runs(
    events: &Arc<dyn EventLog>,
    company: &CompanyId,
    workflow_id: &str,
) -> Vec<DeliveredReport> {
    let stored = match events
        .read_from(company, EventSeq::new(0), usize::MAX)
        .await
    {
        Ok(stored) => stored,
        Err(err) => {
            tracing::warn!(
                %company,
                workflow = %workflow_id,
                %err,
                "could not read the journal to fold this workflow's stranded deliveries; a re-run \
                 will not know what a crashed run already sent"
            );
            return Vec::new();
        }
    };

    // Insertion order preserved, deduped by node — an output node has exactly one
    // destination, so its id is the identity and the first-seen kind describes
    // it. A `Vec` rather than a set keeps the returned order deterministic for
    // the caller's union.
    let mut delivered: Vec<DeliveredReport> = Vec::new();
    for stored in stored {
        match stored.event {
            // The dedupe (`not already present`) rides the guard: a repeat of a
            // node already in the ledger simply does not match, and falls through
            // to the no-op arm below — an `owner` fan-out's one-line-per-admin
            // collapses to a single entry this way.
            CompanyEvent::WorkflowReportDelivered {
                workflow_id: wid,
                node,
                kind,
                ..
            } if wid == workflow_id && !delivered.iter().any(|prior| prior.node == node) => {
                delivered.push(DeliveredReport { node, kind });
            }
            // A clean finish (no error) absorbs everything delivered up to it and
            // resets the ledger — the cadence guarantee. A failed / interrupted
            // finish carries an error and does NOT clear, so its run's stranded
            // deliveries stay owed-nothing to the next run.
            CompanyEvent::WorkflowRunFinished {
                workflow_id: wid,
                error,
                ..
            } if wid == workflow_id && error.is_none() => {
                delivered.clear();
            }
            _ => {}
        }
    }
    delivered
}

#[cfg(test)]
#[path = "workflow_outcome_tests_core.rs"]
mod tests_core;
#[cfg(test)]
#[path = "workflow_outcome_tests_part1.rs"]
mod tests_part1;
#[cfg(test)]
#[path = "workflow_outcome_tests_part2.rs"]
mod tests_part2;
