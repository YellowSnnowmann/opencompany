//! Auth-style, endpoint-normalization and credential-redaction tests,
//! plus the OpenRouter catalogue-query rules (split out of
//! `catalogue_tests.rs`).

use super::*;

// ---- auth style and endpoint normalisation ------------------------------

#[test]
fn anthropic_is_the_only_non_bearer_entry_in_the_catalogue() {
    // A port that assumes one auth style breaks exactly one provider, and it
    // is the one people try first. Worse, the rejection classifies as `auth`
    // — the one destructive class — so the connect flow would delete a key
    // that was never wrong.
    assert_eq!(auth_style_for("anthropic"), AuthStyle::Anthropic);
    for provider in CLOUD_PROVIDERS.iter().filter(|p| p.slug != "anthropic") {
        assert_eq!(
            auth_style_for(provider.slug),
            AuthStyle::Bearer,
            "{} should be bearer",
            provider.slug
        );
    }
}

#[test]
fn a_keyless_local_runtime_sends_no_auth_header_and_omlx_does() {
    assert_eq!(auth_style_for("ollama"), AuthStyle::None);
    assert_eq!(auth_style_for("lmstudio"), AuthStyle::None);
    // Still bearer, though omlx no longer *requires* a key: an operator
    // running `jundot/omlx --api-key` must still be able to authenticate.
    // This is the assertion that would have caught the auth style silently
    // becoming `None` as a side effect of correcting `needs_key`.
    assert_eq!(auth_style_for("omlx"), AuthStyle::Bearer);
    assert!(
        !local_runtime("omlx").expect("omlx row").needs_key,
        "accepting a key is not the same as demanding one"
    );
}

#[test]
fn an_unknown_kind_is_a_custom_openai_compatible_endpoint() {
    // Which by definition speaks bearer — that is what "OpenAI-compatible"
    // means in the field the operator typed it into.
    assert_eq!(auth_style_for("my-gateway"), AuthStyle::Bearer);
    assert_eq!(auth_style_for("custom"), AuthStyle::Bearer);
}

#[test]
fn a_bare_origin_gains_the_v1_an_openai_surface_lives_at() {
    // `http://localhost:11434` is what Ollama's own documentation prints,
    // and it is not where the OpenAI-compatible surface is.
    assert_eq!(
        normalize_local_endpoint("http://localhost:11434").as_deref(),
        Some("http://localhost:11434/v1")
    );
    assert_eq!(
        normalize_local_endpoint("  http://localhost:11434/  ").as_deref(),
        Some("http://localhost:11434/v1")
    );
}

#[test]
fn a_path_the_operator_supplied_is_left_exactly_as_typed() {
    // Appending is not guessing. Someone who typed a path meant it.
    assert_eq!(
        normalize_local_endpoint("https://acme.example/api/gateway").as_deref(),
        Some("https://acme.example/api/gateway")
    );
    assert_eq!(
        normalize_local_endpoint("http://127.0.0.1:1234/v1/").as_deref(),
        Some("http://127.0.0.1:1234/v1")
    );
}

#[test]
fn an_endpoint_carrying_a_credential_is_not_an_endpoint() {
    // The security half of the same refusal. `normalize_local_endpoint` is
    // the funnel every stored endpoint passes through, so refusing here is
    // what makes "no credential is ever stored in a `base_url`" a property
    // of the store rather than of whichever handler remembered to check.
    for bad in [
        "http://alice:hunter2@127.0.0.1:8597/v1",
        "https://alice@api.acme.example/v1",
        "http://alice:hunter2@127.0.0.1:8597",
        // A password may itself contain an `@`; the authority still has one.
        "http://alice:hun@ter2@127.0.0.1:8597/v1",
    ] {
        assert!(endpoint_has_credentials(bad), "`{bad}` carries userinfo");
        assert!(
            normalize_local_endpoint(bad).is_none(),
            "`{bad}` must not normalise into something storable"
        );
    }
}

#[test]
fn a_second_scheme_does_not_hide_the_credential_behind_it() {
    // Codex review on #2281: an uppercase scheme was once prefixed with a
    // second one by setup normalisation, and the first authority (`HTTP:`)
    // has no `@` — so reading only that one missed the credential entirely.
    for bad in [
        "http://HTTP://alice:hunter2@127.0.0.1:8597/v1",
        "https://http://alice@api.acme.example/v1",
    ] {
        assert!(endpoint_has_credentials(bad), "`{bad}` carries userinfo");
        assert!(
            normalize_local_endpoint(bad).is_none(),
            "`{bad}` must not be storable"
        );
        let said = redact_endpoint(bad);
        assert!(
            !said.contains("alice") && !said.contains("hunter2"),
            "`{bad}` redacted to `{said}`"
        );
    }
    // An uppercase scheme on its own is an ordinary endpoint.
    assert!(endpoint_has_credentials(
        "HTTP://alice:hunter2@127.0.0.1:8597/v1"
    ));
    assert!(!endpoint_has_credentials("HTTPS://api.acme.example/v1"));
}

#[test]
fn a_credential_is_found_in_every_authority_an_http_client_could_read() {
    // Codex and CodeRabbit review on #2281. Each shape once hid its
    // credential from the refusal, the redaction, or both.
    for (bad, said) in [
        // One slash: WHATWG URL parsing still reads the authority after it.
        (
            "http:/alice:hunter2@127.0.0.1:8597/v1",
            "http:/***@127.0.0.1:8597/v1",
        ),
        // Three slashes: the extra one is skipped, not an empty authority.
        (
            "http:///alice:hunter2@127.0.0.1:8597/v1",
            "http:///***@127.0.0.1:8597/v1",
        ),
        // Backslashes, which a special scheme reads as slashes.
        (
            "http:\\\\alice:hunter2@127.0.0.1:8597/v1",
            "http:\\\\***@127.0.0.1:8597/v1",
        ),
        // No slash at all.
        (
            "HTTP:alice:hunter2@127.0.0.1:8597/v1",
            "***@127.0.0.1:8597/v1",
        ),
        // Two authorities, two credentials: both go.
        (
            "http://alice:one@outer/http://bob:two@inner/v1",
            "http://***@outer/http://***@inner/v1",
        ),
    ] {
        assert!(endpoint_has_credentials(bad), "`{bad}` carries userinfo");
        assert!(
            normalize_local_endpoint(bad).is_none(),
            "`{bad}` must not be storable"
        );
        assert_eq!(redact_endpoint(bad), said, "`{bad}`");
    }
    // A path is still a path. A gateway that proxies to another URL, with
    // an `@` later in that path, carries no credential and stays storable.
    let gateway = "https://gateway.example/proxy/http://upstream/@me";
    assert!(!endpoint_has_credentials(gateway));
    assert_eq!(normalize_local_endpoint(gateway).as_deref(), Some(gateway));
    for good in [
        gateway,
        "http://127.0.0.1:8597/v1",
        "http://[::1]:11434/v1",
        "https://api.acme.example:8443/v1/@me",
        "localhost:1234/v1",
    ] {
        assert!(!endpoint_has_credentials(good), "`{good}` has no userinfo");
        assert_eq!(redact_endpoint(good), good);
    }
}

#[test]
fn a_scheme_in_a_well_formed_path_is_not_an_authority() {
    // Codex review on #2281: WHATWG parsing gives this endpoint no userinfo.
    // `http:user@example.com` is path text, so the endpoint is accepted.
    let gateway = "https://gateway.example/proxy/http:user@example.com/v1";
    assert!(!endpoint_has_credentials(gateway));
    assert_eq!(normalize_local_endpoint(gateway).as_deref(), Some(gateway));
    // What is *said* about it still masks the segment that looks like one:
    // the redaction reads wider than the refusal, by design.
    assert_eq!(
        redact_endpoint(gateway),
        "https://gateway.example/proxy/http:***@example.com/v1"
    );
    // A port-less host that happens to end in `:` does not start a hop.
    assert!(!endpoint_has_credentials("http://localhost:/v1/@me"));
    // Nor does a host *named* like a scheme with one slash after it: every
    // parser reads `http://http:/v1@beta` as host `http`, empty port, path
    // `/v1@beta`. It is storable; the wider redaction still masks the
    // lookalike when it is said.
    let empty_port = "http://http:/v1@beta";
    assert!(!endpoint_has_credentials(empty_port));
    assert_eq!(
        normalize_local_endpoint(empty_port).as_deref(),
        Some(empty_port)
    );
    assert_eq!(redact_endpoint(empty_port), "http://http:/***@beta");
    // And a doubled scheme still does — the one case a hop exists for.
    assert!(endpoint_has_credentials(
        "https://http://alice@api.acme.example/v1"
    ));
}

#[test]
fn tabs_and_line_breaks_do_not_hide_a_credential() {
    // Codex review on #2281: a URL parser removes ASCII tab, LF and CR
    // wherever they appear, so each of these reaches a client carrying
    // `alice:hunter2`.
    for bad in [
        "http:\t//alice:hunter2@127.0.0.1:8597/v1",
        "http://ali\nce:hunter2@127.0.0.1:8597/v1",
        "http://http:\t//alice:hunter2@127.0.0.1:8597/v1",
        "http://alice:hunter2\r@127.0.0.1:8597/v1",
    ] {
        assert!(endpoint_has_credentials(bad), "{bad:?} carries userinfo");
        assert!(
            normalize_local_endpoint(bad).is_none(),
            "{bad:?} must not be storable"
        );
        let said = redact_endpoint(bad);
        assert!(
            !said.contains("hunter2") && !said.contains("alice"),
            "{bad:?} redacted to {said:?}"
        );
    }
    assert_eq!(
        redact_endpoint("http:\t//alice:hunter2@127.0.0.1:8597/v1"),
        "http://***@127.0.0.1:8597/v1"
    );
}

#[test]
fn an_at_sign_in_the_path_is_not_a_credential() {
    // The `@` has to be inside the authority. A path may legitimately carry
    // one, and refusing those would reject perfectly good endpoints.
    for good in [
        "https://api.acme.example/v1/@me",
        "https://api.acme.example/v1?to=a@b",
        "https://api.acme.example/v1#a@b",
    ] {
        assert!(!endpoint_has_credentials(good), "`{good}` has no userinfo");
        assert_eq!(redact_endpoint(good), good);
    }
}

#[test]
fn redacting_an_endpoint_removes_the_credential_and_nothing_else() {
    // Observed in the incident: reqwest masks userinfo in its own error
    // Display (`for url (http://127.0.0.1:8597/v1/models)`), and then the
    // handler's own `format!` put it back from the endpoint we hold.
    assert_eq!(
        redact_endpoint("http://alice:hunter2@127.0.0.1:8597/v1"),
        "http://***@127.0.0.1:8597/v1"
    );
    assert_eq!(
        redact_endpoint("https://alice@api.acme.example/v1"),
        "https://***@api.acme.example/v1"
    );
    // The last `@` in the authority is the delimiter, so a password
    // containing one is removed whole rather than half-left behind.
    assert_eq!(
        redact_endpoint("http://alice:hun@ter2@127.0.0.1:8597/v1"),
        "http://***@127.0.0.1:8597/v1"
    );
    // Scheme-less, as `normalize_setup_base_url` accepts.
    assert_eq!(
        redact_endpoint("alice:hunter2@localhost:1234/v1"),
        "***@localhost:1234/v1"
    );
    // Nothing to redact: byte-for-byte the same endpoint, trimmed.
    assert_eq!(
        redact_endpoint("  https://api.openai.com/v1  "),
        "https://api.openai.com/v1"
    );
}

#[test]
fn only_http_and_https_are_endpoints() {
    // Rejected here rather than at the probe, because this is the one
    // category whose endpoint the operator types — and the connect flow's
    // ordering says reject before any write.
    for bad in [
        "file:///etc/passwd",
        "ftp://acme.example/v1",
        "localhost:11434",
        "",
        "   ",
        "http://",
    ] {
        assert!(
            normalize_local_endpoint(bad).is_none(),
            "`{bad}` is not an endpoint"
        );
    }
}
/// The defect: `output_modalities` and `limit` were applied on the
/// authenticated path only, so the connect probe and the post-404 fallback
/// took OpenRouter's defaults — text-only, capped at 500 — and a company
/// whose `vision-v1` tier needs a vision model saw a picker with none.
#[test]
fn every_openrouter_catalogue_read_asks_for_the_whole_catalogue() {
    for endpoint in [
        "https://openrouter.ai/api/v1",
        "https://openrouter.ai/api/v1/",
        "https://eu.openrouter.ai/api/v1",
    ] {
        let query = catalog_query(endpoint);
        assert!(
            query.contains("output_modalities=all"),
            "{endpoint} would silently drop every non-text model"
        );
        assert!(
            query.contains("limit=1000"),
            "{endpoint} would truncate at OpenRouter's default of 500"
        );
    }
    // The authenticated path already asked for both; the point is that the
    // two now agree rather than each carrying its own copy.
    let scoped = scoped_catalog_path("https://openrouter.ai/api/v1", true).expect("scoped");
    for parameter in ["output_modalities=all", "limit=1000"] {
        assert!(scoped.contains(parameter), "{scoped}");
        assert!(catalog_query("https://openrouter.ai/api/v1").contains(parameter));
    }
}

#[test]
fn a_non_openrouter_endpoint_gets_no_query_string() {
    // These parameters are OpenRouter's, not the OpenAI dialect's. Fireworks
    // rejects unknown fields outright and several hosts 400 on an
    // unrecognised query, so this must not become a blanket addition.
    for endpoint in [
        "https://api.anthropic.com/v1",
        "http://localhost:11434/v1",
        "https://api.groq.com/openai/v1",
    ] {
        assert_eq!(catalog_query(endpoint), "", "{endpoint}");
    }
}

#[test]
fn only_openrouters_own_host_gets_the_account_scoped_catalogue() {
    assert!(is_openrouter_endpoint("https://openrouter.ai/api/v1"));
    assert!(is_openrouter_endpoint("https://openrouter.ai/api/v1/"));
    assert!(is_openrouter_endpoint("https://eu.openrouter.ai/api/v1"));
    // The platform proxy fronts OpenRouter and serves the same catalogue,
    // but the account behind it is the server's, not the tenant's — and it
    // is a different host, which is the whole point of matching on one.
    assert!(!is_openrouter_endpoint(
        "https://api.tinyhumans.ai/openai/v1"
    ));
    assert!(!is_openrouter_endpoint(
        "https://openrouter.ai.example.com/v1"
    ));
    assert!(!is_openrouter_endpoint("http://127.0.0.1:11434/v1"));
}

#[test]
fn the_scoped_catalogue_needs_both_the_host_and_a_credential() {
    let path = scoped_catalog_path(super::super::OPENROUTER_BASE_URL, true)
        .expect("OpenRouter with a key reads the account-scoped list");
    assert!(path.starts_with("/models/user"));
    // `output_modalities` defaults to `text`, so leaving it off silently
    // drops every image, audio and embedding model.
    assert!(path.contains("output_modalities=all"), "{path}");
    assert!(path.contains("limit=1000"), "{path}");

    // Account-scoping is a question about a key. With none there is nothing
    // to scope to, and the public registry is the honest answer.
    assert!(scoped_catalog_path(super::super::OPENROUTER_BASE_URL, false).is_none());
    // Host-specific on purpose, the same way the Azure deployment-name rule
    // is. Every other provider has its own account restrictions or none.
    assert!(scoped_catalog_path("https://api.openai.com/v1", true).is_none());
    assert!(scoped_catalog_path("https://api.tinyhumans.ai/openai/v1", true).is_none());
}
