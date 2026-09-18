use super::*;

#[test]
fn managed_is_supported_but_is_not_a_byo_provider() {
    assert!(provider_supported(MANAGED_PROVIDER));
    assert!(!provider_is_byo(MANAGED_PROVIDER));
    assert!(!provider_requires_key(MANAGED_PROVIDER));
}

#[test]
fn an_unknown_slug_is_neither_supported_nor_byo() {
    assert!(!provider_supported("google"));
    assert!(!provider_is_byo("google"));
    assert!(!provider_requires_key("google"));
}

#[test]
fn key_providers_need_a_key_and_searxng_needs_an_endpoint() {
    for slug in ["brave", "exa", "querit"] {
        assert!(provider_requires_key(slug), "{slug}");
        assert!(!provider_requires_endpoint(slug), "{slug}");
        assert!(!configuration_complete(slug, false, false), "{slug}");
        assert!(configuration_complete(slug, true, false), "{slug}");
    }
    assert!(!provider_requires_key("searxng"));
    assert!(provider_requires_endpoint("searxng"));
    assert!(!configuration_complete("searxng", true, false));
    assert!(configuration_complete("searxng", false, true));
}

#[test]
fn the_effective_provider_is_the_selection_only_when_it_is_finished() {
    assert_eq!(effective_provider("exa", true, false), "exa");
    assert_eq!(effective_provider("exa", false, false), MANAGED_PROVIDER);
    assert_eq!(effective_provider("searxng", false, true), "searxng");
    assert_eq!(effective_provider("searxng", true, false), MANAGED_PROVIDER);
    assert_eq!(effective_provider("google", true, true), MANAGED_PROVIDER);
}

#[test]
fn managed_is_never_complete_because_it_is_the_fallback_not_a_connection() {
    assert!(!configuration_complete(MANAGED_PROVIDER, true, true));
}
