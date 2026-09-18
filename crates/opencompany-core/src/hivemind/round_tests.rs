//! The round is the unit, not the turn.
//!
//! `HiveStep::Speak` carries a *round* — the turns authorized to run together,
//! and the one state the episode takes once all of them are appended. Every
//! other fixture in this module builds its `HiveTurn` by hand with
//! `round_start = Sequence(u64::MAX)`, which is a round of one: the width at
//! which `readable()`'s `round_start` clause is inert **by construction**, and
//! therefore the width at which none of the round's own guarantees can fail.
//!
//! That band is where both isolation bugs found in review on #2313 lived — the
//! pinboard read and the `elsewhere` read were each bounded one row too tightly,
//! and a width-one test cannot tell `round_start` from `round_start + 1`. These
//! tests drive the real [`EpisodeDriver`] at a width above one instead.
//!
//! **Mutation-checked.** Each test below was confirmed to FAIL against the bug
//! it guards: unbounding the board kills `a_pin_by_a_round_mate_..`, and
//! restoring the `round_start` off-by-one kills
//! `a_pin_on_the_round_boundary_..`. Two earlier drafts of that second test
//! passed against the bug — once because the pinned row's text is in the
//! visible transcript anyway, once because `any()` over the rounds after the
//! boundary finds the pin on a later, safely-above board. Both are written up at
//! their assertions so the next edit does not quietly reintroduce them.
//!
//! ⚠️ **The `elsewhere` bound is NOT covered here.** `elsewhere_for` returns
//! early unless the driver was given `context_desks`, which these fixtures do
//! not set, so mutating that bound kills nothing in this file. Covering it needs
//! a second desk both members sit on and a referral that writes to it mid-round.
//!
//! **What is deliberately not asserted here.** "The round's state is committed
//! once, after the last append" has no outside observable while the host runs
//! every turn it was handed: `next_state` is one value for the whole round, so
//! committing it per-turn and committing it once are indistinguishable from the
//! journal. What *is* observable is the failure path — a turn that fails must
//! not abandon its round-mates — and that is the last test below.

use std::sync::Arc;

use super::moves_fixtures_tests::{Runner, manifest_with, open};
use super::test::{MemoryLog, desk_of};
use super::*;
use crate::ports::events::EventLog;

/// Three distinct opening proposals, one per seat, so each member's line is
/// recognisable in anybody else's prompt by its topic alone.
const STAGE: &str = "!propose #stage Stage the rollout behind a flag.";
const HALT: &str = "!propose #halt Halt the rollout until Tuesday.";
const SHIP: &str = "!propose #ship Ship it whole on Friday.";

/// `blind_round` is on by default and the library caps a blind round at the
/// members not yet heard, so the opening round on this three-member desk is
/// three turns wide rather than one. A budget of twelve leaves room for the
/// rounds after it.
fn wide_manifest() -> String {
    manifest_with("hive = { turn_budget = 12, quorum = 2 }")
}

// ---------------------------------------------------------------------------
// The round runs as a unit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn members_authorized_together_are_all_appended_and_none_reads_another() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(&wide_manifest(), "eng").expect("a room");
    let runner = Runner::new(&[("planner", STAGE), ("scout", HALT), ("critic", SHIP)]);

    EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .run(trigger)
    .await
    .expect("the episode runs");

    let asked = runner.asked();
    assert!(
        asked.len() >= 3,
        "the opening round should authorize all three unheard seats, not one: {asked:?}"
    );

    // The opening round is the first three asks. Proving they were ONE round
    // rather than three rounds of one is exactly the isolation property: in
    // three sequential rounds the second speaker reads the first's row, and
    // here it must not. So this assertion is both the round-width test and the
    // concurrency-semantics test — there is no weaker way to tell them apart
    // from outside the driver.
    let opening = &asked[..3];
    for (speaker, prompt) in opening {
        for (other, line) in [("planner", STAGE), ("scout", HALT), ("critic", SHIP)] {
            if other == speaker {
                continue;
            }
            let topic = line
                .split_whitespace()
                .nth(1)
                .expect("every fixture line names a topic");
            assert!(
                !prompt.contains(topic),
                "`{speaker}` was authorized in the same round as `{other}` and must not \
                 read its row, but {topic} reached the prompt:\n{prompt}"
            );
        }
    }

    // Every turn the round authorized is durably appended — the precondition
    // the library attaches to taking up `next_state` at all.
    let replies = log.replies("eng");
    for topic in ["#stage", "#halt", "#ship"] {
        assert!(
            replies.iter().any(|(_, text)| text.contains(topic)),
            "every authorized turn in the round lands in the journal; {topic} did not: \
             {replies:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The board is bounded at the round, in both directions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pin_by_a_round_mate_stays_off_a_peers_board() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(&wide_manifest(), "eng").expect("a room");
    // Whoever speaks first in the opening round pins something. Its round-mates
    // are writing at the same time and cannot have read it, so the pin must no
    // more reach their board than the row itself reaches their transcript.
    let runner = Runner::new(&[
        ("planner", "!pin #flag The flag is the rollback lever."),
        ("scout", HALT),
        ("critic", SHIP),
    ]);

    EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .run(trigger)
    .await
    .expect("the episode runs");

    let asked = runner.asked();
    let opening = &asked[..3.min(asked.len())];
    for (speaker, prompt) in opening {
        if speaker == "planner" {
            continue;
        }
        assert!(
            !prompt.contains("rollback lever"),
            "`{speaker}` shares a round with the seat that pinned this, so the board \
             must withhold it:\n{prompt}"
        );
    }
}

#[tokio::test]
async fn a_pin_on_the_round_boundary_still_reaches_the_round() {
    // The regression guard for the off-by-one found in review: `readable` keeps
    // a row AT the boundary (`sequence <= round_start`) while the journal cursor
    // is strictly exclusive (`seq < before`). Bounding the board at `round_start`
    // rather than `round_start + 1` hid the boundary row from the board alone —
    // a pin reached the transcript and not the prompt, and only reappeared once
    // some later row moved the boundary past it.
    //
    // A width-one test cannot see this: it needs a round whose `round_start` is
    // an actual pinned row rather than `u64::MAX`.
    //
    // ⚠️ The pin has to be authored by the round's **last** speaker, so that its
    // row IS the boundary the next round folds at. Pinning from the first
    // speaker instead puts the row safely *below* `round_start`, where an
    // exclusive and an inclusive bound agree — verified by mutation: with the
    // pin on `planner` this test passed with the off-by-one still in place.
    // Members take the floor in desk order, so `critic` is last.
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(&wide_manifest(), "eng").expect("a room");
    let runner = Runner::new(&[
        ("planner", STAGE),
        ("scout", HALT),
        ("critic", "!pin #flag The flag is the rollback lever."),
        ("planner", "!support #halt ^3 Tuesday is safer."),
        ("scout", "!support #halt ^3 Agreed."),
        ("critic", "!support #halt ^3 Agreed."),
    ]);

    EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .run(trigger)
    .await
    .expect("the episode runs");

    let asked = runner.asked();
    assert!(
        asked.len() > 3,
        "this test needs a second round to exist: {asked:?}"
    );
    // ⚠️ Assert on the BOARD, not on the prompt as a whole. The pinned row is an
    // ordinary desk row too, so its text is in the next round's visible
    // transcript whatever the board does — a bare `prompt.contains(..)` passes
    // with the off-by-one still in place, verified by mutation. Only the block
    // `EpisodePrompt::board` renders answers the question this test is asking.
    //
    // ⚠️ And on the FIRST turn past the boundary, not `any()` of the turns after
    // it. Only this one folds at a `round_start` equal to the pinned row; every
    // later round folds above it, where an exclusive and an inclusive bound
    // agree and the pin is on the board either way. An `any()` over the tail
    // therefore passes with the off-by-one still in place — also verified by
    // mutation.
    let (speaker, first_past_boundary) = &asked[3];
    assert!(
        board_of(first_past_boundary).contains("rollback lever"),
        "`{speaker}` opens the round that folds AT the pinned row, so that pin is on \
         its board; bounding the board at `round_start` rather than \
         `round_start + 1` is exactly what drops it. Board was:\n{:?}",
        board_of(first_past_boundary)
    );
}

/// The `board()` block of a rendered prompt, or empty when it rendered none.
///
/// Scoping to this block is what keeps the two pin tests honest: a pinned row is
/// still an ordinary desk row, so its text reaches the transcript on its own
/// terms and a whole-prompt substring check cannot tell the board from it.
fn board_of(prompt: &str) -> String {
    prompt
        .split_once("Pinned on this desk, whatever else has scrolled away:")
        .map_or_else(String::new, |(_, rest)| rest.to_string())
}

// ---------------------------------------------------------------------------
// A failed turn does not take its round with it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_failed_turn_does_not_abandon_the_rest_of_its_round() {
    // Before the round existed, the failure path committed the turn's state and
    // `continue`d the *episode* loop, which ended that turn's round. A round's
    // remaining members are authorized independently of whether an earlier seat
    // answered, so the `continue` now has to continue the ROUND.
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(&wide_manifest(), "eng").expect("a room");
    let runner =
        Runner::new(&[("planner", STAGE), ("scout", HALT), ("critic", SHIP)]).failing("planner", 1);

    EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .run(trigger)
    .await
    .expect("a failed turn is not a failed episode");

    let spoke: Vec<String> = runner.asked().into_iter().map(|(id, _)| id).collect();
    for seat in ["scout", "critic"] {
        assert!(
            spoke.iter().any(|id| id == seat),
            "`planner` failed in the opening round; its round-mates were authorized \
             anyway and must still be asked: {spoke:?}"
        );
    }
    // The failed seat still leaves a durable row, which is what lets the round
    // count as fully appended.
    let replies = log.replies("eng");
    assert!(
        replies
            .iter()
            .any(|(author, _)| author == super::HIVE_FAILURE_AUTHOR),
        "a failed turn leaves its system row: {replies:?}"
    );
}
