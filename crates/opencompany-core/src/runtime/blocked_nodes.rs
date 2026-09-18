//! One blocked agent node, one continuation (issue #899, Stage 1).
//!
//! # What this stashes, and why the [`ContinuationQueue`] cannot
//!
//! A policy-gated call inside an agent node's own tool loop parks a
//! tool-call-shaped effect (`agent: Some`). Approving it mints a grant, but
//! nothing re-dispatches the workflow run — the hole issue #899 closes. The fix
//! reuses the [`ContinuationQueue`](crate::runtime::continuation::ContinuationQueue)
//! to count a node's parked calls as one batch (armed at park time under a
//! [`workflow_node_turn_key`](crate::runtime::workflow_resume::workflow_node_turn_key)),
//! and releases once when the last decision lands. But to *spawn* the
//! continuation the host needs two facts that batch cannot carry — the
//! **workflow id** and the paused run's **trigger input** — for the same reason
//! [`WorkflowGateQueue`](crate::runtime::workflow_gates::WorkflowGateQueue)
//! exists beside that queue for gates: the released batch is only
//! `ApprovalResolved` events, and the parked tool-call effect carries no
//! workflow lineage of its own (it is minted by `ApprovalPolicy::effect_for`,
//! which knows nothing of the run).
//!
//! # Armed at node park time, mirrored durably at block-settle (issue #1825, P1)
//!
//! The in-memory stash is armed by `HarnessAgentRunner::park_gated_calls`
//! itself, before that call parks a single tool-call effect for the node's
//! turn — `HarnessAgentRunner` is built with the run's trigger input for
//! exactly this (see [`RunContext::trigger_input`](crate::workflows::caps::RunContext::trigger_input)),
//! so nothing stops arming this queue the moment a node's calls are about to
//! become clickable. Before this, the arm ran only in the runner's
//! block-settle pass — after the agent had returned, the engine had settled,
//! and (on the halt path) the run's output had already been persisted — which
//! left a window where an operator could approve a card that was durably
//! journaled and clickable, but had nothing armed here yet: `continue_turn`
//! consumed the decision against an empty stash, retired the turn without
//! spawning, and the block-settle pass then armed a stash no decision would
//! ever come back for. Arming at park time closes that window instead of
//! narrowing it. [`arm`](BlockedNodeQueue::arm) is first-write-wins, so the
//! block-settle pass's own call is now a harmless no-op for every node this
//! queue already holds a stash for — it stays only because it is also where
//! the durable journal mirror below is written, and that write still needs
//! the full blocked-node list the engine hands back on settle.
//!
//! # Durability, stated plainly (issue #1816, Stage 2)
//!
//! In-memory as the fast path, and — like [`WorkflowGateQueue`] — **rehydrated
//! from the journal at recovery**. The parked tool-call effect still carries no
//! workflow id or trigger input, so those two facts cannot be rebuilt from the
//! effect payload; instead they are written to the journal at park time as a
//! dedicated, host-durable stash record
//! ([`RuntimeJournal::record_blocked_node_stashed`](crate::runtime::journal::RuntimeJournal::record_blocked_node_stashed)),
//! keyed by the same per-(run, node) turn key, and dropped by a paired release
//! record once the run is re-dispatched. At boot the builder re-arms this queue
//! from [`RuntimeJournal::blocked_stashes`](crate::runtime::journal::RuntimeJournal::blocked_stashes)
//! via [`rearm`](BlockedNodeQueue::rearm), exactly as the gate queue re-arms
//! from its still-parked gates.
//!
//! A restart between *park* and *approve* therefore comes back with the
//! [`ContinuationQueue`] counter re-armed (from `parked_turns`) **and** this
//! stash re-armed (from `blocked_stashes`): the batch releases, [`release`] hits
//! the rehydrated stash, and the run re-dispatches instead of stranding the
//! operator on "re-run the workflow". Stage 1 (issue #899) shipped the in-memory
//! fast path; Stage 2 (issue #1816) adds the durable record beneath it so an
//! approval redeems a parked run even across a process/host replacement — the
//! `~90-min` staging cron pod-roll that stranded parked tasks permanently.
//!
//! [`release`]: BlockedNodeQueue::release

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::ports::types::StartedBy;

/// One rehydratable blocked-node stash, as `(turn, workflow_id, input,
/// started_by, thread_id, workflow_fingerprint)` — the shape
/// [`RuntimeJournal::blocked_stashes`](crate::runtime::journal::RuntimeJournal::blocked_stashes)
/// hands to [`BlockedNodeQueue::rearm_checkpointed`]. Named so both sides of
/// that handoff share one type instead of two independently-maintained
/// six-tuples.
pub type BlockedStashRow = (
    String,
    String,
    Value,
    StartedBy,
    Option<String>,
    Option<String>,
);

/// The facts a blocked agent node's continuation needs, stashed at
/// block-settle and handed back on release.
#[derive(Clone, Debug)]
pub struct StashedBlock {
    /// The workflow whose run blocked, to load the graph for the re-run.
    pub workflow_id: String,
    /// The paused run's own trigger input, replayed unchanged — the minted grant
    /// is what lets the identical gated call pass on the re-run.
    pub input: Value,
    /// The blocked run's own attribution (issue #1862 prerequisite), carried
    /// into the continuation so approving the block does not silently reset it
    /// to `Operator` — see `spawn_blocked_node_continuation`.
    pub started_by: StartedBy,
    /// The tinyflows checkpoint lineage this block resumes, when available.
    pub thread_id: Option<String>,
    /// The graph's [`content_fingerprint`](crate::company::WorkflowFile::content_fingerprint)
    /// at the moment this node parked, when one was stashed — the blocked-node
    /// counterpart to [`PAYLOAD_WORKFLOW_FINGERPRINT`](crate::runtime::workflow_resume::PAYLOAD_WORKFLOW_FINGERPRINT)
    /// on a gate's parked effect. `spawn_blocked_node_continuation` refuses
    /// `thread_id`'s checkpoint when this no longer matches the graph it is
    /// about to resume against, on the same terms `graph_unchanged_since_park`
    /// already applies to the gate path.
    pub workflow_fingerprint: Option<String>,
    /// Whether any of this node's parked calls has been approved, banked the
    /// moment that decision lands rather than read off the release batch
    /// (issue #1816).
    ///
    /// [`ContinuationQueue`](crate::runtime::continuation::ContinuationQueue)'s
    /// released batch only carries the verdicts one process held in memory —
    /// a restart between two decisions on the same node drops the earlier ones,
    /// and unlike a workflow gate (which re-parks whatever its own batch
    /// forgets), a blocked node has no re-park: it either spawns the
    /// continuation once or not at all. So the fact an approval landed is kept
    /// here too, set by [`mark_approved`](BlockedNodeQueue::mark_approved) at
    /// decide time and rehydrated at boot by the same restart the batch cannot
    /// survive, and read alongside — not instead of — the batch's own verdicts.
    pub approved: bool,
}

/// Per-(run, node) continuation state for a blocked agent node: the workflow id
/// and trigger input its re-run needs (issue #899, Stage 1).
///
/// Cheap to [`Clone`] — a shared handle like every other queue in the runtime —
/// so the arming side (the workflow runner, through `DeliveryParking`) and the
/// releasing side (the runtime's `continue_turn`) see one set of stashes.
#[derive(Clone, Default)]
pub struct BlockedNodeQueue {
    inner: Arc<Mutex<HashMap<String, StashedBlock>>>,
}

impl BlockedNodeQueue {
    /// Stashes the facts a blocked node's continuation needs, keyed by its
    /// per-(run, node) turn key.
    ///
    /// **First write wins.** Every gated call one node parked shares one turn
    /// key and one trigger input, so a second arm for the same key would carry
    /// identical facts; keeping the first is simplest and cannot disagree.
    pub fn arm(&self, turn: &str, workflow_id: &str, input: &Value, started_by: &StartedBy) {
        self.arm_checkpointed(turn, workflow_id, input, started_by, None, None);
    }

    pub fn arm_checkpointed(
        &self,
        turn: &str,
        workflow_id: &str,
        input: &Value,
        started_by: &StartedBy,
        thread_id: Option<&str>,
        workflow_fingerprint: Option<&str>,
    ) {
        self.inner
            .lock()
            .expect("blocked node queue poisoned")
            .entry(turn.to_string())
            .or_insert_with(|| StashedBlock {
                workflow_id: workflow_id.to_string(),
                input: input.clone(),
                started_by: started_by.clone(),
                thread_id: thread_id.map(str::to_string),
                workflow_fingerprint: workflow_fingerprint.map(str::to_string),
                approved: false,
            });
    }

    /// Banks that at least one of `turn`'s parked calls has been approved,
    /// called the moment that decision lands (issue #1816) rather than only
    /// read off whichever decision happens to release the turn.
    ///
    /// A no-op if `turn` has no stash: nothing to bank the fact against, and
    /// [`resume_blocked_agent_node`](crate::company::runtime::CompanyRuntime::resume_blocked_agent_node)
    /// will find nothing to redeem either. In ordinary operation this never
    /// happens — a decision cannot land before its node's park armed the
    /// stash — so this guards a call ordering the runtime does not exercise
    /// rather than a real case.
    pub fn mark_approved(&self, turn: &str) {
        if let Some(block) = self
            .inner
            .lock()
            .expect("blocked node queue poisoned")
            .get_mut(turn)
        {
            block.approved = true;
        }
    }

    /// Reads `turn`'s stash without removing it (issue #1816, Stage 4).
    ///
    /// The non-destructive counterpart to [`release`](Self::release): a caller
    /// that must inspect the stash *before* committing to retire it — because
    /// what comes next can still fail, or can outlive the process — uses this
    /// instead, and calls `release` only once the outcome the retirement
    /// records is actually final. See
    /// [`resume_blocked_agent_node`](crate::company::runtime::CompanyRuntime::resume_blocked_agent_node)
    /// for the caller this exists for.
    pub fn peek(&self, turn: &str) -> Option<StashedBlock> {
        self.inner
            .lock()
            .expect("blocked node queue poisoned")
            .get(turn)
            .cloned()
    }

    /// Takes `turn`'s stash, dropping it from the queue.
    ///
    /// Called once, by whichever caller the [`ContinuationQueue`] handed the
    /// release to — that queue's counting decides who, under one lock, so this
    /// cannot be entered twice for one blocked node. `None` for a turn this
    /// queue is not holding: a card parked before this issue, or a stash lost to
    /// a restart **and** never durably recorded (since #1816 a restart rehydrates
    /// it from the journal via [`rearm`](Self::rearm), so `Some` is the normal
    /// post-restart case); the caller reports the remaining `None` as "re-run the
    /// workflow".
    pub fn release(&self, turn: &str) -> Option<StashedBlock> {
        self.inner
            .lock()
            .expect("blocked node queue poisoned")
            .remove(turn)
    }

    /// Rehydrates the queue at boot from the journal's still-live stash records
    /// (issue #1816, Stage 2).
    ///
    /// The blocked-node counterpart to
    /// [`WorkflowGateQueue::rearm`](crate::runtime::workflow_gates::WorkflowGateQueue::rearm):
    /// the builder folds the durable
    /// [`blocked_stashes`](crate::runtime::journal::RuntimeJournal::blocked_stashes)
    /// left by a park that outlived its process and re-arms one stash per still-
    /// undelivered `(turn, workflow_id, input, started_by)`. **First write
    /// wins**, on [`arm`](Self::arm)'s terms, so a live stash inherited on a
    /// rebuild is never clobbered by a journal replay of the same turn.
    ///
    /// `started_by` comes straight from the durable
    /// [`BlockedNodeStashed`](crate::runtime::journal::JournalRecord::BlockedNodeStashed)
    /// record now (issue #1862 prerequisite) — the journal itself is what
    /// degrades a stash written before that record carried the field to
    /// `Operator` (its `#[serde(default)]`), so this call site just passes the
    /// fact through rather than re-deciding the fallback.
    pub fn rearm(&self, stashes: impl IntoIterator<Item = (String, String, Value, StartedBy)>) {
        self.rearm_checkpointed(stashes.into_iter().map(
            |(turn, workflow_id, input, started_by)| {
                (turn, workflow_id, input, started_by, None, None)
            },
        ));
    }

    pub fn rearm_checkpointed(&self, stashes: impl IntoIterator<Item = BlockedStashRow>) {
        let mut inner = self.inner.lock().expect("blocked node queue poisoned");
        for (turn, workflow_id, input, started_by, thread_id, workflow_fingerprint) in stashes {
            inner.entry(turn).or_insert(StashedBlock {
                workflow_id,
                input,
                started_by,
                thread_id,
                workflow_fingerprint,
                approved: false,
            });
        }
    }

    /// Whether `turn` is a blocked node this queue is holding a stash for.
    pub fn is_armed(&self, turn: &str) -> bool {
        self.inner
            .lock()
            .expect("blocked node queue poisoned")
            .contains_key(turn)
    }

    /// How many blocked nodes are stashed. For tests and diagnostics.
    pub fn waiting(&self) -> usize {
        self.inner
            .lock()
            .expect("blocked node queue poisoned")
            .len()
    }

    /// Every turn this queue holds a stash for whose `approved` flag is set
    /// (issue #1816, Stage 3).
    ///
    /// Read once, at boot, to find a stash a restart may have stranded:
    /// crossed against the journal's own live [`parked_turns`] (a turn with
    /// nothing left parked there has had every decision it was blocked on
    /// durably resolved), a turn that is both `approved` here and absent from
    /// that set is exactly one whose last decision landed before the crash
    /// that took [`resume_blocked_agent_node`] down with it — the release
    /// recorded nothing wrong, it simply never ran. See
    /// [`CompanyRuntime::reconcile_stranded_blocked_nodes`] for the caller.
    ///
    /// [`parked_turns`]: crate::runtime::journal::RuntimeJournal::parked_turns
    /// [`resume_blocked_agent_node`]: crate::company::runtime::CompanyRuntime::resume_blocked_agent_node
    /// [`CompanyRuntime::reconcile_stranded_blocked_nodes`]: crate::company::runtime::CompanyRuntime::reconcile_stranded_blocked_nodes
    pub fn approved_turns(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("blocked node queue poisoned")
            .iter()
            .filter(|(_, block)| block.approved)
            .map(|(turn, _)| turn.clone())
            .collect()
    }

    /// Every turn this queue holds a stash for, approved or not (issue #1825,
    /// P2 follow-up).
    ///
    /// [`approved_turns`](Self::approved_turns) alone cannot see a stash whose
    /// every decision landed as a denial or an expiry — one never marked
    /// `approved` — so a boot that scanned only that list left such a stash
    /// rehydrated by [`rearm`](Self::rearm) on every restart and never
    /// retired: the same fact recorded live, up front, by
    /// `resume_blocked_agent_node`'s own all-denied branch is exactly what a
    /// crash between that resolution and the retirement it drives (or a
    /// retirement whose durable write itself fails) loses. Read this list
    /// instead, so [`CompanyRuntime::reconcile_stranded_blocked_nodes`] can
    /// tell the two resolved-but-unretired shapes apart — approved (redeem
    /// it) and unapproved (nothing to redeem, just retire it) — rather than
    /// only ever seeing the first.
    ///
    /// [`CompanyRuntime::reconcile_stranded_blocked_nodes`]: crate::company::runtime::CompanyRuntime::reconcile_stranded_blocked_nodes
    pub fn stashed_turns(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("blocked node queue poisoned")
            .keys()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
#[path = "blocked_nodes_tests.rs"]
mod tests;
