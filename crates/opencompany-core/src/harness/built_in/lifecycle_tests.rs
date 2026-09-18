use super::*;
use crate::ports::tasks::TaskTitle;

fn card(column: &str, note: Option<&str>) -> TaskRecord {
    TaskRecord {
        id: "t-1".to_string(),
        title: TaskTitle::authored("Ship the thing"),
        note: note.map(str::to_string),
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: "maya".to_string(),
        updated_at_millis: 0,
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: crate::ports::tasks::TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    }
}

/// A card carrying an `origin_chat_id` — one spawned during a handoff.
fn delegated_card(column: &str) -> TaskRecord {
    let mut c = card(column, None);
    c.origin = crate::ports::TaskOrigin::new(Some("strategy".to_string()), None);
    c
}

/// The mapping every break point in `run_task`'s steer loop defers to.
/// Pinned exhaustively so a new `TaskRunEnd` cannot be added without a
/// deliberate decision about where its card lands.
///
/// **Rewritten by issue #337** — it previously pinned `Completed →
/// in_review` *for a board card* while a delegated one went to `done`. The
/// landing no longer depends on the card at all.
#[test]
fn landing_column_is_the_single_authority_for_a_cards_fate() {
    assert_eq!(landing_column(TaskRunEnd::Completed), COLUMN_IN_REVIEW);
    assert_eq!(
        landing_column(TaskRunEnd::RedirectsExhausted),
        COLUMN_IN_REVIEW
    );
    assert_eq!(landing_column(TaskRunEnd::Failed), COLUMN_TODO);
    assert_eq!(landing_column(TaskRunEnd::Cancelled), COLUMN_TODO);
    assert_eq!(landing_column(TaskRunEnd::Paused), COLUMN_PAUSED);
    assert_eq!(landing_column(TaskRunEnd::Delegated), COLUMN_IN_PROGRESS);
}

/// Issue #204: handing the work off is not finishing it. A delegating turn
/// used to settle as `Completed`, which parked the card under the
/// *delegator* while the delegate had not even run — so the hand-off keeps
/// the card in progress instead.
///
/// It is the one ending that does **not** go through
/// [`column_for_settled_run`], and this pins why: `run_status_for` calls a
/// hand-off `Paused`, which is right for the attempt row and would park the
/// card while its delegate is actively working.
#[test]
fn a_hand_off_keeps_the_card_in_progress_rather_than_finishing_it() {
    assert_eq!(landing_column(TaskRunEnd::Delegated), COLUMN_IN_PROGRESS);
    assert_eq!(run_status_for(TaskRunEnd::Delegated), RunStatus::Paused);
    assert_ne!(
        landing_column(TaskRunEnd::Delegated),
        crate::ports::tasks::column_for_settled_run(RunStatus::Paused).unwrap(),
        "a hand-off must not park the card its delegate is still working"
    );
}

/// **The #337 edit, stated as a test.** A card carrying an
/// `origin_chat_id` — one spawned during an agent-to-agent handoff — used
/// to complete straight to `done` (#171 / PR #179). It now stops in
/// `in_review` like every other card, because the operator decision of
/// 2026-08-05 leaves no automatic route to `done` at all.
///
/// Nothing about the handoff is lost: [`relay_reply`] still answers in the
/// thread the card came from. What changes is that the card stays visible
/// until a person accepts it.
#[test]
fn a_delegated_card_now_stops_in_review_like_every_other_card() {
    // Both success endings agree, or a steered handoff would diverge.
    assert_eq!(landing_column(TaskRunEnd::Completed), COLUMN_IN_REVIEW);
    assert_eq!(
        landing_column(TaskRunEnd::RedirectsExhausted),
        COLUMN_IN_REVIEW
    );
    // A delegated card still relays its answer into the originating thread,
    // which is what that conversation was waiting on.
    let relayed = relay_reply(
        &delegated_card(COLUMN_IN_REVIEW),
        "maya",
        "ceo",
        "strategy".to_string(),
        &[],
    );
    assert_eq!(
        relayed.reply_to.as_ref().map(|r| r.chat_id.as_str()),
        Some("strategy")
    );
    // Issue #1852: the relay carries the card's own id for field-contract
    // consistency, but `CompanyRuntime::journal_dispatch_replies` strips
    // it back to `None` before journaling — the settle already left a
    // `DeskTaskCompleted` link, and this would only duplicate it.
    assert_eq!(relayed.task_id.as_deref(), Some("t-1"));
    assert!(
        relayed.text.contains("is ready for review"),
        "{}",
        relayed.text
    );
}

/// **Done is reached only by a person.** No ending, on any card, may write
/// the terminal column — the only route there is an approving review.
#[test]
fn no_run_ending_ever_writes_done() {
    for end in [
        TaskRunEnd::Completed,
        TaskRunEnd::RedirectsExhausted,
        TaskRunEnd::Failed,
        TaskRunEnd::Cancelled,
        TaskRunEnd::Paused,
        TaskRunEnd::Delegated,
    ] {
        assert_ne!(
            landing_column(end),
            COLUMN_DONE,
            "{end:?} auto-advanced a card to Done"
        );
    }
}

/// #186 part b: the orchestrator's review verdict finishes a card. Since
/// #337 this is the **only** way any card reaches `done` — a person accepts
/// the result, or it stays in review.
#[test]
fn an_approving_review_finishes_the_card_and_revise_sends_it_back() {
    assert_eq!(review_landing_column(ReviewDecision::Approve), COLUMN_DONE);
    assert_eq!(review_landing_column(ReviewDecision::Revise), COLUMN_TODO);
}

/// Every card that produces a result reaches the column review consumes —
/// including a delegated one, which #179 used to route around. So no
/// finished card is unreachable by a reviewer.
#[test]
fn every_finished_card_reaches_the_column_review_consumes() {
    assert_eq!(landing_column(TaskRunEnd::Completed), COLUMN_IN_REVIEW);
    // The review verdict is defined on exactly that column, so the two
    // halves of the lifecycle meet.
    assert_eq!(review_landing_column(ReviewDecision::Approve), COLUMN_DONE);
}

#[test]
fn a_review_decision_accepts_the_synonyms_a_model_reaches_for() {
    for raw in ["approve", "Approved", " ACCEPT ", "ok"] {
        assert_eq!(
            ReviewDecision::parse(raw),
            Some(ReviewDecision::Approve),
            "{raw}"
        );
    }
    for raw in ["revise", "Reject", "rework", "changes"] {
        assert_eq!(
            ReviewDecision::parse(raw),
            Some(ReviewDecision::Revise),
            "{raw}"
        );
    }
    // An unrecognised verdict is rejected rather than silently approved —
    // guessing here would let a card through review on a typo.
    assert_eq!(ReviewDecision::parse("maybe"), None);
    assert_eq!(ReviewDecision::parse(""), None);
}

#[test]
fn a_review_note_records_the_verdict_and_any_reviewer_comment() {
    assert_eq!(
        review_note(ReviewDecision::Approve, None),
        "reviewed: approved"
    );
    assert_eq!(
        review_note(ReviewDecision::Revise, Some("tighten the intro")),
        "reviewed: needs another pass — tighten the intro"
    );
    // A blank comment must not leave a dangling em dash.
    assert_eq!(
        review_note(ReviewDecision::Approve, Some("   ")),
        "reviewed: approved"
    );
}

/// Issue #242: every `TaskRunEnd` maps to exactly one `RunStatus`, and the
/// mapping is pinned exhaustively so a new ending cannot be added without a
/// deliberate decision about how its attempt reads.
#[test]
fn run_status_is_decided_per_ending_not_re_derived_from_the_column() {
    assert_eq!(run_status_for(TaskRunEnd::Completed), RunStatus::Succeeded);
    assert_eq!(
        run_status_for(TaskRunEnd::RedirectsExhausted),
        RunStatus::Succeeded,
        "spending the redirect budget is how the run ended, not a failure"
    );
    assert_eq!(run_status_for(TaskRunEnd::Failed), RunStatus::Failed);
    assert_eq!(run_status_for(TaskRunEnd::Cancelled), RunStatus::Cancelled);
    assert_eq!(run_status_for(TaskRunEnd::Paused), RunStatus::Paused);
    assert_eq!(run_status_for(TaskRunEnd::Delegated), RunStatus::Paused);

    // The reason the status cannot be derived *from* the column, even now
    // that the column is derived from the status: the arrow only points one
    // way. Two endings share a column and are emphatically not the same
    // outcome.
    assert_eq!(
        landing_column(TaskRunEnd::Failed),
        landing_column(TaskRunEnd::Cancelled)
    );
    assert_ne!(
        run_status_for(TaskRunEnd::Failed),
        run_status_for(TaskRunEnd::Cancelled)
    );
}

/// Epic #183 decision 2 at the settle boundary: only a *person* being
/// required parks a run in review. An operator pause is resolved by
/// resuming, so no ending alone may produce `WaitingApproval` — that status
/// is minted solely by a parked approval.
#[test]
fn no_lifecycle_ending_alone_parks_a_run_in_review() {
    for end in ALL_ENDINGS {
        assert_ne!(
            run_status_for(end),
            RunStatus::WaitingApproval,
            "{end:?} must not claim a person is needed on the strength of the ending alone"
        );
        assert_eq!(
            settled_run_status(end, 0),
            run_status_for(end),
            "with nothing parked, the settle is exactly the ending's status"
        );
    }
}

/// Every `TaskRunEnd`, so a table-driven test cannot silently miss a new
/// variant (the exhaustive `match` in `run_status_for` breaks first).
const ALL_ENDINGS: [TaskRunEnd; 6] = [
    TaskRunEnd::Completed,
    TaskRunEnd::RedirectsExhausted,
    TaskRunEnd::Failed,
    TaskRunEnd::Cancelled,
    TaskRunEnd::Paused,
    TaskRunEnd::Delegated,
];

/// Epic #183 decision 2: a run that did its work but left an approval for a
/// **person** to act on is in review, not done. Only a success is
/// reclassified — a failure, a cancellation and a pause keep the reason they
/// stopped, because relabelling them "waiting on you" would hide it.
#[test]
fn a_parked_approval_turns_a_success_into_review_and_nothing_else() {
    assert_eq!(
        settled_run_status(TaskRunEnd::Completed, 1),
        RunStatus::WaitingApproval
    );
    assert_eq!(
        settled_run_status(TaskRunEnd::RedirectsExhausted, 3),
        RunStatus::WaitingApproval,
        "a redirect-capped run that parked something still needs a person"
    );

    for end in [
        TaskRunEnd::Failed,
        TaskRunEnd::Cancelled,
        TaskRunEnd::Paused,
        TaskRunEnd::Delegated,
    ] {
        assert_eq!(
            settled_run_status(end, 2),
            run_status_for(end),
            "{end:?} must keep the reason it stopped rather than reading as a review"
        );
    }
}

/// **Issue #465.** A run that parked an approval has not produced a result,
/// so its card parks instead of presenting as reviewable work.
///
/// The pairing with the row above is the whole point: the *attempt* still
/// settles `WaitingApproval` — "a person must act" is preserved exactly
/// where epic #183 decision 2 put it — while the *column* answers the other
/// question, has the work happened. One ending, two answers, two places.
#[test]
fn a_parked_approval_parks_the_card_instead_of_offering_it_for_review() {
    assert_eq!(
        settled_landing_column(TaskRunEnd::Completed, 1),
        COLUMN_PAUSED
    );
    assert_eq!(
        settled_landing_column(TaskRunEnd::RedirectsExhausted, 2),
        COLUMN_PAUSED
    );
    // The run status is untouched, so the console still says who unblocks it.
    assert_eq!(
        settled_run_status(TaskRunEnd::Completed, 1),
        RunStatus::WaitingApproval
    );

    // A turn that genuinely completed still reaches the reviewer.
    assert_eq!(
        settled_landing_column(TaskRunEnd::Completed, 0),
        COLUMN_IN_REVIEW
    );
}

/// `landing_column` is the zero-parked case of `settled_landing_column`, so
/// the two cannot drift into disagreeing about an ending.
#[test]
fn the_landing_is_one_decision_taken_with_or_without_a_parked_count() {
    for end in ALL_ENDINGS {
        assert_eq!(
            landing_column(end),
            settled_landing_column(end, 0),
            "{end:?} disagrees with itself once the overlay is spelled out"
        );
    }
}

/// **The sibling sweep issue #465 asked for**, as a test rather than a note:
/// every ending answers *did the work happen?*, and only the ones that
/// answer yes may land where a reviewer can approve them to Done.
///
/// `Delegated` is the deliberate exception — it is not an ending at all, so
/// it stays in progress under its delegate rather than settling anywhere.
#[test]
fn only_endings_that_produced_something_land_in_review() {
    // Produced a result — reviewable.
    for end in [TaskRunEnd::Completed, TaskRunEnd::RedirectsExhausted] {
        assert_eq!(landing_column(end), COLUMN_IN_REVIEW, "{end:?}");
    }
    // Produced nothing to accept — must not read as reviewable work.
    for end in [
        TaskRunEnd::Failed,
        TaskRunEnd::Cancelled,
        TaskRunEnd::Paused,
        TaskRunEnd::Delegated,
    ] {
        assert_ne!(
            landing_column(end),
            COLUMN_IN_REVIEW,
            "{end:?} produced no result and must not offer one for review"
        );
    }
    // …and neither may a success that stopped at an unauthorised call.
    for end in ALL_ENDINGS {
        assert_ne!(
            settled_landing_column(end, 1),
            COLUMN_IN_REVIEW,
            "{end:?} left an approval outstanding and must not present as reviewable"
        );
    }
}

/// A hand-off outranks the parked overlay: the delegator may have parked an
/// approval, but it still handed the work on and the delegate is running it.
/// Parking the card here would stall a card somebody is actively working.
#[test]
fn a_hand_off_stays_in_progress_even_if_the_delegator_parked_something() {
    assert_eq!(
        settled_landing_column(TaskRunEnd::Delegated, 3),
        COLUMN_IN_PROGRESS
    );
}

/// The review stop is **not** terminal-by-accident: it is non-terminal and
/// re-enterable, which is what lets a re-dispatched card wait on a second
/// approval instead of forcing #243's single-use grants to be batched.
#[test]
fn the_review_stop_stays_re_enterable() {
    let review = settled_run_status(TaskRunEnd::Completed, 1);
    assert!(review.is_parked());
    assert!(!review.is_terminal());
    assert!(RunStatus::Running.can_transition_to(review));
    assert!(review.can_transition_to(RunStatus::Running));
}

#[test]
fn a_cancellation_is_attributed_to_the_operator_not_the_assignee() {
    assert_eq!(note_attribution(TaskRunEnd::Cancelled, "maya"), "operator");
    assert_eq!(note_attribution(TaskRunEnd::Completed, "maya"), "maya");
    assert_eq!(note_attribution(TaskRunEnd::Paused, "maya"), "maya");
    assert_eq!(note_attribution(TaskRunEnd::Failed, "maya"), "maya");
}

/// The relayed bubble drops each note block's `[<who>]` attribution — the
/// board's internal chrome — while keeping the prose. The headline already
/// says who did the work.
#[test]
fn the_relay_strips_note_attribution_but_keeps_the_prose() {
    let noted = card(
        COLUMN_IN_REVIEW,
        Some("[system] moved to review\n\n[writer] drafted the intro"),
    );
    let text = relay_text(&noted, "writer", "ceo", &[]);
    assert!(!text.contains("[system]"), "{text}");
    assert!(!text.contains("[writer]"), "{text}");
    assert!(text.contains("moved to review"), "{text}");
    assert!(text.contains("drafted the intro"), "{text}");

    // A block that opens with `[` but never closes it stays verbatim.
    assert_eq!(
        strip_note_attribution("[unterminated note", &["writer"]),
        "[unterminated note"
    );
    // Brackets after the leading prefix are left alone.
    assert_eq!(
        strip_note_attribution("[writer] see [ref] below", &["writer"]),
        "see [ref] below"
    );
}

/// A reviewer's feedback block is board chrome the relay strips: once a
/// re-run settles the card back to review, the relayed bubble carries the
/// reviewer's prose without the `[reviewer]` prefix, exactly as it strips
/// the operator's mid-flight redirect.
#[test]
fn the_relay_strips_a_reviewer_feedback_block() {
    let noted = card(
        COLUMN_IN_REVIEW,
        Some("[writer] second draft\n\n[reviewer] tighten the intro"),
    );
    let text = relay_text(&noted, "writer", "ceo", &[]);
    assert!(!text.contains("[reviewer]"), "{text}");
    assert!(text.contains("tighten the intro"), "{text}");
}

/// PR #1949 review (Codex thread 3895066483): the strip used to treat ANY
/// leading `[label] ` span as generated attribution chrome, so an
/// operator-authored note that itself opens with a bracket — a heading, a
/// tag, a callout like "[Important] Keep the legacy API" — got silently
/// mangled by the relay, dropping content the operator wrote through the
/// task create/patch APIs. Only a label the caller actually generated
/// (`OPERATOR_ATTRIBUTION`, `OPERATOR_REDIRECT_ATTRIBUTION`,
/// `SYSTEM_ATTRIBUTION`, the responder, the orchestrator, or a prior
/// responder this same dispatch reassigned away from) may be stripped.
#[test]
fn operator_authored_bracket_survives_the_relay() {
    let noted = card(COLUMN_IN_REVIEW, Some("[Important] Keep the legacy API"));
    let text = relay_text(&noted, "writer", "ceo", &[]);
    assert!(
        text.contains("[Important] Keep the legacy API"),
        "an operator's own bracketed note must not be read as generated attribution: {text}"
    );
}

/// CodeRabbit 3895599021: `known_labels` used to know only this relay's
/// *final* two names, so a card whose note carries a block from BEFORE a
/// mid-flight reassignment (issue #204's hand-off loop reassigns
/// `responder` to the delegate and keeps going) leaked that prior
/// responder's `[<id>]` chrome straight into the operator-facing bubble —
/// the same class of bug `operator_authored_bracket_survives_the_relay`
/// fixed from the opposite direction, this time under-stripping instead
/// of over-stripping. `prior_responders` closes that gap, and an
/// operator-authored bracket that happens to match one of those prior ids
/// is not itself a scenario this needs to protect beyond what the
/// bracket-survives test above already proves for the current two names.
#[test]
fn the_relay_strips_a_reassigned_cards_prior_responder_label() {
    let noted = card(
        COLUMN_IN_REVIEW,
        Some("[writer] delegated to editor: proofread the draft\n\n[editor] done"),
    );
    let text = relay_text(&noted, "editor", "ceo", &["writer"]);
    assert!(!text.contains("[writer]"), "{text}");
    assert!(!text.contains("[editor]"), "{text}");
    assert!(
        text.contains("delegated to editor: proofread the draft"),
        "{text}"
    );
    assert!(text.contains("done"), "{text}");

    // Without the prior-responder hint, the old-responder block is left
    // exactly as `known_labels` used to leave it: attributed and leaking.
    let unaware = relay_text(&noted, "editor", "ceo", &[]);
    assert!(
        unaware.contains("[writer]"),
        "sanity check: an empty prior_responders must reproduce the pre-fix leak: {unaware}"
    );
}

/// CodeRabbit 3895599021: an unresolved dynamic label (an empty
/// `responder`, as `refuse_dispatch` passes when nobody ran the card) must
/// not let a literal `[] ` prefix in operator-authored text read as
/// generated attribution.
#[test]
fn an_empty_responder_does_not_strip_a_literal_bracket_prefix() {
    let noted = card(COLUMN_IN_REVIEW, Some("[] TODO: revisit this"));
    let text = relay_text(&noted, "", "ceo", &[]);
    assert!(
        text.contains("[] TODO: revisit this"),
        "an empty responder must not become a matchable known label: {text}"
    );
}

/// The operator's own mid-flight redirect instruction is recorded with
/// its own generated label (`OPERATOR_REDIRECT_ATTRIBUTION`, distinct
/// from `OPERATOR_ATTRIBUTION`) by `run_task`'s steer loop — see
/// `harness::built_in::brain`. It must be recognized and stripped exactly
/// like the other generated labels.
#[test]
fn an_operator_redirect_label_is_recognized_and_stripped() {
    let noted = card(
        COLUMN_IN_REVIEW,
        Some("[operator redirect] focus on the API instead"),
    );
    let text = relay_text(&noted, "writer", "ceo", &[]);
    assert!(!text.contains("[operator redirect]"), "{text}");
    assert!(text.contains("focus on the API instead"), "{text}");
}

/// The one-voice change: the bubble is the orchestrator's, and the assignee
/// is credited in the text rather than speaking to the operator directly.
#[test]
fn the_relay_bubble_is_the_orchestrators_and_credits_the_assignee() {
    let finished = card(COLUMN_IN_REVIEW, Some("[maya] shipped it"));
    let msg = relay_reply(&finished, "maya", "ceo", "strategy".to_string(), &[]);

    assert_eq!(msg.channel, "ceo", "the orchestrator owns the reply");
    assert_eq!(
        msg.reply_to.as_ref().map(|r| r.chat_id.as_str()),
        Some("strategy")
    );
    assert!(msg.text.contains("Ship the thing"), "{}", msg.text);
    assert!(msg.text.contains("is ready for review"), "{}", msg.text);
    assert!(msg.text.contains("(maya ran it)"), "{}", msg.text);
    assert!(msg.text.contains("shipped it"), "{}", msg.text);
    assert!(
        msg.steps.is_empty(),
        "a dispatched card discards its steps into the note"
    );
}

/// …but it does not credit itself. A card the orchestrator ran reads as one
/// voice, not as the orchestrator narrating its own work in the third
/// person.
#[test]
fn the_relay_does_not_credit_the_orchestrator_to_itself() {
    let finished = card(COLUMN_IN_REVIEW, None);
    let msg = relay_reply(&finished, "ceo", "ceo", "main".to_string(), &[]);
    assert!(!msg.text.contains("ran it"), "{}", msg.text);
    assert_eq!(msg.text, "\"Ship the thing\" is ready for review.");

    // An unresolved assignee credits nobody rather than an empty paren.
    let orphan = relay_reply(&finished, "", "ceo", "main".to_string(), &[]);
    assert!(!orphan.text.contains("ran it"), "{}", orphan.text);
}

/// The relay reports where the card actually landed — a paused or failed
/// run must never read as a success.
#[test]
fn the_relay_reflects_the_landing_column_not_a_presumed_success() {
    let paused = card(COLUMN_PAUSED, None);
    assert!(
        relay_text(&paused, "maya", "ceo", &[]).contains("is paused"),
        "paused card must not read as finished"
    );

    let returned = card(COLUMN_TODO, Some("[operator] cancelled while in flight"));
    let text = relay_text(&returned, "maya", "ceo", &[]);
    assert!(text.contains("is back in Pending"), "{text}");
    // Issue #301: collapsing the backlog pool into To-do is only lossless
    // because the reason rides along on the card. The relay must keep
    // saying why, or "back in Pending" is indistinguishable from fresh work.
    assert!(text.contains("cancelled while in flight"), "{text}");

    // Every board column has a sentence. Planning is inert today, so
    // without an arm of its own a relay would fall through to the raw
    // column id and read `"Ship the thing" planning.`.
    for column in crate::ports::tasks::BOARD_COLUMNS {
        let text = relay_text(&card(column, None), "maya", "ceo", &[]);
        assert!(
            !text.contains(&format!("\" {column}")),
            "column {column} fell through to the raw-id fallback: {text}"
        );
    }
    assert!(
        relay_text(&card(COLUMN_PLANNING, None), "maya", "ceo", &[]).contains("is being planned"),
        "planning needs its own sentence"
    );
}

/// A card with no note (or a whitespace-only one) still relays a complete
/// sentence rather than a dangling blank block.
#[test]
fn a_noteless_card_still_relays_a_complete_sentence() {
    let bare = card(COLUMN_IN_REVIEW, None);
    assert_eq!(
        relay_text(&bare, "maya", "ceo", &[]),
        "\"Ship the thing\" is ready for review (maya ran it)."
    );
    let blank = card(COLUMN_IN_REVIEW, Some("   \n  "));
    assert_eq!(
        relay_text(&blank, "maya", "ceo", &[]),
        "\"Ship the thing\" is ready for review (maya ran it)."
    );
}

/// Issue #1861: a turn that raised a question settles `Blocked`, not
/// `Succeeded` and not `WaitingApproval` — the operator owes an answer, not
/// a decision.
#[test]
fn a_question_outranks_a_plain_success_and_an_approval() {
    assert_eq!(
        settled_run_status_with_blockers(TaskRunEnd::Completed, 0, 1),
        RunStatus::Blocked
    );
    assert_eq!(
        settled_run_status_with_blockers(TaskRunEnd::Completed, 2, 1),
        RunStatus::Blocked,
        "a turn that both asked and parked an approval is waiting on the answer first"
    );
    assert_eq!(
        settled_run_status_with_blockers(TaskRunEnd::Completed, 0, 0),
        RunStatus::Succeeded,
        "no question, no relabel"
    );
}

/// A failure keeps its own status even with a question outstanding: the
/// operator has a bigger problem, and relabelling would hide why the work
/// stopped. The same stance the approval overlay takes.
#[test]
fn a_failure_keeps_its_status_even_with_a_question_pending() {
    assert_eq!(
        settled_run_status_with_blockers(TaskRunEnd::Failed, 0, 1),
        RunStatus::Failed
    );
    assert_eq!(
        settled_run_status_with_blockers(TaskRunEnd::Cancelled, 0, 1),
        RunStatus::Cancelled
    );
}

/// The card parks either way, and the blocker ending lands it in `paused`
/// rather than back in To-do where a bounced card and an open question
/// would look the same.
#[test]
fn a_blocked_ending_lands_the_card_paused() {
    assert_eq!(landing_column(TaskRunEnd::Blocked), COLUMN_PAUSED);
    assert_eq!(run_status_for(TaskRunEnd::Blocked), RunStatus::Blocked);
}
