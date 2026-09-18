use super::*;

/// A catalog read is refused at an address a model endpoint is never on.
///
/// The connect-time probe is not the guard for this call. A provider row
/// outlives a failed probe on purpose, `add anyway` stores one that was
/// refused outright, and an edit can move the URL afterwards — so by the
/// time the picker asks this endpoint what it serves, "it passed once" is
/// not a statement about the URL in hand. On a hosted instance the address
/// below is where the container's credentials live.
#[tokio::test]
async fn a_catalog_read_is_refused_at_a_link_local_address() {
    let refused = discover_models(
        "http://169.254.169.254/latest/meta-data",
        Some("pw-not-a-real-key"),
        AuthStyle::Bearer,
        CatalogShape::OpenAi,
    )
    .await
    .expect_err("a link-local endpoint must not be fetched");

    assert!(
        refused.to_string().contains("link-local"),
        "the refusal should name the reason, said: {refused}"
    );
    assert!(
        refused.credential_status.is_none(),
        "a policy refusal is not evidence about the credential, and must not \
         be classified as one"
    );
}

/// A failure the catalogue cache writes itself names the endpoint redacted.
///
/// The fetch errors were already redacted where they are built; the empty
/// catalogue and the timeout are sentences `catalog_models` composes on its
/// own, and they are cached and replayed to every later reader. An endpoint
/// stored before the credential refusal existed must not come back out of
/// either one.
#[tokio::test]
async fn a_cache_written_catalog_failure_never_names_the_endpoint_credential() {
    let app = axum::Router::new().route(
        "/v1/models",
        axum::routing::get(|| async { axum::Json(serde_json::json!({ "data": [] })) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = format!("http://alice:hunter2@{address}/v1");

    let error = catalog_models(
        &endpoint,
        None,
        None,
        AuthStyle::Bearer,
        CatalogShape::OpenAi,
    )
    .await
    .expect_err("an empty catalogue is reported as a failure");
    server.abort();

    assert!(
        error.contains("empty model catalog"),
        "expected the cache's own sentence, got: {error}"
    );
    assert!(
        !error.contains("hunter2") && !error.contains("alice"),
        "the cached failure must not carry the endpoint's userinfo: {error}"
    );
}

// ---- the paged catalog (keys rework, issue #2306, slice 2a) ------------

/// Serves `/agent-integrations/openrouter/models`, calling `respond(offset)`
/// for each request to build the `(status, body)` it answers, and recording
/// every query string it was called with.
fn spawn_proxy_catalog(
    respond: impl Fn(usize) -> (u16, String) + Clone + Send + Sync + 'static,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use axum::extract::Query;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};

    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_for_route = seen.clone();
    let app = axum::Router::new().route(
        "/agent-integrations/openrouter/models",
        axum::routing::get(move |Query(params): Query<HashMap<String, String>>| {
            let seen = seen_for_route.clone();
            let respond = respond.clone();
            async move {
                let query = params
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&");
                seen.lock().unwrap().push(query);
                let offset: usize = params
                    .get("offset")
                    .and_then(|o| o.parse().ok())
                    .unwrap_or(0);
                let (status, body) = respond(offset);
                let status = StatusCode::from_u16(status).unwrap();
                (status, [("content-type", "application/json")], body).into_response() as Response
            }
        }),
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        format!("http://{address}/agent-integrations/openrouter"),
        seen,
    )
}

#[tokio::test]
async fn the_paged_catalog_is_read_to_total_with_the_bearer() {
    let (base, seen) = spawn_proxy_catalog(|offset| {
        let body = if offset == 0 {
            serde_json::json!({
                "success": true,
                "data": {"data": [{"id": "acme/test-model", "name": "Test", "context_length": 8192}], "total": 2},
            })
        } else {
            serde_json::json!({
                "success": true,
                "data": {"data": [{"id": "acme/other-model"}], "total": 2},
            })
        };
        (200, body.to_string())
    });

    let models = discover_models(
        &base,
        Some("th-not-a-real-key"),
        AuthStyle::Bearer,
        CatalogShape::PagedEnvelope,
    )
    .await
    .expect("the paged catalog reads");

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "acme/test-model");
    assert_eq!(models[0].name.as_deref(), Some("Test"));
    assert_eq!(models[0].context_length, Some(8192));
    assert_eq!(models[1].id, "acme/other-model");
    assert_eq!(seen.lock().unwrap().len(), 2, "one request per page");
}

#[tokio::test]
async fn a_large_catalog_is_read_in_pages_of_500() {
    const TOTAL: usize = 1_200;
    let (base, seen) = spawn_proxy_catalog(|offset| {
        let end = (offset + 500).min(TOTAL);
        let data: Vec<serde_json::Value> = (offset..end)
            .map(|i| serde_json::json!({"id": format!("acme/model-{i:04}")}))
            .collect();
        (
            200,
            serde_json::json!({
                "success": true,
                "data": {"data": data, "total": TOTAL},
            })
            .to_string(),
        )
    });

    let models = discover_models(
        &base,
        Some("th-not-a-real-key"),
        AuthStyle::Bearer,
        CatalogShape::PagedEnvelope,
    )
    .await
    .expect("the paged catalog reads");

    assert_eq!(models.len(), TOTAL);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "offsets 0, 500, 1000: {seen:?}");
    for id in [0usize, 500, 1199] {
        assert!(
            models.iter().any(|m| m.id == format!("acme/model-{id:04}")),
            "missing model-{id:04}"
        );
    }
}

#[tokio::test]
async fn a_503_before_the_snapshot_loads_is_memoized_not_empty() {
    let (base, _seen) = spawn_proxy_catalog(|_offset| {
        (
            503,
            serde_json::json!({"success": false, "error": "loading"}).to_string(),
        )
    });

    let scope = "paged-503-scope";
    let error = catalog_models(
        &base,
        Some("th-not-a-real-key"),
        Some(scope),
        AuthStyle::Bearer,
        CatalogShape::PagedEnvelope,
    )
    .await
    .expect_err("a 503 is a failure");
    assert!(error.contains("503"), "{error}");

    let now = Instant::now();
    assert!(
        catalog_cache_scoped(
            &shaped_endpoint(&base, CatalogShape::PagedEnvelope),
            Some(scope)
        )
        .lookup_failure(now)
        .is_some(),
        "a 503 must be memoized"
    );
}

#[tokio::test]
async fn a_401_or_403_on_the_paged_catalog_is_not_memoized() {
    for status in [401u16, 403] {
        let (base, _seen) = spawn_proxy_catalog(move |_offset| {
            (status, serde_json::json!({"error": "no"}).to_string())
        });
        let scope = format!("paged-{status}-scope");
        catalog_models(
            &base,
            Some("th-not-a-real-key"),
            Some(scope.as_str()),
            AuthStyle::Bearer,
            CatalogShape::PagedEnvelope,
        )
        .await
        .expect_err("a credential failure");
        let now = Instant::now();
        assert!(
            catalog_cache_scoped(
                &shaped_endpoint(&base, CatalogShape::PagedEnvelope),
                Some(scope.as_str())
            )
            .lookup_failure(now)
            .is_none(),
            "a {status} must not be memoized"
        );
    }
}

#[tokio::test]
async fn an_openai_shaped_answer_is_not_read_as_the_paged_catalog() {
    let (base, _seen) = spawn_proxy_catalog(|_offset| {
        (
            200,
            serde_json::json!({"data": [{"id": "acme/test-model"}]}).to_string(),
        )
    });

    let error = discover_models(
        &base,
        Some("th-not-a-real-key"),
        AuthStyle::Bearer,
        CatalogShape::PagedEnvelope,
    )
    .await
    .expect_err("an OpenAI-shaped body is not the envelope");
    assert!(error.to_string().contains("envelope"), "{error}");
}

#[tokio::test]
async fn a_paged_failure_never_names_the_endpoint_credential() {
    let (base, _seen) = spawn_proxy_catalog(|_offset| {
        (
            503,
            serde_json::json!({"success": false, "error": "loading"}).to_string(),
        )
    });
    let base = base.replacen("http://", "http://alice:hunter2@", 1);

    let error = discover_models(
        &base,
        Some("th-not-a-real-key"),
        AuthStyle::Bearer,
        CatalogShape::PagedEnvelope,
    )
    .await
    .expect_err("a 503 is a failure");
    let text = error.to_string();
    assert!(!text.contains("hunter2"), "{text}");
    assert!(!text.contains("alice"), "{text}");
}

#[tokio::test]
async fn an_oversized_page_is_refused_not_buffered() {
    let oversized = "x".repeat(paged_catalog::PAGE_BODY_CAP + 1);
    let (base, _seen) = spawn_proxy_catalog(move |_offset| (200, oversized.clone()));

    let error = discover_models(
        &base,
        Some("th-not-a-real-key"),
        AuthStyle::Bearer,
        CatalogShape::PagedEnvelope,
    )
    .await
    .expect_err("an oversized page is refused");
    assert!(error.to_string().contains("larger than"), "{error}");
}
