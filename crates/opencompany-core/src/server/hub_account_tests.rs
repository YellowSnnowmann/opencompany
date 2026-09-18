use super::*;

#[test]
fn production_api_derives_the_bare_site() {
    assert_eq!(
        site_for_api("https://api.tinyhumans.ai").as_deref(),
        Some("https://tinyhumans.ai")
    );
}

#[test]
fn staging_api_derives_the_staging_site() {
    assert_eq!(
        site_for_api("https://staging-api.tinyhumans.ai/").as_deref(),
        Some("https://staging.tinyhumans.ai")
    );
}

#[test]
fn an_unrecognized_host_derives_nothing() {
    // A self-hosted backend, or a loopback one. Guessing a dashboard origin
    // for these would link to a host that need not exist.
    assert_eq!(site_for_api("http://127.0.0.1:5007"), None);
    assert_eq!(site_for_api("https://hub.example.com"), None);
}

#[test]
fn a_path_prefixed_hub_derives_nothing() {
    assert_eq!(site_for_api("https://example.com/api"), None);
}

#[test]
fn the_connect_page_carries_the_grant_query_through() {
    assert_eq!(
        connect_url(
            "https://staging.tinyhumans.ai",
            "callback_url=x&code_challenge=y"
        ),
        "https://staging.tinyhumans.ai/connect?callback_url=x&code_challenge=y"
    );
}

#[test]
fn tabs_hang_off_the_site_without_doubling_the_slash() {
    assert_eq!(
        manage_keys_url("https://staging.tinyhumans.ai/"),
        "https://staging.tinyhumans.ai/dashboard?tab=api-keys"
    );
    assert_eq!(
        top_up_url("https://tinyhumans.ai"),
        "https://tinyhumans.ai/dashboard?tab=billing"
    );
}
