use super::*;

#[test]
fn a_tier_name_is_recognised_with_whitespace() {
    assert!(is_tier_name(" chat-v1 "));
    assert!(is_tier_name("reasoning-v1"));
    assert!(!is_tier_name("acme/test-model"));
    assert!(!is_tier_name(""));
}

#[test]
fn a_configured_real_id_is_found_for_its_tier() {
    let models = BTreeMap::from([("reasoning-v1".to_string(), " x/r ".to_string())]);
    assert_eq!(
        configured_model_for_tier("reasoning-v1", &models),
        Some("x/r".to_string())
    );
    // Whitespace on the tier key side is tolerated too.
    assert_eq!(
        configured_model_for_tier(" reasoning-v1 ", &models),
        Some("x/r".to_string())
    );
}

#[test]
fn a_configured_tier_name_is_never_returned() {
    let mapped_to_a_tier = BTreeMap::from([("chat-v1".to_string(), "agentic-v1".to_string())]);
    assert_eq!(
        configured_model_for_tier("chat-v1", &mapped_to_a_tier),
        None
    );

    let blank = BTreeMap::from([("chat-v1".to_string(), "   ".to_string())]);
    assert_eq!(configured_model_for_tier("chat-v1", &blank), None);

    assert_eq!(configured_model_for_tier("chat-v1", &BTreeMap::new()), None);
}
