//! Orchestrator-owned lifecycle for a dispatched board task (issue #186).
//!
//! Before this module, `run_task` decided a dispatched card's fate inline: the
//! landing column was a bare `"in_review"` / `"todo"` / `"paused"` string
//! literal written at each of five break points in the steer loop, and the
//! completion bubble was attributed to whichever agent happened to run the
//! turn. Two problems with that:
//!
//! * **No authority.** The intended model is "the orchestrator maintains
//!   assignment, review, and done" — but the transitions were mechanical, with
//!   no single place that owns them and nothing for a policy to hook.
//! * **The wrong voice.** The assignee posted its own result straight back to
//!   the operator, which breaks the single-accountable-voice model the
//!   delegation path already follows (`HarnessBrain::run_delegation`): the
//!   orchestrator is the operator's one point of contact, and a desk member
//!   answering directly bypasses it.
//!
//! This module is the seam. It holds no state and performs no I/O — it is the
//! pure *decision* layer (`TaskRunEnd` → landing column, and the relay bubble's
//! shape), so it is unit-testable without a harness pool, a task store, or a
//! live agent. `run_task` keeps the I/O and calls in here for every choice.
//!
//! # Relationship to neighbouring issues
//!
//! * **#171 (`in_review` → `done`, PR #179) — landed here, then narrowed by
//!   #337.** #179 shipped a `success_terminal_column` helper that sent a
//!   *delegated* card (one carrying an `origin_chat_id`) straight to
//!   [`COLUMN_DONE`], on the argument that its answer was relayed into the
//!   conversation it came from and nobody was watching the board for it.
//!
//!   The operator decision of 2026-08-05 removed that route: **[`COLUMN_DONE`]
//!   is reached only by a person**, through an approving orchestrator verdict
//!   ([`review_landing_column`]). Every card — delegated or board-created —
//!   now stops in [`COLUMN_IN_REVIEW`] first. So there is still exactly one
//!   terminal, which is what #171 was about; there is now exactly one route to
//!   it, and it runs through a human.
//! * **#337 (a settled run advances its card) — where the column table went.**
//!   [`landing_column`] is no longer its own mapping. It adapts a
//!   [`TaskRunEnd`] onto [`crate::ports::tasks::column_for_settled_run`], which
//!   is keyed on the settled [`RunStatus`] and lives on the **port** because
//!   two of its three callers (the cycle's terminality backstop and the boot
//!   reaper's card sweep) are ungated and cannot see this module.
//! * **#185 (per-task event correlation, PR #190, still open).** #190 journals
//!   `CompanyEvent::DeskTaskCompleted { column, .. }` from the tail of
//!   `run_task`. That `column` is exactly what [`landing_column`] decides, so
//!   once it lands the event reports this module's decision rather than a
//!   re-derived literal. **This issue does not emit that event** — doing so
//!   would double-journal the timeline's terminal anchor.

use crate::ports::TaskRecord;
use crate::ports::runs::RunStatus;
use crate::ports::types::{OutboundMessage, ReplyTo};

/// The board's column vocabulary, re-exported so every `lifecycle::COLUMN_*`
/// path here is unchanged.
///
/// The definitions moved to the task **port** in issue #205: this module is
/// `#[cfg(feature = "openhuman")]`, so the REST write boundary — which now
/// validates a card's column — could not see them. `COLUMN_IN_REVIEW` is where
/// a card awaits the orchestrator, `COLUMN_IN_PROGRESS` where a card is being
/// worked — where a dispatched card already is, and where one whose turn
/// **handed the work off** stays while the delegate runs (issue #204) —
/// `COLUMN_TODO` where a stopped or failed run returns it (issue #301 — it used
/// to return to a separate `backlog` pool, which epic #183 §3 collapsed into
/// To-do), `COLUMN_PAUSED` where a paused run parks it (resume is a plain
/// `column → in_progress` PATCH, which re-triggers dispatch), `COLUMN_PLANNING`
/// which nothing here writes yet (§4's planning pass owns it), and `COLUMN_DONE`
/// the terminal — since issue #337 reached **only** through
/// [`review_landing_column`], because Done is a person's decision and no run
/// ending writes it.
pub use crate::ports::tasks::{
    COLUMN_DONE, COLUMN_IN_PROGRESS, COLUMN_IN_REVIEW, COLUMN_PAUSED, COLUMN_PLANNING, COLUMN_TODO,
};

/// The note attribution used for an operator-initiated stop, as opposed to a
/// result the assignee produced.
pub const OPERATOR_ATTRIBUTION: &str = "operator";

/// The note attribution `run_task`'s redirect loop uses for the operator's own
/// mid-flight steer instruction (issue #1949 review, CodeRabbit
/// 3895599021) — a distinct label from [`OPERATOR_ATTRIBUTION`], not the
/// word "operator" alone, so it needs its own entry in `relay_text`'s known
/// set rather than piggybacking on the cancel-attribution constant.
pub const OPERATOR_REDIRECT_ATTRIBUTION: &str = "operator redirect";

/// The note attribution for an operator's review feedback on a settled
/// `in_review` card, relayed back into its origin thread. A distinct label so
/// [`relay_text`] strips it as board chrome the same way it strips the
/// operator's mid-flight redirect, rather than leaking a `[reviewer]` prefix
/// into the relayed bubble.
pub const REVIEWER_ATTRIBUTION: &str = "reviewer";

/// How one dispatch run ended, independent of who ran it or what it said.
///
/// This is the whole input to the lifecycle decision. Keeping it separate from
/// the result *text* is what lets the column choice be tested without running
/// an agent, and what stops a sixth call site inventing a sixth column literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskRunEnd {
    /// The assignee finished its turn and produced a result.
    Completed,
    /// The assignee's turn **handed the work off** to another agent rather than
    /// doing it (issue #204). This is not an ending: the card is reassigned to
    /// the delegate and stays in [`COLUMN_IN_PROGRESS`] while they run, and the
    /// delegate's own ending — a [`Completed`](Self::Completed) with their
    /// output, or a [`Cancelled`](Self::Cancelled) — settles it afterwards.
    ///
    /// Without this, "I finished the work" and "I handed it off" were the same
    /// `Completed`, so a delegating dispatch landed in `in_review` under the
    /// delegator with the delegate never having run.
    Delegated,
    /// The turn itself errored (`dispatch failed: …`).
    Failed,
    /// An operator cancelled mid-flight. Partial work is discarded.
    Cancelled,
    /// An operator paused mid-flight, **or** the turn itself paused for lack
    /// of inference budget/credits (issue #1846 review, Codex #3864988168) —
    /// a background dispatch's mirror of the operator-chat path's own
    /// budget-paused bubble. Partial work is preserved in the note either
    /// way; a budget pause additionally parks a durable per-agent re-issue
    /// marker (`crate::runtime::grants::budget_pauses_for`), redeemable from
    /// the console once credits are added.
    Paused,
    /// The operator spent the redirect budget
    /// (`MAX_REDIRECTS_PER_DISPATCH`); the last run's reply is finalized
    /// rather than looping forever.
    RedirectsExhausted,
    /// The turn stopped on something **a person can answer** and parked a
    /// durable blocker instead of settling (issue #1861): a rejected model id,
    /// an expired credential, a missing prerequisite, or the assignee's own
    /// `escalate_to_human`.
    ///
    /// Separate from [`Failed`](Self::Failed) because the two need opposite
    /// treatment. A failure is over — the card returns to To-do and the reason
    /// is history. A blocker is an open question: the card parks, somebody is
    /// asked, and the answer resumes the step. Reaching this arm means the
    /// classifier decided the stop was answerable; everything it does not
    /// recognise keeps settling `Failed`.
    Blocked,
}

/// The orchestrator's verdict on a card sitting in `in_review` (issue #186
/// part b).
///
/// Deliberately only two outcomes — see [`review_landing_column`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDecision {
    /// The work is accepted, which finishes the card: this is #171's
    /// done-transition for a board-created card.
    Approve,
    /// The work needs another pass. The card returns to `todo` so it can be
    /// re-dispatched, carrying the reviewer's reason in its note (issue #301 /
    /// epic #183 §3: a card that cannot proceed goes back to To-do with the
    /// reason on it, never into a stuck state of its own).
    Revise,
}

impl ReviewDecision {
    /// Parses the `decision` argument of the `review_task` tool. Accepts the
    /// obvious synonyms an LLM reaches for, because a rejected tool call costs
    /// the orchestrator a whole turn.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "approve" | "approved" | "accept" | "accepted" | "ok" => Some(Self::Approve),
            "revise" | "reject" | "rejected" | "rework" | "changes" => Some(Self::Revise),
            _ => None,
        }
    }
}

/// The board column a reviewed card lands in.
///
/// **`Approve` writes [`COLUMN_DONE`].** This is the `in_review → done`
/// transition issue #171 asked for, in the place #186 built for it: the
/// verdict *is* the review the card was parked waiting for, so approving it
/// finishes it. `Revise` sends the card back to be re-dispatched.
///
/// Since issue #337 this is the **only** write of [`COLUMN_DONE`] anywhere —
/// #179's origin-based shortcut is gone and no run ending lands there — so
/// "Done means a person accepted it" is now true by construction rather than
/// by convention.
pub fn review_landing_column(decision: ReviewDecision) -> &'static str {
    match decision {
        ReviewDecision::Approve => COLUMN_DONE,
        ReviewDecision::Revise => COLUMN_TODO,
    }
}

/// The note block a review records on the card, in the orchestrator's voice.
pub fn review_note(decision: ReviewDecision, note: Option<&str>) -> String {
    let verdict = match decision {
        ReviewDecision::Approve => "reviewed: approved",
        ReviewDecision::Revise => "reviewed: needs another pass",
    };
    match note.map(str::trim).filter(|n| !n.is_empty()) {
        Some(note) => format!("{verdict} — {note}"),
        None => verdict.to_string(),
    }
}

/// The board column a run ending this way lands its card in.
///
/// **Issue #337 collapsed this onto [`column_for_settled_run`].** It used to be
/// its own table over [`TaskRunEnd`], which meant the landing was derived from
/// *how the turn ended* while the attempt row was derived from *what the run
/// settled into* — two tables that had already drifted apart. The visible
/// symptom: a run that finished its work but parked an approval settled
/// [`RunStatus::WaitingApproval`] and still landed in whatever its ending said,
/// which for a delegated card was `done`. A card filed as finished while a
/// person still had to authorise the call it was waiting on.
///
/// Now there is one table, keyed on the settled status, and this function is
/// the thin adapter that reaches it: `end` → [`settled_run_status`] →
/// [`column_for_settled_run`].
///
/// # This overload assumes nothing was parked
///
/// `landing_column(end)` is [`settled_landing_column`]`(end, 0)`. It is the
/// right call only where no approval can be outstanding, or where a later
/// authoritative write will land the card again with the count in hand. **A
/// settle that can park an approval must call [`settled_landing_column`]** —
/// issue #465: reading the landing from the ending alone is what let a turn
/// whose first call parked settle as a plain success and present as reviewable
/// work it had never started.
///
/// # Two deliberate losses, both wanted
///
/// * **`Succeeded` lands in `in_review`, never `done`.** Operator decision,
///   2026-08-05: Done is reached only by a person. This supersedes the
///   `Succeeded → Done` row in epic #183 §4.
/// * **`success_terminal_column` is gone with it.** It sent a card carrying an
///   `origin_chat_id` — one spawned during an agent-to-agent handoff — straight
///   to `done` on the argument that nobody was watching the board for it (#171
///   / PR #179). Under the decision above there is no automatic route to `done`
///   for *any* card, so that shortcut cannot survive; a delegated card now stops
///   in `in_review` like every other. Nothing is lost from the handoff itself —
///   the delegate's answer is still relayed straight into the originating thread
///   by [`relay_reply`], which is what that conversation was actually waiting
///   on. What changes is that the card stays visible until a person accepts it,
///   which is the point of the review stop.
///
/// [`column_for_settled_run`]: crate::ports::tasks::column_for_settled_run
pub fn landing_column(end: TaskRunEnd) -> &'static str {
    settled_landing_column(end, 0)
}

/// The board column a run lands its card in, given how it ended **and** whether
/// it left an approval parked (issue #465).
///
/// This is [`landing_column`] with the parked-approval overlay applied — the
/// same overlay [`settled_run_status`] applies to the attempt row, reached
/// through the same one table. `landing_column(end)` is exactly
/// `settled_landing_column(end, 0)`.
///
/// # Why the count has to reach the column
///
/// It already reached the *status*: a success that parked something settles
/// [`RunStatus::WaitingApproval`]. The column was derived from
/// [`run_status_for`] instead, which cannot produce that status, so the board
/// was answering from an ending that had been overtaken — and a turn whose very
/// first call parked, having produced nothing, landed in
/// [`COLUMN_IN_REVIEW`] announcing work to check.
///
/// That was reachable two ways, and this closes both:
///
/// * `run_task`'s settle (`brain.rs`) *did* compute the parked count, but the
///   break-point [`landing_column`] call ran first and the authoritative
///   overwrite went to [`column_for_settled_run`] direct — correct only once
///   that table stopped sending `WaitingApproval` to review.
/// * the delegation seam's `settle_work_card` (issue #442's card, opened by
///   construction for work handed to an agent) hardcoded
///   [`Completed`](TaskRunEnd::Completed) and never consulted the count at all.
///   That is the path in the report: a desk asked directly, its first call
///   parked, and the card settled as a plain success.
///
/// Routing both through here keeps the landing one decision — the property
/// issue #337 collapsed this module onto the port to get — rather than two
/// call sites each remembering to apply an overlay.
///
/// [`column_for_settled_run`]: crate::ports::tasks::column_for_settled_run
pub fn settled_landing_column(end: TaskRunEnd, parked_approvals: usize) -> &'static str {
    // A hand-off is explicitly NOT a settle: the delegate is running, so the
    // card keeps the column it was dispatched in and only lands once they come
    // back with something (issue #204). Handled here rather than in the mapping
    // because `run_status_for(Delegated)` is `Paused` — right for the attempt
    // row (it is waiting on something other than a person) and wrong for the
    // board (the work has changed hands, not stopped).
    //
    // It outranks the parked overlay too: a delegator that parked an approval
    // still handed the work on, and the delegate is running it now.
    if matches!(end, TaskRunEnd::Delegated) {
        return COLUMN_IN_PROGRESS;
    }
    // Every other ending settles, so the mapping always answers. The fallback
    // is unreachable and exists only so this stays total without an `expect`.
    crate::ports::tasks::column_for_settled_run(settled_run_status(end, parked_approvals))
        .unwrap_or(COLUMN_IN_PROGRESS)
}

/// The [`RunStatus`] a run ending this way settles into (issue #242).
///
/// The board column and the run status answer two different questions about the
/// same ending — *where does the card go* and *how did the attempt end* — so
/// both are decided here, from the one [`TaskRunEnd`], rather than one of them
/// being re-derived from the other. Deriving the status from the landing column
/// would be actively wrong: `Failed` and `Cancelled` both land in
/// [`COLUMN_TODO`] and are emphatically not the same outcome.
///
/// The mapping, and why:
///
/// * [`Completed`](TaskRunEnd::Completed) and
///   [`RedirectsExhausted`](TaskRunEnd::RedirectsExhausted) →
///   [`Succeeded`](RunStatus::Succeeded). Both produced a result and both land
///   in the card's success terminal; spending the redirect budget is how the
///   run *ended*, not evidence that it failed.
/// * [`Failed`](TaskRunEnd::Failed) → [`Failed`](RunStatus::Failed), carrying
///   the reason.
/// * [`Cancelled`](TaskRunEnd::Cancelled) →
///   [`Cancelled`](RunStatus::Cancelled) — an operator stopped it, which is
///   neither a success nor a defect.
/// * [`Paused`](TaskRunEnd::Paused) → [`Paused`](RunStatus::Paused). Epic #183
///   decision 2: an operator pause is resolved by *resuming*, not by a person
///   approving something, so it is `Paused` and never `WaitingApproval`.
/// * [`Blocked`](TaskRunEnd::Blocked) → [`Blocked`](RunStatus::Blocked). Epic
///   #183 decision 2 again, and the case it did not have a status for: the
///   attempt is waiting on *a person*, but on an answer rather than on an
///   approval. It parks rather than settling, so the card lands in
///   [`COLUMN_PAUSED`] with the question on it.
/// * [`Delegated`](TaskRunEnd::Delegated) → [`Paused`](RunStatus::Paused). A
///   hand-off is not an ending at all — the card stays in
///   [`COLUMN_IN_PROGRESS`] — and it is unreachable as a run settle today,
///   because `run_task` awaits the delegate inside the same attempt. Named
///   anyway so this mapping is exhaustive: if a future path ever settles a run
///   on a hand-off, the attempt is waiting on *something other than a person*
///   (the delegate), which decision 2 makes `Paused` — non-terminal and
///   resumable — rather than a terminal status that would file unfinished work
///   as done.
pub fn run_status_for(end: TaskRunEnd) -> RunStatus {
    match end {
        TaskRunEnd::Completed | TaskRunEnd::RedirectsExhausted => RunStatus::Succeeded,
        TaskRunEnd::Failed => RunStatus::Failed,
        TaskRunEnd::Cancelled => RunStatus::Cancelled,
        TaskRunEnd::Paused | TaskRunEnd::Delegated => RunStatus::Paused,
        TaskRunEnd::Blocked => RunStatus::Blocked,
    }
}

/// The [`RunStatus`] an attempt settles into, given how it ended **and**
/// whether it left a person something to act on (issue #242).
///
/// `parked_approvals` counts the approval requests this attempt's own turns
/// parked. A run that otherwise succeeded while parking at least one finishes
/// [`RunStatus::WaitingApproval`] rather than [`RunStatus::Succeeded`] — epic
/// #183 decision 2 in its purest form: *who* unblocks the work decides where it
/// parks, and here a person does.
///
/// A run that failed, was cancelled or was paused keeps the status its ending
/// gave it. The operator has a bigger problem than a pending approval, and
/// relabelling a failure as "waiting on you" would hide the reason it stopped.
///
/// [`RunStatus::WaitingApproval`] is terminal-in-v1 (resuming an approved
/// attempt is its own issue) and deliberately **re-enterable across attempts**:
/// the re-dispatch that follows an approval is a *new* run which can wait again.
/// That is what keeps #243's single-use, argument-exact grants coherent instead
/// of forcing an operator to batch several approvals into one.
pub fn settled_run_status(end: TaskRunEnd, parked_approvals: usize) -> RunStatus {
    match run_status_for(end) {
        RunStatus::Succeeded if parked_approvals > 0 => RunStatus::WaitingApproval,
        settled => settled,
    }
}

/// [`settled_run_status`] with issue #1861's blocker overlay: a turn that
/// raised a question the operator has to answer settles
/// [`Blocked`](RunStatus::Blocked).
///
/// `blockers` counts the blocker parks **this attempt's own turns** queued —
/// an `escalate_to_human` call, or a host-classified failure inside a
/// delegated turn.
///
/// # Why it outranks `WaitingApproval` but not a failure
///
/// Both park the card, so the board reads the same either way; the difference
/// is what the run history says the operator owes. An approval is a decision
/// about an effect that is ready to happen. A blocker is a question with
/// nothing behind it yet. Reporting the second as the first sends somebody to
/// the Approvals page looking for something to approve.
///
/// A run that **failed or was cancelled** keeps its own status, exactly as the
/// approval overlay leaves it: the operator has a bigger problem than an
/// unanswered question, and relabelling the failure would hide why the work
/// actually stopped.
///
/// # Why the ending is not rewritten instead
///
/// [`TaskRunEnd`] stays whatever the turn did. The success-terminal check that
/// records a run's artifacts and outputs reads the *ending*, not this status —
/// so an agent that wrote a spec and then asked a question keeps the spec. It
/// is the same separation the approval overlay draws, for the same reason.
pub fn settled_run_status_with_blockers(
    end: TaskRunEnd,
    parked_approvals: usize,
    blockers: usize,
) -> RunStatus {
    match settled_run_status(end, parked_approvals) {
        RunStatus::Succeeded | RunStatus::WaitingApproval if blockers > 0 => RunStatus::Blocked,
        settled => settled,
    }
}

/// Who the note block for this ending is attributed to.
///
/// A cancellation is the operator's act, not the assignee's, so it is recorded
/// as theirs — the assignee never said "cancelled while in flight". Every other
/// ending carries the assignee's own words (or the dispatch error raised while
/// running on their behalf).
pub fn note_attribution(end: TaskRunEnd, responder: &str) -> String {
    match end {
        TaskRunEnd::Cancelled => OPERATOR_ATTRIBUTION.to_string(),
        _ => responder.to_string(),
    }
}

/// The operator-facing sentence for a finished card: what happened to it, who
/// did the work, and the accumulated note.
///
/// The `responder` is named only when it is someone other than the relaying
/// orchestrator. That is the one-voice rule: the orchestrator always speaks,
/// and it credits the doer when the doer is somebody else. A card the
/// orchestrator ran itself would otherwise read "… (ceo ran it)" in a bubble
/// already attributed to `ceo`.
///
/// `prior_responders` names every other id this dispatch's own note blocks
/// may already carry as attribution — chiefly, the pre-hand-off responder a
/// mid-flight reassignment (issue #204, `run_task`'s delegate loop) leaves
/// behind once `responder` itself has moved on to the delegate. Without it,
/// `known_labels` only ever knew this relay's *final* two names, so a
/// reassigned card's earlier `[<old responder>]` block survived the strip and
/// leaked the board's internal chrome into the relay (issue #1949 review,
/// CodeRabbit 3895599021). Pass `&[]` when this relay's dispatch never
/// reassigned the card.
pub fn relay_text(
    card: &TaskRecord,
    responder: &str,
    orchestrator: &str,
    prior_responders: &[&str],
) -> String {
    let status = match card.column.as_str() {
        COLUMN_IN_REVIEW => "is ready for review",
        COLUMN_IN_PROGRESS => "is still in progress",
        COLUMN_PAUSED => "is paused",
        COLUMN_TODO => "is back in Pending",
        // Nothing lands a card here yet (§4's auto-advance will), but naming it
        // keeps a future relay off the raw-column-id fallback below.
        COLUMN_PLANNING => "is being planned",
        COLUMN_DONE => "is done",
        other => other,
    };
    let credit = if responder.is_empty() || responder == orchestrator {
        String::new()
    } else {
        format!(" ({responder} ran it)")
    };
    let headline = format!("\"{}\" {status}{credit}.", card.title);
    match card.note.as_deref().filter(|n| !n.trim().is_empty()) {
        Some(note) => {
            // The only labels a block can legitimately carry are the ones
            // this card's own lifecycle generates: the runtime's own voice,
            // an operator-initiated cancel, an operator's mid-flight redirect,
            // or an identity this dispatch itself produced — the responder
            // this relay is crediting, the orchestrator speaking, and (after a
            // reassignment) whoever held the card before. An operator's own
            // note text is never on this list, no matter how it happens to be
            // bracketed (issue #1949 review, Codex thread 3895066483). Empty
            // dynamic labels are dropped before matching — an unresolved
            // responder/orchestrator must not turn a literal `[] ` prefix in
            // operator-authored text into stripped attribution (CodeRabbit
            // 3895599021).
            let known_labels: Vec<&str> = [
                crate::runtime::advance::SYSTEM_ATTRIBUTION,
                OPERATOR_ATTRIBUTION,
                OPERATOR_REDIRECT_ATTRIBUTION,
                REVIEWER_ATTRIBUTION,
                responder,
                orchestrator,
            ]
            .into_iter()
            .chain(prior_responders.iter().copied())
            .filter(|label| !label.is_empty())
            .collect();
            format!(
                "{headline}\n\n{}",
                strip_note_attribution(note, &known_labels)
            )
        }
        None => headline,
    }
}

/// Strips a leading `[<label>] ` prefix from each block of a card note, so the
/// relayed bubble carries the prose without the board's internal
/// `[system]`/`[writer]` chrome — but only when `label` is one this card's own
/// lifecycle actually generates (`known_labels`). The headline already
/// credits the doer.
///
/// A block is a `\n\n`-separated span. A leading `[word] ` prefix is removed
/// only when `word` is in `known_labels`; any other bracketed opener —
/// including operator-authored content that happens to start with a bracket,
/// like `[Important] Keep the legacy API` — is left verbatim. A block that
/// opens with `[` but has no closing `] ` is also left verbatim, and brackets
/// later in the block are untouched.
fn strip_note_attribution(note: &str, known_labels: &[&str]) -> String {
    note.split("\n\n")
        .map(|block| {
            block
                .strip_prefix('[')
                .and_then(|rest| rest.split_once("] "))
                .filter(|(label, _)| known_labels.contains(label))
                .map_or(block, |(_, body)| body)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The orchestrator's relay of a finished card back into the conversation it
/// was spawned from.
///
/// Attributed to `orchestrator`, **not** to the agent that did the work —
/// mirroring `run_delegation`'s single-accountable-voice model. The assignee is
/// credited inside the text instead, so the operator still knows who did it
/// without a second agent speaking to them directly.
///
/// `steps` is empty by construction: a dispatched card has no chat bubble to
/// render a timeline on, so its steps go into the note (and, once #190 lands,
/// onto the task's own `task_id`-correlated timeline).
///
/// `task_id` carries the card's own id, for field-contract consistency with
/// every other `OutboundMessage` producer — but `CompanyRuntime::
/// journal_dispatch_replies` intentionally strips it back to `None` before
/// journaling this bubble: the settle that already ran left a
/// `DeskTaskCompleted` event pointed at this same thread, and carrying
/// `task_id` here too would render a second "card opened" chip for a card
/// that is not open by the time this bubble lands. It does not reach
/// `AgentReply::task_id` and does not survive a transcript reload.
///
/// `prior_responders` is forwarded to [`relay_text`] unchanged — see its
/// docs for why a reassigned dispatch needs to name more than its own final
/// responder.
pub fn relay_reply(
    card: &TaskRecord,
    responder: &str,
    orchestrator: &str,
    origin_chat_id: String,
    prior_responders: &[&str],
) -> OutboundMessage {
    OutboundMessage {
        message_id: None,
        task_id: Some(card.id.clone()),
        outputs: Vec::new(),
        channel: orchestrator.to_string(),
        agent: None,
        text: relay_text(card, responder, orchestrator, prior_responders),
        mentions: Vec::new(),
        reply_to: Some(ReplyTo {
            chat_id: origin_chat_id,
        }),
        steps: Vec::new(),
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
