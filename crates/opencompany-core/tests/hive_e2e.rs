#![cfg(feature = "openhuman")]
//! **End-to-end: the company hive over the embedded OpenHuman runtime** (OC-2).
//!
//! The unit tests under `src/hive/` pin the routing, the reach policy, the
//! storage port and the projector one at a time; they cannot tell you whether
//! a *company* answers through its hive: whether an operator line on a desk is
//! accepted by the company's Coordinator with the starter its routing named,
//! whether that starter's turn runs as an ordinary harness turn with the
//! `hivemind_*` tools on its belt, whether what it says is projected back into
//! the journal the console and `opencompany measure` fold, and whether a
//! teammate reached by direct message answers without the two ever running
//! the same agent twice at once.
//!
//! So every test here boots a **real company** — `RuntimeBuilder`, the
//! process-wide `openhuman_embed::Runtime`, the filesystem store, the HTTP
//! surface — and drives it through `POST /api/v1/companies/{id}/chat`. Only
//! the model is scripted (`support::script_model`), keyed on the prompt the
//! OpenHuman host renders for a coordinator turn (`support::room::hive_turn`).
//!
//! # What each test proves
//!
//! | Test | Claim |
//! | --- | --- |
//! | `a_desk_line_is_answered_by_the_starter_its_routing_named` | `HiveAccepted` names a desk member as starter; its turn is bracketed with the hive; its completion settles the episode |
//! | `a_desk_of_one_answers_with_one_turn` | a one-member desk starts its only member and settles cleanly |
//! | `a_teammate_reached_directly_answers_its_sender` | `hivemind_send_agent` lands as a `HiveMessage` to the teammate, whose turn's reply goes back to the sender |
//! | `a_shared_agent_on_two_desks_never_runs_twice_at_once` | two desks sharing the CEO both settle; the CEO's turn brackets never overlap |
//! | `the_journal_measures_what_the_hive_did` | `measure_rows` folds the same run into episodes, turns, and a direct message |
//!
//! Every company gets a unique id: the runtime keeps one `Agent` per
//! `(company, agent)` for the life of the process, so two tests naming the
//! same company would share transcripts.

mod support;

use std::time::Duration;

use opencompany::ports::types::{
    CompanyEvent, CompanyId, EventSeq, HiveDestination, StoredEvent,
};
use support::room::{HiveTurn, Room, answer, hive_script, send_agent, settled};
use support::script_model::{Reply, spawn_script_with_latency};

const ENGINEERING: &str = "engineering";
const CONTENT: &str = "content";
const FRONT: &str = "front";
const ENGINEER: &str = "engineer";
const WRITER: &str = "writer";
const CEO: &str = "ceo";
const GREETER: &str = "greeter";

/// How long a test waits on the hive before it fails.
const WAIT: Duration = Duration::from_secs(60);

/// Two desks of two sharing the CEO, plus a desk of one.
///
/// `[policy] mode = "full"` so no tool call parks: this file is about the
/// hive, and a parked turn would hold an episode rather than fail it.
fn manifest(name: &str, base_url: &str) -> String {
    format!(
        r#"
[company]
name = "{name}"
summary = "Proves desks answer through the company hive."

[inference]
provider = "ollama"
base_url = "{base_url}"

[inference.models]
chat-v1 = "llama3"
reasoning-v1 = "llama3"
agentic-v1 = "llama3"

[policy]
mode = "full"

[tools]
allow = []

[[agent]]
id = "{CEO}"
role = "Chief Executive"
tier = "orchestrator"

[[agent]]
id = "{ENGINEER}"
role = "Engineer"

[[agent]]
id = "{WRITER}"
role = "Writer"

[[agent]]
id = "{GREETER}"
role = "Front desk"

[[group_chat]]
id = "{ENGINEERING}"
name = "Engineering"
description = "How things are built."
members = ["{ENGINEER}", "{CEO}"]

[group_chat.routing]
round_width = 2

[[group_chat]]
id = "{CONTENT}"
name = "Content"
description = "Written drafts and copy."
members = ["{WRITER}", "{CEO}"]

[group_chat.routing]
round_width = 2

[[group_chat]]
id = "{FRONT}"
name = "Front"
members = ["{GREETER}"]
"#
    )
}

/// A company id no other test in this process uses.
fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

/// Boots the in-test company against a script that answers with `act`.
async fn boot(
    home: &std::path::Path,
    act: impl Fn(&HiveTurn) -> Reply + Send + Sync + 'static,
) -> Room {
    let (base_url, _script) =
        spawn_script_with_latency(hive_script(act), Duration::from_millis(20)).await;
    let id = unique("hive-lab");
    Room::boot(home, &id, &manifest(&id, &base_url)).await
}

/// Who each accepted line on `chat` named to start it.
fn starters_on(rows: &[StoredEvent], chat: &str) -> Vec<Vec<String>> {
    rows.iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::HiveAccepted {
                chat_id, starters, ..
            } if chat_id == chat => Some(starters.clone()),
            _ => None,
        })
        .collect()
}

/// Every coordinator turn's bracket: `(agent, hive, started seq, closed seq)`.
fn brackets(rows: &[StoredEvent]) -> Vec<(String, String, u64, Option<u64>)> {
    let mut open: Vec<(String, String, String, u64)> = Vec::new();
    let mut out = Vec::new();
    for row in rows {
        match &row.event {
            CompanyEvent::TurnStarted {
                turn_id,
                agent_id: Some(agent),
                hive: Some(hive),
                ..
            } => open.push((
                turn_id.clone(),
                agent.clone(),
                hive.hive_id.clone().unwrap_or_default(),
                row.seq.value(),
            )),
            CompanyEvent::TurnSettled { turn_id, .. } | CompanyEvent::TurnFailed { turn_id, .. } => {
                if let Some(at) = open.iter().position(|(id, ..)| id == turn_id) {
                    let (_, agent, hive, started) = open.remove(at);
                    out.push((agent, hive, started, Some(row.seq.value())));
                }
            }
            _ => {}
        }
    }
    out.extend(
        open.into_iter()
            .map(|(_, agent, hive, started)| (agent, hive, started, None)),
    );
    out
}

/// What `agent` said on `chat`, in journal order.
fn replies(rows: &[StoredEvent], chat: &str, agent: &str) -> Vec<String> {
    rows.iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::AgentReply {
                chat_id,
                agent_id,
                text,
                ..
            } if chat_id == chat && agent_id == agent => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_desk_line_is_answered_by_the_starter_its_routing_named() {
    let home = tempfile::tempdir().unwrap();
    let room = boot(home.path(), |turn| {
        answer(turn, format!("{} has it in hand.", turn.agent))
    })
    .await;

    room.say(ENGINEERING, "How do we ship the new build?").await;
    let rows = room.episodes_settled(1, WAIT).await;

    let starters = starters_on(&rows, ENGINEERING);
    assert_eq!(starters.len(), 1, "one accepted line: {starters:?}");
    assert_eq!(starters[0].len(), 1, "one starter: {starters:?}");
    let starter = starters[0][0].clone();
    assert!(
        [ENGINEER, CEO].contains(&starter.as_str()),
        "the starter sits on the desk: {starter}"
    );
    let ran = brackets(&rows);
    assert!(
        ran.iter()
            .any(|(agent, hive, _, closed)| agent == &starter
                && hive == ENGINEERING
                && closed.is_some()),
        "the starter's turn is bracketed with the desk's hive: {ran:?}"
    );
    assert_eq!(
        settled(&rows),
        vec![(ENGINEERING.to_string(), None)],
        "the episode settled without failure"
    );
    let said = replies(&rows, ENGINEERING, &starter);
    assert!(
        said.iter().any(|text| text.contains("has it in hand")),
        "the completion is projected onto the desk as the starter's reply: {said:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_desk_of_one_answers_with_one_turn() {
    let home = tempfile::tempdir().unwrap();
    let room = boot(home.path(), |turn| answer(turn, "Welcome in.")).await;

    room.say(FRONT, "hello, anyone there?").await;
    let rows = room.episodes_settled(1, WAIT).await;

    assert_eq!(starters_on(&rows, FRONT), vec![vec![GREETER.to_string()]]);
    let ran: Vec<_> = brackets(&rows)
        .into_iter()
        .filter(|(_, hive, ..)| hive == FRONT)
        .collect();
    assert_eq!(ran.len(), 1, "one turn answers a desk of one: {ran:?}");
    assert!(
        replies(&rows, FRONT, GREETER)
            .iter()
            .any(|text| text.contains("Welcome in.")),
        "the greeter's answer reaches the desk"
    );
}

/// The engineer, mid-episode, asks the writer for copy directly; the writer's
/// turn answers it, and the answer goes back to the engineer.
fn asks_the_writer(turn: &HiveTurn) -> Reply {
    if turn.agent == ENGINEER && turn.episode.is_some() && !turn.acted() {
        return send_agent(turn, WRITER, "copy-1", "Can you draft the release note?");
    }
    if turn.agent == ENGINEER && turn.called == ["hivemind_send_agent"] {
        return answer(
            &HiveTurn {
                called: Vec::new(),
                ..turn.clone()
            },
            "Asked the writer for the release note.",
        );
    }
    if turn.agent == WRITER && turn.episode.is_none() {
        return Reply::Say("Here is the release note draft.".to_string());
    }
    answer(turn, format!("{} has nothing to add.", turn.agent))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_teammate_reached_directly_answers_its_sender() {
    let home = tempfile::tempdir().unwrap();
    let room = boot(home.path(), asks_the_writer).await;

    room.say(ENGINEERING, "@engineer ship the release").await;
    let rows = room
        .wait_for("the writer's answer to reach the engineer", WAIT, |rows| {
            rows.iter().any(|row| {
                matches!(&row.event, CompanyEvent::HiveMessage {
                    sender,
                    destination: HiveDestination::Agent(to),
                    ..
                } if sender == WRITER && to == ENGINEER)
            })
        })
        .await;

    assert!(
        rows.iter().any(|row| matches!(&row.event, CompanyEvent::HiveMessage {
            sender,
            destination: HiveDestination::Agent(to),
            text,
            ..
        } if sender == ENGINEER && to == WRITER && text.contains("release note"))),
        "the engineer's direct message is journaled"
    );
    let writer_turns: Vec<_> = rows
        .iter()
        .filter(|row| {
            matches!(&row.event, CompanyEvent::TurnStarted { agent_id: Some(a), .. } if a == WRITER)
        })
        .collect();
    assert!(
        !writer_turns.is_empty(),
        "the direct message ran a turn for the writer"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shared_agent_on_two_desks_never_runs_twice_at_once() {
    let home = tempfile::tempdir().unwrap();
    let room = boot(home.path(), |turn| {
        answer(turn, format!("{} answered.", turn.agent))
    })
    .await;

    room.say(ENGINEERING, "@ceo plan the launch").await;
    room.say(CONTENT, "@ceo approve the copy").await;
    let rows = room.episodes_settled(2, WAIT).await;

    let mut desks: Vec<String> = settled(&rows).into_iter().map(|(hive, _)| hive).collect();
    desks.sort();
    assert_eq!(desks, [CONTENT, ENGINEERING], "both desks settled");
    let ceo: Vec<_> = brackets(&rows)
        .into_iter()
        .filter(|(agent, ..)| agent == CEO)
        .collect();
    for (i, (_, _, a_start, a_end)) in ceo.iter().enumerate() {
        for (_, _, b_start, _) in &ceo[i + 1..] {
            let a_end = a_end.expect("every CEO turn closed");
            assert!(
                *b_start > a_end || *b_start < *a_start,
                "the CEO ran two turns at once: {ceo:?}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_journal_measures_what_the_hive_did() {
    let home = tempfile::tempdir().unwrap();
    let room = boot(home.path(), asks_the_writer).await;

    room.say(ENGINEERING, "@engineer ship the release").await;
    let rows = room
        .wait_for("the episode to settle and the writer to answer", WAIT, |rows| {
            !settled(rows).is_empty()
                && rows.iter().any(|row| {
                    matches!(&row.event, CompanyEvent::HiveMessage { sender, .. } if sender == WRITER)
                })
        })
        .await;

    let report = opencompany::hive::measure::measure_rows(
        &CompanyId::new(room.company.clone()),
        EventSeq::new(0),
        &rows,
    );
    assert!(report.episodes_opened >= 1, "{report:?}");
    assert!(report.episodes_completed >= 1, "{report:?}");
    assert_eq!(report.episodes_failed, 0, "{report:?}");
    assert!(report.direct_messages >= 1, "{report:?}");
    assert_eq!(report.same_agent_overlaps, 0, "{report:?}");
    assert!(
        report.starter_routes.values().sum::<usize>() >= 1,
        "the accepted line's route is counted: {report:?}"
    );
}
