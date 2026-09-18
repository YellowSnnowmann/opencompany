use super::{ComposioProbeClass as C, classify, describe, describe_verdict, has_code};

/// Every class, and the two orderings that exist because of real bugs.
///
/// A table rather than one test per row: the decision is a single pure
/// function over a string, and the thing worth pinning is the *whole*
/// mapping — a reordered arm shows up here as several rows moving at once,
/// which is exactly what a reviewer needs to see.
#[test]
fn the_classifier_covers_every_class_and_never_deletes_a_key_for_a_proxy() {
    let cases: &[(&str, C)] = &[
        // The bug the first arm exists for. "Authentication" is right there
        // in the text, and it is a proxy's word, not Composio's.
        ("407 Proxy Authentication Required", C::Unknown),
        ("HTTP 407", C::Unknown),
        ("Cloudflare: error 1020 access denied", C::Unknown),
        ("502 Bad Gateway", C::Unknown),
        ("proxy connect failed", C::Unknown),
        // A WAF's bare 403 says nothing about the key.
        ("403 Forbidden", C::Unknown),
        // The same code, with wording that *is* about the key.
        ("403 Forbidden: invalid api key", C::Auth),
        ("Composio answered 403 — unauthorized", C::Auth),
        // 401 needs no wording; it is the credential status.
        ("Composio answered 401 Unauthorized", C::Auth),
        ("401", C::Auth),
        // Word boundaries: an identifier that merely contains the digits.
        ("no connection ca_1403 on this account", C::Unknown),
        ("request 4011 failed", C::Unknown),
        ("trace 2401x aborted", C::Unknown),
        // Credit and throttling.
        ("429 Too Many Requests", C::Quota),
        ("rate limit exceeded", C::Quota),
        ("your account is out of credit", C::Quota),
        ("monthly quota reached", C::Quota),
        // Address / reachability.
        ("404 Not Found", C::Endpoint),
        ("error sending request: dns error", C::Endpoint),
        ("tcp connect error: connection refused", C::Endpoint),
        ("could not resolve backend.composio.dev", C::Endpoint),
        // Slow.
        ("operation timed out", C::Timeout),
        ("request timeout after 10s", C::Timeout),
        ("deadline exceeded", C::Timeout),
        // Everything else, including the no-client build's own reason.
        ("Composio is not compiled into this build", C::Unknown),
        ("", C::Unknown),
        ("something went sideways", C::Unknown),
    ];
    for (text, want) in cases {
        assert_eq!(
            classify(text),
            *want,
            "classify({text:?}) must be {want:?}, not {:?}",
            classify(text)
        );
    }
}

/// The whole reason the classifier is not a boolean: exactly one class may
/// take a credential away.
#[test]
fn only_auth_is_destructive() {
    assert!(C::Auth.is_destructive());
    for class in [C::Endpoint, C::Quota, C::Timeout, C::Unknown] {
        assert!(
            !class.is_destructive(),
            "{class} must keep the key — only a rejected credential may be discarded"
        );
    }
}

/// `describe` is copy, and copy only. In particular the `unknown` sentence
/// must not be a place an upstream error string could be interpolated — it
/// lands in a banner an operator screenshots, and upstream text can echo
/// request headers or a key fragment.
#[test]
fn describe_is_a_fixed_sentence_per_class_and_quotes_nothing() {
    let mut seen: Vec<&str> = Vec::new();
    for class in [C::Auth, C::Endpoint, C::Quota, C::Timeout, C::Unknown] {
        let copy = describe(class);
        assert!(!copy.is_empty(), "{class} has no copy");
        assert!(
            !seen.contains(&copy),
            "{class} reuses another class's sentence: {copy}"
        );
        seen.push(copy);
    }
    // The destructive class says nothing was saved; every advisory class
    // says the opposite, because the write did land.
    assert!(describe(C::Auth).contains("nothing was changed"));
    for class in [C::Endpoint, C::Quota, C::Timeout, C::Unknown] {
        assert!(
            describe(class).starts_with("Saved"),
            "{class} is an advisory on a completed write: {}",
            describe(class)
        );
    }
}

/// The check-only route's copy is the same facts with the write claim
/// removed. A `Saved, …` sentence on a route that stores nothing is the
/// exact misreport this second table exists to prevent.
#[test]
fn the_verdict_copy_never_claims_anything_was_saved() {
    let mut seen: Vec<&str> = Vec::new();
    for class in [C::Auth, C::Endpoint, C::Quota, C::Timeout, C::Unknown] {
        let copy = describe_verdict(class);
        assert!(!copy.is_empty(), "{class} has no verdict copy");
        assert!(
            !copy.contains("Saved"),
            "a check writes nothing, so its copy must not say it saved: {copy}"
        );
        assert!(
            !seen.contains(&copy),
            "{class} reuses another class's verdict: {copy}"
        );
        assert_ne!(
            copy,
            describe(class),
            "{class}: the two tables answer different questions and must not silently \
             converge"
        );
        seen.push(copy);
    }
}

/// The boundary rule on its own, since it is what stops an opaque Composio
/// id from being read as an HTTP status.
#[test]
fn status_codes_match_on_word_boundaries_only() {
    assert!(has_code("composio answered 401 unauthorized", "401"));
    assert!(has_code("401", "401"));
    assert!(has_code("http/1.1 403 forbidden", "403"));
    assert!(!has_code("ca_1403", "403"));
    assert!(!has_code("4011", "401"));
    assert!(!has_code("x401", "401"));
    // A second occurrence that *is* a word still counts.
    assert!(has_code("ca_1403 then 403 forbidden", "403"));
}

/// The wire spelling is a contract the console keys on.
#[test]
fn the_wire_spelling_is_lowercase_and_stable() {
    for (class, wire) in [
        (C::Auth, "auth"),
        (C::Endpoint, "endpoint"),
        (C::Quota, "quota"),
        (C::Timeout, "timeout"),
        (C::Unknown, "unknown"),
    ] {
        assert_eq!(class.as_str(), wire);
        assert_eq!(serde_json::to_value(class).unwrap(), wire);
        assert_eq!(class.to_string(), wire);
    }
}
