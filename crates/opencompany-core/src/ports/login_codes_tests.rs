use super::*;

#[test]
fn code_is_redeemable_only_while_pending_and_unexpired() {
    let mut code = LoginCodeRecord {
        id: "c1".to_string(),
        code_hash: "abc".to_string(),
        email: "ada@example.com".to_string(),
        created_at_millis: 0,
        expires_at_millis: 100,
        consumed_at_millis: None,
    };
    assert!(code.is_redeemable(99));
    assert!(!code.is_redeemable(100), "expiry is exclusive");

    code.consumed_at_millis = Some(50);
    assert!(!code.is_redeemable(60), "a redeemed code is single-use");
}

#[test]
fn login_code_record_round_trips_as_camel_case() {
    let code = LoginCodeRecord {
        id: "c1".to_string(),
        code_hash: "abc".to_string(),
        email: "ada@example.com".to_string(),
        created_at_millis: 0,
        expires_at_millis: 100,
        consumed_at_millis: None,
    };
    let json = serde_json::to_value(&code).unwrap();
    assert_eq!(json["codeHash"], "abc");
    assert!(json.get("consumedAtMillis").is_none());
    assert_eq!(
        serde_json::from_value::<LoginCodeRecord>(json).unwrap(),
        code
    );
}
