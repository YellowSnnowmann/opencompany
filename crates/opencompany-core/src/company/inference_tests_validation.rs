//! Write-only-key, validation and first-run-probe tests (split out of
//! `inference_tests.rs`).

use super::inference_tests_support::*;
use super::*;

// ---- write-only key ----------------------------------------------------

#[tokio::test]
async fn key_is_write_only_and_never_serialized() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    store_key(&company, &secrets, "sk-super-secret")
        .await
        .unwrap();
    let decl = resolve_effective(&company, &inference("openrouter"), None, &secrets)
        .await
        .unwrap()
        .unwrap();
    // The key resolves for request building…
    assert_eq!(bearer(&decl).await.as_deref(), Some("sk-super-secret"));
    // …but never appears in the Debug rendering.
    let debug = format!("{decl:?}");
    assert!(!debug.contains("sk-super-secret"), "{debug}");
    assert!(debug.contains("<redacted>"), "{debug}");
}

#[tokio::test]
async fn cleared_key_reads_back_unconfigured() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    store_key(&company, &secrets, "tok").await.unwrap();
    assert!(key_configured(&company, &secrets, None).await.unwrap());
    clear_key(&company, &secrets).await.unwrap();
    assert!(!key_configured(&company, &secrets, None).await.unwrap());
}

#[tokio::test]
async fn manifest_api_key_secret_is_the_fallback_key() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    // Only the manifest-named key holds a token; canonical key is cold.
    secrets
        .set(
            &company,
            "byo/openrouter",
            SecretValue("named-secret".into()),
        )
        .await
        .unwrap();
    let mut manifest = inference("openrouter");
    manifest.api_key_secret = Some("byo/openrouter".into());
    let decl = resolve_effective(&company, &manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bearer(&decl).await.as_deref(), Some("named-secret"));
}

#[tokio::test]
async fn managed_gateway_uses_its_explicitly_named_secret() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company,
            "byo/gateway",
            SecretValue("gateway-secret".into()),
        )
        .await
        .unwrap();
    let mut manifest = inference(LEGACY_MANAGED);
    manifest.base_url = Some("https://gateway.example/v1".into());
    manifest.api_key_secret = Some("byo/gateway".into());

    let decl = resolve_effective(&company, &manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(decl.base_url, "https://gateway.example/v1");
    assert!(!decl.is_proxied());
    assert_eq!(bearer(&decl).await.as_deref(), Some("gateway-secret"));
}

// ---- validation --------------------------------------------------------

#[test]
fn absent_section_is_inert() {
    assert!(validate_inference(&Inference::default()).is_empty());
}

#[test]
fn valid_configs_pass() {
    assert!(validate_inference(&inference("managed")).is_empty());
    assert!(validate_inference(&inference("openrouter")).is_empty());
    let mut ollama = inference("ollama");
    ollama.base_url = Some("http://localhost:11434/v1".into());
    assert!(
        validate_inference(&ollama).is_empty(),
        "{:?}",
        validate_inference(&ollama)
    );
}

#[test]
fn unknown_provider_is_rejected() {
    let problems = validate_inference(&inference("gpt5"));
    assert!(
        problems.iter().any(|p| p.contains("provider")),
        "{problems:?}"
    );
}

#[test]
fn ollama_and_openai_compatible_require_base_url() {
    let ollama = validate_inference(&inference("ollama"));
    assert!(
        ollama
            .iter()
            .any(|p| p.contains("base_url") && p.contains("required"))
    );
    let compat = validate_inference(&inference("openai_compatible"));
    assert!(
        compat
            .iter()
            .any(|p| p.contains("base_url") && p.contains("required"))
    );
}

#[test]
fn non_http_base_url_is_rejected() {
    let mut m = inference("openai_compatible");
    m.base_url = Some("ftp://x/v1".into());
    let problems = validate_inference(&m);
    assert!(problems.iter().any(|p| p.contains("http")), "{problems:?}");
}

#[test]
fn a_base_url_carrying_a_credential_is_rejected_and_never_echoed() {
    // Same rule as `api_key_secret` below, one field over: a credential
    // belongs in the write-only key slot, and a `base_url` is stored as
    // written and read back by every console reader.
    let mut m = inference("openai_compatible");
    m.base_url = Some("http://alice:hunter2@127.0.0.1:8597/v1".into());
    let problems = validate_inference(&m);
    assert!(
        problems.iter().any(|p| p.contains("username or password")),
        "{problems:?}"
    );
    // The refusal is the one moment this value is guaranteed to be shown to
    // somebody, so it must not quote the credential back.
    for problem in &problems {
        assert!(
            !problem.contains("hunter2") && !problem.contains("alice"),
            "a rejection echoed the credential it was rejecting: {problem}"
        );
    }

    // A malformed URL is quoted back redacted too — and the malformed ones
    // are the likeliest to have been typed by hand with a password in them.
    let mut bad = inference("openai_compatible");
    bad.base_url = Some("ftp://alice:hunter2@127.0.0.1/v1".into());
    for problem in validate_inference(&bad) {
        assert!(
            !problem.contains("hunter2"),
            "a rejection echoed the credential it was rejecting: {problem}"
        );
    }
}

#[test]
fn inline_credential_in_key_name_is_rejected() {
    let mut m = inference("openrouter");
    m.api_key_secret = Some("sk-or-v1-abcdef0123456789".into());
    let problems = validate_inference(&m);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("names a secret-store key")),
        "{problems:?}"
    );

    // A long opaque token with no separators is also caught.
    let mut m2 = inference("openrouter");
    m2.api_key_secret = Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEF".into());
    assert!(!validate_inference(&m2).is_empty());

    // A structured key name is accepted.
    let mut ok = inference("openrouter");
    ok.api_key_secret = Some("byo/openrouter".into());
    assert!(
        validate_inference(&ok).is_empty(),
        "{:?}",
        validate_inference(&ok)
    );
}

/// The isolation property named harnesses exist for: two `built_in`
/// harnesses on one company resolve independently, so one can ride the
/// subscription while the other runs on a key of its own.
#[tokio::test]
async fn two_harnesses_on_one_company_resolve_independently() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://env.example/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    let embedded = HarnessScope::default_harness("embedded");
    let deep = HarnessScope::named("deep");

    // Only `deep` gets a key.
    store_key_scoped(&company, &secrets, "sk-or-deep", &deep)
        .await
        .unwrap();

    let d = resolve_effective_scoped(
        &company,
        &inference("openrouter"),
        Some(&env),
        &secrets,
        &deep,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!d.is_proxied(), "deep pays its own way");
    assert_eq!(d.base_url, OPENROUTER_BASE_URL);
    assert_eq!(bearer(&d).await.as_deref(), Some("sk-or-deep"));

    let e = resolve_effective_scoped(
        &company,
        &inference("openrouter"),
        Some(&env),
        &secrets,
        &embedded,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(e.is_proxied(), "embedded is untouched by deep's key");
    assert_eq!(e.base_url, "https://env.example/v1");
    assert_eq!(bearer(&e).await.as_deref(), Some("platform-key"));
}

/// The default harness keeps the flat legacy keys, so a tenant whose console
/// already wrote `inference/key` keeps working with no migration — the store
/// has no rename, so getting this wrong would orphan every running company.
#[tokio::test]
async fn the_default_harness_reads_the_legacy_flat_keys() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    // Written the pre-harness way.
    store_key(&company, &secrets, "legacy-key").await.unwrap();
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: "openrouter".into(),
            base_url: None,
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();

    // Read back through the scoped path, as the default harness.
    let scope = HarnessScope::default_harness("embedded");
    assert_eq!(scope.key_key(), KEY_KEY);
    assert_eq!(scope.config_key(), RUNTIME_CONFIG_KEY);

    let decl = resolve_effective_scoped(&company, &Inference::default(), None, &secrets, &scope)
        .await
        .unwrap()
        .expect("the legacy config resolves");
    assert_eq!(decl.source, InferenceSource::Runtime);
    assert_eq!(bearer(&decl).await.as_deref(), Some("legacy-key"));

    // A named harness namespaces instead, and sees none of it.
    let named = HarnessScope::named("deep");
    assert_eq!(named.key_key(), "harness/deep/inference/key");
    assert_eq!(named.config_key(), "harness/deep/inference/config");
    assert!(
        load_runtime_config_scoped(&company, &secrets, &named)
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn provider_slugs_map_as_documented() {
    assert_eq!(provider_slug("openrouter"), "openrouter");
    assert_eq!(provider_slug("openai_compatible"), "byok");
    assert_eq!(provider_slug("ollama"), "ollama");
    // The legacy kind slugs as what it now is, so historical usage rows and
    // new ones aggregate together.
    assert_eq!(provider_slug(LEGACY_MANAGED), "openrouter");
    // An unknown kind is never folded into a real provider's attribution.
    assert_eq!(provider_slug("mystery"), "unknown");
}

#[test]
fn effective_base_url_defaults_per_provider() {
    assert_eq!(
        effective_base_url(LEGACY_MANAGED, None),
        OPENROUTER_BASE_URL
    );
    assert_eq!(effective_base_url("openrouter", None), OPENROUTER_BASE_URL);
    assert_eq!(effective_base_url("ollama", None), OLLAMA_DEFAULT_BASE_URL);
    assert_eq!(
        effective_base_url("openrouter", Some("https://proxy/v1")),
        "https://proxy/v1"
    );
}

/// The defect: `lmstudio` and `omlx` had no arm, so the `_ =>` fallback
/// handed a **local** runtime OpenRouter's URL — and the decl carries the
/// credential the operator typed for the machine on their desk. A local
/// runtime's turns, and its key, would have left the host.
#[test]
fn a_local_runtime_never_falls_back_to_a_third_party_endpoint() {
    for kind in ["lmstudio", "omlx", "openai_compatible", "some-unknown-kind"] {
        let resolved = effective_base_url(kind, None);
        assert_ne!(
            resolved, OPENROUTER_BASE_URL,
            "{kind} must not inherit a third-party endpoint"
        );
        assert!(
            resolved.is_empty(),
            "{kind} has no guessable endpoint, so it must fail loudly: {resolved}"
        );
    }
    // An override is still honoured, which is the whole of how these kinds
    // are meant to be addressed.
    assert_eq!(
        effective_base_url("lmstudio", Some("http://localhost:1234/v1")),
        "http://localhost:1234/v1"
    );
}

#[test]
fn setup_accepts_the_localhost_spelling_local_model_apps_display() {
    assert_eq!(
        normalize_setup_base_url("ollama", Some("localhost:6969")),
        Some("http://localhost:6969/v1".to_string())
    );
    assert_eq!(
        normalize_setup_base_url("openai_compatible", Some("http://127.0.0.1:1234/v1/")),
        Some("http://127.0.0.1:1234/v1".to_string())
    );
    assert_eq!(
        normalize_setup_base_url("openai_compatible", Some("https://llm.test/api")),
        Some("https://llm.test/api".to_string())
    );
}

#[test]
fn setup_normalisation_reads_an_uppercase_scheme_as_a_scheme() {
    // Never a second scheme in front of the first: that shape is how a
    // credential once hid from `endpoint_has_credentials`.
    assert_eq!(
        normalize_setup_base_url("openai_compatible", Some("HTTP://127.0.0.1:1234")).as_deref(),
        Some("HTTP://127.0.0.1:1234/v1")
    );
    assert_eq!(
        normalize_setup_base_url("ollama", Some("HTTPS://llm.test/api")).as_deref(),
        Some("HTTPS://llm.test/api")
    );
    let credentialed =
        normalize_setup_base_url("openai_compatible", Some("HTTP://alice:hunter2@host/v1"))
            .expect("normalised");
    assert!(
        catalogue::endpoint_has_credentials(&credentialed),
        "`{credentialed}` must still read as carrying a credential"
    );
    // A single-slash scheme is a scheme, not a host: it is repaired to
    // `http://` rather than having a second one prepended, so the credential
    // stays in the first authority where the refusal reads it.
    let single_slash =
        normalize_setup_base_url("openai_compatible", Some("http:/alice:hunter2@host/v1"))
            .expect("normalised");
    assert_eq!(single_slash, "http://alice:hunter2@host/v1");
    assert!(
        catalogue::endpoint_has_credentials(&single_slash),
        "`{single_slash}` must still read as carrying a credential"
    );
    assert_eq!(
        normalize_setup_base_url("ollama", Some("http:/localhost:11434")).as_deref(),
        Some("http://localhost:11434/v1")
    );
    assert_eq!(
        normalize_setup_base_url("openai_compatible", Some("HTTPS:///llm.test/api")).as_deref(),
        Some("HTTPS://llm.test/api")
    );
}

// ---- first-run probe (decl_for_probe) ----------------------------------

pub(super) fn managed_env() -> EnvDefault {
    EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::from_value("platform-key"),
    }
}

/// The managed card sends `provider = "managed"` and, because it has no URL
/// field, whatever `base_url` a previously-picked provider left in the form —
/// `openrouter.ai` here. The probe must ignore that stale endpoint and reach
/// the managed endpoint with the managed credential. On the pre-fix code this
/// went direct to `openrouter.ai` with no credential and 401'd.
#[tokio::test]
async fn managed_probe_ignores_stale_base_url_and_uses_managed_endpoint() {
    let env = managed_env();
    let decl = decl_for_probe(
        "managed",
        Some("https://openrouter.ai/api/v1"),
        None,
        Some(&env),
    );
    assert_eq!(decl.base_url, "https://env.example/openai/v1");
    assert!(decl.is_proxied());
    assert_eq!(bearer(&decl).await.as_deref(), Some("platform-key"));
}

/// A managed probe where the operator supplied their own TinyHumans key still
/// reaches the managed endpoint — not `openrouter.ai` — carrying that key.
#[tokio::test]
async fn managed_probe_with_own_key_keeps_the_managed_endpoint() {
    let env = managed_env();
    let decl = decl_for_probe(
        "managed",
        Some("https://openrouter.ai/api/v1"),
        Some("th-key"),
        Some(&env),
    );
    assert_eq!(decl.base_url, "https://env.example/openai/v1");
    assert!(decl.is_proxied());
    assert_eq!(bearer(&decl).await.as_deref(), Some("th-key"));
}

/// A host holding no managed credential probes the managed endpoint honestly
/// unauthenticated — so the failure names `api.tinyhumans.ai`, not the stale
/// `openrouter.ai` the form carried over.
#[tokio::test]
async fn managed_probe_without_env_default_reports_the_platform_endpoint() {
    let decl = decl_for_probe("managed", Some("https://openrouter.ai/api/v1"), None, None);
    assert_eq!(decl.base_url, PLATFORM_BASE_URL);
    assert_eq!(bearer(&decl).await, None);
}

/// The real providers must keep honouring the form's `base_url` and `key` —
/// the managed diversion must not over-correct them.
#[tokio::test]
async fn other_provider_probes_still_honour_the_form_endpoint_and_key() {
    let openrouter = decl_for_probe("openrouter", Some("https://proxy/v1"), Some("or-key"), None);
    assert_eq!(openrouter.base_url, "https://proxy/v1");
    assert!(!openrouter.is_proxied());
    assert_eq!(bearer(&openrouter).await.as_deref(), Some("or-key"));

    let compatible = decl_for_probe(
        "openai_compatible",
        Some("https://llm.test/v1"),
        Some("k"),
        None,
    );
    assert_eq!(compatible.base_url, "https://llm.test/v1");
    assert_eq!(bearer(&compatible).await.as_deref(), Some("k"));

    let ollama = decl_for_probe("ollama", None, None, None);
    assert_eq!(ollama.base_url, OLLAMA_DEFAULT_BASE_URL);
    assert_eq!(bearer(&ollama).await, None);
}

/// A keyless `openrouter` with its own `base_url` override still goes direct
/// and keyless — the platform credential must never ride an arbitrary
/// endpoint. Unchanged by the managed fix.
#[tokio::test]
async fn keyless_openrouter_override_probe_stays_direct_and_keyless() {
    let env = managed_env();
    let decl = decl_for_probe(
        "openrouter",
        Some("https://attacker.example/v1"),
        None,
        Some(&env),
    );
    assert_eq!(decl.base_url, "https://attacker.example/v1");
    assert!(!decl.is_proxied());
    assert_eq!(bearer(&decl).await, None);
}
