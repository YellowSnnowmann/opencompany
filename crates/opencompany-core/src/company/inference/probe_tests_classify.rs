//! Probe-failure classification tests: the four branch-order cases, all
//! six classes, and the copy each one produces (split out of
//! `probe_tests.rs`).

use super::*;

// ---- the four cases the branch order exists for -------------------------
//
// architecture.md names these four by hand, and they are the whole reason
// the ordering is what it is. If one of them starts failing, the ordering
// has been "simplified" back into a bug.

#[test]
fn a_407_proxy_challenge_is_unknown_not_auth() {
    // It contains the word "authentication". Check auth first and a
    // corporate proxy deletes a valid key.
    assert_eq!(
        classify("HTTP 407 Proxy Authentication Required"),
        ProbeClass::Unknown
    );
    assert!(!classify("HTTP 407 Proxy Authentication Required").destroys_credential());
}

#[test]
fn a_bare_waf_403_is_unknown_not_auth() {
    // An unidentified intermediary saying "forbidden" is not proof the key
    // is bad. Cloudflare is named explicitly because it is the common one.
    assert_eq!(classify("error from cloudflare: 403"), ProbeClass::Unknown);
    assert_eq!(classify("502 Bad Gateway"), ProbeClass::Unknown);
}

#[test]
fn a_403_that_names_no_credential_refusal_keeps_the_key() {
    // Every one of these is a documented 403 from a provider we ship, and in
    // every one the credential is **valid**. Before the positive list they
    // all classified as `Auth` and deleted it — the string the classifier
    // read carried our own `Forbidden` reason phrase, which was one of the
    // four words the rule accepted as credential wording.
    for body in [
        // Together, for a prompt that ran past the context window. One long
        // message and the key was gone.
        "403: Input token count + max_tokens must be less than the context \
         length of the model being queried",
        // OpenRouter, whose 403 covers moderation as well as permissions.
        "403: Forbidden (insufficient permissions, guardrail block, or \
         moderation flag)",
        // OpenAI, for where the request came from.
        "403: Country, region, or territory not supported",
        // Anthropic's permission_error. It names the API key in its own text,
        // which is why matching the bare word `key` was never safe.
        "403: Your API key does not have permission to use the specified \
         resource.",
        // Google, Groq, xAI and Cerebras, in their own words.
        "403: PERMISSION_DENIED",
        "403: not allowed due to permission restrictions",
        "403: Ask your team admin for permission.",
        "403: PermissionDeniedError",
        // Fireworks' non-credential 403s.
        "403: FireRouter is not available for Fireworks accounts with data \
         residency enabled",
    ] {
        assert!(
            !classify(body).destroys_credential(),
            "this 403 must not delete the key: {body:?}"
        );
    }
}

#[test]
fn a_403_that_does_name_a_credential_refusal_is_still_auth() {
    // Fireworks is the reason the fix could not be "403 is never auth": it
    // maps a genuinely bad credential to 403 as well as 401, and these are
    // the only two bad-key messages it documents. Neither matches
    // `invalid api key`, so both are listed by hand.
    assert_eq!(
        classify("403: The API key you provided is invalid"),
        ProbeClass::Auth
    );
    assert_eq!(
        classify("403: You must provide an API key"),
        ProbeClass::Auth
    );
    assert!(classify("403: invalid credential").destroys_credential());
}

#[test]
fn the_reason_phrase_is_not_part_of_what_is_classified() {
    // The bug, stated as the one-line property that prevents its return: the
    // text handed to `classify` carries the vendor's body and the status
    // code, and nothing this module wrote. `Forbidden` appearing here would
    // put the old failure back whatever the rules say.
    let text = build_failure_text(
        reqwest::StatusCode::FORBIDDEN,
        "{\"error\":\"context length exceeded\"}",
    );
    assert!(!text.to_ascii_lowercase().contains("forbidden"), "{text}");
    assert!(text.starts_with("403: "));
    assert!(!classify(&text).destroys_credential());

    // And 401 keeps working with an empty body, which is how several
    // providers send it — the status is the whole signal there.
    let text = build_failure_text(reqwest::StatusCode::UNAUTHORIZED, "");
    assert!(
        !text.to_ascii_lowercase().contains("unauthorized"),
        "{text}"
    );
    assert_eq!(classify(&text), ProbeClass::Auth);
}

#[test]
fn a_400_about_our_request_shape_does_not_delete_the_key() {
    // `authentication` used to match as a bare word. An endpoint telling us
    // we used the wrong auth header is talking about our request, not about
    // the operator's key, and the key it is refusing to look at is fine.
    assert!(
        !classify("400: Bearer authentication is not supported, use x-api-key")
            .destroys_credential()
    );
    // Groq's 424 for a failed downstream dependency, which it documents as
    // "(e.g., Remote MCP authentication)".
    assert!(
        !classify("424: dependent request failed (Remote MCP authentication)")
            .destroys_credential()
    );
    // But a typed authentication error from Anthropic or DeepSeek still is
    // one.
    assert_eq!(classify("401: authentication_error"), ProbeClass::Auth);
    assert_eq!(
        classify("400: Authentication Fails (no such user)"),
        ProbeClass::Auth
    );
}

#[test]
fn a_status_code_inside_an_id_does_not_match() {
    // Word boundaries. Without them a request id or a model name carrying
    // these digits reads as a status code and deletes the operator's key.
    assert_eq!(classify("request id req_1403 failed"), ProbeClass::Unknown);
    assert_eq!(classify("trace 4032 aborted"), ProbeClass::Unknown);
    assert_eq!(classify("model gpt-4010 is odd"), ProbeClass::Unknown);
}

// ---- all six classes ----------------------------------------------------

#[test]
fn every_class_has_a_real_error_string_that_reaches_it() {
    let cases: &[(&str, ProbeClass)] = &[
        ("401 Unauthorized", ProbeClass::Auth),
        ("Incorrect API key provided", ProbeClass::Auth),
        ("invalid_api_key", ProbeClass::Auth),
        (
            "The model `gpt-5.6-sol-pro` does not exist",
            ProbeClass::Model,
        ),
        ("model_not_found", ProbeClass::Model),
        (
            "Model 'anthropic/claude-sonnet-5' is not available",
            ProbeClass::Model,
        ),
        ("You exceeded your current quota", ProbeClass::Quota),
        ("429 Too Many Requests", ProbeClass::Quota),
        ("insufficient credits", ProbeClass::Quota),
        ("404 Not Found", ProbeClass::Endpoint),
        ("dns error: not found", ProbeClass::Endpoint),
        ("operation timed out", ProbeClass::Timeout),
        ("request timeout after 10s", ProbeClass::Timeout),
        ("something nobody has seen before", ProbeClass::Unknown),
        ("", ProbeClass::Unknown),
    ];
    for (raw, expected) in cases {
        assert_eq!(classify(raw), *expected, "classifying {raw:?}");
    }
}

#[test]
fn a_missing_model_is_not_read_as_a_missing_endpoint() {
    // The endpoint branch matches a bare "not found". Checked after `model`,
    // so this sends the operator to their model id rather than their URL.
    assert_eq!(
        classify("The model `acme-1` was not found"),
        ProbeClass::Model
    );
    // And a genuine endpoint miss still reaches `endpoint`.
    assert_eq!(classify("404 page not found"), ProbeClass::Endpoint);
}

#[test]
fn exactly_one_class_deletes_the_credential() {
    let destructive: Vec<&str> = [
        ProbeClass::Auth,
        ProbeClass::Model,
        ProbeClass::Quota,
        ProbeClass::Endpoint,
        ProbeClass::Timeout,
        ProbeClass::Unknown,
    ]
    .into_iter()
    .filter(|c| c.destroys_credential())
    .map(|c| c.as_str())
    .collect();
    assert_eq!(destructive, vec!["auth"]);
}

#[test]
fn classification_is_case_insensitive_and_ignores_surrounding_noise() {
    assert_eq!(classify("  \n401 UNAUTHORIZED\n "), ProbeClass::Auth);
}

// ---- the copy -----------------------------------------------------------

#[test]
fn the_copy_never_echoes_the_upstream_string() {
    // That text can carry headers or key fragments, and this sentence lands
    // in a screenshot-able banner.
    let raw = "401 Unauthorized: Bearer sk-not-a-real-key rejected";
    let sentence = describe(classify(raw), "Acme");
    assert!(!sentence.contains("sk-not-a-real-key"));
    assert!(!sentence.contains(raw));
}

#[test]
fn only_the_destructive_class_fails_to_say_saved() {
    for class in [
        ProbeClass::Model,
        ProbeClass::Quota,
        ProbeClass::Endpoint,
        ProbeClass::Timeout,
        ProbeClass::Unknown,
    ] {
        assert!(
            describe(class, "Acme").starts_with("Saved"),
            "{class:?} kept the record, so its copy must say so"
        );
    }
    assert!(describe(ProbeClass::Auth, "Acme").starts_with("Could not reach Acme"));
}
