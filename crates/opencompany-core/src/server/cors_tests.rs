use super::*;

fn cfg(origins: &[&str]) -> CorsConfig {
    CorsConfig {
        allowed_origins: origins.iter().map(|o| o.to_string()).collect(),
    }
}

fn with_origin(origin: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::ORIGIN, origin.parse().unwrap());
    h
}

#[test]
fn disabled_by_default() {
    assert!(!CorsConfig::default().is_enabled());
    assert!(
        CorsConfig::default()
            .headers_for(&with_origin("http://localhost:5173"))
            .is_empty()
    );
}

#[test]
fn an_allowed_origin_is_echoed_with_credentials() {
    let headers =
        cfg(&["http://localhost:5173"]).headers_for(&with_origin("http://localhost:5173"));
    let map: std::collections::HashMap<_, _> = headers
        .iter()
        .map(|(n, v)| (n.as_str(), v.to_str().unwrap()))
        .collect();
    assert_eq!(
        map.get("access-control-allow-origin"),
        Some(&"http://localhost:5173")
    );
    assert_eq!(map.get("access-control-allow-credentials"), Some(&"true"));
    // Or a cache could hand one origin's response to another.
    assert_eq!(map.get("vary"), Some(&"Origin"));
}

#[test]
fn the_origin_is_never_a_wildcard() {
    // A wildcard with credentials is rejected by the browser, so emitting
    // one would silently break the console rather than loosen it — but the
    // deeper point is that we must never echo an origin we were not told
    // about.
    let headers = cfg(&["http://localhost:5173"]).headers_for(&with_origin("https://evil.test"));
    assert!(
        headers.is_empty(),
        "an unlisted origin must get no CORS headers at all"
    );
}

#[test]
fn matching_is_exact() {
    let c = cfg(&["http://localhost:5173"]);
    // Substring and suffix tricks are how CORS allowlists usually break.
    for hostile in [
        "http://localhost:5173.evil.test",
        "https://localhost:5173",
        "http://localhost:51730",
        "http://evil.test?http://localhost:5173",
        "null",
    ] {
        assert!(
            c.headers_for(&with_origin(hostile)).is_empty(),
            "{hostile:?} must not match"
        );
    }

    // The same discipline for a webview origin. Admitting the `tauri://`
    // scheme widened what may be *written* in the allowlist; if it ever
    // widened what *matches*, every one of these would be a live origin.
    let t = cfg(&["tauri://localhost"]);
    for hostile in [
        "tauri://localhost.evil.test",
        "tauri://localhostx",
        "tauri://evil.test",
        "http://localhost",
        "https://localhost",
        "tauri://localhost:1420",
        "capacitor://localhost",
    ] {
        assert!(
            t.headers_for(&with_origin(hostile)).is_empty(),
            "{hostile:?} must not match tauri://localhost"
        );
    }
    assert!(
        !t.headers_for(&with_origin("tauri://localhost")).is_empty(),
        "the exact webview origin must still match"
    );
}

#[test]
fn a_webview_origin_can_be_allow_listed_at_all() {
    // Before schemes were a list, this failed configuration outright: a
    // desktop client could not be allowed even deliberately.
    for origin in [
        "tauri://localhost",
        "capacitor://localhost",
        // Tauri on Windows, which was always expressible.
        "http://tauri.localhost",
    ] {
        let parsed = CorsConfig::from_env_value(origin).expect("{origin} should configure");
        assert_eq!(parsed.allowed_origins, vec![origin.to_string()]);
    }
}

#[test]
fn an_unknown_scheme_is_still_refused() {
    // The list is an allowlist, not a suggestion. A scheme nobody vetted —
    // or a bare host with no scheme — is a configuration error, named at
    // boot rather than silently never matching.
    for bad in [
        "ftp://localhost",
        "localhost:5173",
        "javascript:alert(1)",
        "file://",
        "tauri:/localhost",
    ] {
        assert!(
            CorsConfig::from_env_value(bad).is_err(),
            "{bad:?} must be refused"
        );
    }
}

#[test]
fn no_origin_header_means_no_cors() {
    assert!(
        cfg(&["http://localhost:5173"])
            .headers_for(&HeaderMap::new())
            .is_empty()
    );
}

#[test]
fn preflight_answers_only_for_an_allowed_origin() {
    let c = cfg(&["http://localhost:5173"]);
    assert!(c.preflight(&with_origin("https://evil.test")).is_none());

    let response = c
        .preflight(&with_origin("http://localhost:5173"))
        .expect("an allowed origin gets a preflight response");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let headers = response.headers();
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .unwrap(),
        "true"
    );
    assert!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("PATCH")
    );
}

#[test]
fn preflight_echoes_the_requested_headers() {
    let mut h = with_origin("http://localhost:5173");
    h.insert(
        header::ACCESS_CONTROL_REQUEST_HEADERS,
        "content-type, x-custom".parse().unwrap(),
    );
    let response = cfg(&["http://localhost:5173"]).preflight(&h).unwrap();
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .unwrap(),
        "content-type, x-custom"
    );
}
