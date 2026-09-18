use super::*;

#[test]
fn a_record_round_trips_as_camel_case_json() {
    let record = WorkflowRunOutputRecord {
        run_id: "run-1".to_string(),
        workflow_id: "greet".to_string(),
        at_millis: 42,
        nodes: serde_json::json!({ "ceo": { "items": ["hi"] } }),
        truncated: false,
        partial: false,
    };
    let json = serde_json::to_string(&record).unwrap();
    assert!(json.contains("\"runId\""), "{json}");
    assert!(json.contains("\"workflowId\""), "{json}");
    assert!(json.contains("\"atMillis\""), "{json}");
    assert_eq!(record, serde_json::from_str(&json).unwrap());
}

#[test]
fn partial_round_trips_and_defaults_false_for_a_pre_feature_payload() {
    // Issue #1008: a `partial` capture round-trips as camelCase.
    let record = WorkflowRunOutputRecord {
        run_id: "run-2".to_string(),
        workflow_id: "greet".to_string(),
        at_millis: 7,
        nodes: serde_json::json!({ "ceo": { "items": ["hi"] } }),
        truncated: false,
        partial: true,
    };
    let json = serde_json::to_string(&record).unwrap();
    assert!(json.contains("\"partial\":true"), "{json}");
    assert_eq!(record, serde_json::from_str(&json).unwrap());

    // A payload persisted before this field existed carries no `partial`
    // key; `#[serde(default)]` reads it back `false` — a pre-#1008 snapshot
    // is from a run that settled cleanly, so "not partial" is the honest
    // default.
    let pre_feature = serde_json::json!({
        "runId": "old-run",
        "workflowId": "greet",
        "atMillis": 1,
        "nodes": { "ceo": { "items": ["hi"] } },
        "truncated": false,
    });
    let decoded: WorkflowRunOutputRecord = serde_json::from_value(pre_feature).unwrap();
    assert!(
        !decoded.partial,
        "a pre-feature payload with no `partial` key must default to false"
    );
}

#[test]
fn a_small_map_is_unchanged_and_not_truncated() {
    let nodes = serde_json::json!({ "ceo": { "items": ["hello world"] } });
    let (bounded, truncated) = bound_node_output(&nodes);
    assert!(!truncated, "a small map must not be flagged");
    assert_eq!(bounded, nodes, "a small map must survive byte-for-byte");
}

#[test]
fn an_oversized_string_is_clipped_on_a_char_boundary_and_flagged() {
    // A string well past the per-item cap, ending in a multi-byte codepoint
    // to prove the clip never splits one (the byte-slice panic class).
    let long = "a".repeat(NODE_OUTPUT_ITEM_CHAR_CAP + 500) + "🚀🚀🚀";
    let nodes = serde_json::json!({ "writer": { "items": [long] } });

    let (bounded, truncated) = bound_node_output(&nodes);
    assert!(truncated, "an oversized item must be flagged truncated");

    let clipped = bounded["writer"]["items"][0].as_str().unwrap();
    // Clipped to the cap (+ the ellipsis marker), never the original length.
    assert!(
        clipped.chars().count() <= NODE_OUTPUT_ITEM_CHAR_CAP + CLIP_MARKER.chars().count(),
        "clipped length {} exceeds the cap",
        clipped.chars().count()
    );
    assert!(clipped.ends_with(CLIP_MARKER), "a clip must be marked");
    // Valid UTF-8 by construction — the assertion is that we got here without
    // a panic mid-codepoint, which char-based truncation guarantees.
}

#[test]
fn a_multibyte_string_under_the_cap_is_untouched() {
    let text = "héllo 🚀 wörld";
    let nodes = serde_json::json!({ "n": { "items": [text] } });
    let (bounded, truncated) = bound_node_output(&nodes);
    assert!(!truncated);
    assert_eq!(bounded["n"]["items"][0], text);
}

#[test]
fn sort_is_newest_first_with_id_tiebreak() {
    let rec = |run_id: &str, at: u64| WorkflowRunOutputRecord {
        run_id: run_id.to_string(),
        workflow_id: "wf".to_string(),
        at_millis: at,
        nodes: Value::Null,
        truncated: false,
        partial: false,
    };
    let mut recs = vec![rec("a", 10), rec("c", 20), rec("b", 20)];
    sort_newest_first(&mut recs);
    let ids: Vec<&str> = recs.iter().map(|r| r.run_id.as_str()).collect();
    assert_eq!(ids, ["c", "b", "a"], "newest first, id-desc tiebreak");
}

#[test]
fn from_raw_nodes_bounds_and_flags() {
    let long = "z".repeat(NODE_OUTPUT_ITEM_CHAR_CAP + 10);
    let raw = serde_json::json!({ "n": { "items": [long] } });
    let record = WorkflowRunOutputRecord::from_raw_nodes("r", "wf", 5, &raw, false);
    assert!(record.truncated, "from_raw_nodes must bound its input");
    assert_eq!(record.run_id, "r");
    assert_eq!(record.workflow_id, "wf");
    assert_eq!(record.at_millis, 5);
    assert!(!record.partial, "a clean settle passes partial=false");

    // Issue #1008: the failure arms hand `partial=true`; it survives onto
    // the record unchanged.
    let flagged = WorkflowRunOutputRecord::from_raw_nodes("r2", "wf", 6, &raw, true);
    assert!(flagged.partial, "from_raw_nodes must carry partial through");
}
