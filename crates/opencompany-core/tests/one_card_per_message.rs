//! One operator message opens **one** card — issue #463, on the company hive
//! (OC-2).
//!
//! Two independent paths can card the same message: the REST chat handler's
//! `detect_task_intent`, and the publish drain's minted card (#445). Each was
//! correct alone; together in one turn they once produced two cards, and the
//! reply linked to the one with no artifacts on it. Since OC-2 the turn that
//! answers is a coordinator turn on the hive's own task, so the drain runs in
//! the hive hooks' settle step rather than in a chat cycle — this proves the
//! invariant survived that move.
//!
//! No unit test can see it: the doubling only exists where the paths are
//! wired together. So this stands up a real host — real `FsOps` stores, real
//! platform auth, the production `server::router` on a real TCP socket — and
//! drives it over HTTP, with a scripted OpenAI-compatible endpoint keyed on the
//! coordinator turn's own prompt (`support::room::hive_turn`).
//!
//! Feature-gated on `openhuman`, which is what wires the harness pool, the
//! company hive and the hosted inference provider.
#![cfg(feature = "openhuman")]

mod support;

use std::time::Duration;

use serde_json::{Value, json};

use opencompany::company::task_intent::detect_task_intent;
use support::room::{HiveTurn, Room, answer, call, hive_script, settled};
use support::script_model::{Reply, spawn_script};

/// How long a test waits on the hive before it fails.
const WAIT: Duration = Duration::from_secs(60);

fn manifest(base_url: &str) -> String {
    format!(
        r#"
[company]
name = "Acme"
output = "Written deliverables"
human_role = "Steering"

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
allow = ["*"]

[[agent]]
id = "ceo"
role = "Chief Executive"
description = "Sets direction."
tier = "orchestrator"
tools = ["*"]

[[agent]]
id = "writer"
role = "Writer"
description = "Turns rough notes into short, clear written drafts."
tools = ["*"]

[[agent]]
id = "engineer"
role = "Engineer"
description = "Builds things."
tools = ["*"]

[[group_chat]]
id = "content"
name = "Content desk"
description = "Written drafts and copy."
members = ["writer"]

[[group_chat]]
id = "engineering"
name = "Engineering desk"
description = "How things are built."
members = ["engineer"]
"#
    )
}

/// A host whose model answers every coordinator turn with `act`.
async fn host(act: impl Fn(&HiveTurn) -> Reply + Send + Sync + 'static) -> (Room, tempfile::TempDir) {
    let (base_url, _script) = spawn_script(hive_script(act)).await;
    let home = tempfile::tempdir().expect("tempdir");
    let company = format!("acme-{}", uuid::Uuid::new_v4().simple());
    let room = Room::boot(home.path(), &company, &manifest(&base_url)).await;
    (room, home)
}

/// Puts a file in `agent`'s sandbox so it has something real to publish.
fn seed_file(room: &Room, home: &std::path::Path, agent: &str, name: &str, body: &str) {
    let dir = home
        .join("harness")
        .join(&room.company)
        .join(agent)
        .join("workspace");
    std::fs::create_dir_all(&dir).expect("sandbox");
    std::fs::write(dir.join(name), body).expect("seed file");
}

/// The script for "the writer publishes the memo and completes".
fn publishes(title: &'static str) -> impl Fn(&HiveTurn) -> Reply + Send + Sync + 'static {
    move |turn| match (turn.agent.as_str(), turn.called.as_slice()) {
        ("writer", []) if turn.episode.is_some() => call(
            "publish_artifact",
            json!({ "path": "memo.md", "title": title }),
        ),
        ("writer", [_]) => support::room::complete(turn, "Drafted and published it."),
        _ => answer(turn, "Noted."),
    }
}

/// Every card on the board.
async fn board(room: &Room) -> Vec<Value> {
    room.cards().await
}

fn describe(cards: &[Value]) -> String {
    cards
        .iter()
        .map(|c| {
            format!(
                "\n  - id={} column={} assignee={:?} title={:?}",
                c["id"].as_str().unwrap_or("?"),
                c["column"].as_str().unwrap_or("?"),
                c["assignee"].as_str().unwrap_or("?"),
                c["title"].as_str().unwrap_or("?"),
            )
        })
        .collect()
}

/// The titles of the artifacts filed on `task_id`.
async fn artifact_titles(room: &Room, task_id: &str) -> Vec<String> {
    let (status, body) = room.get(&format!("/tasks/{task_id}/artifacts")).await;
    assert_eq!(status, 200, "{body}");
    body.as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["title"].as_str().map(str::to_string))
        .collect()
}

/// Asserts the invariant: one card, it holds the delivered file, and it is
/// filed under the agent that published.
async fn assert_sole_card_carries(room: &Room, publisher: &str, artifact: &str) {
    let cards = board(room).await;
    assert_eq!(
        cards.len(),
        1,
        "expected exactly one card for one message:{}",
        describe(&cards)
    );
    let card = &cards[0];
    let id = card["id"].as_str().expect("card id");
    assert_eq!(
        artifact_titles(room, id).await,
        [artifact],
        "the one card must hold the deliverable, not an orphan beside it"
    );
    assert_eq!(
        card["assignee"].as_str(),
        Some(publisher),
        "the card belongs to the agent that published"
    );
}

/// The headline: a substantial ask the REST detector leaves alone, answered by
/// a desk member who publishes, opens exactly the one card the publish mints.
#[tokio::test(flavor = "multi_thread")]
async fn a_substantial_ask_that_publishes_opens_one_card() {
    assert!(
        detect_task_intent("assemble the Q3 board pack").is_none(),
        "this case must NOT reach the REST detector, or it proves something else"
    );
    let (room, home) = host(publishes("Q3 board memo")).await;
    seed_file(&room, home.path(), "writer", "memo.md", "# Q3 board memo\n");

    room.say("content", "assemble the Q3 board pack").await;
    room.episodes_settled(1, WAIT).await;
    assert_sole_card_carries(&room, "writer", "Q3 board memo").await;
}

/// A recognised imperative is carded by the chat handler before the hive sees
/// it; the publish then lands on that card instead of minting a second one.
#[tokio::test(flavor = "multi_thread")]
async fn a_recognised_imperative_that_publishes_opens_one_card() {
    let ask = "Write the launch memo for the content desk";
    assert!(
        detect_task_intent(ask).is_some(),
        "this case must reach the REST detector, or it proves something else"
    );
    let (room, home) = host(publishes("Launch memo")).await;
    seed_file(&room, home.path(), "writer", "memo.md", "# Launch memo\n");

    room.say("content", ask).await;
    let rows = room.episodes_settled(1, WAIT).await;
    assert_eq!(settled(&rows).len(), 1);
    let cards = board(&room).await;
    assert_eq!(
        cards.len(),
        1,
        "the handler's card and the publish's must be one:{}",
        describe(&cards)
    );
    let id = cards[0]["id"].as_str().expect("card id");
    assert_eq!(artifact_titles(&room, id).await, ["Launch memo"]);
}

/// The constraint that stops the fix becoming its own bug: asking a question
/// is not commissioning work, on the general line or a desk's.
#[tokio::test(flavor = "multi_thread")]
async fn a_trivial_question_opens_no_card() {
    for chat in ["general", "engineering"] {
        let (room, _home) = host(|turn| answer(turn, "All green.")).await;
        room.say(chat, "what's the status of the build?").await;
        room.episodes_settled(1, WAIT).await;
        let cards = board(&room).await;
        assert!(
            cards.is_empty(),
            "a question is not work ({chat}):{}",
            describe(&cards)
        );
    }
}

/// One episode may open at most three cards: the fourth and fifth `spawn_task`
/// calls of the same episode are refused in-turn rather than queued.
#[tokio::test(flavor = "multi_thread")]
async fn five_spawns_in_one_episode_open_three_cards() {
    let titles = [
        "Draft the memo",
        "Pull the numbers",
        "Check the pipeline",
        "Draft the press note",
        "Chart the churn",
    ];
    let (room, _home) = host(move |turn| {
        if turn.agent != "engineer" || turn.episode.is_none() {
            return answer(turn, "Noted.");
        }
        match titles.get(turn.called.len()) {
            Some(title) => call(
                "spawn_task",
                json!({ "title": title, "note": "Part of the plan." }),
            ),
            None if turn.called.len() == titles.len() => {
                support::room::complete(turn, "Handed out the work.")
            }
            None => Reply::Say("Done.".to_string()),
        }
    })
    .await;

    room.say("engineering", "split the launch plan into cards please")
        .await;
    room.episodes_settled(1, WAIT).await;
    let cards = board(&room).await;
    assert_eq!(
        cards.len(),
        3,
        "three spawns open three cards, two are refused:{}",
        describe(&cards)
    );
}
