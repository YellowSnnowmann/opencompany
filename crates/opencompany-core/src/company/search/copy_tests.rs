use super::*;

#[test]
fn default_provider_unavailable_names_the_provider_the_state_and_the_page() {
    assert_eq!(
        default_provider_unavailable("Exa", ProviderGone::Removed),
        "The search default uses Exa, which is removed. Choose a new search \
         default in Connections \u{2192} API Keys \u{2192} Search."
    );
    assert_eq!(
        default_provider_unavailable("SearXNG", ProviderGone::TurnedOff),
        "The search default uses SearXNG, which is turned off. Choose a new \
         search default in Connections \u{2192} API Keys \u{2192} Search."
    );
}

#[test]
fn provider_in_use_message_names_only_the_provider() {
    assert_eq!(provider_in_use_message("Exa"), "Exa is the search default.");
}
