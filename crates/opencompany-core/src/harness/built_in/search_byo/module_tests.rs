//! The private module configuration is the only thing that carries a company's
//! key, so these pin its shape, and the lock's precondition.

use super::*;

fn tenant(provider: &str, key: Option<&str>, endpoint: Option<&str>) -> TenantSearch {
    TenantSearch::for_test(provider, key, endpoint)
}

#[test]
fn only_the_configured_provider_is_usable() {
    let config = configuration(&tenant("brave", Some("brave-key"), None));
    assert_eq!(config.providers.len(), 1, "one provider, never a ladder");
    let brave = config.providers.get("brave").expect("brave");
    assert!(brave.enabled);
    assert_eq!(brave.credential.as_deref(), Some("brave-key"));
    assert_eq!(
        brave.route,
        ProviderRoute::Direct,
        "never the managed route"
    );
    assert!(config.enabled);
}

#[test]
fn the_managed_backend_is_left_unconfigured() {
    // A BYO call must not be able to reach the platform's account: the bill
    // would move from the company to the host.
    let config = configuration(&tenant("exa", Some("exa-key"), None));
    let backend = &config.backend;
    assert!(backend.credential.is_none(), "no managed credential");
    assert!(backend.base_url.is_none(), "no managed base url");
}

#[test]
fn every_provider_tool_is_presented_under_its_own_name() {
    let config = configuration(&tenant("exa", Some("exa-key"), None));
    assert_eq!(
        config.presentation.mode,
        PresentationMode::AllTools,
        "roles mode would collapse the extras into one role tool",
    );
}

#[test]
fn a_self_hosted_instance_is_addressed_by_url_and_needs_no_key() {
    let config = configuration(&tenant("searxng", None, Some("https://searx.example")));
    let searxng = config.providers.get("searxng").expect("searxng");
    assert_eq!(searxng.base_url.as_deref(), Some("https://searx.example"));
    assert!(searxng.credential.is_none());
    assert_eq!(searxng.default_language.as_deref(), Some("all"));
}

#[test]
fn a_keyed_provider_gets_no_endpoint_from_a_stray_stored_one() {
    // `endpoint` is SearXNG's addressing. A stored endpoint on a keyed provider
    // must not redirect that provider's traffic somewhere else.
    let config = configuration(&tenant("brave", Some("k"), Some("https://evil.example")));
    let brave = config.providers.get("brave").expect("brave");
    assert!(brave.base_url.is_none(), "{:?}", brave.base_url);
}

#[test]
fn the_configuration_round_trips_as_the_module_reads_it() {
    let config = configuration(&tenant("querit", Some("q-key"), None));
    let encoded = serde_json::to_value(&config).expect("encodes");
    let decoded: tinysearch_bus::SearchConfig =
        serde_json::from_value(encoded).expect("the module's own type decodes it");
    assert_eq!(
        decoded
            .providers
            .get("querit")
            .and_then(|p| p.credential.clone()),
        Some("q-key".to_string()),
    );
}

#[test]
fn the_module_id_is_the_one_openhuman_registers() {
    assert_eq!(MODULE_ID, "tinysearch");
}

#[test]
fn the_same_connection_fingerprints_the_same_and_is_not_re_sent() {
    let first = serde_json::to_value(configuration(&tenant("exa", Some("k"), None))).expect("json");
    let again = serde_json::to_value(configuration(&tenant("exa", Some("k"), None))).expect("json");
    assert_eq!(fingerprint(&first), fingerprint(&again));
}

#[test]
fn a_rotated_key_fingerprints_differently_and_so_reconfigures() {
    // The whole point of hashing the credential: a company that rotates its key
    // with everything else unchanged must not keep searching on the old one.
    let before =
        serde_json::to_value(configuration(&tenant("exa", Some("old"), None))).expect("json");
    let after =
        serde_json::to_value(configuration(&tenant("exa", Some("new"), None))).expect("json");
    assert_ne!(fingerprint(&before), fingerprint(&after));
}

#[test]
fn a_different_provider_fingerprints_differently() {
    let brave =
        serde_json::to_value(configuration(&tenant("brave", Some("k"), None))).expect("json");
    let exa = serde_json::to_value(configuration(&tenant("exa", Some("k"), None))).expect("json");
    assert_ne!(fingerprint(&brave), fingerprint(&exa));
}

/// The one test that exercises the chain this module exists for, end to end,
/// against a real provider: load the module, hand it a company's own key as
/// private configuration, call a catalogue tool, and render what comes back.
///
/// Everything else here asserts the configuration we *build*. Nothing else
/// asserts that `ensure_loaded_within` finds and loads the artifact, that
/// `reinitialize_module` accepts our `SearchConfig`, that the `(request,)` tuple
/// is the encoding the bus expects, that `ExecuteToolResponse` deserializes, or
/// that `render` produces text an agent can read. Those are five links, and a
/// green suite says nothing about any of them.
///
/// `#[ignore]`d: it needs a real Exa key and the built module artifact, and it
/// reaches the network. Run it by hand:
///
/// ```text
/// cargo build --release --lib \
///   --manifest-path vendor/openhuman/vendor/tinysearch/crates/tinysearch/Cargo.toml
/// set -a; . ~/.config/tinyhivemind/live.env; set +a
/// OPENCOMPANY_TINYSEARCH_MODULE=$PWD/vendor/openhuman/vendor/tinysearch/target/release/libtinysearch.dylib \
///   cargo test -p opencompany-core --features openhuman,tinymemory --lib \
///   live_a_byo_key_searches_and_returns_results -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "needs a real Exa key, the built TinySearch artifact, and the network"]
async fn live_a_byo_key_searches_and_returns_results() {
    let key = std::env::var("EXA_API_KEY").expect("EXA_API_KEY");
    let artifact = std::env::var(MODULE_PATH_ENV)
        .unwrap_or_else(|_| panic!("set {MODULE_PATH_ENV} to the built libtinysearch artifact"));
    assert!(
        std::path::Path::new(&artifact).is_file(),
        "{MODULE_PATH_ENV} does not name a file: {artifact}",
    );

    let tenant = TenantSearch::for_test("exa", Some(&key), None);
    let request = tinysearch_bus::ExecuteToolRequest {
        name: "exa_search".into(),
        arguments: serde_json::json!({
            "query": "OpenCompany company runtime for AI agents",
            "max_results": 3,
        }),
    };

    let response = match execute(&tenant, request).await {
        Ok(response) => response,
        Err(error) => panic!(
            "the BYO chain failed. This is the failure the unit tests cannot see — \
             module load, reconfiguration, bus encoding, decode or render: {error}"
        ),
    };

    eprintln!("provider: {}", response.provider);
    eprintln!("status: {:?}", response.status);
    eprintln!("fallbacks: {:?}", response.fallback_from);
    eprintln!("results: {}", response.results.len());
    for result in &response.results {
        eprintln!("  - {} :: {}", result.title, result.url);
    }

    assert_eq!(
        response.provider, "exa",
        "the search did not run on the company's own provider",
    );
    assert!(
        response.fallback_from.is_empty(),
        "the configured provider failed and something else answered: {:?}",
        response.fallback_from,
    );
    assert!(
        !response.results.is_empty(),
        "no results came back, so the key or the route is wrong",
    );

    // And the rendering an agent would actually read.
    let rendered = openhuman_core::search::render::render(&response, "OpenCompany", 3, false);
    let text = format!("{rendered:?}");
    eprintln!("--- rendered for the model ---\n{text}");
    assert!(
        text.contains("http"),
        "the rendered result carries no URL for an agent to cite: {text}",
    );
}
