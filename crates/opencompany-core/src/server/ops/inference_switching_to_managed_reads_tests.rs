use axum::http::StatusCode;
use serde_json::json;

use super::inference_test_support::*;

use crate::ports::types::CompanyId;

/// Saving the managed brain has to be *visible*, not merely stored.
///
/// The write always landed — `PUT` persisted `managed` verbatim — but every
/// read reported the resolved kind, and `normalize_provider` folds the
/// managed alias onto `openrouter`. The console seeds its provider select
/// from this field verbatim (which is what keeps the select and the header
/// beside it from naming different providers), so the operator pressed Save,
/// got "Inference updated", and watched the card go straight back to
/// OpenRouter — taking the Connect-TinyHumans button, which only the managed
/// route renders, with it.
///
/// A save that cannot be observed is indistinguishable from one that did not
/// happen, so this asserts the round trip on both the mutation response and
/// a fresh read, from a company already saved on another provider.
#[tokio::test]
async fn switching_to_managed_reads_back_as_managed_rather_than_its_alias() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    // Start somewhere else, so "unchanged" and "reverted to OpenRouter"
    // cannot pass for the same answer.
    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "openrouter" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    let (status, resp, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "managed" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(
        resp["status"]["provider"], "managed",
        "the save answers with the choice that was made"
    );

    let (_, dto, raw) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(
        dto["provider"], "managed",
        "and it survives a reload: {raw}"
    );
    // Resolution is untouched — this is a read-back fix, not a routing one.
    assert_eq!(dto["slug"], "subscription");
    assert_eq!(dto["proxied"], true);
    assert_eq!(dto["source"], "runtime");
}

#[tokio::test]
async fn invalid_provider_config_is_rejected() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    // Ollama requires a base_url.
    let (status, err, _) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "ollama" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("base_url"),
        "{err}"
    );
}

#[tokio::test]
async fn key_never_leaks_across_any_response() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (_, _, put_raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "openai_compatible", "baseUrl": "https://byo.example/v1", "key": TOKEN })),
    )
    .await;
    let (_, get_dto, get_raw) = send(&state, "GET", "/api/v1/company/inference", None).await;
    // The live probe path returns an error (unreachable host) — assert the
    // scrubbed error body still never contains the token.
    let (_, _, test_raw) = send(&state, "POST", "/api/v1/company/inference/test", None).await;

    for raw in [put_raw, get_raw, test_raw] {
        assert!(!raw.contains(TOKEN), "a response leaked the token: {raw}");
    }

    // The list route, extended here **before** there was anything to leak.
    // The provider list is the newest way a credential could reach a wire,
    // and the point of adding it to this test on the same change that adds
    // the field is that the assertion exists before the mistake can.
    let providers = get_dto["providers"]
        .as_array()
        .expect("the status carries a provider list");
    assert_eq!(
        providers.len(),
        1,
        "one provider: the flat slot, as entry zero"
    );
    let entry_zero = &providers[0];
    assert_eq!(entry_zero["slug"], "openai_compatible");
    assert_eq!(
        entry_zero["keyConfigured"], true,
        "the boolean is the only thing a read may say about a key"
    );
    // Not "no field called `key`" — no field with the VALUE, whatever it is
    // called. A convenience rename would pass the narrower assertion.
    for (name, value) in entry_zero.as_object().expect("a provider object") {
        assert!(
            !value.to_string().contains(TOKEN),
            "provider field `{name}` leaked the token"
        );
    }

    // Every WRITE route, extended on the change that adds them rather than
    // afterwards. A credential reaches this subsystem through four bodies
    // now, and each one is a separate chance to echo it back.
    const SECOND: &str = "sk-not-a-real-key-for-the-second-provider";
    let (_, _, add_raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({
            "kind": "custom",
            "label": "Acme gateway",
            "baseUrl": UNREACHABLE,
            "key": SECOND,
        })),
    )
    .await;
    let (_, _, edit_raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/providers/acme-gateway",
        Some(json!({ "key": SECOND })),
    )
    .await;
    let (_, _, probe_raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/probe",
        Some(json!({ "baseUrl": UNREACHABLE, "key": SECOND })),
    )
    .await;
    let (_, list_dto, list_raw) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let (_, _, delete_raw) = send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/acme-gateway",
        None,
    )
    .await;

    for raw in [add_raw, edit_raw, probe_raw, list_raw, delete_raw] {
        for token in [TOKEN, SECOND] {
            assert!(!raw.contains(token), "a write route leaked a token: {raw}");
        }
    }
    // And the value assertion again, over a list that now holds two
    // credentials rather than one.
    for provider in list_dto["providers"].as_array().expect("a provider list") {
        for (name, value) in provider.as_object().expect("a provider object") {
            for token in [TOKEN, SECOND] {
                assert!(
                    !value.to_string().contains(token),
                    "provider field `{name}` leaked a token"
                );
            }
        }
    }
}

// --- the connect flow ------------------------------------------------------

#[tokio::test]
async fn a_cloud_provider_takes_its_endpoint_from_the_catalogue() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, resp, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        // A base URL is sent and must be ignored: the paths in that table
        // are too varied for an override to be anything but a mistake.
        Some(json!({ "kind": "groq", "baseUrl": "https://wrong.example/v1", "model": "acme/test-model" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let providers = resp["status"]["providers"].as_array().unwrap();
    let groq = providers.iter().find(|p| p["slug"] == "groq").unwrap();
    assert_eq!(groq["baseUrl"], "https://api.groq.com/openai/v1");
    assert_eq!(groq["label"], "Groq");
    assert_eq!(groq["enabled"], true, "a new provider arrives on");
}

#[tokio::test]
async fn a_rename_that_posts_back_the_served_endpoint_keeps_the_stored_one() {
    // Codex review on #2281. The row is served through `redact_endpoint`,
    // which masks a path segment that only looks like userinfo. A console
    // that posts that value back on a rename must not overwrite a working
    // endpoint with the mask.
    const GATEWAY: &str = "http://127.0.0.1:9/proxy/http:user@example.com/v1";
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, _, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({
            "kind": "custom",
            "label": "Acme gateway",
            "baseUrl": GATEWAY,
            "key": "sk-not-a-real-key",
            "model": "acme-model",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a path is not a credential: {raw}");

    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let served = dto["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "acme-gateway")
        .expect("the row was created")["baseUrl"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(served, GATEWAY, "the row is served redacted: {served}");

    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/providers/acme-gateway",
        Some(json!({ "label": "Acme renamed", "baseUrl": served })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    use crate::company::inference::store;
    let runtime = state
        .registry()
        .get(&CompanyId::new("acme"))
        .expect("registered");
    let stored = store::list_providers(runtime.id(), runtime.secrets().as_ref())
        .await
        .unwrap();
    let acme = stored
        .iter()
        .find(|p| p.slug == "acme-gateway")
        .expect("still listed");
    assert_eq!(
        acme.base_url, GATEWAY,
        "the stored endpoint survived the rename"
    );
    assert_eq!(acme.label, "Acme renamed");
}

#[tokio::test]
async fn a_second_provider_holds_its_own_credential() {
    // The first moment two keys exist at once, which is the whole point of
    // the list: today one slot per company means switching provider strands
    // a credential for the wrong vendor in the only slot there is.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    for (label, key) in [
        ("First", "sk-not-a-real-key-1"),
        ("Second", "sk-not-a-real-key-2"),
    ] {
        let (status, _, raw) = send(
            &state,
            "POST",
            "/api/v1/company/inference/providers",
            Some(
                json!({ "kind": "custom", "label": label, "baseUrl": UNREACHABLE, "key": key, "model": "acme-model" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{raw}");
    }

    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let providers = dto["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 2);
    assert!(
        providers.iter().all(|p| p["keyConfigured"] == true),
        "each provider holds its own credential: {providers:?}"
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_keeps_the_key_and_creates_the_row() {
    // The non-destructive path, which is the one the naive implementation
    // gets wrong: a proxy, a WAF, a rate limit or a mistyped model id all
    // fail a probe while the key is perfectly good.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, resp, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({
            "kind": "custom",
            "label": "Acme gateway",
            "baseUrl": UNREACHABLE,
            "key": "sk-not-a-real-key",
            "model": "acme-model",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the save succeeded: {raw}");
    assert_eq!(resp["probe"]["ok"], false);
    assert_eq!(resp["probe"]["class"], "endpoint");
    let acme = resp["status"]["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "acme-gateway")
        .expect("the row was created");
    assert_eq!(acme["keyConfigured"], true, "the key was kept");
    assert_eq!(acme["health"]["state"], "endpoint");
}

#[tokio::test]
async fn a_slug_that_shadows_a_builtin_is_refused_before_anything_is_written() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, err, _) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Groq", "baseUrl": UNREACHABLE, "key": "sk-not-a-real-key" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");

    // And nothing landed: not the record, and not the credential.
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert!(
        dto["providers"].as_array().unwrap().is_empty(),
        "a refused add writes nothing: {dto}"
    );
}

#[tokio::test]
async fn a_custom_provider_with_no_name_has_no_slug_to_write_under() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, _, _) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "   ", "baseUrl": UNREACHABLE })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn deleting_a_provider_clears_its_credential_and_scrubs_its_routes() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "key": "sk-not-a-real-key", "model": "acme-model" })),
    )
    .await;
    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/routes",
        Some(json!({ "routes": { "reasoning-v1": "acme:gpt-5" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    // The company's only provider auto-became its default (X1), so the
    // delete needs confirmation like any other in-use row.
    let (status, resp, raw) = send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/acme?confirmInUse=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(
        resp["affectedTiers"],
        json!(["reasoning-v1"]),
        "the operator is told which rows moved"
    );

    let (_, routes, _) = send(&state, "GET", "/api/v1/company/inference/routes", None).await;
    assert!(
        routes["routes"].as_object().unwrap().is_empty(),
        "the orphaned route was reset: {routes}"
    );

    // Re-adding the slug must not inherit the old credential. The store has
    // no delete, so this only holds because the clear was actually issued.
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "model": "acme-model" })),
    )
    .await;
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let acme = dto["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "acme")
        .unwrap();
    assert_eq!(
        acme["keyConfigured"], false,
        "a re-added slug must not silently reuse the removed key"
    );
}

/// Bug KR-L1-01 (live E2E, orchestrator-reported): a provider whose
/// `/models` catalog is real, valid JSON but too large for the probe's
/// success-body cap used to be silently read as zero models, reporting a
/// healthy `ok` add — the operator's key was never at fault, and nothing
/// said so. Now the add still saves the row (this failure is
/// non-destructive: `ProbeClass::Unknown` never rolls a credential back),
/// but the probe result is an explicit failure naming the model list
/// itself, never a bare `ok: true` over zero models.
#[tokio::test]
async fn an_add_whose_catalog_is_too_large_to_read_never_reports_ok() {
    use axum::routing::get;

    // One entry whose filler alone exceeds the probe's cap — a cheap way
    // to produce a real over-the-wire body larger than
    // `probe::CATALOG_BODY_CAP` without generating (and comparing) many
    // megabytes of meaningful content.
    let oversized = "x".repeat(17 * 1024 * 1024);
    let body = format!(r#"{{"data":[{{"id":"acme/test-model","description":"{oversized}"#);
    let app = axum::Router::new().route(
        "/v1/models",
        get(move || {
            let body = body.clone();
            async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    body,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, resp, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({
            "kind": "custom",
            "label": "Acme",
            "baseUrl": format!("http://{address}/v1"),
            "key": "sk-not-a-real-key",
            "model": "acme/test-model",
        })),
    )
    .await;
    server.abort();
    assert_eq!(
        status,
        StatusCode::OK,
        "a non-destructive probe failure still saves: {raw}"
    );
    assert_eq!(resp["probe"]["ok"], false, "{resp}");
    assert_eq!(resp["probe"]["modelCount"], 0, "{resp}");
    let message = resp["probe"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("could not be read"),
        "must say the model list itself could not be read, not a generic failure: {message}"
    );

    // And the row's own recorded health must not be "ok" either — the
    // whole point being that the console's health column must not read
    // as a working, connected provider.
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let acme = dto["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "acme")
        .unwrap();
    assert_ne!(acme["health"]["state"], "ok", "{acme}");
}
