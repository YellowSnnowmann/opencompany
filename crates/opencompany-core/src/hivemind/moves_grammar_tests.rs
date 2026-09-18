//! Tests for the per-member move grammar, desk memory, and speaker diversity.
//!
//! Everything here is scripted through [`HiveTurnRunner`]: no model, no store,
//! no provider. A room whose members are handed exact lines is the only way to
//! assert that a *barred* move is corrected and then demoted — a live room
//! would be asserting the model's compliance rather than this host's
//! enforcement.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::moves_fixtures_tests::*;
use super::test::{MemoryLog, desk_of};
use super::*;
use crate::ports::events::EventLog;

// ---------------------------------------------------------------------------
// The grammar itself
// ---------------------------------------------------------------------------

#[test]
pub(super) fn an_unnamed_member_keeps_every_move() {
    let config = HiveConfig::default();
    assert_eq!(config.moves_for("planner"), MOVE_KINDS.to_vec());
    assert!(config.may("planner", "propose"));
    // And so does a member named with an empty list: a table somebody started
    // and never filled in must not silence a seat.
    let mut moves = BTreeMap::new();
    moves.insert("planner".to_owned(), Vec::new());
    let config = HiveConfig {
        moves,
        ..HiveConfig::default()
    };
    assert_eq!(config.moves_for("planner"), MOVE_KINDS.to_vec());
}

#[test]
pub(super) fn a_line_kind_reads_the_marker_and_folds_unpin_onto_pin() {
    assert_eq!(moves::line_kind("!support #a ^2 why"), Some("support"));
    assert_eq!(moves::line_kind("  !propose #a x"), Some("propose"));
    assert_eq!(moves::line_kind("!unpin ^2"), Some("pin"));
    assert_eq!(moves::line_kind("!shout at everyone"), None);
    assert_eq!(moves::line_kind("no marker here"), None);
}

#[test]
pub(super) fn demoting_keeps_the_words_and_loses_the_trace() {
    let demoted = moves::demote("!propose #ship Ship it all at once.");
    assert_eq!(demoted, "propose #ship Ship it all at once.");
    // The property the whole mechanism turns on: the fold reads nothing off it.
    let traces = tinyhivemind_hive::trace::resolve(
        &demoted,
        None,
        &tinyhivemind_hive::SessionAuthor::Agent {
            id: "planner".into(),
            label: "planner".into(),
        },
        tinyhivemind_hive::Sequence(4),
    );
    assert!(traces.is_empty(), "a demoted line must fold to nothing");
}

#[tokio::test]
async fn a_forbidden_move_is_re_prompted_once_and_then_demoted() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let manifest = manifest_with(
        "hive = { turn_budget = 4, quorum = 2, blind_round = false, \
         moves = { scout = [\"support\", \"evidence\", \"question\"] } }",
    );
    let desk = desk_of(&manifest, "eng").expect("a room");
    // Scout may not propose, and tries twice.
    let runner = Runner::new(&[
        ("planner", "!propose #stage Stage the rollout."),
        ("scout", "!propose #ship Ship it all at once."),
        ("scout", "!propose #ship I still say ship it."),
        ("critic", "!question What broke last time?"),
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
    .expect("a barred move never fails the episode");

    // Asked twice, and the second prompt carries the one-line correction.
    let prompts = runner.prompts_for("scout");
    assert!(prompts.len() >= 2, "scout must be re-prompted: {prompts:?}");
    assert!(
        prompts[1].contains("You may not `!propose`"),
        "the correction names the move: {}",
        prompts[1]
    );
    assert!(prompts[1].contains("!support"), "{}", prompts[1]);

    // The second attempt is journaled with its marker stripped.
    let replies = log.replies("eng");
    assert!(
        replies
            .iter()
            .any(|(author, text)| author == "scout" && text == "propose #ship I still say ship it."),
        "{replies:?}"
    );
    assert!(
        !replies
            .iter()
            .any(|(author, text)| author == "scout" && text.starts_with("!propose")),
        "no barred proposal may keep its marker: {replies:?}"
    );

    // And the operator is told, in the closing row.
    assert_eq!(outcome.violations.len(), 1, "{:?}", outcome.violations);
    assert_eq!(outcome.violations[0].agent_id, "scout");
    assert_eq!(outcome.violations[0].attempted, "propose");
    assert!(
        outcome.summary().contains("@scout !propose"),
        "{}",
        outcome.summary()
    );
}

#[tokio::test]
async fn a_corrected_member_that_complies_is_not_recorded_as_a_violation() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let manifest = manifest_with(
        "hive = { turn_budget = 3, quorum = 2, blind_round = false, \
         moves = { scout = [\"support\", \"evidence\"] } }",
    );
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(&[
        ("planner", "!propose #stage Stage the rollout."),
        ("scout", "!propose #ship Ship it."),
        (
            "scout",
            "!evidence #stage ^1 The last rollout took checkout down.",
        ),
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
    assert!(outcome.violations.is_empty(), "{:?}", outcome.violations);
    let replies = log.replies("eng");
    assert!(
        replies
            .iter()
            .any(|(author, text)| author == "scout" && text.starts_with("!evidence")),
        "{replies:?}"
    );
}

#[tokio::test]
async fn the_prompt_renders_only_the_moves_a_member_has() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let manifest = manifest_with(
        "hive = { turn_budget = 2, blind_round = false, \
         moves = { planner = [\"propose\", \"commit\"], scout = [\"object\", \"refute\"] } }",
    );
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(&[
        ("planner", "!propose #stage Stage it."),
        ("scout", "!object >1 ^1 That is slower."),
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

    let planner = runner.prompts_for("planner");
    let planner = planner.first().expect("planner spoke");
    assert!(planner.contains("!propose #topic"), "{planner}");
    // `commit` is phase-gated on top of the grammar, so its move line is absent
    // while the room deliberates even though the seat holds it. (The rules
    // block still names the marker, to say not to write it.)
    assert!(!planner.contains("!commit #topic ^N"), "{planner}");
    assert!(!planner.contains("!support #topic"), "{planner}");
    assert!(
        planner.contains("These are the ONLY markers this desk gives you"),
        "an assigned seat is told the grammar is assigned: {planner}"
    );

    let scout = runner.prompts_for("scout");
    let scout = scout.first().expect("scout spoke");
    assert!(scout.contains("!object >N ^M"), "{scout}");
    assert!(scout.contains("!refute #topic ^N"), "{scout}");
    assert!(!scout.contains("!propose #topic"), "{scout}");
}

// ---------------------------------------------------------------------------
// The quorum knobs
// ---------------------------------------------------------------------------

#[test]
pub(super) fn require_evidential_and_the_caps_reach_the_policy() {
    let config = HiveConfig {
        require_evidential: Some(true),
        refutation_cap: Some(2),
        dominance_cap: Some(4),
        repetition_cap: Some(1),
        ..HiveConfig::default()
    };
    let policy = HivePolicy::from_config(&config, 3).episode;
    assert!(policy.quorum.require_evidential);
    assert!(
        policy.quorum.require_grounded,
        "require_evidential implies require_grounded"
    );
    assert_eq!(policy.quorum.refutation_cap, Some(2));
    assert_eq!(policy.dominance_cap, 4);
    assert_eq!(policy.repetition_cap, 1);
    // The defaults are the library's own, unchanged.
    let default = HivePolicy::from_config(&HiveConfig::default(), 3).episode;
    assert!(!default.quorum.require_evidential);
    assert_eq!(default.quorum.refutation_cap, None);
    assert_eq!(default.dominance_cap, 50);
    assert_eq!(default.repetition_cap, 3);
}

#[tokio::test]
async fn under_require_evidential_a_proposal_plus_an_evidential_support_carries() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    // One proposer, and support that has to reach an `!evidence` to count.
    let manifest = manifest_with(
        "hive = { turn_budget = 9, quorum = 2, blind_round = false, require_evidential = true, \
         moves = { scout = [\"evidence\", \"support\", \"commit\", \"question\"], \
         critic = [\"evidence\", \"support\", \"object\", \"commit\", \"question\"] } }",
    );
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(&[
        (
            "planner",
            "!propose #stage Stage the rollout behind a flag.",
        ),
        (
            "scout",
            "!evidence #stage ^1 The last full rollout took checkout down for 40 minutes.",
        ),
        (
            "critic",
            "!support #stage ^3 The outage is the reason to stage.",
        ),
        ("planner", "!commit #stage ^3 Recorded."),
        ("scout", "!commit #stage ^3 Recorded."),
        ("critic", "!commit #stage ^3 Recorded."),
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
        "an evidential chain carries: {outcome:?}"
    );
}

/// **The evidential retry is graded against the seat's moves too.**
///
/// `line_from` enforces `moves_for` on a member's first reply and on the
/// move-correction retry, then hands whatever clears that off to `grounded`,
/// which may send the member back once more for citation discipline. Before
/// this test's fix, whatever `grounded`'s retry answered was journaled
/// as-is with no re-check at all — a member could answer the
/// citation-correction prompt with a marker kind its seat is not entitled to
/// make, and it folded into the transcript as a legitimate move. Here critic
/// may not `!propose`; its first `!support` cites the proposal rather than the
/// evidence line (missing evidence, so `grounded` retries it), and the retry
/// answer switches straight to `!propose`.
#[tokio::test]
async fn an_evidential_retry_is_still_graded_against_the_seats_moves() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let manifest = manifest_with(
        "hive = { turn_budget = 9, quorum = 2, blind_round = false, require_evidential = true, \
         moves = { scout = [\"evidence\", \"support\", \"commit\", \"question\"], \
         critic = [\"evidence\", \"support\", \"object\", \"commit\", \"question\"] } }",
    );
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(&[
        (
            "planner",
            "!propose #stage Stage the rollout behind a flag.",
        ),
        (
            "scout",
            "!evidence #stage ^1 The last full rollout took checkout down for 40 minutes.",
        ),
        (
            "critic",
            // Cites the proposal (^1), not the evidence (^2) — misses evidence,
            // which sends critic back for the evidential retry.
            "!support #stage ^1 The proposal alone is reason enough.",
        ),
        (
            "critic",
            // The evidential retry's answer: a move barred for this seat.
            "!propose #rush Ship immediately without staging.",
        ),
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
    let replies = log.replies("eng");
    let critic_retry = replies
        .iter()
        .find(|(author, text)| author == "critic" && text.contains("Ship immediately"))
        .map(|(_, text)| text.clone())
        .expect("critic's evidential retry answer is journaled somewhere");
    assert!(
        !critic_retry.starts_with('!'),
        "a barred move on the evidential retry must be demoted, not journaled as a legitimate \
         move: {critic_retry:?} (outcome: {outcome:?})"
    );
    assert_eq!(
        outcome.violations.len(),
        1,
        "the evidential retry's barred move must be reported as a seat violation too: {outcome:?}"
    );
}

#[tokio::test]
async fn two_bare_proposals_of_one_topic_do_not_carry_when_only_one_seat_may_propose() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    // The live failure, in miniature: two members both `!propose #answer`, and
    // in tinyhivemind a proposal counts as its author's own support — so
    // without a move grammar the topic reaches a quorum of two on nobody having
    // read anybody. Here only planner may propose, so scout's proposal is
    // demoted and deposits nothing.
    // `scout` holds `support` so the desk can reach its quorum of two at all —
    // `desk_episode` now declines a room whose eligible supporters are fewer
    // than its quorum, and `manifest.rs` refuses the same shape outright. That
    // is orthogonal to what this test asserts: `scout` still may not
    // `!propose`, which is the whole claim, and the script never has it
    // `!support` anything, so the topic still carries nothing.
    let manifest = manifest_with(
        "hive = { turn_budget = 4, quorum = 2, blind_round = false, \
         moves = { scout = [\"question\", \"evidence\", \"support\"], \
         critic = [\"question\", \"evidence\"] } }",
    );
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(&[
        ("planner", "!propose #answer 233168"),
        ("scout", "!propose #answer 233168"),
        ("scout", "!propose #answer 233168"),
        ("critic", "!question Has anybody checked the bound?"),
        ("planner", "!question Anybody?"),
    ]);
    let outcome = EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Sum the multiples of 3 or 5 below 1000.",
    )
    .run(trigger)
    .await
    .expect("the episode runs");
    assert!(
        !matches!(outcome.ending, EpisodeEnding::Converged { .. }),
        "a demoted second proposal must not complete a quorum: {outcome:?}"
    );
    assert_eq!(outcome.violations.len(), 1, "{:?}", outcome.violations);
}
