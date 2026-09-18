use super::*;

fn session() -> SessionRecord {
    SessionRecord {
        id: "s1".to_string(),
        token_hash: "abc".to_string(),
        user_id: "u1".to_string(),
        created_at_millis: 0,
        expires_at_millis: 100,
        user_agent: None,
        kind: SessionKind::Browser,
        label: None,
    }
}

#[test]
fn session_is_live_until_its_expiry() {
    let s = session();
    assert!(s.is_live(99));
    // Expiry is exclusive, matching InviteRecord::is_redeemable.
    assert!(!s.is_live(100));
    assert!(!s.is_live(101));
}

#[test]
fn session_record_round_trips_as_camel_case() {
    let s = session();
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["tokenHash"], "abc");
    assert_eq!(json["userId"], "u1");
    assert!(json.get("userAgent").is_none());
    assert_eq!(json["kind"], "browser");
    assert!(json.get("label").is_none());
    assert_eq!(serde_json::from_value::<SessionRecord>(json).unwrap(), s);
}

/// The property the whole device design rests on.
///
/// Every backend persists this record as JSON — fs to an array file, sqlite
/// to a `session_json` column, mongo through serde — so a record written
/// before `kind` existed must still load, and must load as a browser
/// session. If this ever stopped holding, upgrading a host would log every
/// existing user out, and there is no schema migration anywhere that would
/// have caught it.
#[test]
fn a_record_written_before_devices_existed_still_loads_as_a_browser() {
    let legacy = serde_json::json!({
        "id": "s1",
        "tokenHash": "abc",
        "userId": "u1",
        "createdAtMillis": 0,
        "expiresAtMillis": 100,
    });
    let loaded: SessionRecord = serde_json::from_value(legacy).unwrap();
    assert_eq!(loaded, session());
    assert_eq!(loaded.kind, SessionKind::Browser);
    assert!(!loaded.kind.is_device());
}

#[test]
fn a_device_record_round_trips_with_its_label() {
    let s = SessionRecord {
        kind: SessionKind::Device,
        label: Some("Ada's MacBook".to_string()),
        ..session()
    };
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["kind"], "device");
    assert_eq!(json["label"], "Ada's MacBook");
    assert_eq!(serde_json::from_value::<SessionRecord>(json).unwrap(), s);
}
