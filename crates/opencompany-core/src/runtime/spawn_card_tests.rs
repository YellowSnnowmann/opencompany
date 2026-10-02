use super::*;
use crate::ports::types::EventSeq;

#[test]
fn a_spawned_card_opens_in_todo_carrying_its_provenance() {
    let card = SpawnCard {
        title: "Draft the launch post".to_string(),
        note: Some("Two paragraphs.".to_string()),
        assignee: "writer".to_string(),
        origin: TaskOrigin::new(Some("studio".to_string()), Some(EventSeq::new(7))),
        parent_task_id: None,
        origin_run_id: None,
        origin_workflow_id: None,
        opened_by: Some(TaskOpener {
            agent_id: "writer".to_string(),
            episode_id: Some("ep-1".to_string()),
        }),
    }
    .into_record();
    assert_eq!(card.title, "Draft the launch post");
    assert_eq!(card.column, COLUMN_TODO);
    assert_eq!(card.priority, "medium");
    assert_eq!(card.assignee, "writer");
    assert!(card.deliverable.is_once());
    assert_eq!(card.origin_chat_id(), Some("studio"));
    assert_eq!(card.origin_parent(), Some(EventSeq::new(7)));
    assert_eq!(
        card.opened_by.as_ref().map(|by| by.agent_id.as_str()),
        Some("writer")
    );
    assert!(card.origin_message_seq.is_none());
    assert!(!card.id.is_empty());
}

#[test]
fn opened_by_round_trips_and_is_absent_from_the_wire_when_unset() {
    let mut card = SpawnCard {
        title: "A card".to_string(),
        ..SpawnCard::default()
    }
    .into_record();
    let bare = serde_json::to_value(&card).unwrap();
    assert!(bare.get("openedBy").is_none(), "{bare}");

    card.opened_by = Some(TaskOpener {
        agent_id: "ceo".to_string(),
        episode_id: Some("ep-9".to_string()),
    });
    let wire = serde_json::to_value(&card).unwrap();
    assert_eq!(wire["openedBy"]["agentId"], "ceo");
    assert_eq!(wire["openedBy"]["episodeId"], "ep-9");
    let back: TaskRecord = serde_json::from_value(wire).unwrap();
    assert_eq!(back, card);
}
