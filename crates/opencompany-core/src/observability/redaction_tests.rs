use super::*;

/// Every one of these is a shape a credential has actually been found in,
/// in this tree or the vendored one.
#[test]
fn a_labelled_credential_loses_its_value() {
    for (input, expected) in [
        ("api_key=hunter2", "api_key=[redacted]"),
        ("api-key: hunter2", "api-key: [redacted]"),
        ("apiKey=hunter2", "apiKey=[redacted]"),
        (r#""token":"hunter2""#, r#""token":"[redacted]""#),
        ("password = hunter2", "password = [redacted]"),
        ("client_secret=hunter2", "client_secret=[redacted]"),
        ("--token hunter2", "--token [redacted]"),
        (
            "Authorization: Bearer hunter2",
            "Authorization: Bearer [redacted]",
        ),
        (
            "authorization: Basic aGk6dGhlcmU",
            "authorization: Basic [redacted]",
        ),
        (
            "config.token = hunter2 and settings.api_key = hunter3",
            "config.token = [redacted] and settings.api_key = [redacted]",
        ),
    ] {
        assert_eq!(scrub(input), expected, "input: {input}");
    }
}

#[test]
fn an_auth_scheme_is_never_mistaken_for_the_credential() {
    // The failure this guards is worse than no scrubbing at all: redacting
    // `Bearer` leaves the credential standing in a message that now looks
    // as though it was cleaned.
    let scrubbed = scrub("Authorization: Bearer hunter2");
    assert!(scrubbed.contains("Bearer"), "{scrubbed}");
    assert!(!scrubbed.contains("hunter2"), "{scrubbed}");
}

#[test]
fn a_self_identifying_credential_needs_no_label() {
    // One per issuer prefix in `SECRET_PREFIXES` that a scanner recognises,
    // plus the JWT arm. Assembled by `credential_shaped`, which explains
    // why they are not written out.
    for input in [
        credential_shaped("sk-ant-api03-", 28),
        credential_shaped("sk-proj-", 20),
        credential_shaped("sk_live_", 20),
        credential_shaped("ghp_", 36),
        format!("{}_{}", credential_shaped("github_pat_", 20), "B".repeat(8)),
        credential_shaped("glpat-", 20),
        format!(
            "xoxb-{}-{}-{}",
            "1".repeat(10),
            "2".repeat(10),
            "A".repeat(12)
        ),
        credential_shaped("AKIA", 16),
        credential_shaped("th_live_", 20),
        credential_shaped("npm_", 28),
        format!("{}.{}", credential_shaped("SG.", 22), "B".repeat(12)),
        // A JWT: `eyJ` and two dots are the whole of the rule.
        format!(
            "{}.{}.{}",
            credential_shaped("eyJ", 17),
            "B".repeat(16),
            "c2ln"
        ),
    ] {
        let sentence = format!("the provider said: {input} was rejected");
        let scrubbed = scrub(&sentence);
        assert!(!scrubbed.contains(&input), "{input} survived: {scrubbed}");
        assert!(scrubbed.contains(REDACTED), "{scrubbed}");
        // Only the credential goes — the sentence around it is the
        // diagnostic and has to survive.
        assert!(scrubbed.contains("was rejected"), "{scrubbed}");
    }
}

#[test]
fn a_credential_with_a_separator_leaves_no_fragment() {
    // The vendored runtime's `sk-[A-Za-z0-9]{20,}` left `[REDACTED]_uv`
    // behind on exactly this shape, because the character class stopped at
    // the underscore. A whole token cannot half-match.
    let input = format!("key {}_uv-9 here", credential_shaped("sk-", 20));
    let scrubbed = scrub(&input);
    assert_eq!(scrubbed, "key [redacted] here");
}

#[test]
fn the_word_boundary_false_positive_cannot_arise() {
    // The regexes this replaces matched all four of these. A tokenizer
    // cannot: none of them normalise to a key in `SECRET_KEYS`.
    for input in [
        "cancellation_token=abc123",
        "next_page_token=abc123",
        "csrf_token_name=session",
        "idempotency_key=abc123",
    ] {
        assert_eq!(scrub(input), input, "{input} must survive untouched");
    }
}

#[test]
fn prose_is_left_alone() {
    for input in [
        "the token was rejected by the provider",
        "no credential is configured for this company",
        "password reset requested",
        "reading the api key from config.toml",
        "sk- is the prefix these keys use",
        // `token` is a scheme in `Authorization: token <pat>` and a common
        // English word everywhere else. The word wins; see `SCHEME_KEYS`.
        "the token expired and no credential was refreshed",
        "storage=mongodb companies=3 outcome=ok",
        "GET /api/v1/companies/acme/agents -> 500 in 42ms",
    ] {
        assert_eq!(scrub(input), input, "{input} must survive untouched");
    }
}

#[test]
fn a_url_loses_its_userinfo() {
    assert_eq!(
        scrub("posting to https://user:hunter2@collector.internal/track failed"),
        "posting to https://[redacted]@collector.internal/track failed"
    );
    // A password containing an `@` still ends at the last one.
    assert_eq!(
        scrub("mongodb://admin:p@ss@db.internal:27017/oc"),
        "mongodb://[redacted]@db.internal:27017/oc"
    );
    // A URL with no userinfo is untouched, including the port colon that
    // looks like an assignment.
    assert_eq!(
        scrub("connecting to https://db.internal:27017/oc"),
        "connecting to https://db.internal:27017/oc"
    );
}

#[test]
fn a_key_that_names_a_credential_is_recognised_whatever_its_spelling() {
    // The same normalisation the inline rule uses, so a structured
    // `{"api-key": …}` and a flat `api-key=…` cannot disagree.
    for key in [
        "token",
        "api_key",
        "api-key",
        "apiKey",
        "Authorization",
        "client_secret",
        "config.password",
        "privateKey",
        "dsn",
    ] {
        assert!(key_names_a_secret(key), "{key} should name a credential");
    }
    // And the ones that must not, or half of every structured log goes.
    for key in [
        "cancellation_token",
        "next_page_token",
        "idempotency_key",
        "id",
        "key",
        "url",
        "http.url",
        "code",
        "company",
        "status_code",
    ] {
        assert!(!key_names_a_secret(key), "{key} must not name a credential");
    }
}

#[test]
fn a_url_query_loses_the_parameters_that_name_a_credential() {
    // The magic-link sign-in code. `App.tsx` clears it from the address bar
    // with `history.replaceState`, but the navigation breadcrumb recorded
    // the URL as it was, so this is the pass that has to catch it.
    assert_eq!(
        scrub("GET https://console.example/#/settings?code=abc123def456 -> 200"),
        "GET https://console.example/#/settings?code=[redacted] -> 200"
    );
    // Every pair is considered, not just the first, and the rest of the
    // URL — which is the diagnostic — survives.
    assert_eq!(
        scrub("https://h/api?company=acme&token=hunter2&code=xyz&page=2"),
        "https://h/api?company=acme&token=[redacted]&code=[redacted]&page=2"
    );
    // A parameter with no value is left alone: replacing it would invent a
    // credential that was never there.
    assert_eq!(
        scrub("https://h/api?code=&page=2"),
        "https://h/api?code=&page=2"
    );
}

#[test]
fn a_question_mark_in_prose_is_not_a_query_string() {
    for input in [
        "did the token expire?",
        "what happened to company=acme?",
        "is 2 > 1? yes",
    ] {
        assert_eq!(scrub(input), input, "{input} must survive untouched");
    }
}

#[test]
fn a_line_break_is_not_an_assignment() {
    // Two adjacent log lines are not a key and its value.
    let input = "refreshing the token\nGET /healthz -> 200";
    assert_eq!(scrub(input), input);
}

#[test]
fn several_credentials_in_one_string_all_go() {
    let input = format!(
        "POST https://key:secret@ingest.example/1 \
         api_key=hunter2 authorization: Bearer {}",
        credential_shaped("ghp_", 36)
    );
    let scrubbed = scrub(&input);
    for leaked in ["secret", "hunter2", "ghp_AAAA"] {
        assert!(!scrubbed.contains(leaked), "{leaked} survived: {scrubbed}");
    }
    assert_eq!(scrubbed.matches(REDACTED).count(), 3, "{scrubbed}");
}

#[test]
fn a_clean_string_is_borrowed_rather_than_copied() {
    // This runs on every string of every event; the common case must not
    // allocate.
    assert!(matches!(
        scrub("company acme finished a cycle in 42ms"),
        Cow::Borrowed(_)
    ));
    assert!(matches!(scrub("api_key=hunter2"), Cow::Owned(_)));
}

#[test]
fn non_ascii_text_is_not_split_mid_character() {
    // The scanner indexes by byte; every index it slices on has to be a
    // char boundary or this panics rather than merely being wrong.
    let input = "l'agent a échoué — token: hunter2 — 完了 🙂";
    let scrubbed = scrub(input);
    assert!(!scrubbed.contains("hunter2"), "{scrubbed}");
    assert!(scrubbed.contains("échoué"), "{scrubbed}");
    assert!(scrubbed.contains("完了 🙂"), "{scrubbed}");
}

#[test]
fn scrubbing_is_idempotent() {
    // A string can pass through more than one seam — a message that was
    // scrubbed at the call site and again in `before_send` — and the second
    // pass must not eat the marker or the text around it.
    let once = scrub("api_key=hunter2 and https://u:p@h/1").into_owned();
    assert_eq!(scrub(&once), once);
}

#[test]
fn an_empty_string_is_handled() {
    assert_eq!(scrub(""), "");
    assert_eq!(scrub("://"), "://");
    assert_eq!(scrub("@"), "@");
}
