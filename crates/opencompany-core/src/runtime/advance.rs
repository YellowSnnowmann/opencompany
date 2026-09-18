//! The one guarded mover for the board's automatic edge (issue #337, epic
//! #183 §4).
//!
//! [`column_for_settled_run`] decides *where* a settled attempt's card belongs.
//! This module owns the far narrower question of *whether it is allowed to move
//! it*, and it is the only place outside `run_task`'s own settle that writes a
//! card's column off the back of a run.
//!
//! # Why the guard, and why it is code rather than intent
//!
//! Epic #337's acceptance criteria include one that reads as a prohibition
//! rather than a feature: *"a card in Paused or In Review is never moved
//! automatically — only the person, or the unblocking event, moves it."*
//!
//! Three system paths settle a run they did not dispatch — the cycle's
//! terminality backstop, [`CompanyRuntime::abandon_run`], and the boot reaper's
//! card sweep. Each of them can fire long after the card has moved on: an
//! operator can drag a card out of In Progress, an approval can park it in
//! Paused, a *later* attempt can land it in In Review, all while the row this
//! path is settling is still claiming to be live. A mover that trusted its
//! caller's idea of where the card was would happily yank a parked card back to
//! To-do and destroy real pending work.
//!
//! So [`advance_settled_card`] **re-reads the card** and refuses unless it is
//! still in [`COLUMN_IN_PROGRESS`]. The guard is a structural property of the
//! only function that can do the move, not a rule each of the three callers has
//! to remember.
//!
//! # Why it cannot re-fire dispatch
//!
//! The write goes through the plain [`TaskStore::upsert`] port, never
//! [`CompanyRuntime::upsert_task`]. Only the latter carries the
//! `task_enters_in_progress` edge, and every column this module writes is a
//! *departure* from In Progress in any case — but routing through the port is
//! what makes "settling a run cannot start another one" true by construction
//! rather than by inspection of the mapping.
//!
//! [`column_for_settled_run`]: crate::ports::tasks::column_for_settled_run
//! [`CompanyRuntime::abandon_run`]: crate::company::runtime::CompanyRuntime
//! [`CompanyRuntime::upsert_task`]: crate::company::runtime::CompanyRuntime

use crate::Result;
use crate::ports::TaskStore;
use crate::ports::notifications::{Notification, NotificationStore, Subject, SubjectKind};
use crate::ports::now_millis;
use crate::ports::runs::RunStatus;
use crate::ports::tasks::{
    COLUMN_IN_PROGRESS, COLUMN_PAUSED, COLUMN_PLANNING, COLUMN_TODO, column_for_settled_run,
};
use crate::ports::types::CompanyId;

/// The note attribution used when the *runtime* settles a card, as opposed to
/// an agent that produced a result or an operator who stopped one.
///
/// Its own word rather than reusing the assignee's or `"operator"`: a card that
/// came back to To-do because the host died must not read as though a teammate
/// gave up or a person cancelled it.
pub const SYSTEM_ATTRIBUTION: &str = "system";

/// Why a card moved, as one note block: `[<who>] <what>`.
///
/// The card has no first-class `result` field, so every outcome — an agent's
/// reply, an operator redirect, a dispatch failure, and now a system settle —
/// lands as an attributed block appended below whatever the note already said.
/// Nothing is ever overwritten: the note is the card's history.
///
/// Ungated and shared, so the harness settle and the three system paths append
/// in one shape. A second copy of this two-line function is exactly how a card
/// ends up with two different-looking note formats depending on which path
/// touched it last.
pub fn append_result(prev: Option<&str>, attribution: &str, body: &str) -> String {
    let block = format!("[{attribution}] {body}");
    match prev.filter(|p| !p.is_empty()) {
        Some(p) => format!("{p}\n\n{block}"),
        None => block,
    }
}

/// Moves `task_id`'s card to wherever `status` lands it, carrying `reason` onto
/// the note — but **only** if the card is still in [`COLUMN_IN_PROGRESS`].
///
/// Returns the column it wrote, or `None` when nothing moved. `None` covers
/// four distinct no-ops, all of them correct and none of them an error:
///
/// * `status` is not settled ([`RunStatus::Pending`] / [`RunStatus::Running`]),
///   so there is no landing to write;
/// * the card is gone (deleted between dispatch and settle);
/// * the card has already left In Progress under its own steam — an operator
///   dragged it, a later attempt landed it, an approval parked it. **This is
///   the guard**, and it is what keeps a Paused or In Review card untouched by
///   a late settle;
/// * the card was never in In Progress to begin with.
///
/// Errors only on a store fault. Every caller treats that as best-effort and
/// logs it: the attempt row is already settled by the time this runs, so a
/// board write that cannot land must not undo it.
pub async fn advance_settled_card(
    tasks: &dyn TaskStore,
    company: &CompanyId,
    task_id: &str,
    status: RunStatus,
    reason: &str,
) -> Result<Option<&'static str>> {
    let Some(column) = column_for_settled_run(status) else {
        return Ok(None);
    };
    // Re-read rather than trusting a card the caller is holding: the whole
    // point of the guard is that the board may have moved since.
    let Some(mut card) = tasks
        .list(company)
        .await?
        .into_iter()
        .find(|t| t.id == task_id)
    else {
        return Ok(None);
    };
    if card.column != COLUMN_IN_PROGRESS {
        return Ok(None);
    }
    card.note = Some(append_result(
        card.note.as_deref(),
        SYSTEM_ATTRIBUTION,
        reason,
    ));
    card.column = column.to_string();
    // Issue #1865: the board's bounce chip, set on the exact same landing this
    // function already computed and cleared on any other one. `column` is
    // `column_for_settled_run`'s answer, so this cannot drift from the write
    // above into a second, independent reading of "did this bounce".
    card.bounced = bounced_reason(column, status, reason);
    card.updated_at_millis = now_millis();
    tasks.upsert(company, &card).await?;
    Ok(Some(column))
}

/// Returns a card whose blocker expired unanswered to [`COLUMN_TODO`],
/// carrying the question nobody answered (issue #1861). `true` when the card
/// moved.
///
/// The approval TTL's default-deny reaching the board. A blocker parks the card
/// in `paused` and asks; if nothing answers before the deadline the question is
/// retired, and this is what stops the card sitting in `paused` forever waiting
/// on a decision that has already been made against it. Epic #183's rule
/// applies again at that point: a card that cannot proceed goes back to To-do
/// carrying its reason, never into a stuck column of its own.
///
/// # Why the question is preserved rather than dropped
///
/// The unanswered question is the single most useful thing on the card. It is
/// what a person needs in order to unblock the work whenever they next look —
/// the TTL expiring does not make the work possible, it only stops pretending
/// somebody is about to answer.
///
/// # The guard, and why it differs from [`advance_settled_card`]'s
///
/// That function guards on [`COLUMN_IN_PROGRESS`] because it settles a run that
/// was running. This one guards on [`COLUMN_PAUSED`], the column the blocker
/// itself put the card in. Same principle either way: a card an operator has
/// since dragged somewhere is theirs, and an expiry must not drag it back.
///
/// # Why the chip is set here rather than through [`bounced_reason`]
///
/// `bounced_reason` answers "did this *settle* bounce", from a
/// [`RunStatus`] — and there is no run settling here. The attempt this blocker
/// came from ended long ago; what expired is a park. The chip is still exactly
/// right for the board's purpose (#1865): this card is not fresh, and an
/// operator scanning To-do must be able to see that without opening it.
pub async fn return_expired_blocker_card(
    tasks: &dyn TaskStore,
    company: &CompanyId,
    task_id: &str,
    question: &str,
) -> Result<bool> {
    let Some(mut card) = tasks
        .list(company)
        .await?
        .into_iter()
        .find(|t| t.id == task_id)
    else {
        return Ok(false);
    };
    if card.column != COLUMN_PAUSED {
        return Ok(false);
    }
    let reason = format!("{EXPIRED_BLOCKER}: {question}");
    card.note = Some(append_result(
        card.note.as_deref(),
        SYSTEM_ATTRIBUTION,
        &reason,
    ));
    card.column = COLUMN_TODO.to_string();
    card.bounced = Some(reason);
    card.updated_at_millis = now_millis();
    tasks.upsert(company, &card).await?;
    Ok(true)
}

/// The lead-in on a card returned by an unanswered blocker (issue #1861).
///
/// Its own wording rather than the failure one: nothing failed. The work is
/// exactly as possible as it was, and the only thing that changed is that
/// nobody answered in time.
pub const EXPIRED_BLOCKER: &str =
    "nobody answered this in time, so it is back in To-do — it still needs";

/// Whether a settle lands a card back on [`COLUMN_TODO`] because the attempt
/// **failed or was cancelled**, as opposed to any other landing this function
/// writes (issue #1865).
///
/// The single rule both card-write sites (this module's system mover, and
/// `run_task`'s own rich settle in `harness::built_in::brain`) apply, so
/// "which card gets the bounce chip" cannot answer differently depending on
/// which of the two paths happened to settle a given run.
///
/// `WaitingApproval`/`Paused` also land on a column other than
/// [`COLUMN_IN_PROGRESS`] but never on `COLUMN_TODO` — see
/// [`column_for_settled_run`] — so checking the column alone already excludes
/// them; the status check on top is what tells a genuine failure apart from
/// the one other status [`column_for_settled_run`] maps to `COLUMN_TODO`... in
/// practice there is none today, but the explicit check keeps this correct by
/// construction rather than by the current shape of that mapping.
pub fn bounced_reason(column: &str, status: RunStatus, reason: &str) -> Option<String> {
    (column == COLUMN_TODO && matches!(status, RunStatus::Failed | RunStatus::Cancelled))
        .then(|| reason.to_string())
}

/// Files the durable "a board card's dispatch failed and bounced back to
/// To-do" notification (issue #1865).
///
/// Shared by every system path that settles a run its own turn did not —
/// [`CompanyRuntime::abandon_run`](crate::company::runtime::CompanyRuntime::abandon_run),
/// the cycle's terminality backstop, and the boot reaper's card sweep in
/// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) — so a
/// crash-recovered dispatch failure is announced exactly like a live one
/// instead of only picking up the bounce chip silently. Call this only after
/// [`advance_settled_card`] actually reports the card landed on
/// [`COLUMN_TODO`]; a run that settled without moving the card raises nothing.
///
/// Whole-company audience: a bounced card has no single decider the way a
/// mention does, and its assignee is exactly who the card's own `assignee`
/// field already names for anyone who opens it.
///
/// Best-effort and logged, never propagated: the dispatch has already failed,
/// and a bookkeeping write cannot make that better or worse.
pub async fn notify_dispatch_failed(
    notifications: &dyn NotificationStore,
    company: &CompanyId,
    task_id: &str,
    reason: &str,
) {
    // `Notification.title` is documented as one line. `reason` is a free-form
    // failure text (an error's `Display`, in practice) and is not guaranteed
    // not to carry `\r`/`\n`, so normalize before interpolating — otherwise a
    // multiline reason persists a multiline title.
    let one_line_reason = reason.replace(['\r', '\n'], " ");
    let note = Notification {
        id: crate::ports::generate_id(),
        kind: "dispatch_failed".to_string(),
        subject: Subject {
            kind: SubjectKind::Task,
            id: task_id.to_string(),
        },
        created_at: now_millis(),
        title: format!("A card's dispatch failed and returned to To-do: {one_line_reason}"),
        audience: None,
        context: None,
    };
    if let Err(err) = notifications.append(company, &note).await {
        tracing::warn!(
            company = %company,
            task = %task_id,
            error = %err,
            "[runs] a dispatch-failure notification could not be recorded; the card still \
             bounced, but nobody is badged for it"
        );
    }
}

/// The note a card gets when a planning pass was interrupted by the host going
/// away underneath it (issue #337).
///
/// Its own wording rather than the orphan-run one: nothing *ran*, so "an
/// attempt was abandoned" would be false. What happened is smaller and the
/// operator's recovery is a single drag.
pub const PLANNING_INTERRUPTED: &str = "the host restarted during planning, so the pass never finished — drag the card back into \
     Planning to try again";

/// Returns every card found sitting in [`COLUMN_PLANNING`] to
/// [`COLUMN_TODO`], carrying [`PLANNING_INTERRUPTED`] on its note. Returns the
/// ids it moved.
///
/// # Why a planning pass needs its own sweep
///
/// The orphan-run reaper cannot see this. A pass mints **no**
/// [`RunRecord`](crate::ports::runs::RunRecord) — deliberately: there is no
/// agent turn, no tool loop and nothing to steer, so an attempt row would be a
/// fiction (see `docs/spec/runtime/planning.md`). But that is exactly what
/// makes the crash case invisible: a host that dies mid-pass leaves a card in
/// Planning with nothing anywhere claiming to be working it, and because the
/// trigger is the *transition* into the column — which already happened —
/// nothing will ever re-drive it. The card would sit there forever looking
/// busy.
///
/// So the boot sweep reads the board directly. It is safe for the same reason
/// the run reaper is and for one more of its own:
///
///  * **Boot-only.** Nothing from this process can be in flight at boot, so
///    every Planning card provably belongs to a dead process. Like the run
///    reaper, this must NOT run on a rebuild ([`RuntimeRebuilder`]), where that
///    premise is false and a live pass would be yanked out from under itself.
///  * **Planning is transient by construction.** Every terminating path of a
///    pass leaves the column — to In Progress on success, to To-do otherwise.
///    A card resting in Planning is therefore never a state an operator chose
///    and never a state a healthy pass leaves behind, which is what makes
///    "found here at boot ⇒ interrupted" a sound inference rather than a guess.
///
/// It writes through the plain [`TaskStore::upsert`] port, never
/// [`CompanyRuntime::upsert_task`], so returning a card cannot fire the
/// planning edge again and put the company straight back into the pass that was
/// just interrupted.
///
/// Best-effort per card: one card that will not move must not stop the rest and
/// must not fail boot.
///
/// [`RuntimeRebuilder`]: crate::runtime::rebuild::RuntimeRebuilder
/// [`CompanyRuntime::upsert_task`]: crate::company::runtime::CompanyRuntime
pub async fn sweep_stranded_planning(
    tasks: &dyn TaskStore,
    company: &CompanyId,
) -> Result<Vec<String>> {
    let stranded: Vec<_> = tasks
        .list(company)
        .await?
        .into_iter()
        .filter(|t| t.column == COLUMN_PLANNING)
        .collect();
    let mut returned = Vec::with_capacity(stranded.len());
    for mut card in stranded {
        card.note = Some(append_result(
            card.note.as_deref(),
            SYSTEM_ATTRIBUTION,
            PLANNING_INTERRUPTED,
        ));
        card.column = COLUMN_TODO.to_string();
        card.updated_at_millis = now_millis();
        match tasks.upsert(company, &card).await {
            Ok(()) => returned.push(card.id),
            Err(err) => tracing::warn!(
                company = %company,
                task = %card.id,
                error = %err,
                "[planning] could not return a card stranded in Planning by a previous host \
                 process; it stays there until the next boot"
            ),
        }
    }
    Ok(returned)
}

#[cfg(test)]
#[path = "advance_tests.rs"]
mod tests;
