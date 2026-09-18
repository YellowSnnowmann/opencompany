use super::*;
use crate::ports::tasks::TaskOrigin;
use crate::ports::tasks::TaskTitle;

fn plain_record() -> TaskRecord {
    TaskRecord {
        id: "t-1".to_string(),
        title: TaskTitle::authored("Draft the spec"),
        note: None,
        column: "todo".to_string(),
        priority: "medium".to_string(),
        assignee: "maya".to_string(),
        updated_at_millis: 7,
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    }
}

#[test]
fn a_card_raised_inside_a_thread_carries_its_thread_root_onto_the_card() {
    let mut record = plain_record();
    record.origin = TaskOrigin::new(Some("engineering".to_string()), Some(EventSeq::new(41)));
    let card = TaskCard::from(record);
    assert_eq!(card.origin_chat_id.as_deref(), Some("engineering"));
    assert_eq!(card.origin_parent, Some(EventSeq::new(41)));

    let json = serde_json::to_string(&card).expect("serializes");
    assert!(json.contains(r#""originChatId":"engineering""#), "{json}");
    assert!(json.contains(r#""originParent":41"#), "{json}");
}

/// A card raised straight into a channel carries no thread root, and the
/// field stays absent on the wire rather than serializing `null`.
#[test]
fn a_channel_level_origin_carries_no_thread_root() {
    let mut record = plain_record();
    record.origin = TaskOrigin::new(Some("engineering".to_string()), None);
    let card = TaskCard::from(record);
    assert_eq!(card.origin_parent, None);

    let json = serde_json::to_string(&card).expect("serializes");
    assert!(!json.contains("originParent"), "{json}");
}
