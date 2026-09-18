//! Auth-style-on-the-wire tests: which headers a probe sends for each
//! auth style (split out of `probe_tests.rs`).

use super::*;

// ---- auth style on the wire --------------------------------------------
//
// Asserted on the **headers actually sent**, not on a return value: this bug
// was invisible to every test that only checked what a function returned,
// because the function returned fine and the request was malformed.

/// The headers one `apply_auth` call produces, as `(name, value)` pairs.
fn headers_for(auth: catalogue::AuthStyle, key: Option<&str>) -> Vec<(String, String)> {
    let client = reqwest::Client::new();
    let request = apply_auth(client.get("https://example.test/v1/models"), auth, key);
    let built = request.build().expect("a request");
    built
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header, _)| header == name)
        .map(|(_, value)| value.as_str())
}

#[test]
fn anthropic_gets_x_api_key_and_a_version_and_no_authorization() {
    // A bearer with no `anthropic-version` is rejected by Anthropic's native
    // API as MALFORMED — a 400, not a 401 — which is the diagnostic that
    // tells a broken request from a bad key. The reported symptom was
    // exactly that 400 on a key that was fine.
    let headers = headers_for(
        catalogue::AuthStyle::Anthropic,
        Some("sk-ant-not-a-real-key"),
    );
    assert_eq!(header(&headers, "x-api-key"), Some("sk-ant-not-a-real-key"));
    assert_eq!(
        header(&headers, "anthropic-version"),
        Some(catalogue::ANTHROPIC_VERSION)
    );
    assert_eq!(
        header(&headers, "authorization"),
        None,
        "a bearer alongside x-api-key is the shape that was failing"
    );
}

#[test]
fn a_bearer_provider_is_unchanged() {
    let headers = headers_for(catalogue::AuthStyle::Bearer, Some("sk-not-a-real-key"));
    assert_eq!(
        header(&headers, "authorization"),
        Some("Bearer sk-not-a-real-key")
    );
    assert_eq!(header(&headers, "x-api-key"), None);
    assert_eq!(header(&headers, "anthropic-version"), None);
}

#[test]
fn a_provider_with_no_key_gets_no_auth_header_at_all() {
    // The keyless local runtime. An empty header is worse than none.
    for key in [None, Some(""), Some("   ")] {
        for auth in [
            catalogue::AuthStyle::Bearer,
            catalogue::AuthStyle::Anthropic,
            catalogue::AuthStyle::None,
        ] {
            let headers = headers_for(auth, key);
            assert_eq!(header(&headers, "authorization"), None, "{auth:?} {key:?}");
            assert_eq!(header(&headers, "x-api-key"), None, "{auth:?} {key:?}");
        }
    }
}

#[test]
fn a_keyless_auth_style_sends_nothing_even_with_a_key() {
    // `AuthStyle::None` is a statement about the endpoint, not about whether
    // we happen to hold a credential.
    let headers = headers_for(catalogue::AuthStyle::None, Some("sk-not-a-real-key"));
    assert_eq!(header(&headers, "authorization"), None);
    assert_eq!(header(&headers, "x-api-key"), None);
}
