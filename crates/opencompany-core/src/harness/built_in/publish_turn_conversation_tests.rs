use super::publish_turn_helpers_tests::NoopHost;
use std::sync::Arc;

use serde_json::Value;

use super::publish_turn_helpers_tests::*;
use crate::harness::publish::PUBLISH_ARTIFACT_TOOL;
use crate::ports::brain::Brain;
use crate::ports::tasks::{COLUMN_IN_REVIEW, TaskRecord, TaskStore};
use crate::ports::types::{CompanyEvent, CycleRequest};
use crate::store::FsOps;

// ---------------------------------------------------------------------------
// Issue #445: publishing outside a task run
// ---------------------------------------------------------------------------

/// An operator message, the shape a chat turn arrives as.
fn chat(text: &str) -> CycleRequest {
    CycleRequest {
        cycle_id: "cycle-1".to_string(),
        company_id: company(),
        events: vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            text: text.to_string(),
            by: None,
            chat: None,
            parent: None,
            deliverable: None,
            attachments: Vec::new(),
        }],
        event_seqs: Vec::new(),
        policy: None,
    }
}

/// Every card on the board, oldest-id first — a chat publish mints one, so the
/// tests have to find a card they did not create.
async fn all_cards(ops: &Arc<FsOps>) -> Vec<TaskRecord> {
    let mut cards = TaskStore::list(&**ops, &company()).await.expect("list");
    cards.sort_by(|a, b| a.id.cmp(&b.id));
    cards
}

/// **The headline for #445.** A model publishes during a *conversation* — no
/// card, no dispatch, no run — and the file must end up somewhere the operator
/// can open.
///
/// Before this the tool returned success, the queue was never drained, and the
/// next turn cleared it. Everything was green and the deliverable was gone, so
/// this asserts on the **stored artifact**, not on the receipt.
#[tokio::test]
async fn a_conversation_publish_is_recorded_on_a_card_it_mints() {
    let (base_url, _script) = spawn_script(vec![
        write("brief.md", "# Brief\nThe thing you asked for."),
        publish("brief.md"),
        Turn::Say("Written up and published."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());

    // No card exists, and none is dispatched — this is purely a chat turn.
    assert!(all_cards(&ops).await.is_empty());

    brain
        .run_cycle(chat("Draft me a brief."), &NoopHost)
        .await
        .expect("cycle runs");

    // A card was minted to carry the deliverable…
    let cards = all_cards(&ops).await;
    assert_eq!(
        cards.len(),
        1,
        "publishing in a conversation must mint exactly one card: {cards:?}"
    );
    let card = &cards[0];
    assert_eq!(
        card.column, COLUMN_IN_REVIEW,
        "finished agent work awaiting a person"
    );
    assert_eq!(card.assignee, AGENT);
    assert_eq!(card.title, "brief.md", "titled from what was published");
    assert!(
        card.note
            .as_deref()
            .unwrap_or_default()
            .contains("conversation"),
        "the card must explain why it exists: {:?}",
        card.note
    );
    // …and it is reachable by the one route the console actually reads.
    let artifacts = artifacts_on(&ops, &card.id).await;
    assert_eq!(artifacts.len(), 1, "the deliverable must be on the card");
    assert_eq!(artifacts[0].source.as_deref(), Some("brief.md"));
    assert_eq!(
        artifacts[0].versions[0].body, "# Brief\nThe thing you asked for.",
        "the stored body must be the file the agent wrote"
    );
}

/// The receipt the *model* was handed must name the destination it actually
/// got. This reads the wire, so it pins what the agent was told — the sentence
/// that, when wrong, is laundered into a false claim to the operator.
#[tokio::test]
async fn a_conversation_publish_receipt_does_not_promise_a_task_tab() {
    let (base_url, script) = spawn_script(vec![
        write("brief.md", "# Brief"),
        publish("brief.md"),
        Turn::Say("Published."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    brain
        .run_cycle(chat("Draft me a brief."), &NoopHost)
        .await
        .expect("cycle runs");
    let _ = &ops;

    // Find the tool result the model was sent back.
    let receipts: Vec<String> = script
        .seen
        .lock()
        .unwrap()
        .iter()
        .flat_map(|body| {
            body.get("messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
        .filter_map(|m| m.get("content").and_then(Value::as_str).map(str::to_string))
        .filter(|c| c.contains("Published `brief.md`"))
        .collect();

    assert!(
        !receipts.is_empty(),
        "the model was never handed a publish receipt"
    );
    for receipt in &receipts {
        assert!(
            !receipt.contains("this task's Artifacts tab"),
            "a chat turn has no task, so the receipt must not name one: {receipt}"
        );
        assert!(
            receipt.contains("card"),
            "the receipt must name where the file actually went: {receipt}"
        );
    }
}

/// The other half of the floor: with no artifact store there is no claim, so
/// the tool must **refuse** rather than stage into a queue nothing drains.
///
/// `build_agent` already declines to wire the tool without a store, so this
/// proves the two gates agree — the agent cannot publish, by either route.
#[tokio::test]
async fn a_conversation_without_an_artifact_store_cannot_publish() {
    let (base_url, script) = spawn_script(vec![
        write("brief.md", "# Brief"),
        publish("brief.md"),
        Turn::Say("Tried to publish."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain_without_artifacts(base_url, dir.path());

    brain
        .run_cycle(chat("Draft me a brief."), &NoopHost)
        .await
        .expect("cycle runs");

    assert!(
        all_cards(&ops).await.is_empty(),
        "nothing to record means no card is minted"
    );
    // The tool is not even advertised — the failure is closed at both ends.
    let advertised = advertised_tools(&script);
    assert!(
        !advertised.contains(&PUBLISH_ARTIFACT_TOOL.to_string()),
        "publish_artifact must not be offered without a store: {advertised:?}"
    );
}

/// A dispatched card must behave **exactly** as it did before #445 — the
/// artifact lands on the card that was dispatched, and no extra card appears.
#[tokio::test]
async fn a_task_run_still_files_onto_its_own_card_and_mints_nothing() {
    let (base_url, _script) = spawn_script(vec![
        write("launch.md", "# Launch spec"),
        publish("launch.md"),
        Turn::Say("Published."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();

    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("cycle runs");

    let cards = all_cards(&ops).await;
    assert_eq!(
        cards.len(),
        1,
        "a task run must not mint a second card: {cards:?}"
    );
    assert_eq!(cards[0].id, "t-1");
    assert_eq!(artifacts_on(&ops, "t-1").await.len(), 1);
}
