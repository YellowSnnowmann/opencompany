//! Catalogue-property tests: entries, endpoints, auth styles and slug
//! uniqueness across the whole cloud/local/CLI catalogue (split out of
//! `catalogue_tests.rs`).

use super::*;

#[test]
fn the_catalogue_ships_the_counts_the_plan_names() {
    assert_eq!(CLOUD_PROVIDERS.len(), 27, "cloud providers");
    assert_eq!(LOCAL_RUNTIMES.len(), 3, "local runtimes");
    assert_eq!(CLI_LOGINS.len(), 2, "CLI logins");
}

#[test]
fn every_entry_has_a_parseable_endpoint_and_a_known_auth_style() {
    for provider in CLOUD_PROVIDERS {
        assert!(!provider.slug.is_empty(), "empty slug");
        assert!(!provider.label.is_empty(), "{}: empty label", provider.slug);
        assert!(
            provider.endpoint.starts_with("https://"),
            "{}: cloud endpoints are https",
            provider.slug
        );
        assert!(
            endpoint_host(provider.endpoint).is_some(),
            "{}: endpoint has no parseable host",
            provider.slug
        );
        assert!(
            matches!(provider.auth, AuthStyle::Bearer | AuthStyle::Anthropic),
            "{}: a cloud provider authenticates",
            provider.slug
        );
    }
    for runtime in LOCAL_RUNTIMES {
        assert!(!runtime.slug.is_empty(), "empty slug");
        assert!(!runtime.label.is_empty(), "{}: empty label", runtime.slug);
        if let Some(endpoint) = runtime.default_endpoint {
            assert!(
                endpoint_host(endpoint).is_some(),
                "{}: default endpoint has no parseable host",
                runtime.slug
            );
        }
    }
}

#[test]
fn slugs_are_unique_across_the_whole_catalogue() {
    let mut seen: Vec<&str> = Vec::new();
    for slug in CLOUD_PROVIDERS
        .iter()
        .map(|p| p.slug)
        .chain(LOCAL_RUNTIMES.iter().map(|r| r.slug))
    {
        assert!(!seen.contains(&slug), "duplicate slug {slug}");
        seen.push(slug);
    }
}

#[test]
fn anthropic_is_the_only_non_bearer_cloud_entry() {
    let anthropic: Vec<&str> = CLOUD_PROVIDERS
        .iter()
        .filter(|p| p.auth == AuthStyle::Anthropic)
        .map(|p| p.slug)
        .collect();
    assert_eq!(anthropic, vec!["anthropic"]);
}

#[test]
fn minimax_keeps_the_openai_surface_that_is_the_fix() {
    // Reverting to `/anthropic` 404s both chat and the model listing. The
    // comment on the row says why; this makes reverting it fail loudly.
    let minimax = cloud_provider("minimax").expect("minimax is in the catalogue");
    assert_eq!(minimax.endpoint, "https://api.minimax.io/v1");
    assert_eq!(minimax.auth, AuthStyle::Bearer);
}

#[test]
fn the_managed_first_party_backend_is_not_a_row() {
    // openhuman's 27th entry is their own managed backend. Ours is modelled
    // separately, with its own auth path; a row here would give it a second
    // bearer-shaped identity.
    assert!(cloud_provider("openhuman").is_none());
}

#[test]
fn endpoint_host_drops_scheme_userinfo_port_and_path() {
    assert_eq!(
        endpoint_host("https://api.openai.com/v1").as_deref(),
        Some("api.openai.com")
    );
    assert_eq!(
        endpoint_host("api.groq.com/openai/v1").as_deref(),
        Some("api.groq.com")
    );
    assert_eq!(
        endpoint_host("http://user:pw@host.example:8080/v1").as_deref(),
        Some("host.example")
    );
    assert_eq!(
        endpoint_host("http://[::1]:11434/v1").as_deref(),
        Some("::1")
    );
    assert_eq!(
        endpoint_host("HTTPS://API.OpenAI.com/v1").as_deref(),
        Some("api.openai.com")
    );
    assert_eq!(endpoint_host("   "), None);
}

#[test]
fn only_openai_serves_the_responses_api_fallback() {
    assert!(!endpoint_is_chat_completions_only(
        "https://api.openai.com/v1"
    ));
    // The custom-slug gap: a user-defined slug aimed at a known chat-only
    // host still must not try `/responses`.
    assert!(endpoint_is_chat_completions_only(
        "https://integrate.api.nvidia.com/v1"
    ));
    assert!(endpoint_is_chat_completions_only(
        "https://api.groq.com/openai/v1"
    ));
    // A genuinely unknown proxy keeps the permissive fallback.
    assert!(!endpoint_is_chat_completions_only(
        "https://proxy.acme.dev/v1"
    ));
}

#[test]
fn azure_is_detected_by_host_including_subdomains_and_sovereign_clouds() {
    assert!(is_azure_endpoint(
        "https://my-resource.openai.azure.com/openai/v1"
    ));
    assert!(is_azure_endpoint(
        "https://r.services.ai.azure.com/openai/v1"
    ));
    assert!(is_azure_endpoint(
        "https://r.cognitiveservices.azure.com/openai/v1"
    ));
    assert!(is_azure_endpoint("https://r.openai.azure.us/openai/v1"));
    assert!(is_azure_endpoint("https://r.openai.azure.cn/openai/v1"));
    assert!(!is_azure_endpoint("https://api.openai.com/v1"));
}

#[test]
fn the_foundry_serverless_hosts_are_deliberately_not_azure() {
    // Classifying these would relabel a correct model id as a deployment
    // name — the exact confusion the Azure rule exists to prevent. This test
    // is the guard against adding them back for symmetry.
    assert!(!is_azure_endpoint(
        "https://r.inference.ai.azure.com/models"
    ));
    assert!(!is_azure_endpoint("https://r.models.ai.azure.com/models"));
}

#[test]
fn a_suffix_that_merely_ends_with_an_azure_host_is_not_azure() {
    // `notopenai.azure.com.evil.test` must not match, and neither must a
    // host that merely ends in the same characters without a dot boundary.
    assert!(!is_azure_endpoint("https://myopenai.azure.comx/v1"));
    assert!(!is_azure_endpoint("https://openai.azure.com.evil.test/v1"));
}

#[test]
fn codex_stores_under_openai_so_its_connected_check_can_match() {
    let codex = cli_login("codex").expect("codex is a CLI login");
    assert_eq!(codex.stored_slug, "openai");
    assert!(codex.probes);
    let claude = cli_login("claude-code").expect("claude-code is a CLI login");
    assert_eq!(claude.stored_slug, "claude-code");
    // No key changes hands, so there is nothing for a probe to present.
    assert!(!claude.probes);
}

#[test]
fn reserved_slugs_cover_cloud_local_and_stored_cli_names() {
    assert!(is_reserved_slug("openrouter"));
    assert!(is_reserved_slug("ollama"));
    assert!(is_reserved_slug("claude-code"));
    // Codex stores under `openai`, which is already reserved as a cloud row.
    assert!(is_reserved_slug("openai"));
    assert!(!is_reserved_slug("acme-gateway"));
}

#[test]
fn the_proxy_url_follows_api_url_and_defaults_to_the_catalogue_endpoint() {
    let row = cloud_provider("tinyhumans").expect("tinyhumans is in the catalogue");
    assert_eq!(
        tinyhumans_proxy_url(crate::app::config::DEFAULT_API_URL),
        row.endpoint
    );
    assert_eq!(
        tinyhumans_proxy_url("http://localhost:5005/"),
        "http://localhost:5005/agent-integrations/openrouter"
    );
    assert_eq!(
        tinyhumans_proxy_url("https://staging-api.tinyhumans.ai"),
        "https://staging-api.tinyhumans.ai/agent-integrations/openrouter"
    );
    // Blank falls back to the row rather than minting a bare path.
    assert_eq!(tinyhumans_proxy_url("  "), row.endpoint);
    // Whatever it is, the shape reads as paged.
    assert_eq!(
        catalog_shape_for("openrouter", &tinyhumans_proxy_url("http://localhost:5005")),
        CatalogShape::PagedEnvelope
    );
}

#[test]
fn tinyhumans_owns_its_slug_and_is_a_catalogue_row() {
    // `provider/tinyhumans/key` is where the managed credential lives, and
    // `managed` is the word the route grammar uses. A custom provider named
    // "TinyHumans" slugified straight into the first: adding it stored a
    // vendor key where managed reads it, a managed test presented that key
    // to the platform endpoint, and removing the custom row took managed's
    // credential with it.
    assert!(is_reserved_slug(super::super::MANAGED_SLUG));
    assert!(is_reserved_slug("tinyhumans"));
    assert!(is_reserved_slug("managed"));
    // Keys rework (#2306) slice 2a: unlike before, `tinyhumans` IS now a
    // catalogue row too — reservation and catalogue membership are no
    // longer mutually exclusive for this slug.
    assert!(cloud_provider("tinyhumans").is_some());
}

#[test]
fn tinyhumans_is_a_bearer_row_on_the_proxy() {
    let tinyhumans = cloud_provider("tinyhumans").expect("tinyhumans is in the catalogue");
    assert_eq!(
        tinyhumans.endpoint,
        "https://api.tinyhumans.ai/agent-integrations/openrouter"
    );
    assert_eq!(auth_style_for("tinyhumans"), AuthStyle::Bearer);
    assert_eq!(tinyhumans.key_placeholder, Some("th-..."));
}

#[test]
fn catalog_shape_is_paged_only_for_tinyhumans_or_the_proxy_path() {
    let proxy = "https://api.tinyhumans.ai/agent-integrations/openrouter";
    assert_eq!(
        catalog_shape_for("tinyhumans", proxy),
        CatalogShape::PagedEnvelope
    );
    // Whitespace around the kind, and a blank base URL, still recognise
    // the kind on its own.
    assert_eq!(
        catalog_shape_for(" tinyhumans ", ""),
        CatalogShape::PagedEnvelope
    );
    // A trailing slash on the proxy path still matches.
    assert_eq!(
        catalog_shape_for("openrouter", &format!("{proxy}/")),
        CatalogShape::PagedEnvelope
    );
    assert_eq!(
        catalog_shape_for("openrouter", "https://api.tinyhumans.ai/openai/v1"),
        CatalogShape::OpenAi
    );
    assert_eq!(
        catalog_shape_for("custom", "http://127.0.0.1:8099/v1"),
        CatalogShape::OpenAi
    );
    for provider in CLOUD_PROVIDERS.iter().filter(|p| p.slug != "tinyhumans") {
        assert_eq!(
            catalog_shape_for(provider.slug, provider.endpoint),
            CatalogShape::OpenAi,
            "{}: only tinyhumans/the proxy path pages",
            provider.slug
        );
    }
}

#[test]
fn a_kind_lands_in_the_category_its_scrub_rule_needs() {
    assert_eq!(category_of("openrouter"), Category::Cloud);
    assert_eq!(category_of("ollama"), Category::Local);
    assert_eq!(category_of("lmstudio"), Category::Local);
    assert_eq!(category_of("claude-code"), Category::Cli);
    // Codex stores under `openai`, and `openai` is a cloud row in its own
    // right — so the shared slug stays Cloud. Reading it as a CLI login
    // would give the OpenAI row the CLI's slug-less scrub rule and orphan
    // every route naming it.
    assert_eq!(category_of("openai"), Category::Cloud);
    assert_eq!(category_of("acme-gateway"), Category::Cloud);
}

/// `needs_key` is enforced by the host, so a wrong `true` is not a cosmetic
/// defect — it makes the runtime unaddable. None of the three projects called
/// "omlx" requires a key, and two have no auth mechanism at all, so no local
/// runtime may demand one.
#[test]
fn no_local_runtime_demands_a_key() {
    let with_keys: Vec<&str> = LOCAL_RUNTIMES
        .iter()
        .filter(|r| r.needs_key)
        .map(|r| r.slug)
        .collect();
    assert!(
        with_keys.is_empty(),
        "a local runtime that demands a key cannot be added at all: {with_keys:?}"
    );
}

#[test]
fn no_other_module_writes_its_own_openrouter_attribution_headers() {
    // The failure this guards is not a wrong value, it is a SECOND value.
    // `harness::built_in::provider` and `harness::roster_build` each spelled
    // the referer out, with different hosts, so one company's turn traffic
    // and its roster-build traffic reached OpenRouter's dashboard as two
    // apps. Both copies looked right in isolation, which is why reading
    // either one never found it.
    //
    // So the assertion is about shape rather than content: any file that
    // mentions the header must reach this constant for its value.
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            if path.file_name().and_then(|n| n.to_str()) == Some("catalogue.rs") {
                continue;
            }
            // Both headers, not just the first. `X-Title` has the same
            // divergence mode — a module spells it out with a value of its
            // own and nothing notices — and checking one of a pair is how
            // the guard ends up proving less than it appears to.
            let spells_out_referer =
                source.contains("\"HTTP-Referer\"") && !source.contains("OPENROUTER_REFERER");
            let spells_out_title =
                source.contains("\"X-Title\"") && !source.contains("OPENROUTER_TITLE");
            if spells_out_referer || spells_out_title {
                offenders.push(path.display().to_string());
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these files write an OpenRouter attribution header without reading \
             `catalogue::OPENROUTER_REFERER`, which is how the two copies \
             diverged the first time: {offenders:?}"
    );
}
