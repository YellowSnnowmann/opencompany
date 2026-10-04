use super::*;

#[test]
fn a_puts_label_is_read_back_from_its_tag() {
    let item = MemoryItem {
        id: "7".into(),
        kind: super::super::MemoryItemKind::Learning,
        title: "t".into(),
        body: "héllo world".into(),
        agent_id: None,
        namespace: "team:acme".into(),
        source: None,
        tags: vec![CONTEXT_TAG.into(), "label:notes/q4".into()],
        updated_at: 42,
        score: 0.0,
        editable: true,
    };
    assert_eq!(label_of(&item).as_deref(), Some("notes/q4"));
    let meta = meta(&item, "notes/q4".into());
    assert_eq!(meta.addr.as_ref(), "7");
    assert_eq!(meta.len, "héllo world".len());
    assert_eq!(meta.stored_at_millis, 42);
}

#[test]
fn a_peek_range_never_splits_a_character() {
    assert_eq!(slice("héllo", None), "héllo");
    assert_eq!(slice("héllo", Some(0..2)), "hé");
}
