//! Tests for the per-member move grammar, desk memory, and speaker diversity.
//!
//! Everything here is scripted through [`HiveTurnRunner`]: no model, no store,
//! no provider. A room whose members are handed exact lines is the only way to
//! assert that a *barred* move is corrected and then demoted — a live room
//! would be asserting the model's compliance rather than this host's
//! enforcement.

use std::sync::Arc;

use super::moves_fixtures_tests::*;
use super::test::{MemoryLog, desk_of, record};
use super::*;
use crate::ports::events::EventLog;

// ---------------------------------------------------------------------------
// A failed turn is not a failed room
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_members_failed_turn_does_not_end_the_episode() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(
        &manifest_with("hive = { turn_budget = 9, quorum = 2, blind_round = false }"),
        "eng",
    )
    .expect("a room");
    let runner = Runner::new(&[
        ("planner", "!propose #stage Stage the rollout."),
        (
            "critic",
            "!evidence #stage ^1 The last rollout took checkout down.",
        ),
        ("scout", "!support #stage ^3 Staging bounds the damage."),
        ("planner", "!commit #stage ^3 Recorded."),
        ("critic", "!commit #stage ^3 Recorded."),
        ("scout", "!commit #stage ^3 Recorded."),
    ])
    .failing("scout", 1);

    let outcome = EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .run(trigger)
    .await
    .expect("one member's turn timing out must not throw away the room's work");

    assert_eq!(outcome.failed_turns, 1, "{outcome:?}");
    assert!(
        matches!(&outcome.ending, EpisodeEnding::Converged { topic, .. } if topic == "stage"),
        "{outcome:?}"
    );
    // The miss is on the desk, authored by the room rather than by the member,
    // and carries no marker — so the fold reads nothing off it.
    let replies = log.replies("eng");
    let note = replies
        .iter()
        .find(|(author, text)| author == HIVE_FAILURE_AUTHOR && text.contains("did not finish"))
        .expect("the miss is journaled");
    assert!(note.1.contains("@scout's turn did not finish"), "{note:?}");
    assert!(note.1.contains("wall-clock ceiling"), "{note:?}");
    assert!(!note.1.trim_start().starts_with('!'), "{note:?}");
    assert!(
        outcome.summary().contains("1 turn did not finish"),
        "{}",
        outcome.summary()
    );
}

#[tokio::test]
async fn a_room_where_every_seat_fails_twice_over_stops() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(
        &manifest_with("hive = { turn_budget = 12, blind_round = false }"),
        "eng",
    )
    .expect("a room");
    let runner = Runner::new(&[])
        .failing("planner", 99)
        .failing("scout", 99)
        .failing("critic", 99);
    let failed = EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .run(trigger)
    .await;
    let error = failed.expect_err("a harness that is down is not a room having a bad turn");
    assert!(
        error.to_string().contains("turns in a row failed"),
        "{error}"
    );
    // Six failures for a desk of three: the cap is members x 2, not the budget.
    assert_eq!(
        log.replies("eng")
            .iter()
            .filter(|(author, _)| author == HIVE_FAILURE_AUTHOR)
            .count(),
        6,
        "the cap is members x 2"
    );
}

// ---------------------------------------------------------------------------
// Manifest validation
// ---------------------------------------------------------------------------

#[test]
pub(super) fn validation_rejects_an_unknown_move_kind_and_an_unknown_member() {
    let problems = record(&manifest_with(
        "hive = { moves = { scout = [\"support\", \"shout\"] } }",
    ))
    .manifest
    .validate();
    assert!(
        problems.iter().any(|problem| problem.contains("shout")),
        "{problems:?}"
    );

    let problems = record(&manifest_with(
        "hive = { moves = { nobody = [\"support\"] } }",
    ))
    .manifest
    .validate();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("`nobody`") && problem.contains("not a member")),
        "{problems:?}"
    );
}

#[test]
pub(super) fn a_table_that_names_nobody_for_commit_is_accepted() {
    // `commit` is not a gated move: the fold hands the Commit phase to whoever
    // the attention market picks, so a desk that could bar a seat from
    // recording a decision would reach quorum and then have nothing legal to
    // say. A table naming no committer is therefore an ordinary table.
    let problems = record(&manifest_with(
        "hive = { moves = { planner = [\"propose\"], scout = [\"support\"], \
         critic = [\"object\"] } }",
    ))
    .manifest
    .validate();
    assert!(
        !problems.iter().any(|problem| problem.contains("!commit")),
        "{problems:?}"
    );
    // And a table that does name one is accepted too — the entry describes
    // what the seat could already do, so it is ignored rather than refused.
    let problems = record(&manifest_with(
        "hive = { moves = { planner = [\"propose\", \"commit\"], scout = [\"support\"], \
         critic = [\"object\"] } }",
    ))
    .manifest
    .validate();
    assert!(
        !problems.iter().any(|problem| problem.contains("!commit")),
        "{problems:?}"
    );
}

#[test]
pub(super) fn validation_rejects_a_zero_cap() {
    for key in ["dominance_cap", "repetition_cap", "refutation_cap"] {
        let problems = record(&manifest_with(&format!("hive = {{ {key} = 0 }}")))
            .manifest
            .validate();
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains(&format!("hive.{key} = 0"))),
            "{key}: {problems:?}"
        );
    }
}

#[test]
pub(super) fn the_manifest_round_trips_the_new_keys() {
    let desk = desk_of(
        &manifest_with(
            "hive = { require_evidential = true, refutation_cap = 2, dominance_cap = 4, \
             repetition_cap = 1, moves = { scout = [\"support\", \"evidence\"] } }",
        ),
        "eng",
    )
    .expect("a room");
    assert_eq!(desk.config.require_evidential, Some(true));
    assert_eq!(desk.config.refutation_cap, Some(2));
    assert_eq!(desk.config.dominance_cap, Some(4));
    assert_eq!(desk.config.repetition_cap, Some(1));
    // The declared kinds, plus the three no table can take away.
    assert_eq!(
        desk.config.moves_for("scout"),
        vec!["support", "evidence", "question", "defer", "commit"]
    );
    assert_eq!(desk.config.moves_for("planner"), MOVE_KINDS.to_vec());
}
