//! Tests for the per-member move grammar, desk memory, and speaker diversity.
//!
//! Everything here is scripted through [`HiveTurnRunner`]: no model, no store,
//! no provider. A room whose members are handed exact lines is the only way to
//! assert that a *barred* move is corrected and then demoted — a live room
//! would be asserting the model's compliance rather than this host's
//! enforcement.

use std::sync::Arc;

use super::memory::HiveMemory;
use super::moves_fixtures_tests::*;
use super::test::{MemoryLog, desk_of};
use super::*;
use crate::ports::events::EventLog;

// ---------------------------------------------------------------------------
// Desk memory
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_recall_block_reaches_every_prompt_of_the_episode() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(
        &manifest_with("hive = { turn_budget = 2, blind_round = false }"),
        "eng",
    )
    .expect("a room");
    let memory = Arc::new(ScriptedMemory::with_hits(&[
        "Decide the rollout — #stage\n\nCarried: #stage\nSupporters: planner, scout",
    ]));
    let runner = Runner::new(&[
        ("planner", "!propose #stage Stage it."),
        ("scout", "!support #stage ^1 Agreed."),
    ]);
    EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .with_memory(Arc::clone(&memory) as Arc<dyn HiveMemory>)
    .run(trigger)
    .await
    .expect("the episode runs");

    let asked = runner.asked();
    assert!(!asked.is_empty());
    for (agent, prompt) in &asked {
        assert!(
            prompt.contains("The desk remembers:"),
            "{agent} was not shown the recall block:\n{prompt}"
        );
        assert!(prompt.contains("Carried: #stage"), "{prompt}");
        // Attributed as memory, never as a transcript line: it carries no
        // sequence, so nothing can cite it.
        assert!(
            prompt.contains("cannot be cited with ^"),
            "the block must say what it is: {prompt}"
        );
    }
}

#[tokio::test]
async fn a_converged_episode_writes_exactly_one_note_with_the_topic_and_the_evidence() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(
        &manifest_with("hive = { turn_budget = 9, quorum = 2, blind_round = false }"),
        "eng",
    )
    .expect("a room");
    let memory = Arc::new(ScriptedMemory::default());
    let runner = Runner::new(&[
        (
            "planner",
            "!propose #stage Stage the rollout behind a flag.",
        ),
        (
            "critic",
            "!evidence #stage ^1 The last full rollout took checkout down.",
        ),
        (
            "scout",
            "!support #stage ^3 Staging bounds the blast radius.",
        ),
        ("planner", "!pin ^3 Keep the outage on the board."),
        ("critic", "!commit #stage ^3 The room settled on staging."),
        ("scout", "!commit #stage ^3 Recorded."),
        ("planner", "!commit #stage ^3 Recorded."),
    ]);
    let outcome = EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.\nSecond line the note must not carry.",
    )
    .with_memory(Arc::clone(&memory) as Arc<dyn HiveMemory>)
    .run(trigger)
    .await
    .expect("the episode runs");
    assert!(matches!(outcome.ending, EpisodeEnding::Converged { .. }));

    let notes = memory.notes();
    assert_eq!(notes.len(), 1, "exactly one note per episode: {notes:?}");
    let note = &notes[0];
    assert_eq!(note.desk_id, "eng");
    assert!(note.title.starts_with("Decide the rollout."), "{note:?}");
    assert!(!note.title.contains("Second line"), "{note:?}");
    assert!(note.body.contains("Carried: #stage"), "{}", note.body);
    assert!(note.body.contains("Supporters:"), "{}", note.body);
    assert!(
        note.body
            .contains("!evidence #stage ^1 The last full rollout"),
        "the evidence lines are the point of the note:\n{}",
        note.body
    );
    assert!(note.body.contains("Pinned:"), "{}", note.body);
    assert!(note.body.contains("Committed:"), "{}", note.body);
    // And the label it lands under is desk-scoped.
    assert!(note_label(&note.desk_id, &note.title).starts_with("hive/eng/"));
}

#[tokio::test]
async fn an_unresolved_episode_writes_a_short_note_naming_the_competing_topics() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(
        &manifest_with("hive = { turn_budget = 3, quorum = 3, blind_round = false }"),
        "eng",
    )
    .expect("a room");
    let memory = Arc::new(ScriptedMemory::default());
    let runner = Runner::new(&[
        ("planner", "!propose #stage Stage it."),
        ("scout", "!propose #ship Ship it."),
        ("critic", "!question Which is reversible?"),
    ]);
    let outcome = EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .with_memory(Arc::clone(&memory) as Arc<dyn HiveMemory>)
    .run(trigger)
    .await
    .expect("the episode runs");
    assert!(!matches!(outcome.ending, EpisodeEnding::Converged { .. }));
    let notes = memory.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].title.starts_with("Unresolved:"), "{:?}", notes[0]);
    assert!(notes[0].body.contains("#stage"), "{}", notes[0].body);
    assert!(notes[0].body.contains("#ship"), "{}", notes[0].body);
}

#[tokio::test]
async fn a_broken_memory_never_fails_the_episode() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let desk = desk_of(
        &manifest_with("hive = { turn_budget = 2, blind_round = false }"),
        "eng",
    )
    .expect("a room");
    let memory = Arc::new(ScriptedMemory {
        broken: true,
        ..ScriptedMemory::default()
    });
    let runner = Runner::new(&[
        ("planner", "!propose #stage Stage it."),
        ("scout", "!support #stage ^1 Agreed."),
    ]);
    let outcome = EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .with_memory(Arc::clone(&memory) as Arc<dyn HiveMemory>)
    .run(trigger)
    .await
    .expect("a store that is down is not a reason to refuse the operator");
    assert!(outcome.turns >= 1, "{outcome:?}");
    let asked = runner.asked();
    assert!(
        asked
            .iter()
            .all(|(_, prompt)| !prompt.contains("The desk remembers:")),
        "a failed recall renders no block"
    );
}

// ---------------------------------------------------------------------------
// Speaker diversity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_unspoken_line_appears_when_the_floor_comes_straight_back() {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    // `dominance_cap` and `repetition_cap` are the library's own damping; this
    // is the host's addition on top, and it never overrides the fold. A room of
    // one speaker is forced by giving everybody but the planner nothing to say
    // and letting the market hand the floor back.
    let manifest = manifest_with(
        "hive = { turn_budget = 6, quorum = 3, blind_round = false, dominance_cap = 50 }",
    );
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(&[]);
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

    // Whenever the same member is asked twice in a row while somebody has still
    // not spoken, that second prompt names the missing members.
    let asked = runner.asked();
    let repeated: Vec<usize> = asked
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0].0 == pair[1].0)
        .map(|(index, _)| index + 1)
        .collect();
    if repeated.is_empty() {
        // The fold rotated the floor on its own, which is the outcome the line
        // exists to encourage — nothing to assert, and nothing wrong.
        return;
    }
    let spoke_before: Vec<String> = asked[..repeated[0]]
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    let everybody_spoke = ["planner", "scout", "critic"]
        .iter()
        .all(|id| spoke_before.iter().any(|seen| seen == id));
    if everybody_spoke {
        return;
    }
    let (_, prompt) = &asked[repeated[0]];
    assert!(
        prompt.contains("Members who have not spoken yet:"),
        "a repeated speaker must be told who is missing:\n{prompt}"
    );
    assert!(prompt.contains("!defer #topic"), "{prompt}");
}

#[test]
pub(super) fn the_unspoken_block_is_rendered_from_the_builder() {
    // The rendering itself, asserted without depending on the market handing
    // the floor back — which is the library's decision, not this host's.
    let desk = desk_of(
        &manifest_with("hive = { turn_budget = 4, blind_round = false }"),
        "eng",
    )
    .expect("a room");
    let unspoken = vec!["scout".to_owned(), "critic".to_owned()];
    let prompt = EpisodePrompt::new(
        &desk.members[0],
        &desk,
        "Decide the rollout.",
        desk.policy().quorum,
        &[],
    )
    .with_unspoken(&unspoken);
    let turn = tinyhivemind_hive::HiveTurn {
        agent_id: "planner".into(),
        phase: tinyhivemind_hive::Phase::Deliberate,
        visibility: tinyhivemind_hive::Visibility::Full,
        reason: tinyhivemind_hive::BidReason::Salience,
        watermark: tinyhivemind_hive::Sequence(1),
        // A round of one: nothing was authored concurrently with this
        // turn, so the round boundary sits above every row and withholds
        // nothing — which is what made a width-one round bit-identical to
        // the sequential episode this fixture was written against.
        round_start: tinyhivemind_hive::Sequence(u64::MAX),
    };
    let rendered = prompt.render(&turn, &[]);
    assert!(
        rendered.contains("Members who have not spoken yet: @scout, @critic"),
        "{rendered}"
    );
}
