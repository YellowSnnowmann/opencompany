use super::*;

#[test]
fn text_revisions_refuse_stale_tokens_and_never_wrap() {
    let current = i64::MAX as u64 / 2;
    assert_eq!(
        next_write_revision(current, Some(current)).unwrap(),
        current + 1
    );
    assert_eq!(next_write_revision(current, None).unwrap(), current + 1);
    let stale = next_write_revision(current, Some(current - 1)).unwrap_err();
    assert!(matches!(stale, crate::error::OpenCompanyError::Conflict(_)));
    let exhausted = next_write_revision(u64::MAX, Some(u64::MAX)).unwrap_err();
    assert!(matches!(
        exhausted,
        crate::error::OpenCompanyError::Conflict(_)
    ));
}

/// Every backend persists a node as opaque JSON, so a node written before
/// authorship existed has neither field. It must still load — and must load
/// as `Operator`, the conservative answer that never credits an agent for
/// something it did not write.
///
/// This is the whole of the migration story: no `ALTER TABLE`, no
/// `add_column_if_missing`, no backfill.
#[test]
fn a_legacy_node_without_origins_loads_as_operator() {
    let legacy = r#"{
        "id": "n-1",
        "name": "voice.md",
        "kind": "file",
        "parentId": null,
        "updatedAtMillis": 1700000000000
    }"#;
    let node: WorkspaceNode = serde_json::from_str(legacy).expect("legacy node must load");
    assert_eq!(node.created_by, WorkspaceOrigin::Operator);
    assert_eq!(node.updated_by, WorkspaceOrigin::Operator);
    // Issue #1839: the adoption lease is `#[serde(default)]`, so a node
    // written before it existed loads as `false` — the conservative reading
    // that leaves a pre-#1839 empty folder exactly as rollback-eligible as it
    // is today. That default IS the whole migration: no rewrite, no backfill.
    assert!(
        !node.adopted,
        "a legacy node without the field must load unadopted"
    );
}

/// The internally-tagged wire shape, pinned.
///
/// The same bytes are read by three independent consumers — the stores'
/// `node_json`, the REST body the console parses, and the GraphQL
/// projection — so a stray `rename_all` or a switch to an adjacently-tagged
/// representation would break the console at runtime with nothing in Rust
/// CI noticing. This test is what turns that into a compile-suite failure.
#[test]
fn the_agent_origin_wire_shape_is_tagged_kind_plus_id() {
    let agent = WorkspaceOrigin::Agent {
        id: "ceo".to_string(),
    };
    assert_eq!(
        serde_json::to_value(&agent).unwrap(),
        serde_json::json!({ "kind": "agent", "id": "ceo" })
    );
    assert_eq!(
        serde_json::to_value(WorkspaceOrigin::Seed).unwrap(),
        serde_json::json!({ "kind": "seed" })
    );
    assert_eq!(
        serde_json::to_value(WorkspaceOrigin::Operator).unwrap(),
        serde_json::json!({ "kind": "operator" })
    );

    // …and back, so the shape is a round trip rather than a one-way render.
    let parsed: WorkspaceOrigin =
        serde_json::from_value(serde_json::json!({ "kind": "agent", "id": "ceo" })).unwrap();
    assert_eq!(parsed, agent);
}

/// A node carrying both origins round-trips through the exact `node_json`
/// path the backends use.
#[test]
fn a_node_round_trips_both_origins() {
    let node = WorkspaceNode {
        id: "n-1".to_string(),
        name: "brief.md".to_string(),
        kind: NodeKind::File,
        parent_id: Some("f-1".to_string()),
        updated_at_millis: 42,
        created_by: WorkspaceOrigin::Agent {
            id: "cmo".to_string(),
        },
        updated_by: WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    };
    let json = serde_json::to_string(&node).unwrap();
    assert_eq!(serde_json::from_str::<WorkspaceNode>(&json).unwrap(), node);
    // camelCase on the node, matching every other field on it.
    assert!(json.contains("\"createdBy\""), "{json}");
    assert!(json.contains("\"updatedBy\""), "{json}");
}

/// A prose note carries no blob metadata **on the wire at all** — the three
/// fields are `skip_serializing_if`, so the tree read the console makes on
/// every mount does not grow three nulls per node, and `mime` being present
/// is a reliable "this is binary" test rather than a present-but-null
/// ambiguity.
#[test]
fn a_text_node_serializes_without_the_blob_fields() {
    let node = WorkspaceNode {
        id: "n-1".to_string(),
        name: "voice.md".to_string(),
        kind: NodeKind::File,
        parent_id: None,
        updated_at_millis: 1,
        created_by: WorkspaceOrigin::Operator,
        updated_by: WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    };
    let json = serde_json::to_string(&node).unwrap();
    assert!(!json.contains("mime"), "{json}");
    assert!(!json.contains("size"), "{json}");
    assert!(!json.contains("sha256"), "{json}");
    assert!(!node.is_binary());
}

/// A binary node round-trips all three fields through the exact `node_json`
/// path every backend persists, in camelCase like the rest of the node.
#[test]
fn a_binary_node_round_trips_its_blob_metadata() {
    let node = WorkspaceNode {
        id: "n-2".to_string(),
        name: "chart.png".to_string(),
        kind: NodeKind::File,
        parent_id: None,
        updated_at_millis: 1,
        created_by: WorkspaceOrigin::Operator,
        updated_by: WorkspaceOrigin::Operator,
        mime: Some("image/png".to_string()),
        size: Some(1234),
        sha256: Some("abc123".to_string()),
        adopted: false,
    };
    let json = serde_json::to_string(&node).unwrap();
    assert_eq!(serde_json::from_str::<WorkspaceNode>(&json).unwrap(), node);
    assert!(json.contains("\"sha256\""), "{json}");
    assert!(node.is_binary());
}

/// The digest is over the bytes, not over any text rendering of them — so a
/// payload that is not UTF-8 at all still has one, which is the whole point.
#[test]
fn blob_metadata_is_the_sha256_of_the_raw_bytes() {
    // The empty-input SHA-256, a value with a published constant to check
    // against, so this pins the encoding (lowercase hex) and not just
    // self-consistency.
    assert_eq!(
        blob_metadata(b""),
        (
            0,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string()
        )
    );
    // Invalid UTF-8: a lone continuation byte. Digesting must not care.
    let (size, sha) = blob_metadata(&[0xff, 0xfe, 0x00]);
    assert_eq!(size, 3);
    assert_eq!(sha.len(), 64);
    assert!(
        sha.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
    );
}
