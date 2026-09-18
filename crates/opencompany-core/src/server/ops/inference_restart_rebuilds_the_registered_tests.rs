use axum::body::Body;
use axum::http::{Request, StatusCode};
#[cfg(feature = "openhuman")]
use serde_json::Value;
use serde_json::json;
use tower::ServiceExt;

use super::inference_test_support::*;
#[cfg(feature = "openhuman")]
use super::*;

use crate::ports::types::CompanyId;
use crate::server::router;

/// `POST …/inference/restart` rebuilds the runtime in place, so the console's
/// "Restart required" notice has an action behind it rather than being a
/// dead end pointing at a container the operator may not be able to touch.
#[tokio::test]
async fn restart_rebuilds_the_registered_runtime() {
    let home_dir = home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let state = state_with_company(home)
        .await
        .with_rebuilder(std::sync::Arc::new(Working {
            home: home.to_path_buf(),
        }));
    state.set_boot_inputs(id.clone(), crate::runtime::BootInputs::default());
    let before = state.registry().get(&id).expect("registered");

    let (status, resp, raw) = send(&state, "POST", "/api/v1/company/inference/restart", None).await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    // A genuinely different runtime is registered — the point of the route.
    let after = state.registry().get(&id).expect("registered");
    assert!(
        !std::sync::Arc::ptr_eq(&before, &after),
        "the restart must actually swap the runtime, not report success and leave it"
    );
    // And it is taking work, rather than stuck in the quiesce window. A
    // company parked there refuses every cycle forever, which is worse than
    // the stale brain the rebuild was replacing.
    assert!(!after.is_quiesced());
    assert!(resp["status"].is_object(), "{raw}");
}

/// Calling it twice is not an error. The console offers the button off a
/// status read, so it can always be a moment stale — refusing when nothing
/// is pending would turn a harmless retry into a failure an operator has to
/// interpret.
#[tokio::test]
async fn restarting_a_healthy_company_is_a_no_op_not_a_refusal() {
    let home_dir = home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let state = state_with_company(home)
        .await
        .with_rebuilder(std::sync::Arc::new(Working {
            home: home.to_path_buf(),
        }));
    state.set_boot_inputs(id, crate::runtime::BootInputs::default());

    for attempt in 1..=2 {
        let (status, _, raw) =
            send(&state, "POST", "/api/v1/company/inference/restart", None).await;
        assert_eq!(status, StatusCode::OK, "attempt {attempt}: {raw}");
    }
}

/// A host that wired no rebuilder cannot do this, and must say so rather
/// than report a success that changed nothing. This is the pre-#290
/// deployment, and the console keeps showing the restart notice.
#[tokio::test]
async fn a_host_that_cannot_rebuild_says_so() {
    let home_dir = home();
    let state = state_with_company(home_dir.path()).await;

    let (status, _, raw) = send(&state, "POST", "/api/v1/company/inference/restart", None).await;
    assert_ne!(status, StatusCode::OK, "{raw}");
    assert!(
        raw.contains("restart the process"),
        "the failure must tell the operator what will work instead: {raw}"
    );

    // Critically, the company is still serving. A failed rebuild that left
    // it quiesced would turn a cosmetic dead end into an outage.
    let (status, _, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(status, StatusCode::OK);
}

/// `restart_runtime` takes `AdminScopedCompany` in its signature, but
/// nothing here had actually driven a plain member against it over HTTP —
/// every other test in this module authenticates as the seeded admin.
/// Rebuilding a company's runtime on demand is at least as sharp a
/// boundary as any other admin-only write in this module.
#[tokio::test]
async fn a_member_may_not_restart_the_runtime() {
    let home_dir = home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let state = state_with_company(home)
        .await
        .with_rebuilder(std::sync::Arc::new(Working {
            home: home.to_path_buf(),
        }));
    state.set_boot_inputs(id.clone(), crate::runtime::BootInputs::default());
    crate::server::test_support::seed_fixed_member(&state, "acme").await;
    let before = state.registry().get(&id).expect("registered");

    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/company/inference/restart")
        .header("cookie", crate::server::test_support::member_cookie("acme"))
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // A refused request must not have rebuilt the runtime either.
    let after = state.registry().get(&id).expect("still registered");
    assert!(
        std::sync::Arc::ptr_eq(&before, &after),
        "a forbidden restart must not swap the runtime"
    );
}

/// Issue #1736: the console cannot offer a restart it has no way to know is
/// available, so the status carries the capability rather than leaving the
/// card to guess from the deployment shape.
///
/// The pairing is the whole point — a flag that is always `false` would
/// satisfy the "no button on a host that cannot" half while silently
/// removing the action from every host that can.
#[tokio::test]
async fn the_status_says_whether_this_host_can_rebuild_in_place() {
    let bare_home = home();
    let bare = state_with_company(bare_home.path()).await;
    let (status, body, raw) = send(&bare, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(
        body["canRebuildInPlace"],
        json!(false),
        "a host with no rebuilder must say so, or the console renders a \
         Restart now button whose route can only answer with a config error: {raw}"
    );

    let wired_home = home();
    let wired = state_with_company(wired_home.path())
        .await
        .with_rebuilder(std::sync::Arc::new(Working {
            home: wired_home.path().to_path_buf(),
        }));
    let (status, body, raw) = send(&wired, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(
        body["canRebuildInPlace"],
        json!(true),
        "a host that wired one must keep offering the action: {raw}"
    );
}

/// Issue #1737: a probe the process already knows cannot authenticate is
/// refused here rather than sent.
///
/// The endpoint is the discard port, so a regression does not merely fail
/// this assertion — it makes an outbound connection, which is the behaviour
/// under test. A keyless `openrouter` carrying its own `base_url` resolves
/// direct and credential-less by construction, so this does not depend on
/// whatever the process environment happens to hold.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_probe_with_no_credential_is_refused_before_it_is_sent() {
    let home_dir = home();
    let state = state_with_company(home_dir.path()).await;

    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "openrouter", "baseUrl": "http://127.0.0.1:9/v1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    let (status, body, raw) = send(&state, "POST", "/api/v1/company/inference/test", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    assert_eq!(body["code"], json!("no_key"), "{raw}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no Authorization header"),
        "the refusal names the cause the vendor's 401 would have hidden: {raw}"
    );
}

/// The other half of that judgement: an endpoint the operator supplied may
/// legitimately want no bearer, so it is still probed. Refusing there would
/// turn a working local server into a false alarm — worse than the outbound
/// request it saves.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_keyless_custom_endpoint_is_still_probed() {
    let home_dir = home();
    let state = state_with_company(home_dir.path()).await;

    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "openai_compatible", "baseUrl": "http://127.0.0.1:9/v1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    let (status, body, raw) = send(&state, "POST", "/api/v1/company/inference/test", None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{raw}");
    assert_eq!(body["code"], json!("probe_failed"), "{raw}");
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn saved_company_probe_sends_its_resolved_model() {
    let sent_model = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let model_for_route = sent_model.clone();
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let sent_model = model_for_route.clone();
            async move {
                *sent_model.lock().unwrap() = body["model"].as_str().map(str::to_string);
                axum::Json(json!({
                    "choices": [{ "message": { "content": "pong" } }]
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let home_dir = home();
    let state = state_with_company(home_dir.path()).await;

    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({
            "provider": "openai_compatible",
            "baseUrl": format!("http://{address}/v1"),
            "models": { "chat-v1": "provider/model" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    let (status, body, raw) = send(&state, "POST", "/api/v1/company/inference/test", None).await;
    server.abort();

    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(body["ok"], true, "{raw}");
    assert_eq!(
        sent_model.lock().unwrap().as_deref(),
        Some("provider/model")
    );
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn saved_company_probe_keeps_typed_credential_failures() {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(|| async {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": {
                        "message": "Missing Authentication header",
                        "code": 401
                    }
                })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let home_dir = home();
    let state = state_with_company(home_dir.path()).await;

    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({
            "provider": "openai_compatible",
            "baseUrl": format!("http://{address}/v1"),
            "key": "not-a-real-key",
            "models": { "chat-v1": "provider/model" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    let (status, body, raw) = send(&state, "POST", "/api/v1/company/inference/test", None).await;
    server.abort();

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{raw}");
    assert_eq!(body["code"], json!("credential_rejected"), "{raw}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("did carry an Authorization header"),
        "{raw}"
    );
}

/// Issue #1737, the sentence that would have saved an hour: OpenRouter
/// answers a credential it cannot parse with `Missing Authentication
/// header`, which reads as "nothing was sent" and is how the issue came to
/// be filed against the wrong layer. The header *was* sent.
#[cfg(feature = "openhuman")]
#[test]
fn a_401_is_reported_as_a_rejected_credential_rather_than_a_missing_header() {
    let decl = inference::decl_for_probe(
        "openrouter",
        None,
        Some("a-key-for-some-other-vendor"),
        None,
    );
    let error = anyhow::Error::new(tinyinference::Error::Provider(Box::new(
        tinyinference::model::ProviderError {
            provider: "inference".to_string(),
            status: Some(401),
            message: "Missing Authentication header".to_string(),
            ..Default::default()
        },
    )));
    let (message, code) = probe_failure(&decl, &error);

    assert_eq!(code, "credential_rejected");
    assert!(
        message.contains("did carry an Authorization header"),
        "the console must not repeat the vendor's reading back at the operator: {message}"
    );
    assert!(
        message.contains("stored against the provider selected when it was saved"),
        "and it must name the reason a stored key can still be the wrong one: {message}"
    );
    assert!(
        message.contains("Missing Authentication header"),
        "the vendor's own words stay attached as evidence: {message}"
    );
}
/// The catalog is read from the endpoint **this company** is configured
/// against, not from OpenRouter's public registry.
///
/// This is the defect in one assertion. The route used to call
/// `openrouter_models()` with no reference to the company at all, so a
/// company pointed at a TinyHumans base URL was shown OpenRouter's 421
/// models — `anthropic/claude-sonnet-5` among them — and the endpoint then
/// answered `Model 'anthropic/claude-sonnet-5' is not available`.
#[tokio::test]
async fn model_catalog_route_lists_the_configured_endpoints_own_catalog() {
    const ENDPOINT: &str = "http://127.0.0.1:9/tier-native/v1";
    // Its own company id. Saving a key evicts that company's authenticated
    // catalogs, and a dozen tests in this module save one under `acme`; with
    // a shared id, whichever of them libtest happens to run alongside this
    // one throws the seeded fixture away. Locally the interleaving hid it;
    // CI's found it (Codex review on #2045).
    const COMPANY: &str = "catalog-tiers";
    let home_dir = home();
    let state = state_with_company_named(home_dir.path(), COMPANY).await;
    let (status, _, raw) = send_as(
        &state,
        COMPANY,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({
            "provider": "openai_compatible",
            "baseUrl": ENDPOINT,
            "key": "test-token",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    // Seeded *after* the save, not before: storing a key evicts this
    // company's authenticated catalogs, because a rotation changes what the
    // endpoint will answer without changing the cache key (Codex review on
    // #2045). Seeding first meant the save threw the fixture away and the
    // route fell through to a real request. This order is also what happens
    // in life — the cache is warmed by a read, which comes after the config
    // exists to be read against.
    seed_catalog_for(
        COMPANY,
        ENDPOINT,
        &["agentic-v1", "chat-v1", "reasoning-v1", "vision-v1"],
    );

    let (status, body, raw) = send_as(
        &state,
        COMPANY,
        "GET",
        "/api/v1/company/inference/models",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(
        body["baseUrl"], ENDPOINT,
        "the catalog names the endpoint it came from: {raw}"
    );
    let ids: Vec<&str> = body["models"]
        .as_array()
        .expect("models array")
        .iter()
        .map(|m| m["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        vec!["agentic-v1", "chat-v1", "reasoning-v1", "vision-v1"],
        "the configured endpoint's own ids, not OpenRouter's: {raw}"
    );
    // Keys rework (#2306), slice 2d: no vocabulary classification is
    // shipped on this DTO any more — the console never read it either.
    assert!(
        body.get("tierVocabulary").is_none() && body.get("tierDefaults").is_none(),
        "{raw}"
    );
}
