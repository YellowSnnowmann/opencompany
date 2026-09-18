use super::provider_tenant_tests::MemSecrets;
use super::provider_test_helpers_tests::*;
use super::*;
use crate::company::Inference;
use std::collections::BTreeMap;

// ---- agent pin: `pinned` / `AgentPin` (keys rework, issue #2306, slice 3a) ----

// Each request's `(model, Authorization header)`, in arrival order, as
// [`spawn_capturing_stub`] records them.

/// Two agents pinned to two different providers reach two different
/// endpoints with their own chosen models and their own keys, from one
/// shared base provider — proving the pin, not any default or vocabulary
/// lookup, drives the turn, and that the unpinned base's own resolution
/// is unaffected by either agent's pin.
#[tokio::test]
async fn two_agents_pinned_to_two_providers_reach_two_endpoints() {
    let (url_a, seen_a) = spawn_capturing_stub().await;
    let (url_b, seen_b) = spawn_capturing_stub().await;

    let company = CompanyId::new("acme");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    for (slug, url) in [("acme", &url_a), ("other-co", &url_b)] {
        inference::store::put_provider(
            &company,
            secrets.as_ref(),
            inference::store::ProviderDraft {
                slug: slug.to_string(),
                label: slug.to_string(),
                kind: "openai_compatible".to_string(),
                base_url: url.to_string(),
                models: BTreeMap::new(),
                enabled: true,
            },
        )
        .await
        .unwrap();
        secrets
            .set(
                &company,
                &inference::store::provider_key_key(slug),
                crate::ports::types::SecretValue("sk-not-a-real-key".to_string()),
            )
            .await
            .unwrap();
    }

    let base = TenantProvider::new(company.clone(), secrets.clone(), Inference::default(), None);
    let a = base
        .pinned(
            "researcher",
            "Researcher",
            &choice("acme", "test-model-large"),
        )
        .expect("this provider can pin");
    let b = base
        .pinned(
            "web_search",
            "Web search",
            &choice("other-co", "test-model-small"),
        )
        .expect("this provider can pin");

    a.invoke(&(), user_request("hi")).await.expect("turn a");
    b.invoke(&(), user_request("hi")).await.expect("turn b");

    let calls_a = seen_a.lock().unwrap().clone();
    let calls_b = seen_b.lock().unwrap().clone();
    assert_eq!(calls_a.len(), 1, "{calls_a:?}");
    assert_eq!(calls_a[0].0, "test-model-large", "{calls_a:?}");
    assert_eq!(calls_a[0].1.as_deref(), Some("Bearer sk-not-a-real-key"));
    assert_eq!(calls_b.len(), 1, "{calls_b:?}");
    assert_eq!(calls_b[0].0, "test-model-small", "{calls_b:?}");
    assert_eq!(calls_b[0].1.as_deref(), Some("Bearer sk-not-a-real-key"));

    // The base (unpinned) provider resolves independently of either pin
    // — but with no company default and an empty `models` map on "acme"
    // (the first provider added, the pre-2b positional-primary rule,
    // untouched by 3a), there is no tier mapping for `chat-v1` either.
    // Keys rework, issue #2306, slice 2d: an unmapped tier is refused
    // before it ever reaches the wire, never guessed — and never either
    // agent's pinned model.
    let err = base
        .invoke(&(), user_request("hi"))
        .await
        .expect_err("no default and no tier mapping must fail closed, not guess");
    assert!(err.to_string().contains("No model is chosen"), "{err}");
    let calls_a = seen_a.lock().unwrap().clone();
    assert_eq!(
        calls_a.len(),
        1,
        "the unpinned base's own refusal must never reach mock A: {calls_a:?}"
    );
    assert_eq!(
        seen_b.lock().unwrap().len(),
        1,
        "mock B must see only the pin"
    );
}

/// A pin naming a provider this company does not have fails closed and
/// names the agent — never the default, which must receive no request at
/// all.
#[tokio::test]
async fn a_pinned_turn_naming_a_gone_provider_names_the_agent() {
    let (default_url, default_seen) = spawn_capturing_stub().await;
    let company = CompanyId::new("acme");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    let mut manifest = manifest_inference("openai_compatible");
    manifest.base_url = Some(default_url);
    let base = TenantProvider::new(company.clone(), secrets.clone(), manifest, None);

    let pinned = base
        .pinned(
            "researcher",
            "Researcher",
            &choice("acme", "test-model-large"),
        )
        .expect("this provider can pin");
    let err = pinned
        .invoke(&(), user_request("hi"))
        .await
        .expect_err("the pinned provider does not exist");
    let text = err.to_string();
    // Round-3a review P1-1: the shared `copy::pair_broken` sentence, in
    // the agent's display name — no raw id, no raw slug, no
    // hand-written "Team → …" path.
    assert!(
        text.contains("Researcher uses acme, which is removed."),
        "{text}"
    );
    assert!(
        text.contains("Choose another provider and model for Researcher"),
        "{text}"
    );
    assert!(
        default_seen.lock().unwrap().is_empty(),
        "the default must never receive the turn a bad pin refused"
    );
}

/// A pin naming a provider that is switched off fails closed with a
/// distinct sentence naming that state.
#[tokio::test]
async fn a_pinned_turn_naming_a_switched_off_provider_names_the_agent() {
    let company = CompanyId::new("acme");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    inference::store::put_provider(
        &company,
        secrets.as_ref(),
        inference::store::ProviderDraft {
            slug: "acme".to_string(),
            label: "Acme".to_string(),
            kind: "openai_compatible".to_string(),
            base_url: "http://127.0.0.1:9/v1".to_string(),
            models: BTreeMap::new(),
            enabled: false,
        },
    )
    .await
    .unwrap();

    let base = TenantProvider::new(company.clone(), secrets.clone(), Inference::default(), None);
    let pinned = base
        .pinned(
            "researcher",
            "Researcher",
            &choice("acme", "test-model-large"),
        )
        .expect("this provider can pin");
    let err = pinned
        .invoke(&(), user_request("hi"))
        .await
        .expect_err("the pinned provider is switched off");
    let text = err.to_string();
    assert!(
        text.contains("Researcher uses Acme, which is turned off."),
        "{text}"
    );
    assert!(
        text.contains("Choose another provider and model for Researcher"),
        "{text}"
    );
}

/// The trait default: an implementation that reports no telemetry
/// identity of its own (a test double) also reports it cannot pin,
/// rather than fabricating a sibling.
#[tokio::test]
async fn the_default_pinned_is_none_for_a_double() {
    let double = MockProvider::default();
    assert!(
        double
            .pinned(
                "researcher",
                "Researcher",
                &choice("acme", "test-model-large")
            )
            .is_none()
    );
}

/// The product-identity contract at the transport: `HostedProvider::invoke`
/// — the sole production inference path — must tag every chat-completions
/// request with `x-sdk-name: opencompany`, mirroring the embeddings client
/// and the openhuman-core call sites. This is the header the platform uses
/// to attribute backend traffic to the `opencompany` SDK.
#[tokio::test]
async fn hosted_provider_invoke_carries_the_product_identity_header() {
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};

    let seen: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let capture = Arc::clone(&seen);
    let app = Router::new().route(
        "/chat/completions",
        post(move |headers: HeaderMap| {
            let capture = Arc::clone(&capture);
            async move {
                *capture.lock().unwrap() = headers
                    .get(crate::product::PRODUCT_IDENTITY_HEADER)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                Json(serde_json::json!({
                    "choices": [{ "message": { "role": "assistant", "content": "ok" } }]
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let provider = HostedProvider::new(HostedProviderConfig {
        base_url: format!("http://{addr}"),
        credential: Credential::None,
        extra_headers: Vec::new(),
    });
    provider
        .invoke(
            &(),
            ModelRequest {
                model: Some("stub-model".into()),
                ..user_request("hi")
            },
        )
        .await
        .expect("turn against the stub");

    assert_eq!(
        seen.lock().unwrap().as_deref(),
        Some(crate::product::PRODUCT_IDENTITY),
        "every hosted chat-completions request must attach the product identity header"
    );
}

/// Issue #1749, the concurrency half: a turn that **failed** must not
/// publish its model into the shared cache.
///
/// One `TenantProvider` is shared by every agent on a company, and
/// `telemetry_model()` is read after a turn finishes — by whichever turn
/// finishes, not necessarily the one that wrote last. So publishing before
/// the request is issued lets a rejected turn (or one still in flight) name
/// the model for a *different* agent's successful turn, attributing real
/// tokens to a model that produced none. That is strictly worse than the
/// documented approximation, which is bounded to two models that both ran.
///
/// A failed turn meters nothing of its own, so the honest state after one
/// is the last **successful** turn's model, unchanged.
#[tokio::test]
async fn a_rejected_tenant_turn_leaves_the_last_successful_model_in_place() {
    let ok = spawn_stub("ok").await;
    let rejecting = spawn_rejecting_stub(axum::http::StatusCode::UNAUTHORIZED).await;

    let company = CompanyId::new("acme");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    let mut manifest = manifest_inference("openai_compatible");
    manifest.base_url = Some(ok.clone());
    let provider = TenantProvider::new(company.clone(), secrets.clone(), manifest, None);

    let point_at = |base_url: String, model: &str| {
        let mut models = BTreeMap::new();
        models.insert("chat-v1".to_string(), model.to_string());
        let secrets = Arc::clone(&secrets);
        let company = company.clone();
        async move {
            inference::save_runtime_config(
                &company,
                secrets.as_ref(),
                &inference::RuntimeInference {
                    provider: "openai_compatible".into(),
                    base_url: Some(base_url),
                    models,
                },
            )
            .await
            .unwrap();
        }
    };

    // A turn that runs, on a model the vocabulary names.
    point_at(ok.clone(), "anthropic/claude-sonnet-4-6").await;
    provider
        .invoke(&(), user_request("hi"))
        .await
        .expect("the successful turn");
    assert_eq!(
        provider.telemetry_model().map(|m| m.as_str()),
        Some("anthropic-sonnet"),
        "a completed turn names its model"
    );

    // …then a turn on a *differently* named model that the endpoint
    // rejects outright.
    point_at(rejecting.clone(), "openai/gpt-5.2").await;
    let err = provider
        .invoke(&(), user_request("hi"))
        .await
        .expect_err("the endpoint rejects this turn");
    assert!(err.to_string().contains("401"), "{err}");

    assert_eq!(
        provider.telemetry_model().map(|m| m.as_str()),
        Some("anthropic-sonnet"),
        "a rejected turn produced no usage, so it must not overwrite the \
         model of the turn that did run — a concurrent agent's cost hook \
         reads this value"
    );
}

/// The same contract on [`HostedProvider`], whose cache is behind an `Arc`
/// precisely so every clone of the handle shares it — which is what makes a
/// premature write observable by another agent's turn.
#[tokio::test]
async fn a_rejected_hosted_turn_leaves_the_last_successful_model_in_place() {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Answers the first turn and rejects every one after it, so a single
    // endpoint gives us one success followed by one 401.
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let app = Router::new().route(
        "/chat/completions",
        post(move || {
            let counter = Arc::clone(&counter);
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    Json(serde_json::json!({
                        "choices": [{ "message": { "role": "assistant", "content": "ok" } }]
                    }))
                    .into_response()
                } else {
                    (StatusCode::UNAUTHORIZED, "rotated").into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let provider = HostedProvider::new(HostedProviderConfig {
        base_url: format!("http://{addr}"),
        credential: Credential::None,
        extra_headers: Vec::new(),
    });

    let asking_for = |model: &str| ModelRequest {
        model: Some(model.to_string()),
        ..user_request("hi")
    };

    provider
        .invoke(&(), asking_for("anthropic/claude-sonnet-4-6"))
        .await
        .expect("the successful turn");
    assert_eq!(
        provider.telemetry_model().map(|m| m.as_str()),
        Some("anthropic-sonnet"),
        "a completed turn names its model"
    );

    let err = provider
        .invoke(&(), asking_for("openai/gpt-5.2"))
        .await
        .expect_err("the endpoint rejects this turn");
    assert!(err.to_string().contains("401"), "{err}");

    assert_eq!(
        provider.telemetry_model().map(|m| m.as_str()),
        Some("anthropic-sonnet"),
        "a rejected turn produced no usage, so it must not overwrite the \
         model of the turn that did run — every clone of this handle shares \
         the cache it would have overwritten"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "both turns reached the stub"
    );
}

/// Codex review on #1779 (comment 3864824480): `model_response_from_payload`
/// learned to parse array-shaped `content` (`parses_content_as_array_of_text_parts`
/// above), but `probe` — the setup wizard's and the console's "Test" button
/// connectivity check — still read `content.as_str()` directly. An endpoint
/// answering with array-shaped content therefore passed every real turn
/// while its own connection probe reported the connection broken. `probe`
/// must route through `model_response_from_payload` itself, the same
/// parser the turn path calls, rather than any narrower stand-in for it.
#[tokio::test]
async fn probe_accepts_array_shaped_content() {
    let url = spawn_stub_content(serde_json::json!([
        { "type": "text", "text": "pong" }
    ]))
    .await;

    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let mut manifest = manifest_inference("openai_compatible");
    manifest.base_url = Some(url);
    let mut decl = inference::resolve_effective(&company, &manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();
    decl.models
        .insert("chat-v1".to_string(), "stub-model".to_string());

    probe(&decl, "stub-model", None)
        .await
        .expect("array-shaped content must be recognized as a successful probe");
}

#[tokio::test]
async fn probe_refuses_a_tier_before_sending() {
    let decl =
        inference::decl_for_probe("openai_compatible", Some("http://127.0.0.1:9"), None, None);

    let err = probe(&decl, DEFAULT_HOSTED_MODEL, None)
        .await
        .expect_err("a tier name is not a provider model id");
    let typed = err
        .downcast_ref::<InferenceError>()
        .expect("configuration failures stay typed");
    assert!(matches!(typed, InferenceError::Model(_)), "{typed}");
    assert!(!typed.to_string().contains("Connections"), "{typed}");
}

#[tokio::test]
async fn probe_preserves_a_provider_status() {
    let (base_url, server) = spawn_rejection(axum::http::StatusCode::NOT_FOUND).await;
    let decl = inference::decl_for_probe("openai_compatible", Some(&base_url), None, None);

    let err = probe(&decl, "provider/model", None)
        .await
        .expect_err("the stub rejects the model");
    server.abort();
    let typed = err
        .downcast_ref::<InferenceError>()
        .expect("provider failures stay typed");
    let InferenceError::Provider(error) = typed else {
        panic!("expected a provider error, got {typed}");
    };
    assert_eq!(error.status, Some(404));
    assert_eq!(error.raw, None);
}
