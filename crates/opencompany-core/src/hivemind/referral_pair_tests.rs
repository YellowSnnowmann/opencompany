//! Tests for the pair leg of referral: a question put to a teammate rather
//! than to a desk, the private `dm:<a>+<b>` thread it runs in, and what each
//! seat reads back off it.

use std::sync::Arc;

use super::moves_fixtures_tests::Runner;
use super::referral_fixtures_tests::*;
use super::test::{MemoryLog, desk_of, record};
use super::*;
use crate::ports::events::EventLog;
use crate::ports::types::CompanyEvent;

/// The peer cap bounds questions that CROSS, and a teammate is not a crossing.
#[tokio::test]
async fn a_question_to_a_teammate_does_not_spend_the_peer_desk_allowance() {
    let far = FarDesk::answering("Answered.");
    let hive = "hive = { turn_budget = 8, quorum = 2, blind_round = false, \
                referral = { enabled = true, peer_cap = 1 } }";
    let (log, outcome) = run(
        hive,
        &[
            ("planner", "!question #a A peer desk question. @#platform"),
            ("scout", "!question #b A teammate question. @critic"),
            ("critic", "!propose #stage Stage it."),
            ("planner", "!support #stage ^3 Fine."),
            ("scout", "!commit #stage ^4 Recorded."),
        ],
        Some(&far),
    )
    .await;

    let crossed: Vec<_> = far
        .asked()
        .into_iter()
        .filter(|(conversation, _, _)| !conversation.starts_with("dm:"))
        .collect();
    assert_eq!(
        crossed.len(),
        1,
        "exactly one question left the desk: {crossed:?}"
    );
    let paired: Vec<_> = far
        .asked()
        .into_iter()
        .filter(|(conversation, _, _)| conversation.starts_with("dm:"))
        .collect();
    assert_eq!(
        paired.len(),
        1,
        "and the teammate was asked despite the cap being spent: {paired:?}"
    );
    assert_eq!(
        outcome.referrals.over_cap,
        0,
        "the teammate question was not refused: {}",
        outcome.summary()
    );

    let pair = super::referral::pair_conversation("scout", "critic");
    let said = log.replies(&pair);
    assert_eq!(
        said.len(),
        2,
        "the teammate exchange actually ran, in its own thread: {said:?}"
    );
    assert_eq!(said[0].0, "scout", "the asker speaks first: {said:?}");
}

/// A pair exchange that breaks mid-way keeps what was already said.
#[tokio::test]
async fn a_pair_exchange_that_breaks_keeps_the_rows_it_already_has() {
    const CHATTY: &str = "hive = { turn_budget = 8, quorum = 2, blind_round = false, \
                          referral = { enabled = true, pair_messages = 6 } }";
    let lines = &[
        (
            "planner",
            "!question #lag What is the replica lag budget? @sre",
        ),
        ("scout", "!propose #stage Stage the rollout behind a flag."),
        ("critic", "!support #stage ^1 Staging fits the lag budget."),
        ("planner", "!commit #stage ^3 Recorded."),
    ];
    let pair = super::referral::pair_conversation("planner", "sre");

    let falters = FarDesk::failing_after(1, "The replica lag budget is 400ms.");
    let (log, _) = run(CHATTY, lines, Some(&falters)).await;
    let said = log.replies(&pair);
    assert_eq!(
        said.len(),
        2,
        "a failed follow-up ends the exchange and keeps the rest: {said:?}"
    );
    assert!(said[1].1.contains("400ms"), "{said:?}");

    let silent = FarDesk::silent_after(1, "The replica lag budget is 400ms.");
    let (log, _) = run(CHATTY, lines, Some(&silent)).await;
    let said = log.replies(&pair);
    assert_eq!(
        said.len(),
        2,
        "an empty follow-up is not written as a row: {said:?}"
    );
    assert!(
        said.iter().all(|(_, text)| !text.trim().is_empty()),
        "no blank row reaches the thread: {said:?}"
    );
}

/// The asker's next turn is handed the exchange it just had, and no older one.
#[tokio::test]
async fn the_asker_is_handed_the_exchange_it_just_had_and_nothing_older() {
    const CHATTY: &str = "hive = { turn_budget = 8, quorum = 2, blind_round = false, \
                          referral = { enabled = true, pair_messages = 4 } }";
    let log = Arc::new(MemoryLog::default());

    let pair = super::referral::pair_conversation("planner", "sre");
    for (who, text) in [
        ("planner", "an ancient and unrelated question"),
        ("sre", "an ancient and unrelated answer"),
    ] {
        log.append(
            &MemoryLog::company(),
            CompanyEvent::AgentReply {
                audience: Vec::new(),
                chat_id: pair.clone(),
                agent_id: who.to_owned(),
                text: text.to_owned(),
                steps: Vec::new(),
                task_id: None,
                outputs: Vec::new(),
                parent: None,
                mentions: Vec::new(),
                mention_depth: 0,
                episode: None,
            },
        )
        .await
        .expect("the older exchange is journaled");
    }

    let trigger = open(&log).await;
    let manifest = two_desks(CHATTY);
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(&[
        (
            "planner",
            "!question #lag What is the replica lag budget? @sre",
        ),
        ("planner", "!propose #stage Stage it behind a flag."),
        ("scout", "!support #stage ^2 Fine."),
        ("critic", "!commit #stage ^3 Recorded."),
    ]);
    let federation = desk_federation(&record(&manifest), &desk).expect("a federation");
    let far = FarDesk::answering("The replica lag budget is 400ms.");
    EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .with_federation(federation, &far)
    .run(trigger)
    .await
    .expect("the episode runs");

    let continuation = runner
        .asked()
        .into_iter()
        .filter(|(who, _)| who == "planner")
        .nth(1)
        .map(|(_, prompt)| prompt)
        .expect("the asker speaks again after its question");

    assert!(
        continuation.contains("400ms"),
        "the answer it was given is in front of it: {continuation}"
    );
    assert!(
        continuation.contains("Your exchange with @sre"),
        "and labelled as the exchange it was: {continuation}"
    );
    assert!(
        !continuation.contains("ancient and unrelated"),
        "but not every other exchange this pair has ever had: {continuation}"
    );
}

/// A seat reads its own agent-to-agent history, on both sides of it, and
/// nobody else's.
#[tokio::test]
async fn a_seat_reads_the_private_lines_it_is_part_of() {
    let log = Arc::new(MemoryLog::default());
    let pair = super::referral::pair_conversation("planner", "sre");
    for (who, text) in [
        ("planner", "what is the replica lag budget?"),
        ("sre", "400ms, measured at the edge"),
    ] {
        log.append(
            &MemoryLog::company(),
            CompanyEvent::AgentReply {
                audience: Vec::new(),
                chat_id: pair.clone(),
                agent_id: who.to_owned(),
                text: text.to_owned(),
                steps: Vec::new(),
                task_id: None,
                outputs: Vec::new(),
                parent: None,
                mentions: Vec::new(),
                mention_depth: 0,
                episode: None,
            },
        )
        .await
        .expect("the earlier exchange is journaled");
    }

    let trigger = open(&log).await;
    let manifest = two_desks(REFERRING);
    let record = record(&manifest);
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(&[
        (
            "planner",
            "!propose #stage Stage the rollout behind a flag.",
        ),
        ("scout", "!support #stage ^1 Fine."),
        ("critic", "!commit #stage ^2 Recorded."),
    ]);
    EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .with_context_desks(crate::hivemind::company_desks(&record))
    .run(trigger)
    .await
    .expect("the episode runs");

    let planner = runner
        .asked()
        .into_iter()
        .find(|(who, _)| who == "planner")
        .map(|(_, prompt)| prompt)
        .expect("planner takes a turn");

    assert!(
        planner.contains("400ms, measured at the edge"),
        "the asker keeps what it was told, past the turn it was told on:\n{planner}"
    );
    assert!(
        planner.contains("Your exchange with @sre"),
        "labelled by the teammate it was with:\n{planner}"
    );
    assert!(
        planner.contains("planner (you): what is the replica lag budget?"),
        "and its own question is attributed to it:\n{planner}"
    );

    let scout = runner
        .asked()
        .into_iter()
        .find(|(who, _)| who == "scout")
        .map(|(_, prompt)| prompt)
        .expect("scout takes a turn");
    assert!(
        !scout.contains("400ms, measured at the edge"),
        "somebody else's private line is not readable:\n{scout}"
    );
}
