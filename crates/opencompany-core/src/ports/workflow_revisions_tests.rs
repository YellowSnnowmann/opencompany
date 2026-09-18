use super::*;

#[test]
fn new_mints_a_distinct_id_each_time() {
    let a = WorkflowRevisionRecord::new("wf", "Greeter", "id = \"wf\"", 1);
    let b = WorkflowRevisionRecord::new("wf", "Greeter", "id = \"wf\"", 1);
    assert_ne!(a.id, b.id, "identical bodies must still get distinct ids");
    assert_eq!(a.workflow_id, "wf");
    assert_eq!(a.toml, "id = \"wf\"");
}

#[test]
fn a_record_round_trips_as_camel_case_json() {
    let rev = WorkflowRevisionRecord::new("wf", "Greeter", "id = \"wf\"", 7);
    let json = serde_json::to_string(&rev).unwrap();
    assert!(json.contains("\"workflowId\""), "{json}");
    assert!(json.contains("\"createdAtMillis\""), "{json}");
    assert_eq!(rev, serde_json::from_str(&json).unwrap());
}

#[test]
fn sort_is_newest_first_with_id_tiebreak() {
    // Three revisions, two sharing a millisecond — the burst-of-edits case.
    let mut revs = vec![
        WorkflowRevisionRecord {
            id: "a".to_string(),
            workflow_id: "wf".to_string(),
            name: "n".to_string(),
            toml: String::new(),
            created_at_millis: 10,
        },
        WorkflowRevisionRecord {
            id: "c".to_string(),
            workflow_id: "wf".to_string(),
            name: "n".to_string(),
            toml: String::new(),
            created_at_millis: 20,
        },
        WorkflowRevisionRecord {
            id: "b".to_string(),
            workflow_id: "wf".to_string(),
            name: "n".to_string(),
            toml: String::new(),
            created_at_millis: 20,
        },
    ];
    sort_newest_first(&mut revs);
    let ids: Vec<&str> = revs.iter().map(|r| r.id.as_str()).collect();
    // 20/c and 20/b outrank 10/a; within the tick the higher id wins.
    assert_eq!(ids, ["c", "b", "a"]);
}
