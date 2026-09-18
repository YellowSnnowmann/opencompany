//! Tests for the [`EpisodeDriver`] host loop.

use std::sync::Arc;

use super::super::*;
use super::fixtures::*;
use super::log_adapter::seed_desk;
use crate::ports::events::EventLog;

#[tokio::test]
async fn a_scripted_room_converges_and_journals_the_right_authors() {
    let log = Arc::new(MemoryLog::default());
    let trigger = seed_desk(&log).await;
    let desk = desk_of(&three_member_manifest(), "eng").expect("a room");
    let runner = ScriptedRunner::new(&[
        (
            "planner",
            "!propose #stage Stage the rollout behind a flag.",
        ),
        ("scout", "!propose #ship Ship it all at once."),
        (
            "critic",
            "!evidence #stage ^3 The last full rollout took the checkout down.",
        ),
        (
            "planner",
            "!support #stage ^3 Staging bounds the blast radius.",
        ),
        ("scout", "!support #stage ^3 Agreed, and it is reversible."),
        ("critic", "!commit #stage ^3 The room settled on staging."),
        ("planner", "!commit #stage ^3 Recorded."),
        ("scout", "!commit #stage ^3 Recorded."),
    ]);

    let outcome = EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .run(trigger)
    .await
    .expect("the episode runs");

    assert!(
        matches!(&outcome.ending, EpisodeEnding::Converged { topic, .. } if topic == "stage"),
        "{outcome:?}"
    );
    assert!(outcome.turns >= 3, "{outcome:?}");
    assert!(outcome.first_seq.is_some() && outcome.last_seq.is_some());
    assert!(outcome.report_seq.is_some());

    let replies = log.replies("eng");
    // Every deliberation turn is authored by the teammate that took it, and the
    // one closing row by the reserved, unmintable outcome author.
    let (last_author, last_text) = replies.last().expect("a closing row");
    assert_eq!(last_author, HIVE_REPORT_AUTHOR);
    assert!(last_text.contains("#stage"), "{last_text}");
    assert!(
        replies[..replies.len() - 1]
            .iter()
            .all(|(author, _)| ["planner", "scout", "critic"].contains(&author.as_str())),
        "{replies:?}"
    );
    // One line per turn: what the room counts, never a paragraph that would
    // crowd out the transcript window on the next prompt.
    assert!(
        replies.iter().all(|(_, text)| !text.contains('\n')),
        "{replies:?}"
    );
}

#[tokio::test]
async fn the_blind_round_hides_peers_and_the_prompt_says_so() {
    let log = Arc::new(MemoryLog::default());
    let trigger = seed_desk(&log).await;
    let desk = desk_of(&three_member_manifest(), "eng").expect("a room");
    let runner = ScriptedRunner::new(&[
        ("planner", "!propose #stage Stage the rollout."),
        ("scout", "!propose #ship Ship it all at once."),
        ("critic", "!propose #wait Wait a week."),
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
    assert!(asked.len() >= 3, "{asked:?}");
    // The opening round is blind: the second speaker is told so, and the first
    // speaker's line is not in the transcript it was handed. The operator's own
    // message is — it is the task, and it predates the watermark.
    let (_, second) = &asked[1];
    assert!(
        second.contains("You cannot yet see your peers' positions")
            && second.contains("Put what you *know* on the floor"),
        "{second}"
    );
    assert!(
        !second.contains("#stage"),
        "a peer's position leaked:\n{second}"
    );
    assert!(second.contains("Decide the rollout."), "{second}");
    // And every seat is told who else is in the room, and what its own id is.
    let (first_agent, first) = &asked[0];
    assert!(
        first.contains(&format!("You are @{first_agent}")),
        "{first}"
    );
    assert!(first.contains("In the room with you: @"), "{first}");
    // **But NOT how to ask one of them, because this room cannot.** This
    // driver is built with no federation, so `consider` never runs for its
    // lines and an `@handle` here dispatches nothing — the same state a
    // referred room is in. Promising the move anyway sends a seat to a dead
    // end, which is the failure `peers()` avoids for desks.
    assert!(
        !first.contains("To get an ANSWER"),
        "a room that cannot dispatch must not be told the @handle asks anybody:\n{first}"
    );
}

#[test]
fn the_marker_line_is_what_the_room_keeps() {
    assert_eq!(
        marker_line("Here is my thinking.\n\n!support #stage ^3 It is reversible.\n\nThanks!"),
        "!support #stage ^3 It is reversible."
    );
    // ANSI escapes a tool's captured output may have left behind.
    assert_eq!(
        marker_line("\u{1b}[32m!propose #ship Ship it.\u{1b}[0m"),
        "!propose #ship Ship it."
    );
    // A turn that deposits no trace is still a legal turn.
    assert_eq!(marker_line("I am not sure yet."), "I am not sure yet.");
    assert_eq!(marker_line("   \n\n"), "(no answer)");
}
