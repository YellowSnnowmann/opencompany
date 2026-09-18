use super::provider_test_helpers_tests::*;
use super::*;
use crate::company::Inference;
use crate::ports::types::SecretValue;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Mutex;

// ---- TenantProvider (issue #56 — BYOK) --------------------------------

#[derive(Default)]
pub(super) struct MemSecrets {
    map: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl SecretStore for MemSecrets {
    async fn get(&self, _c: &CompanyId, key: &str) -> crate::Result<Option<SecretValue>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(key)
            .map(|v| SecretValue(v.clone())))
    }
    async fn set(&self, _c: &CompanyId, key: &str, value: SecretValue) -> crate::Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.0);
        Ok(())
    }
}

#[tokio::test]
async fn request_plan_maps_tier_and_injects_openrouter_headers() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let mut manifest = manifest_inference("openrouter");
    manifest.models =
        BTreeMap::from([("chat-v1".to_string(), "deepseek/deepseek-chat".to_string())]);
    inference::store_key(&company, &secrets, "or-key")
        .await
        .unwrap();
    let decl = inference::resolve_effective(&company, &manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();

    let plan = request_plan(
        &decl,
        "chat-v1",
        Vec::new(),
        inference::dialect::Sampling::Exact(0.2),
        None,
        Vec::new(),
        &ToolChoice::Auto,
    )
    .await
    .expect("plan");
    assert_eq!(
        plan.model, "deepseek/deepseek-chat",
        "tier maps through table"
    );
    // A toolless turn omits both `tools` and `tool_choice` entirely.
    assert!(
        plan.body.get("tools").is_none(),
        "no tools key when toolless"
    );
    assert!(
        plan.body.get("tool_choice").is_none(),
        "no tool_choice without tools"
    );
    assert!(
        plan.body.get("parallel_tool_calls").is_none(),
        "no parallel-tool setting without tools"
    );
    // Asked for, so sent.
    assert_eq!(plan.body["temperature"], serde_json::json!(0.2));
    assert_eq!(plan.bearer.as_deref(), Some("or-key"));
    assert!(plan.url.ends_with("/chat/completions"), "{}", plan.url);
    assert!(
        plan.headers
            .contains(&("HTTP-Referer", OPENROUTER_REFERER.to_string()))
    );
    assert!(
        plan.headers
            .contains(&("X-Title", OPENROUTER_TITLE.to_string()))
    );

    // A tier the manifest does not map is refused rather than guessed or
    // passed through as a bare tier name (keys rework, issue #2306, slice
    // 2d): this decl is DIRECT, OpenRouter has never heard of
    // `reasoning-v1`, and there is no shipped substitute any more.
    let refused = request_plan(
        &decl,
        "reasoning-v1",
        Vec::new(),
        inference::dialect::Sampling::Exact(0.2),
        None,
        Vec::new(),
        &ToolChoice::Auto,
    )
    .await
    .expect_err("an unmapped tier must be refused, not guessed");
    assert!(
        refused.to_string().contains("No model is chosen"),
        "{refused}"
    );

    // A concrete slug is still forwarded untouched, so a caller can name any
    // model in OpenRouter's catalog.
    let explicit = request_plan(
        &decl,
        "anthropic/claude-sonnet-4.5",
        Vec::new(),
        inference::dialect::Sampling::Exact(0.2),
        None,
        Vec::new(),
        &ToolChoice::Auto,
    )
    .await
    .expect("plan");
    assert_eq!(explicit.model, "anthropic/claude-sonnet-4.5");
}

#[tokio::test]
async fn request_plan_omits_bearer_for_keyless_ollama() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let mut manifest = manifest_inference("ollama");
    manifest.base_url = Some("http://localhost:11434/v1".into());
    let decl = inference::resolve_effective(&company, &manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();
    let plan = request_plan(
        &decl,
        "stub-model",
        Vec::new(),
        inference::dialect::Sampling::Deterministic,
        None,
        Vec::new(),
        &ToolChoice::Auto,
    )
    .await
    .expect("plan");
    assert!(plan.bearer.is_none(), "keyless Ollama sends no bearer");
    assert!(plan.headers.is_empty(), "no OpenRouter headers for Ollama");
}

/// The positive half of issue #376 (AC #1): a **proxied** config targets a
/// TinyHumans-owned endpoint, so [`request_plan`] must attach our
/// `x-sdk-name: opencompany` product header alongside the tier's other
/// headers.
///
/// After `managed`'s removal the provider *kind* no longer tells our
/// endpoint from OpenRouter's — the same `openrouter` kind reaches both.
/// `is_proxied()` is what distinguishes them, and this test and its negative
/// twin below pin both sides of that one bit.
#[tokio::test]
async fn request_plan_attaches_the_product_header_when_proxied() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = crate::company::inference::EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    // A keyless `openrouter` resolves through the env default — this is the
    // shape a real company manifest produces, not a synthetic decl, and it
    // is the config a company that has configured nothing runs on.
    let decl = inference::resolve_effective(
        &company,
        &manifest_inference("openrouter"),
        Some(&env),
        &secrets,
    )
    .await
    .unwrap()
    .expect("keyless openrouter resolves via the env default");
    assert!(decl.is_proxied());

    let plan = request_plan(
        &decl,
        "stub-model",
        Vec::new(),
        inference::dialect::Sampling::Exact(0.2),
        None,
        Vec::new(),
        &ToolChoice::Auto,
    )
    .await
    .expect("plan");
    assert!(
        plan.headers
            .contains(&("x-sdk-name", "opencompany".to_string())),
        "a proxied config must carry the product header: {:?}",
        plan.headers
    );
    assert!(
        plan.headers
            .contains(&("HTTP-Referer", OPENROUTER_REFERER.to_string())),
        "and OpenRouter's own attribution rides the proxied path too: {:?}",
        plan.headers
    );
}

/// The negative half of issue #376 (AC #1) — and the important one, per
/// the task: `openrouter` and `openai_compatible` are bring-your-own-key
/// THIRD-PARTY endpoints (OpenRouter's own API, and any OpenAI-compatible
/// host an operator points at — OpenAI, DeepSeek, a self-hosted proxy,
/// …). Sending them our product identity would tell a company we have no
/// relationship with which product a tenant is running, for no benefit to
/// anyone. Only a **proxied** config (see the test above) may ever carry
/// the header — and note the first case here is the *same provider kind* as
/// that test, differing only in holding a tenant key. That is precisely the
/// distinction this rule now turns on.
#[tokio::test]
async fn request_plan_never_attaches_the_product_header_for_third_party_providers() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    // openrouter DIRECT (the tenant's own key): gets ITS OWN attribution
    // headers, never ours.
    let mut or_manifest = manifest_inference("openrouter");
    or_manifest.models =
        BTreeMap::from([("chat-v1".to_string(), "deepseek/deepseek-chat".to_string())]);
    inference::store_key(&company, &secrets, "or-key")
        .await
        .unwrap();
    let or_decl = inference::resolve_effective(&company, &or_manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();
    let or_plan = request_plan(
        &or_decl,
        "chat-v1",
        Vec::new(),
        inference::dialect::Sampling::Exact(0.2),
        None,
        Vec::new(),
        &ToolChoice::Auto,
    )
    .await
    .expect("plan");
    assert!(
        !or_plan
            .headers
            .iter()
            .any(|(name, _)| *name == "x-sdk-name"),
        "openrouter is third-party and must never see our product identity: {:?}",
        or_plan.headers
    );
    assert!(
        or_plan
            .headers
            .contains(&("HTTP-Referer", OPENROUTER_REFERER.to_string())),
        "openrouter's own attribution headers must be unaffected: {:?}",
        or_plan.headers
    );

    // openai_compatible: a bring-your-own-endpoint host — no headers at all.
    let mut compat_manifest = manifest_inference("openai_compatible");
    compat_manifest.base_url = Some("https://byok.example/v1".into());
    let compat_decl = inference::resolve_effective(&company, &compat_manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();
    let compat_plan = request_plan(
        &compat_decl,
        "stub-model",
        Vec::new(),
        inference::dialect::Sampling::Exact(0.2),
        None,
        Vec::new(),
        &ToolChoice::Auto,
    )
    .await
    .expect("plan");
    assert!(
        compat_plan.headers.is_empty(),
        "openai_compatible is third-party and must carry no headers at all: {:?}",
        compat_plan.headers
    );
}

/// The live-switch contract: the same `TenantProvider` instance routes turn
/// 1 to stub A, then — after the operator flips the runtime override in the
/// secret store — routes turn 2 to stub B, with **no rebuild** of the
/// provider or the agent. This is what makes a console switch take effect on
/// the next turn.
#[tokio::test]
async fn tenant_provider_live_switches_between_turns_without_rebuild() {
    let url_a = spawn_stub("reply-from-A").await;
    let url_b = spawn_stub("reply-from-B").await;

    let company = CompanyId::new("acme");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    let mut manifest = manifest_inference("openai_compatible");
    manifest.base_url = Some(url_a.clone());
    let provider = TenantProvider::new(company.clone(), secrets.clone(), manifest, None);

    // Turn 1 → stub A.
    let first = provider
        .invoke(
            &(),
            ModelRequest {
                model: Some("stub-model".into()),
                ..user_request("hi")
            },
        )
        .await
        .expect("turn 1");
    assert_eq!(first.text(), "reply-from-A");
    assert_eq!(provider.telemetry_provider_id(), "byok");

    // Operator flips the provider to stub B via a runtime override — no
    // rebuild, just a secret-store write.
    inference::save_runtime_config(
        &company,
        secrets.as_ref(),
        &inference::RuntimeInference {
            provider: "openai_compatible".into(),
            base_url: Some(url_b.clone()),
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();

    // Turn 2 → stub B, same provider instance.
    let second = provider
        .invoke(
            &(),
            ModelRequest {
                model: Some("stub-model".into()),
                ..user_request("hi")
            },
        )
        .await
        .expect("turn 2");
    assert_eq!(
        second.text(),
        "reply-from-B",
        "the switch took effect next turn"
    );
}

/// Issue #1749: the model half of the same live-attribution contract, and
/// the BYOK containment it exists for.
///
/// A tenant `[inference].models` entry is **operator free text** — this one
/// is named after a customer, which is exactly the shape of the leak. The
/// provider must report a vocabulary member for it, and the raw name must
/// not appear anywhere in what the meter would persist.
#[tokio::test]
async fn a_tenant_model_is_reported_as_a_slug_and_never_as_the_operators_name() {
    let url = spawn_stub("ok").await;
    let company = CompanyId::new("acme");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    let mut manifest = manifest_inference("openai_compatible");
    manifest.base_url = Some(url.clone());
    let provider = TenantProvider::new(company.clone(), secrets.clone(), manifest, None);

    assert_eq!(
        provider.telemetry_model(),
        None,
        "no turn has run, so there is no model to name"
    );

    let save = |model: &str| {
        let mut models = BTreeMap::new();
        models.insert("chat-v1".to_string(), model.to_string());
        let secrets = Arc::clone(&secrets);
        let company = company.clone();
        let url = url.clone();
        async move {
            inference::save_runtime_config(
                &company,
                secrets.as_ref(),
                &inference::RuntimeInference {
                    provider: "openai_compatible".into(),
                    base_url: Some(url),
                    models,
                },
            )
            .await
            .unwrap();
        }
    };

    // A self-hosted model named after the customer it was built for.
    save("northwind-legal-review-v2").await;
    provider
        .invoke(&(), user_request("hi"))
        .await
        .expect("turn");
    assert_eq!(
        provider.telemetry_model(),
        Some(crate::metering::ModelSlug::OTHER),
        "a model this build cannot name reports the fallback"
    );
    let sample = crate::metering::inference_sample(
        &crate::ports::types::TokenUsage {
            input: 10,
            output: 5,
            cached_input: 0,
            cost_usd: 0.01,
        },
        "ceo",
        &provider.telemetry_provider_id(),
        provider.telemetry_model(),
    )
    .expect("a real turn meters");
    let persisted = serde_json::to_string(&sample).expect("serialize");
    assert!(
        !persisted.to_ascii_lowercase().contains("northwind"),
        "the operator's model name reached what the meter persists: {persisted}"
    );

    // …and a model the vocabulary does know, through the same path.
    save("anthropic/claude-sonnet-4-6").await;
    provider
        .invoke(&(), user_request("hi"))
        .await
        .expect("turn");
    assert_eq!(
        provider.telemetry_model().map(|m| m.as_str()),
        Some("anthropic-sonnet"),
        "a table switch re-attributes the next turn, exactly as the provider slug does"
    );
}

#[tokio::test]
async fn tenant_provider_errors_when_nothing_is_configured() {
    let company = CompanyId::new("acme");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    let provider = TenantProvider::new(company, secrets, Inference::default(), None);
    let err = provider
        .invoke(&(), user_request("hi"))
        .await
        .expect_err("no provider configured");
    // Keys rework (#2306), slice 2b: `resolve_for_turn`'s refusal replaces
    // the old "no inference provider is configured" sentence.
    assert!(
        err.to_string()
            .contains("No model is chosen for this company"),
        "{err}"
    );
}
