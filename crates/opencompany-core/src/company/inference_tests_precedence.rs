//! Precedence-matrix tests: how a resolved bearer is chosen among the
//! manifest, secret store and legacy sources (split out of
//! `inference_tests.rs`).

use super::inference_tests_support::*;
use super::*;

// ---- precedence matrix -------------------------------------------------

#[tokio::test]
async fn runtime_beats_manifest_beats_env() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://env.example/v1".into(),
        credential: Credential::from_value("env-key"),
    };
    let mut manifest = inference("openai_compatible");
    manifest.base_url = Some("https://manifest.example/v1".into());

    // Env only.
    let decl = resolve_effective(&company, &Inference::default(), Some(&env), &secrets)
        .await
        .unwrap()
        .expect("env default resolves");
    assert_eq!(decl.source, InferenceSource::Default);
    assert_eq!(decl.provider, DEFAULT_PROVIDER);
    assert!(decl.is_proxied(), "the default rides the subscription");
    assert_eq!(decl.telemetry_slug(), "subscription");
    assert_eq!(bearer(&decl).await.as_deref(), Some("env-key"));

    // Manifest beats env.
    let decl = resolve_effective(&company, &manifest, Some(&env), &secrets)
        .await
        .unwrap()
        .expect("manifest resolves");
    assert_eq!(decl.source, InferenceSource::Manifest);
    assert_eq!(decl.provider, "openai_compatible");
    assert_eq!(decl.base_url, "https://manifest.example/v1");

    // Runtime beats manifest.
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
    store_key(&company, &secrets, "or-secret").await.unwrap();
    let decl = resolve_effective(&company, &manifest, Some(&env), &secrets)
        .await
        .unwrap()
        .expect("runtime resolves");
    assert_eq!(decl.source, InferenceSource::Runtime);
    assert_eq!(decl.provider, "openrouter");
    assert_eq!(decl.base_url, OPENROUTER_BASE_URL);
    assert!(!decl.is_proxied(), "a tenant key goes direct");
    assert_eq!(decl.telemetry_slug(), "openrouter");
    assert_eq!(bearer(&decl).await.as_deref(), Some("or-secret"));
    assert!(decl.key_configured());
}

/// A keyless `openrouter` inherits the platform endpoint and credential
/// rather than dropping them — the branch `managed` used to own. Without it a
/// company that names its provider but holds no key of its own would 401
/// instead of riding the subscription.
#[tokio::test]
async fn keyless_openrouter_inherits_the_platform_endpoint_and_credential() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    let decl = resolve_effective(&company, &inference("openrouter"), Some(&env), &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.source, InferenceSource::Manifest);
    assert_eq!(decl.provider, "openrouter");
    assert_eq!(decl.base_url, "https://env.example/openai/v1");
    assert!(decl.is_proxied());
    assert_eq!(bearer(&decl).await.as_deref(), Some("platform-key"));
}

/// A keyless `openrouter` with a tenant-supplied `base_url` override goes
/// direct with **no** credential — the platform token must not ride an
/// arbitrary endpoint the operator pointed it at.
#[tokio::test]
async fn keyless_openrouter_never_sends_the_platform_credential_to_an_override() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    let mut manifest = inference("openrouter");
    manifest.base_url = Some("https://attacker.example/v1".into());
    let decl = resolve_effective(&company, &manifest, Some(&env), &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.base_url, "https://attacker.example/v1");
    assert!(
        !decl.is_proxied(),
        "an arbitrary endpoint is not the subscription"
    );
    assert!(
        !decl.key_configured(),
        "a keyless config holds no credential to send"
    );
    assert_eq!(decl.telemetry_slug(), "openrouter");
    assert_eq!(
        bearer(&decl).await,
        None,
        "the platform credential stays home"
    );
}

/// A company that declares nothing and rides the platform's endpoint is on
/// the managed route, and says so.
///
/// The console sends a keyless managed save as a *revert* — a managed brain
/// with no key of its own is the platform default rather than an override —
/// so this arm is what answers the operator immediately after they choose
/// "Managed (TinyHumans)" and press Save. Answering `openrouter` (the
/// provider underneath the platform endpoint) is the second way that choice
/// used to disappear from the card, and the one a stored-override fix alone
/// does not reach.
#[tokio::test]
async fn the_platform_default_reports_itself_as_the_managed_route() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    let decl = resolve_effective(&company, &Inference::default(), Some(&env), &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.selected_provider(), LEGACY_MANAGED);
    assert_eq!(
        decl.provider, DEFAULT_PROVIDER,
        "and it still resolves to, and is billed as, proxied OpenRouter"
    );
    assert!(decl.is_proxied());
    assert_eq!(decl.telemetry_slug(), "subscription");
    assert_eq!(decl.source, InferenceSource::Default);
}

/// The operator's own choice survives the round trip, so a console can echo
/// it back.
///
/// `provider` answers "where does this resolve to", which is the right
/// question everywhere but the read-back: `managed` and `openrouter` resolve
/// identically, so reporting the resolved kind made "Managed (TinyHumans)"
/// impossible to hold on screen — the console seeds its provider select from
/// the status verbatim, so a `managed` save that read back `openrouter`
/// snapped the select (and the managed-only Connect button with it) straight
/// back. The two facts are now separate fields rather than one field asked
/// two questions.
#[tokio::test]
async fn a_saved_managed_choice_reads_back_as_managed() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: LEGACY_MANAGED.to_string(),
            base_url: None,
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();

    let decl = resolve_effective(&company, &Inference::default(), Some(&env), &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        decl.selected_provider(),
        LEGACY_MANAGED,
        "the console asked what was chosen"
    );
    assert_eq!(
        decl.provider, DEFAULT_PROVIDER,
        "and resolution is unchanged — every request path still sees OpenRouter"
    );
    assert!(decl.is_proxied());
    assert_eq!(decl.telemetry_slug(), "subscription");
}

/// Every other route reports one answer to both questions, so nothing but
/// the managed alias can drift between them.
#[tokio::test]
async fn a_non_aliased_choice_reads_back_unchanged() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    for kind in ["openrouter", "ollama", "openai_compatible"] {
        save_runtime_config(
            &company,
            &secrets,
            &RuntimeInference {
                provider: kind.to_string(),
                base_url: Some("http://127.0.0.1:9/v1".into()),
                models: BTreeMap::new(),
            },
        )
        .await
        .unwrap();
        let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(decl.selected_provider(), kind);
        assert_eq!(decl.provider, kind);
    }
}

/// `tinyhumans` is the wizard's spelling of the same route. It has to
/// canonicalize to the one spelling the console renders, or the select is
/// handed a value its own provider table has no row for and falls back.
#[test]
fn both_spellings_of_the_managed_route_canonicalize() {
    assert_eq!(selected_kind(LEGACY_MANAGED), LEGACY_MANAGED);
    assert_eq!(selected_kind("tinyhumans"), LEGACY_MANAGED);
    assert_eq!(selected_kind("  managed  "), LEGACY_MANAGED);
    assert_eq!(selected_kind("openrouter"), "openrouter");
    assert_eq!(
        selected_kind(""),
        DEFAULT_PROVIDER,
        "naming nothing is not naming the managed route"
    );
}

/// The first-run wizard's TinyHumans card writes `provider = "managed"`
/// with no `base_url` into the manifest and the typed key into the store.
/// That company must resolve to the platform proxy with that key — not to
/// `openrouter.ai`, which is where the normalized kind sent it (and the
/// key with it) until the umbrella e2e caught a wizard-built company
/// answering every turn with OpenRouter's 401.
#[tokio::test]
async fn a_managed_manifest_with_a_stored_key_stays_on_the_platform() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    store_key(&company, &secrets, "th-not-a-real-key")
        .await
        .unwrap();
    let decl = resolve_effective(&company, &inference(LEGACY_MANAGED), None, &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.source, InferenceSource::Manifest);
    assert_eq!(decl.base_url, platform_base_url());
    assert!(decl.is_proxied());
    assert_eq!(bearer(&decl).await.as_deref(), Some("th-not-a-real-key"));

    // A manifest that ALSO names an endpoint is a gateway the operator
    // chose on purpose, and keeps resolving to it as a vendor.
    let mut gateway = inference(LEGACY_MANAGED);
    gateway.base_url = Some("https://gateway.example/v1".into());
    let decl = resolve_effective(&company, &gateway, None, &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.base_url, "https://gateway.example/v1");
    assert!(!decl.is_proxied());
}

/// The runtime-override arm makes the identical "named a gateway or not"
/// choice the manifest arm above does, and for the same reason:
/// `validate_runtime` accepts `provider: "managed"` with a valid non-blank
/// `base_url`, so a console `PUT` naming a gateway is exactly as valid a
/// runtime config as a manifest one — `resolve_endpoint`'s managed branch
/// must not silently discard it for the platform's own endpoint
/// (CodeRabbit review).
#[tokio::test]
async fn a_managed_runtime_override_with_a_gateway_keeps_it() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    store_key(&company, &secrets, "th-not-a-real-key")
        .await
        .unwrap();

    // No endpoint named: stays on the platform proxy, same as the
    // manifest arm.
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: LEGACY_MANAGED.into(),
            base_url: None,
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();
    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.source, InferenceSource::Runtime);
    assert_eq!(decl.base_url, platform_base_url());
    assert!(decl.is_proxied());

    // A runtime config that ALSO names an endpoint is a gateway the
    // operator chose on purpose, and keeps resolving to it as a vendor —
    // not the platform's own endpoint with the gateway silently dropped.
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: LEGACY_MANAGED.into(),
            base_url: Some("https://gateway.example/v1".into()),
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();
    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.base_url, "https://gateway.example/v1");
    assert!(!decl.is_proxied());
}

/// A blank or whitespace-only `base_url` is not "the operator named a
/// gateway" — it must resolve exactly like naming none at all, staying on
/// the platform proxy with the stored key (tinysweeper/CodeRabbit review:
/// `manifest.base_url.is_some()` used to read a blank string as an
/// explicit endpoint, which sent a `managed` company with a blank
/// `base_url` and a stored key straight to `openrouter.ai` — the same 401
/// `a_managed_manifest_with_a_stored_key_stays_on_the_platform` exists to
/// prevent for the no-`base_url` case).
#[tokio::test]
async fn a_blank_manifest_base_url_is_treated_as_absent() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    store_key(&company, &secrets, "th-not-a-real-key")
        .await
        .unwrap();

    for blank in ["", "   ", "\t\n"] {
        let mut manifest = inference(LEGACY_MANAGED);
        manifest.base_url = Some(blank.to_string());
        let decl = resolve_effective(&company, &manifest, None, &secrets)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(decl.base_url, platform_base_url(), "blank: {blank:?}");
        assert!(decl.is_proxied(), "blank: {blank:?}");
        assert_eq!(
            bearer(&decl).await.as_deref(),
            Some("th-not-a-real-key"),
            "blank: {blank:?}"
        );
    }
}

/// A committed manifest still saying `provider = "managed"` resolves as
/// proxied OpenRouter rather than failing. It was valid when written, and the
/// intent — "the platform's brain" — is exactly what proxied OpenRouter is.
#[tokio::test]
async fn a_legacy_managed_manifest_aliases_to_proxied_openrouter() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    let decl = resolve_effective(&company, &inference(LEGACY_MANAGED), Some(&env), &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.provider, DEFAULT_PROVIDER);
    assert!(decl.is_proxied());
    assert_eq!(decl.base_url, "https://env.example/openai/v1");
    assert_eq!(bearer(&decl).await.as_deref(), Some("platform-key"));
    assert_eq!(
        decl.selected_provider(),
        LEGACY_MANAGED,
        "the alias resolves onto OpenRouter without the console losing which \
         route was actually named"
    );
    assert!(
        validate_inference(&inference(LEGACY_MANAGED)).is_empty(),
        "and it still validates"
    );

    // The same alias applies to a stored runtime blob, which an operator
    // cannot hand-edit — the case that would otherwise strand a tenant.
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: LEGACY_MANAGED.into(),
            base_url: None,
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();
    let decl = resolve_effective(&company, &Inference::default(), Some(&env), &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.source, InferenceSource::Runtime);
    assert_eq!(decl.provider, DEFAULT_PROVIDER);
    assert!(decl.is_proxied());
}

/// A stored runtime blob naming a provider this build does not know fails
/// loudly rather than resolving to whatever the fallback happened to be.
#[tokio::test]
async fn an_unknown_stored_provider_is_an_error_not_a_silent_fallback() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: "telepathy".into(),
            base_url: None,
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();
    let err = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .expect_err("unknown provider must fail");
    let msg = err.to_string();
    assert!(msg.contains("telepathy"), "{msg}");
    assert!(msg.contains("openrouter"), "names what is valid: {msg}");
}

/// Issue #585: the company's own key is the admin's to set, and a key stored
/// through the console wins over the deploy-time env credential — otherwise
/// the only way to pay for a tenant is an environment variable the admin
/// cannot reach.
///
/// **What changed with `managed`'s removal.** Under `managed`, a console key
/// kept the *platform* endpoint, so an admin could bill their own account
/// through the proxy. `openrouter` is dual-mode instead: a key means an
/// OpenRouter key, so it goes direct to OpenRouter — sending an `sk-or-…` to
/// the platform proxy would simply be rejected. An admin who wants the
/// platform endpoint with a credential of their own now names
/// `openai_compatible` with that `base_url`.
#[tokio::test]
async fn a_console_key_wins_over_the_env_credential_and_goes_direct() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
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
    store_key(&company, &secrets, "company-key").await.unwrap();

    let decl = resolve_effective(&company, &Inference::default(), Some(&env), &secrets)
        .await
        .unwrap()
        .expect("runtime openrouter resolves");
    assert_eq!(decl.source, InferenceSource::Runtime);
    assert_eq!(decl.provider, "openrouter");
    assert_eq!(bearer(&decl).await.as_deref(), Some("company-key"));
    assert!(decl.key_configured());
    assert!(!decl.is_proxied(), "the tenant's own account pays");
    assert_eq!(decl.base_url, OPENROUTER_BASE_URL);

    // Clearing it falls back to the subscription rather than 401ing — the
    // property that makes a key genuinely optional in both directions.
    clear_key(&company, &secrets).await.unwrap();
    let decl = resolve_effective(&company, &Inference::default(), Some(&env), &secrets)
        .await
        .unwrap()
        .expect("still resolves with no key");
    assert!(decl.is_proxied());
    assert_eq!(decl.base_url, "https://env.example/openai/v1");
    assert_eq!(bearer(&decl).await.as_deref(), Some("platform-key"));
}

/// Issue #585 adds a second writer to `key_configured` — an admin setting the
/// company's key from the console — alongside the platform default the
/// manager injects. #636's `effective_status` split exists precisely to keep
/// the console's `keyConfigured` reporting *tenant* config and never the
/// platform token. Nothing asserted the two stay distinguishable, so this
/// does: on one company, the same call answers differently depending on
/// which source is in play.
#[tokio::test]
async fn a_console_key_and_the_platform_default_are_distinguishable() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let platform = EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: "managed".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Nothing tenant-scoped is stored yet. The platform-aware resolve is
    // credentialled — that is the value the console must NOT surface — while
    // the tenant-only resolve the read route uses reports "no key".
    let with_platform =
        resolve_effective(&company, &Inference::default(), Some(&platform), &secrets)
            .await
            .unwrap()
            .expect("platform default resolves");
    let tenant_only = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .expect("runtime config resolves");
    assert!(
        with_platform.key_configured(),
        "the platform token is a real credential"
    );
    assert!(
        !tenant_only.key_configured(),
        "an injected platform token must never light up the console's `keyConfigured`"
    );

    // An admin sets the company's key. Now both agree — and the bearer the
    // agents present is the tenant's, not the platform's.
    store_key(&company, &secrets, "company-key").await.unwrap();
    let with_platform =
        resolve_effective(&company, &Inference::default(), Some(&platform), &secrets)
            .await
            .unwrap()
            .expect("platform default resolves");
    let tenant_only = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .expect("runtime config resolves");
    assert!(
        tenant_only.key_configured(),
        "a console-set key is tenant config"
    );
    assert_eq!(bearer(&with_platform).await.as_deref(), Some("company-key"));
}

#[tokio::test]
async fn no_source_resolves_to_none() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap();
    assert!(
        decl.is_none(),
        "no source means the managed/echo brain stays"
    );
}

#[tokio::test]
async fn clearing_runtime_reverts_to_manifest() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let manifest = inference("openrouter");
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: "ollama".into(),
            base_url: Some("http://localhost:11434/v1".into()),
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        resolve_effective(&company, &manifest, None, &secrets)
            .await
            .unwrap()
            .unwrap()
            .provider,
        "ollama"
    );
    clear_runtime_config(&company, &secrets).await.unwrap();
    assert_eq!(
        resolve_effective(&company, &manifest, None, &secrets)
            .await
            .unwrap()
            .unwrap()
            .provider,
        "openrouter"
    );
}
