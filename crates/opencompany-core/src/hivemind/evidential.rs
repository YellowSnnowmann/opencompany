//! Citation discipline under `require_evidential`: does this `!support`
//! actually reach a stated fact?
//!
//! Under `require_evidential` the fold counts a `!support` only when its
//! citation chain reaches a [`TraceKind::Evidence`] — `quorum::standings` walks
//! `^N` transitively through the traces in the window and drops any support
//! that never lands on one. Two properties of that walk are easy for a live
//! model to miss, and both were missed in the same run:
//!
//! - **A `!propose` is not evidence.** `!support #euler12 ^2` citing the
//!   proposal itself is well-formed, reads as grounded, and counts for exactly
//!   nothing.
//! - **Topic ids are not compared.** The chain is followed by sequence, so
//!   citing an `!evidence` about a *different* topic still counts, and citing
//!   the right topic through the wrong marker does not.
//!
//! A support that counts for nothing is worse than a missing turn: the room
//! reads its own transcript as having three supporters where the fold sees one,
//! and it spends the rest of its budget waiting for a quorum that already
//! looked reached. So the driver checks the same thing the fold will, before
//! the line is journaled, and hands one correction back naming the sequence
//! numbers that would have worked.
//!
//! This is a *host-side pre-check*, never a second opinion: it re-implements
//! the library's walk over the transcript this turn could see, and its only
//! effect is one extra prompt. A line that survives the retry is journaled
//! as-is and the fold, not this module, decides what it is worth.

use std::collections::{BTreeMap, BTreeSet};

use tinyhivemind_hive::{
    Sequence, SessionAuthor, SessionMessage,
    trace::{Trace, TraceKind, resolve},
};

/// Read the traces a candidate line would deposit, as `agent_id` at `at`.
///
/// The sequence is a placeholder — nothing cites the line that has not been
/// journaled yet — but it has to be past everything visible, or a self-citation
/// in the chain walk below could resolve to a real message.
fn traces_of(line: &str, agent_id: &str, at: Sequence) -> Vec<Trace> {
    let author = SessionAuthor::Agent {
        id: agent_id.to_owned(),
        label: agent_id.to_owned(),
    };
    resolve(line, None, &author, at)
}

/// Every trace in the visible transcript, indexed by the sequence carrying it.
fn by_sequence(visible: &[SessionMessage]) -> BTreeMap<Sequence, Vec<Trace>> {
    let mut indexed: BTreeMap<Sequence, Vec<Trace>> = BTreeMap::new();
    for message in visible {
        for trace in resolve(&message.content, None, &message.author, message.sequence) {
            indexed.entry(message.sequence).or_default().push(trace);
        }
    }
    indexed
}

/// Whether `cites` reaches an `!evidence` line, following citations
/// transitively through the visible transcript.
///
/// A port of `tinyhivemind_hive::quorum`'s own private walk, and deliberately
/// the same shape: the visited set is over sequences, so two lines citing each
/// other terminate rather than recurring, and a citation that leaves the window
/// is not chased — a member's standing must not depend on how far back it
/// happened to have paged.
fn reaches_evidence(cites: &[Sequence], indexed: &BTreeMap<Sequence, Vec<Trace>>) -> bool {
    let mut visited: BTreeSet<Sequence> = BTreeSet::new();
    let mut pending: Vec<Sequence> = cites.to_vec();
    while let Some(sequence) = pending.pop() {
        if !visited.insert(sequence) {
            continue;
        }
        let Some(traces) = indexed.get(&sequence) else {
            continue;
        };
        for trace in traces {
            if trace.kind == TraceKind::Evidence {
                return true;
            }
            pending.extend(trace.cites.iter().copied());
        }
    }
    false
}

/// The sequences carrying an `!evidence` line, in transcript order.
#[must_use]
pub fn evidence_sequences(visible: &[SessionMessage]) -> Vec<u64> {
    let mut sequences: Vec<u64> = Vec::new();
    for message in visible {
        let carries_evidence = resolve(&message.content, None, &message.author, message.sequence)
            .iter()
            .any(|trace| trace.kind == TraceKind::Evidence);
        if carries_evidence && !sequences.contains(&message.sequence.0) {
            sequences.push(message.sequence.0);
        }
    }
    sequences
}

/// Whether `line` is a `!support` the fold will discard for want of evidence.
///
/// `false` for every line that is not a support, and for a support whose chain
/// already lands on a fact — the check exists to catch the one shape that
/// silently counts for nothing.
#[must_use]
pub fn support_misses_evidence(line: &str, agent_id: &str, visible: &[SessionMessage]) -> bool {
    let at = Sequence(
        visible
            .iter()
            .map(|message| message.sequence.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
    );
    let traces = traces_of(line, agent_id, at);
    let supports: Vec<&Trace> = traces
        .iter()
        .filter(|trace| trace.kind == TraceKind::Support)
        .collect();
    if supports.is_empty() {
        return false;
    }
    let indexed = by_sequence(visible);
    supports
        .iter()
        .all(|trace| !reaches_evidence(&trace.cites, &indexed))
}

/// The one-line correction a support that reaches no evidence gets.
///
/// It names the numbers that would have worked, because the failure is never
/// "you did not cite" — the line under correction usually cites the proposal —
/// it is "you cited the wrong kind of line", and a member cannot fix that from
/// a rule alone.
#[must_use]
pub fn correction(evidence: &[u64]) -> String {
    if evidence.is_empty() {
        return "Your `!support` reaches no `!evidence`, so this desk counts it for nothing: \
                citing a `!propose` is not grounds. No evidence is on the floor yet — deposit a \
                fact yourself with `!evidence #topic ^N`, or `!question` for the one you need. \
                Reply again with ONE line."
            .to_owned();
    }
    let cited = evidence
        .iter()
        .map(|sequence| format!("^{sequence}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Your `!support` reaches no `!evidence`, so this desk counts it for nothing: citing a \
         `!propose` is not grounds. Evidence on the floor: {cited}. Reply again with ONE line \
         citing one of those, or deposit your own fact with `!evidence #topic ^N`."
    )
}
