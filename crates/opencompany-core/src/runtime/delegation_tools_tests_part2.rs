use super::tests_core::*;

/// The allowlist refusal must name what the member CAN reach: the model has
/// no other way to learn its own `delegates_to`.
#[test]
fn an_auto_channel_has_no_lead_but_a_deterministic_responder() {
    let mut record = record();
    record.overlay_desks.push(crate::ports::types::OverlayDesk {
        id: "launch".to_string(),
        name: "Launch week".to_string(),
        description: None,
        members: vec!["ceo".to_string(), "writer".to_string()],
        responder: crate::ports::types::ResponderMode::Auto,
        hive: Default::default(),
    });
    assert_eq!(
        desk_lead(&record, "launch"),
        None,
        "auto channels have no lead"
    );
    assert_eq!(
        desk_default_responder(&record, "launch").as_deref(),
        Some("ceo"),
        "the deterministic fallback is the first roster member"
    );
    assert_eq!(
        chat_responder(&record, "launch").as_deref(),
        Some("ceo"),
        "the shared seam answers the fallback, not None"
    );
    // An overlay desk that never states a mode is lead-routed — the
    // pre-#1835 behaviour, byte-for-byte.
    record.overlay_desks.push(crate::ports::types::OverlayDesk {
        id: "growth".to_string(),
        name: "Growth".to_string(),
        description: None,
        members: vec!["writer".to_string()],
        responder: crate::ports::types::ResponderMode::default(),
        hive: Default::default(),
    });
    assert_eq!(desk_lead(&record, "growth").as_deref(), Some("writer"));
}
