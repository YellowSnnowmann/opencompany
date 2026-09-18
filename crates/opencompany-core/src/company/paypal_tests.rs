use super::*;

#[test]
fn an_unrecognised_environment_falls_back_to_sandbox_not_live() {
    // The whole point: a typo must not aim an agent at real money.
    for raw in [
        "", "  ", "sandbox", "SANDBOX", "prod", "liv", "nonsense", "Live-ish",
    ] {
        assert_eq!(
            PaypalEnvironment::parse(raw),
            PaypalEnvironment::Sandbox,
            "{raw:?} must not resolve to live"
        );
    }
    // Only the two exact spellings reach live.
    assert_eq!(PaypalEnvironment::parse("live"), PaypalEnvironment::Live);
    assert_eq!(PaypalEnvironment::parse(" LIVE "), PaypalEnvironment::Live);
    assert_eq!(
        PaypalEnvironment::parse("production"),
        PaypalEnvironment::Live
    );
}

#[test]
fn each_environment_names_its_own_host() {
    assert_eq!(
        PaypalEnvironment::Sandbox.base_url(),
        "https://api-m.sandbox.paypal.com"
    );
    assert_eq!(
        PaypalEnvironment::Live.base_url(),
        "https://api-m.paypal.com"
    );
    assert_ne!(
        PaypalEnvironment::Sandbox.base_url(),
        PaypalEnvironment::Live.base_url()
    );
}

#[test]
fn the_default_is_sandbox() {
    assert_eq!(PaypalEnvironment::default(), PaypalEnvironment::Sandbox);
}
