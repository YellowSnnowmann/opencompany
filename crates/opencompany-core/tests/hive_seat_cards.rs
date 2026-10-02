#![cfg(feature = "openhuman")]
//! **End-to-end: HiveMind seats open task cards** on a desk of four.
//!
//! Every case boots a real company (`support::room`) and sends one message to
//! the `studio` desk through the chat route with a platform credential. Only
//! the model is scripted: a seat turn is read off the episode brief, and the
//! script calls `spawn_task`, the file tools and the speech tools the seat's
//! own belt carries. What is asserted is the board the console reads.
//!
//! | Test | Claim |
//! | --- | --- |
//! | `a_spawn_and_a_publish_make_one_card_that_links_back_and_names_its_opener` | the publish lands on the seat's card; the card points at the desk and thread, and says who opened it in which episode |
//! | `the_first_card_takes_over_the_card_the_message_opened` | a To-do card the chat handler opened for the message is taken over rather than duplicated |
//! | `two_publishers_share_one_card` | two seats' publishes land on the one card the first minted |
//! | `a_question_opens_no_card` | a message read as a question refuses `spawn_task` in the seat's turn |
//! | `five_spawns_open_three_cards` | the episode's budget refuses the fourth and fifth in-turn |
//! | `the_same_title_from_two_seats_is_one_card` | the second seat is refused as a duplicate |
//! | `a_spawn_before_an_approval_park_is_written_once` | a seat that parks after queuing a card has it written at the park, and not again on resume |

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::room::{Room, Seat, ask, call, complete, operator_message, room_script};
use support::script_model::{Reply, spawn_script_with_latency};

use opencompany::ports::types::{CompanyEvent, StoredEvent};

const STUDIO: &str = "studio";
const CEO: &str = "ceo";
const WRITER: &str = "writer";
const ROLES: &[(&str, &str)] = &[
    (CEO, "Chief Executive"),
    (WRITER, "Writer"),
    ("analyst", "Analyst"),
    ("engineer", "Engineer"),
];
const EPISODE: Duration = Duration::from_secs(90);
const REQUEST: &str = "Write the launch post and get it ready to ship.";

fn manifest(name: &str, base_url: &str) -> String {
    format!(
        r#"
[company]
name = "{name}"
summary = "Proves seats open cards."

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

[users]
admins = ["operator@opencompany.local"]

[[agent]]
id = "ceo"
role = "Chief Executive"
tier = "orchestrator"

[[agent]]
id = "writer"
role = "Writer"

[[agent]]
id = "analyst"
role = "Analyst"

[[agent]]
id = "engineer"
role = "Engineer"

[[group_chat]]
id = "{STUDIO}"
name = "Studio"
description = "Launch work."
members = ["ceo", "writer", "analyst", "engineer"]

[group_chat.routing]
round_width = 2
"#
    )
}

async fn room(
    home: &std::path::Path,
    act: impl Fn(&Seat) -> Reply + Send + Sync + 'static,
) -> (Room, Arc<support::script_model::Script>) {
    let (base_url, script) =
        spawn_script_with_latency(room_script(ROLES, act), Duration::from_millis(30)).await;
    let id = format!("seat-cards-{}", uuid::Uuid::new_v4().simple());
    let room = Room::boot(home, &id, &manifest(&id, &base_url)).await;
    (room, script)
}

fn spawn(title: &str) -> Reply {
    call(
        "spawn_task",
        json!({ "title": title, "note": "From the studio desk." }),
    )
}

fn write_and_publish(seat: &Seat, path: &'static str, title: &str) -> Option<Reply> {
    if !seat.called("file_write") {
        return Some(call(
            "file_write",
            json!({ "path": path, "content": format!("# {title}\n\nDraft.\n") }),
        ));
    }
    if !seat.called("publish_artifact") {
        return Some(call(
            "publish_artifact",
            json!({ "path": path, "title": title }),
        ));
    }
    None
}

/// Every `spawn_task` result any seat saw, across the run.
fn spawn_results(script: &support::script_model::Script) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for request in script.asks() {
        if let Some(seat) = support::room::seat_of(&request, ROLES) {
            for result in seat.results_of("spawn_task") {
                if !seen.iter().any(|known| known == result) {
                    seen.push(result.to_string());
                }
            }
        }
    }
    seen
}

fn episode_id(rows: &[StoredEvent]) -> String {
    rows.iter()
        .find_map(|row| match &row.event {
            CompanyEvent::EpisodeOpened { episode_id, .. } => Some(episode_id.clone()),
            _ => None,
        })
        .expect("an episode opened")
}

async fn artifacts(room: &Room, card: &Value) -> Vec<Value> {
    let id = card["id"].as_str().unwrap();
    let (status, body) = room.get(&format!("/tasks/{id}/artifacts")).await;
    assert_eq!(status, 200, "{body}");
    body.as_array().cloned().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_and_a_publish_make_one_card_that_links_back_and_names_its_opener() {
    let home = tempfile::tempdir().unwrap();
    let (room, script) = room(home.path(), |seat| {
        if seat.speaker != CEO || seat.closing {
            return complete(seat, "Nothing to add.");
        }
        if !seat.called("spawn_task") {
            return spawn("Draft the launch post");
        }
        if let Some(next) = write_and_publish(seat, "launch-post.md", "Launch post") {
            return next;
        }
        complete(seat, "The launch post is drafted and published.")
    })
    .await;
    room.say(STUDIO, REQUEST).await;
    let rows = room.episodes_completed(1, EPISODE).await;

    let cards = room.cards().await;
    assert_eq!(cards.len(), 1, "{cards:#?}");
    let card = &cards[0];
    assert_eq!(card["title"], "Draft the launch post");
    assert_eq!(card["originChatId"], STUDIO);
    assert_eq!(
        card["originParent"].as_u64(),
        operator_message(&rows, STUDIO),
        "the card answers in the thread the message opened"
    );
    assert_eq!(card["openedBy"]["agentId"], CEO);
    assert_eq!(card["openedBy"]["episodeId"], episode_id(&rows));
    assert_eq!(
        artifacts(&room, card).await.len(),
        1,
        "the publish is on it"
    );
    let receipts = spawn_results(&script);
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt.contains("Do not describe it as open yet")),
        "{receipts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_card_takes_over_the_card_the_message_opened() {
    let home = tempfile::tempdir().unwrap();
    let (room, _script) = room(home.path(), |seat| {
        if seat.speaker != CEO || seat.closing {
            return complete(seat, "Nothing to add.");
        }
        if !seat.called("spawn_task") {
            return spawn("Draft the launch post");
        }
        complete(seat, "Tracked.")
    })
    .await;
    room.say_with(STUDIO, REQUEST, json!({ "deliverable": "workflow" }))
        .await;
    room.episodes_completed(1, EPISODE).await;

    let cards = room.cards().await;
    assert_eq!(cards.len(), 1, "one message, one card: {cards:#?}");
    let card = &cards[0];
    assert_ne!(
        card["title"], "Draft the launch post",
        "it is the message's card"
    );
    assert_eq!(card["assignee"], CEO);
    assert_eq!(card["openedBy"]["agentId"], CEO);
    assert!(
        card["note"]
            .as_str()
            .unwrap_or_default()
            .contains("Tracking this as \"Draft the launch post\""),
        "{card:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_publishers_share_one_card() {
    let home = tempfile::tempdir().unwrap();
    let asked = Arc::new(AtomicBool::new(false));
    let (room, _script) = room(home.path(), move |seat| {
        if seat.closing {
            return complete(seat, "Nothing to add.");
        }
        match seat.speaker.as_str() {
            CEO if seat.parent.is_none() => {
                if let Some(next) = write_and_publish(seat, "plan.md", "Launch plan") {
                    return next;
                }
                if !asked.swap(true, Ordering::SeqCst) {
                    return ask(seat, WRITER, "Please publish the launch copy.");
                }
                complete(seat, "Plan and copy are published.")
            }
            WRITER if seat.parent.is_some() => {
                if let Some(next) = write_and_publish(seat, "copy.md", "Launch copy") {
                    return next;
                }
                complete(seat, "The copy is published.")
            }
            _ => complete(seat, "Nothing to add."),
        }
    })
    .await;
    room.say(STUDIO, REQUEST).await;
    room.episodes_completed(1, EPISODE).await;

    let cards = room.cards().await;
    assert_eq!(cards.len(), 1, "{cards:#?}");
    assert_eq!(artifacts(&room, &cards[0]).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_opens_no_card() {
    let home = tempfile::tempdir().unwrap();
    let (room, script) = room(home.path(), |seat| {
        if seat.speaker == CEO && !seat.closing && !seat.called("spawn_task") {
            return spawn("Review the board");
        }
        complete(seat, "Here is what is on the board.")
    })
    .await;
    room.say(STUDIO, "What is on the board this week?").await;
    room.episodes_completed(1, EPISODE).await;

    assert!(room.cards().await.is_empty());
    let receipts = spawn_results(&script);
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt.contains("read as a question")),
        "{receipts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn five_spawns_open_three_cards() {
    let home = tempfile::tempdir().unwrap();
    let titles = ["Copy", "Visuals", "Pricing", "Press list", "Launch party"];
    let (room, script) = room(home.path(), move |seat| {
        if seat.speaker != CEO || seat.closing {
            return complete(seat, "Nothing to add.");
        }
        let done = seat.times("spawn_task");
        if done < titles.len() {
            return spawn(titles[done]);
        }
        complete(seat, "Tracked what the budget allowed.")
    })
    .await;
    room.say(STUDIO, REQUEST).await;
    room.episodes_completed(1, EPISODE).await;

    let cards = room.cards().await;
    assert_eq!(cards.len(), 3, "{cards:#?}");
    let refused = spawn_results(&script)
        .into_iter()
        .filter(|receipt| receipt.contains("already opened 3 cards"))
        .count();
    assert_eq!(refused, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_title_from_two_seats_is_one_card() {
    let home = tempfile::tempdir().unwrap();
    let asked = Arc::new(AtomicBool::new(false));
    let (room, script) = room(home.path(), move |seat| {
        if seat.closing {
            return complete(seat, "Nothing to add.");
        }
        match seat.speaker.as_str() {
            CEO if seat.parent.is_none() => {
                if !seat.called("spawn_task") && !asked.load(Ordering::SeqCst) {
                    return spawn("Draft the launch post");
                }
                if !asked.swap(true, Ordering::SeqCst) {
                    return ask(seat, WRITER, "Can you take the launch post?");
                }
                complete(seat, "Tracked once.")
            }
            WRITER if seat.parent.is_some() => {
                if !seat.called("spawn_task") {
                    return spawn("draft the launch post!");
                }
                complete(seat, "It is already tracked.")
            }
            _ => complete(seat, "Nothing to add."),
        }
    })
    .await;
    room.say(STUDIO, REQUEST).await;
    room.episodes_completed(1, EPISODE).await;

    assert_eq!(room.cards().await.len(), 1);
    assert!(
        spawn_results(&script)
            .iter()
            .any(|receipt| receipt.contains("already open or queued")),
        "{:?}",
        spawn_results(&script)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_before_an_approval_park_is_written_once() {
    let home = tempfile::tempdir().unwrap();
    let spawned = Arc::new(AtomicBool::new(false));
    let (room, _script) = room(home.path(), move |seat| {
        if seat.speaker != CEO || seat.closing {
            return complete(seat, "Nothing to add.");
        }
        if seat.prompt.contains("approved your request") {
            return complete(seat, "Approved; the card is tracked.");
        }
        if !spawned.swap(true, Ordering::SeqCst) {
            return spawn("Email the launch partners");
        }
        if !seat.called("request_approval") {
            return call(
                "request_approval",
                json!({
                    "title": "Email the partners",
                    "question": "May I email the launch partners?"
                }),
            );
        }
        Reply::Say("done".to_string())
    })
    .await;
    room.say(STUDIO, REQUEST).await;

    let started = Instant::now();
    let approval = loop {
        if let Some(found) = room
            .approvals()
            .await
            .into_iter()
            .find(|approval| approval.get("episode").is_some())
        {
            break found;
        }
        assert!(started.elapsed() < EPISODE, "no seat approval appeared");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    room.wait_for("the seat's park row", EPISODE, |rows| {
        rows.iter()
            .any(|row| matches!(row.event, CompanyEvent::EpisodeSeatParked { .. }))
    })
    .await;
    assert_eq!(room.cards().await.len(), 1, "written when the turn parked");

    let id = approval["id"].as_str().unwrap();
    let (status, body) = room
        .post(
            &format!("/approvals/{id}"),
            json!({ "verdict": "approve", "detach": true }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    room.episodes_completed(1, EPISODE).await;

    assert_eq!(room.cards().await.len(), 1, "and not again on resume");
}
