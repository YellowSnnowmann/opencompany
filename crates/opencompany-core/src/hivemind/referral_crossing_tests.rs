//! Tests for cross-desk referral: what crosses, what does not, and what a
//! host owes the library.
//!
//! Everything here is scripted through [`HiveTurnRunner`] and
//! [`HiveReferralRunner`] — no model, no store, no provider — because the
//! properties under test are this host's, not a model's. Whether a room asks a
//! good question is a model's business; whether the answer lands on the right
//! desk, under an author that cannot be counted as a supporter, and only when
//! the desk opted in, is entirely ours.

use super::referral_fixtures_tests::*;
use super::*;

// ---------------------------------------------------------------------------
// What actually crosses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_desk_mention_runs_one_turn_on_the_far_desk_and_carries_the_answer_home() {
    let far =
        FarDesk::answering("The replica lag budget is 400ms, measured over the last quarter.");
    let (log, outcome) = run(
        REFERRING,
        &[
            (
                "planner",
                "!question #lag What is the replica lag budget? @#platform",
            ),
            ("scout", "!propose #stage Stage the rollout behind a flag."),
            ("critic", "!support #stage ^1 Staging fits the lag budget."),
            ("planner", "!commit #stage ^3 Recorded."),
        ],
        Some(&far),
    )
    .await;

    // Exactly one question, run on the far desk, by the far desk's own first
    // eligible member. Not a fan-out: `platform` has two members and one
    // answered.
    let asked = far.asked();
    assert_eq!(asked.len(), 1, "one question, one turn: {asked:?}");
    assert_eq!(asked[0].0, "platform", "the turn ran on the far desk");
    assert_eq!(asked[0].1, "sre", "the far desk's first eligible member");
    assert!(
        !asked[0].2.contains("Decide the rollout."),
        "the far teammate is asked a colleague's question, not handed this room's task:\n{}",
        asked[0].2
    );
    assert!(
        asked[0].2.contains("What is the replica lag budget?"),
        "and it is handed the line that asked it:\n{}",
        asked[0].2
    );

    // The far turn is journaled on the far desk, under the teammate that took
    // it, because that is what happened there.
    let far_rows = log.replies("platform");
    assert_eq!(far_rows.len(), 1, "{far_rows:?}");
    assert_eq!(far_rows[0].0, "sre");
    assert!(far_rows[0].1.contains("400ms"));

    // The answer comes home attributed, and authored by the room.
    let home = log.replies("eng");
    let returned = home
        .iter()
        .find(|(_, text)| text.contains("400ms"))
        .expect("the answer came home");
    assert_eq!(
        returned.0, HIVE_REFERRAL_AUTHOR,
        "the row that carries it is the room's, not the far teammate's: {home:?}"
    );
    assert!(
        returned.1.contains("@sre") && returned.1.contains("Platform"),
        "and it names who said it and where: {}",
        returned.1
    );

    assert_eq!(outcome.referrals.asked.len(), 1);
    assert!(
        outcome.referrals.asked[0].returned,
        "{:?}",
        outcome.referrals
    );
    assert!(
        outcome
            .summary()
            .contains("asked 1 question of another desk"),
        "the operator is told the room went outside: {}",
        outcome.summary()
    );
}

/// **A pair may keep talking, the way a desk keeps deliberating.**
///
/// A crossing to a DESK convenes it and it runs until it settles. A crossing to
/// a PERSON ran exactly one turn, so two seats working something out got one
/// question and one reply and no way to clarify — the same asymmetry
/// `deliberates` removed for desks, left standing on the other target. Every
/// pair exchange in eight live episodes was exactly two rows, which was the
/// mechanism and not the agents' choice.
///
/// `pair_messages` bounds it. The default of 2 IS that single exchange, so this
/// pins both: raised, the pair alternates; unset, nothing changes.
#[tokio::test]
async fn a_pair_may_keep_talking_up_to_its_bound() {
    const CHATTY: &str = "hive = { turn_budget = 8, quorum = 2, blind_round = false, \
                          referral = { enabled = true, pair_messages = 4 } }";
    let far = FarDesk::answering("The replica lag budget is 400ms.");
    let (log, _) = run(
        CHATTY,
        &[
            (
                "planner",
                "!question #lag What is the replica lag budget? @sre",
            ),
            ("scout", "!propose #stage Stage the rollout behind a flag."),
            ("critic", "!support #stage ^1 Staging fits the lag budget."),
            ("planner", "!commit #stage ^3 Recorded."),
        ],
        Some(&far),
    )
    .await;

    let said = log.replies(&super::referral::pair_conversation("planner", "sre"));
    assert_eq!(
        said.len(),
        4,
        "the pair runs to its bound rather than stopping at one reply: {said:?}"
    );
    // Alternating, because a pair is two people: whoever did not just speak
    // goes next.
    let voices: Vec<&str> = said.iter().map(|(who, _)| who.as_str()).collect();
    assert_eq!(
        voices,
        vec!["planner", "sre", "planner", "sre"],
        "they take it in turns: {said:?}"
    );
}

/// The same desk with `pair_messages` unset: one question, one reply, as before.
#[tokio::test]
async fn a_pair_bound_left_unset_is_the_single_exchange_it_always_was() {
    let far = FarDesk::answering("The replica lag budget is 400ms.");
    let (log, _) = run(
        REFERRING,
        &[
            (
                "planner",
                "!question #lag What is the replica lag budget? @sre",
            ),
            ("scout", "!propose #stage Stage the rollout behind a flag."),
            ("critic", "!support #stage ^1 Staging fits the lag budget."),
            ("planner", "!commit #stage ^3 Recorded."),
        ],
        Some(&far),
    )
    .await;

    assert_eq!(
        log.replies(&super::referral::pair_conversation("planner", "sre"))
            .len(),
        2,
        "a company that says nothing behaves exactly as it did"
    );
}

/// **Naming a PERSON puts the exchange in their pair thread, not on a desk.**
///
/// A crossing addressed to somebody by name is a conversation between the two
/// of them. Run on the answerer's desk, it published a question that desk was
/// never asked, in front of colleagues who had no part in it, in a transcript
/// whose job is to record what THAT desk did.
///
/// The sibling test above pins the other half: `@#platform` is a question put
/// to the desk, and still runs there so the room can settle it. Both forms are
/// indistinguishable by the time the library hands back a `Referral` — each
/// carries the target's home desk — so the two tests together are what keep
/// the distinction from collapsing back into one.
#[tokio::test]
async fn naming_a_person_holds_the_exchange_in_their_pair_thread() {
    let far = FarDesk::answering("The replica lag budget is 400ms.");
    let (log, _outcome) = run(
        REFERRING,
        &[
            (
                "planner",
                "!question #lag What is the replica lag budget? @sre",
            ),
            ("scout", "!propose #stage Stage the rollout behind a flag."),
            ("critic", "!support #stage ^1 Staging fits the lag budget."),
            ("planner", "!commit #stage ^3 Recorded."),
        ],
        Some(&far),
    )
    .await;

    let pair = super::referral::pair_conversation("planner", "sre");
    assert_eq!(pair, "dm:planner+sre", "sorted, so one thread not two");

    // Both sides are there, in order, so the thread reads as the conversation
    // it was.
    let said = log.replies(&pair);
    assert_eq!(said.len(), 2, "question then answer: {said:?}");
    assert_eq!(said[0].0, "planner", "the asker speaks first");
    assert!(said[0].1.contains("replica lag budget"), "{said:?}");
    assert_eq!(said[1].0, "sre", "and the person asked answers");
    assert!(said[1].1.contains("400ms"), "{said:?}");

    // And the answerer's desk carries none of it. This is the whole point: the
    // question was put to a person, so their colleagues are not made to read it.
    assert!(
        log.replies("platform").is_empty(),
        "a desk that was never asked holds nothing: {:?}",
        log.replies("platform")
    );

    // The room still gets the conclusion, under the room's own author — the
    // property this must not break.
    let home = log.replies("eng");
    let returned = home
        .iter()
        .find(|(_, text)| text.contains("400ms"))
        .expect("the answer still comes home");
    assert_eq!(returned.0, HIVE_REFERRAL_AUTHOR, "{home:?}");
}

#[tokio::test]
async fn the_answer_comes_home_under_the_room_so_it_can_never_be_counted_as_support() {
    // The property the whole design turns on. A row authored by a roster id
    // folds as a trace and can be counted as a supporter; if the far desk's
    // answer came back under `@sre`, one supporter would count on two desks —
    // which is voting twice, not pooling information.
    let far = FarDesk::answering("!support #stage The platform desk agrees.");
    let (log, _) = run(
        REFERRING,
        &[
            ("planner", "!question #lag Ask them. @#platform"),
            ("scout", "!propose #stage Stage it."),
            ("critic", "!support #stage ^1 Fine."),
            ("planner", "!commit #stage ^3 Recorded."),
        ],
        Some(&far),
    )
    .await;

    let home = log.replies("eng");
    let carried: Vec<&(String, String)> = home
        .iter()
        .filter(|(_, text)| text.contains("The platform desk agrees."))
        .collect();
    assert_eq!(carried.len(), 1, "{home:?}");
    assert_eq!(
        carried[0].0, HIVE_REFERRAL_AUTHOR,
        "an answer that crossed is authored by the room, never by the far teammate"
    );
    assert!(
        !carried[0].1.trim_start().starts_with('!'),
        "and its leading marker is stripped, so a far desk's `!support` cannot fold as \
         a trace on this one: {}",
        carried[0].1
    );
}

#[tokio::test]
async fn a_line_that_asks_nobody_refers_nothing() {
    let far = FarDesk::answering("unused");
    let (_, outcome) = run(
        REFERRING,
        &[
            (
                "planner",
                "!propose #stage Stage the rollout behind a flag.",
            ),
            ("scout", "!support #stage ^1 Agreed."),
            ("critic", "!commit #stage ^1 Recorded."),
        ],
        Some(&far),
    )
    .await;

    assert!(far.asked().is_empty(), "{:?}", far.asked());
    assert!(outcome.referrals.asked.is_empty());
    assert_eq!(
        outcome.referral_summary(),
        "",
        "a room that asked nothing says nothing about referral"
    );
}

/// **A question put to a seat on this desk is not a question of another desk.**
///
/// `reach` widens strictly (`local` → `channels` → `desks`), so a desk that
/// opted in to referral may legitimately put a question to one of its own
/// seats. The close used to describe every recorded question as being "of
/// another desk" regardless, and a live six-day `companies/vending_machine_co`
/// run reported "The room asked 2 questions of another desk (@fleet_tech on
/// ops, @field_realist on ops)" — on the ops desk, naming two ops seats. An
/// operator reading that has been told the room reached outside when it did
/// not, which is exactly the kind of claim the close exists to make reliably.
#[test]
pub(super) fn the_close_tells_a_local_question_from_a_crossing_one() {
    use crate::hivemind::referral::{AskedQuestion, ReferralLedger};

    let outcome = |asked: Vec<AskedQuestion>| EpisodeOutcome {
        ending: EpisodeEnding::Exhausted,
        turns: 4,
        first_seq: None,
        last_seq: None,
        report_seq: None,
        violations: Vec::new(),
        failed_turns: 0,
        referrals: ReferralLedger {
            asked,
            over_cap: 0,
            failed: 0,
        },
    };
    let question = |target: &str, desk: &str, crossed: bool| AskedQuestion {
        asker: "route_planner".to_owned(),
        target: target.to_owned(),
        desk: desk.to_owned(),
        // These fixtures describe crossings that ran on a desk, not in a pair.
        conversation: None,
        opened: None,
        returned: false,
        crossed,
    };

    let local = outcome(vec![question("field_realist", "ops", false)]).referral_summary();
    assert!(
        local.contains("put 1 question to a seat on this desk (@field_realist)"),
        "{local}"
    );
    assert!(
        !local.contains("another desk"),
        "a question that never left the desk must not be reported as crossing: {local}"
    );

    let crossing =
        outcome(vec![question("account_manager", "commercial", true)]).referral_summary();
    assert!(
        crossing.contains("asked 1 question of another desk (@account_manager on commercial)"),
        "{crossing}"
    );

    // Both in one episode: each is counted under its own heading, and the
    // crossing one still names the desk it reached.
    let both = outcome(vec![
        question("account_manager", "commercial", true),
        question("field_realist", "ops", false),
    ])
    .referral_summary();
    assert!(both.contains("asked 1 question of another desk"), "{both}");
    assert!(
        both.contains("put 1 question to a seat on this desk"),
        "{both}"
    );
}

/// **Every close is a sentence, whatever the episode's referral facts were.**
///
/// The summary used to hang every clause off one "The room …" prefix, so an
/// episode whose only referral fact was a failure printed "The room 1 went
/// unanswered." — which a live `companies/vending_machine_co` run duly did.
#[test]
pub(super) fn the_close_reads_as_english_for_every_combination_of_referral_facts() {
    use crate::hivemind::referral::{AskedQuestion, ReferralLedger};

    let outcome = |asked: Vec<AskedQuestion>, failed: u32, over_cap: u32| EpisodeOutcome {
        ending: EpisodeEnding::Exhausted,
        turns: 4,
        first_seq: None,
        last_seq: None,
        report_seq: None,
        violations: Vec::new(),
        failed_turns: 0,
        referrals: ReferralLedger {
            asked,
            over_cap,
            failed,
        },
    };
    let asked = || {
        vec![AskedQuestion {
            asker: "route_planner".to_owned(),
            target: "account_manager".to_owned(),
            desk: "commercial".to_owned(),
            conversation: None,
            opened: None,
            returned: false,
            crossed: true,
        }]
    };

    // A failure on its own is its own sentence, with its own subject.
    let only_failed = outcome(Vec::new(), 1, 0).referral_summary();
    assert!(
        only_failed.contains("1 question went unanswered."),
        "{only_failed}"
    );
    assert!(
        !only_failed.contains("The room 1"),
        "the failure clause was hung off a prefix it does not continue: {only_failed}"
    );

    // Plural agreement on the same clause.
    let two_failed = outcome(Vec::new(), 2, 0).referral_summary();
    assert!(
        two_failed.contains("2 questions went unanswered."),
        "{two_failed}"
    );

    // And it still composes with a question the room did ask.
    let both = outcome(asked(), 1, 0).referral_summary();
    assert!(
        both.contains("The room asked 1 question of another desk"),
        "{both}"
    );
    assert!(both.contains("1 question went unanswered."), "{both}");

    // A cap refusal alone, likewise.
    let capped = outcome(Vec::new(), 0, 2).referral_summary();
    assert!(capped.starts_with(" 2 more were declined"), "{capped}");
}

/// **A desk whose name already ends in "desk" does not get a second one.**
///
/// Every desk in this repo is named "… desk", and the referral note appended
/// the word unconditionally: a live run printed "@route_planner on the
/// Operations desk desk did not answer the question."
#[test]
pub(super) fn a_referral_note_names_a_desk_once() {
    use crate::hivemind::referral::{returned_note, unanswered_note};

    let named = returned_note("account_manager", "Commercial desk", "no idea");
    assert!(named.contains("on the Commercial desk answered"), "{named}");
    assert!(!named.contains("desk desk"), "{named}");

    let missing = unanswered_note("route_planner", "Operations desk");
    assert!(
        missing.contains("on the Operations desk did not answer"),
        "{missing}"
    );
    assert!(!missing.contains("desk desk"), "{missing}");

    // A name that does not carry the word still gets it.
    let bare = unanswered_note("planner", "eng");
    assert!(bare.contains("on the eng desk did not answer"), "{bare}");
}

/// **A barred move demoted for its grammar violation must not still trigger a
/// referral.**
///
/// `moves::demote` (called from `line_from` after a second grammar violation)
/// strips only the leading `!` off a barred move and leaves the rest of the
/// text — including any `@#desk` mention it names — intact. Before this fix,
/// `consider` resolved mentions from that raw, demoted content with no check
/// that the line still carried an allowed marker, so a member using a move
/// its seat does not have could still spend a far desk's turn and bring an
/// answer home, even though the line itself was refused as illegitimate.
#[tokio::test]
async fn a_demoted_barred_move_never_reaches_the_far_desk() {
    let far = FarDesk::answering("unused");
    // `planner`'s seat may only `!propose`; `!object` is barred for it. Two
    // consecutive barred attempts (the seat is asked once, corrected once,
    // and demoted on the second miss) both name `@#platform` in the body, the
    // same shape a legitimate referral-worthy line would use.
    let hive = "hive = { turn_budget = 6, quorum = 2, blind_round = false, \
                referral = { enabled = true }, moves = { planner = [\"propose\"] } }";
    let (_, outcome) = run(
        hive,
        &[
            (
                "planner",
                "!object >1 ^1 @#platform can you check the migration plan?",
            ),
            (
                "planner",
                "!object >1 ^1 @#platform can you check the migration plan?",
            ),
            ("scout", "!support #stage ^1 Agreed."),
            ("critic", "!commit #stage ^1 Recorded."),
        ],
        Some(&far),
    )
    .await;

    assert!(
        far.asked().is_empty(),
        "a demoted, illegitimate move must never spend a far desk's turn: {:?}",
        far.asked()
    );
    assert!(outcome.referrals.asked.is_empty());
    assert_eq!(
        outcome.violations.len(),
        1,
        "the barred move is still recorded as a violation: {:?}",
        outcome.violations
    );
}

#[tokio::test]
async fn a_desk_that_did_not_opt_in_never_leaves_the_room() {
    let far = FarDesk::answering("unused");
    // The same line, on a desk with no `referral` block: the mention is text
    // the room reads and nothing more.
    let (log, outcome) = run(
        PLAIN,
        &[
            ("planner", "!question #lag Ask them. @#platform"),
            ("scout", "!propose #stage Stage it."),
            ("critic", "!support #stage ^1 Fine."),
            ("planner", "!commit #stage ^3 Recorded."),
        ],
        Some(&far),
    )
    .await;

    assert!(far.asked().is_empty(), "{:?}", far.asked());
    assert!(
        log.replies("platform").is_empty(),
        "the far desk is untouched"
    );
    assert!(outcome.referrals.asked.is_empty());
}

#[tokio::test]
async fn a_far_turn_that_does_not_finish_leaves_the_room_running() {
    let far = FarDesk::broken();
    let (log, outcome) = run(
        REFERRING,
        &[
            ("planner", "!question #lag Ask them. @#platform"),
            ("scout", "!propose #stage Stage it."),
            ("critic", "!support #stage ^1 Fine."),
            ("planner", "!commit #stage ^3 Recorded."),
        ],
        Some(&far),
    )
    .await;

    assert_eq!(far.asked().len(), 1, "the question was put");
    assert!(
        log.replies("platform").is_empty(),
        "nothing is journaled on a desk whose turn failed"
    );
    // The asking room is told, on its own desk, that it got nothing back — and
    // it converges anyway. A question that went unanswered is a worse episode,
    // not a broken one.
    let home = log.replies("eng");
    assert!(
        home.iter()
            .any(|(author, text)| author == HIVE_REFERRAL_AUTHOR
                && text.contains("did not answer the question")),
        "{home:?}"
    );
    assert_eq!(outcome.referrals.failed, 1);
    assert!(outcome.referrals.asked.is_empty());
    assert!(matches!(outcome.ending, EpisodeEnding::Converged { .. }));
    assert!(
        outcome.summary().contains("1 question went unanswered"),
        "{}",
        outcome.summary()
    );
}

#[tokio::test]
async fn the_peer_cap_bounds_how_many_questions_one_episode_asks() {
    // `max_hops` bounds how *deep* a chain goes; the library deliberately
    // bounds nothing about how wide it is, because only a host knows what a
    // question costs. Here it costs a full model turn on another desk.
    let far = FarDesk::answering("Answered.");
    let hive = "hive = { turn_budget = 8, quorum = 2, blind_round = false, \
                referral = { enabled = true, peer_cap = 1 } }";
    let (_, outcome) = run(
        hive,
        &[
            ("planner", "!question #a First question. @#platform"),
            ("scout", "!question #b Second question. @#platform"),
            ("critic", "!propose #stage Stage it."),
            ("planner", "!support #stage ^3 Fine."),
            ("scout", "!commit #stage ^4 Recorded."),
        ],
        Some(&far),
    )
    .await;

    assert_eq!(
        far.asked().len(),
        1,
        "the second question never reached the far desk: {:?}",
        far.asked()
    );
    assert_eq!(outcome.referrals.over_cap, 1);
    assert!(
        outcome
            .summary()
            .contains("declined by this desk's `referral.peer_cap`"),
        "{}",
        outcome.summary()
    );
}

#[tokio::test]
async fn the_same_line_is_never_asked_twice() {
    // The idempotency the port exists for, scoped as this host can honestly
    // scope it: per episode, because an episode is not resumable. The driver
    // considers each committed line once, and the queue refuses a repeat of the
    // same `(conversation, trigger sequence)` even if it were handed one.
    let far = FarDesk::answering("Answered.");
    let (_, outcome) = run(
        REFERRING,
        &[
            ("planner", "!question #a Ask them. @#platform"),
            ("scout", "!propose #stage Stage it."),
            ("critic", "!support #stage ^1 Fine."),
            ("planner", "!commit #stage ^3 Recorded."),
        ],
        Some(&far),
    )
    .await;
    assert_eq!(far.asked().len(), 1, "{:?}", far.asked());
    assert_eq!(outcome.referrals.asked.len(), 1);
}
