//! The workflow half of a card's output link (issue #339, epic #183 §6).
//!
//! # Why a queue and not a return value
//!
//! Exactly the [`PendingPublishQueue`](crate::harness::publish::PendingPublishQueue)
//! problem, with exactly the same answer. `run_workflow` and `create_workflow`
//! are built **once per agent**, while the card they are working varies **per
//! dispatch** — so neither tool can hold a task id, and neither can reach the
//! task store. They stage a [`TaskOutputWorkflow`] here; the brain drains it
//! inside the settle, where it already holds the card.
//!
//! # Correlation exists only for the orchestrator desk
//!
//! Both tools are orchestrator-only
//! ([`orchestrator_tools`](crate::harness::orchestrator::orchestrator_tools)),
//! so a card worked by any other teammate can never stage one. That is a real
//! limit of the correlation and it is named rather than papered over: workflow
//! links appear on orchestrator cards and nowhere else. Workflow *runs*
//! themselves have deliberately zero task correlation
//! ([`CompanyEvent::WorkflowRunFinished`](crate::ports::types::CompanyEvent)
//! carries no task id), which is precisely why the correlation has to be
//! captured here, at the moment the tool runs inside a dispatch, rather than
//! joined afterwards.
//!
//! Compiled only under `feature = "openhuman"`.

use std::sync::{Arc, Mutex};

use crate::ports::tasks::{TaskOutputAction, TaskOutputWorkflow};

/// The most workflow references one card's output stamp keeps.
///
/// A bound, not a design constraint. The stamp rides every board poll, so an
/// agent that ran forty workflows in one turn must not turn a card into a
/// payload. Far above any plausible real dispatch — and the collapse in
/// [`WorkflowRefQueue::drain`] already folds repeats of one workflow into a
/// single entry, so reaching this means forty *distinct* workflows.
pub const MAX_WORKFLOW_REFS: usize = 20;

/// A shared, in-memory queue of workflow references staged during a dispatch.
///
/// Cheap to [`Clone`] (a shared handle); the tools built into the agent and the
/// brain that drains it see the same queue because
/// [`HarnessDeps`](crate::harness::HarnessDeps) clones share this handle.
#[derive(Clone, Default)]
pub struct WorkflowRefQueue {
    inner: Arc<Mutex<Vec<TaskOutputWorkflow>>>,
}

impl WorkflowRefQueue {
    /// Stages a reference to a workflow this turn ran or authored.
    pub fn push(&self, reference: TaskOutputWorkflow) {
        self.inner
            .lock()
            .expect("workflow ref queue")
            .push(reference);
    }

    /// Empties the queue. Called before each turn so nothing a prior turn — an
    /// operator chat turn earlier in the same cycle, or an abandoned redirect
    /// re-run — staged can be attributed to this card.
    pub fn clear(&self) {
        self.inner.lock().expect("workflow ref queue").clear();
    }

    /// How many references are staged, before collapsing.
    pub fn queued(&self) -> usize {
        self.inner.lock().expect("workflow ref queue").len()
    }

    /// Drains every staged reference, emptying the queue and collapsing the raw
    /// list into what the card should carry.
    pub fn drain(&self) -> Vec<TaskOutputWorkflow> {
        let raw = std::mem::take(&mut *self.inner.lock().expect("workflow ref queue"));
        collapse(raw)
    }
}

/// Folds the raw staged list into one entry per workflow, newest-first-wins on
/// having actually run, capped at [`MAX_WORKFLOW_REFS`].
///
/// # Why collapse at all
///
/// The common shape is an agent that authors a graph and then runs it in the
/// same turn — two entries naming one workflow. The card shows *one* link, and
/// "created" is the weaker of the two answers: a run has a run id, so it can
/// open the overlay showing what actually executed. Keeping both would also
/// inflate the `+N more` count with a duplicate the operator cannot tell apart.
///
/// So per workflow id: the **last** entry that ran wins; failing that, the last
/// entry at all. Order otherwise follows first appearance, so the list reads in
/// the order the turn did things rather than in hash order.
fn collapse(raw: Vec<TaskOutputWorkflow>) -> Vec<TaskOutputWorkflow> {
    let mut out: Vec<TaskOutputWorkflow> = Vec::new();
    for reference in raw {
        match out
            .iter_mut()
            .find(|held| held.workflow_id == reference.workflow_id)
        {
            // A later *run* supersedes whatever we held; a later *create* does
            // not overwrite a run we already saw, because the run is the
            // stronger link.
            Some(held) if reference.action == TaskOutputAction::Ran => *held = reference,
            Some(_) => {}
            None => out.push(reference),
        }
    }
    out.truncate(MAX_WORKFLOW_REFS);
    out
}

#[cfg(test)]
#[path = "workflow_refs_tests.rs"]
mod tests;
