//! Credential-scrubbing, paged-catalog and catalog-body-cap tests
//! (split out of `probe_tests.rs`).

use super::tests_ssrf::LOCAL_OFFERED;
use super::*;

// ---- what a failure may write down ---------------------------------------

#[test]
fn a_basic_token_is_encoded_the_way_reqwest_sends_it() {
    assert_eq!(base64_standard(b"alice:hunter2"), "YWxpY2U6aHVudGVyMg==");
    assert_eq!(base64_standard(b"a"), "YQ==");
    assert_eq!(base64_standard(b"ab"), "YWI=");
    assert_eq!(base64_standard(b""), "");
    assert_eq!(percent_decode("p%40ss%zz"), "p@ss%zz");
}

#[test]
fn every_form_of_the_endpoint_credential_is_scrubbed_from_text() {
    let endpoint = "http://alice:p%40ss@127.0.0.1:9/v1/models";
    let token = base64_standard(b"alice:p@ss");
    let echoed = format!(
        "rejected Basic {token} / {} for alice:p@ss (raw p%40ss)",
        token.trim_end_matches('=')
    );
    let scrubbed = scrub_endpoint_credential(endpoint, &echoed);
    for secret in [
        "p@ss",
        "p%40ss",
        token.as_str(),
        token.trim_end_matches('='),
    ] {
        assert!(
            !scrubbed.contains(secret),
            "{secret:?} survived: {scrubbed}"
        );
    }
    assert!(
        scrubbed.contains("alice"),
        "the account name still reads: {scrubbed}"
    );

    // Username only: that username is the token, in every form it can echo.
    let token_only = "http://sk-not%2Ba-real-key@127.0.0.1:9/v1/models";
    let basic = base64_standard(b"sk-not+a-real-key:");
    let echoed = format!(
        "bad key sk-not+a-real-key (sent sk-not%2Ba-real-key) in Basic {basic} / {}",
        basic.trim_end_matches('=')
    );
    let scrubbed = scrub_endpoint_credential(token_only, &echoed);
    for secret in [
        "sk-not+a-real-key",
        "sk-not%2Ba-real-key",
        basic.as_str(),
        basic.trim_end_matches('='),
    ] {
        assert!(
            !scrubbed.contains(secret),
            "{secret:?} survived: {scrubbed}"
        );
    }
    // No userinfo: the text is untouched.
    assert_eq!(
        scrub_endpoint_credential("http://127.0.0.1:9/v1", "Basic abc"),
        "Basic abc"
    );
}

/// An upstream that echoes the request's `Authorization` header back in its
/// 401 body. Served on loopback by hand, so the test needs nothing but tokio.
#[tokio::test]
async fn a_probe_failure_never_logs_the_basic_credential_an_endpoint_carried() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let authorization = request
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("authorization")
                    .then(|| value.trim().to_string())
            })
            .unwrap_or_default();
        let body = format!("{{\"error\":\"rejected [{authorization}] for alice:hunter2\"}}");
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.ok();
        authorization
    });

    let failure = probe_models(
        &format!("http://alice:hunter2@{address}/v1"),
        None,
        catalogue::AuthStyle::None,
        LOCAL_OFFERED,
        catalogue::CatalogShape::OpenAi,
    )
    .await
    .expect_err("a 401 is a failure");
    let sent = server.await.unwrap();

    assert_eq!(
        sent, "Basic YWxpY2U6aHVudGVyMg==",
        "the premise: reqwest sends the endpoint's userinfo as Basic auth"
    );
    assert!(
        failure.raw.contains("[Basic ***]"),
        "the echo reached the log text, scrubbed: {}",
        failure.raw
    );
    for secret in ["hunter2", "YWxpY2U6aHVudGVyMg"] {
        assert!(
            !failure.raw.contains(secret),
            "{secret} reached the log text: {}",
            failure.raw
        );
    }
}

// ---- the paged catalog (keys rework, issue #2306, slice 2a) ------------

/// A `PagedEnvelope` probe reads every page, following `total`, and sends
/// the bearer on every request — not just the first.
#[tokio::test]
async fn a_paged_probe_reads_every_page_with_the_bearer() {
    use axum::extract::Query;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    // Each page's `(query string, bearer)` as the fake server saw it.
    type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let seen_for_route = seen.clone();
    let app = axum::Router::new().route(
        "/agent-integrations/openrouter/models",
        axum::routing::get(
            move |Query(params): Query<HashMap<String, String>>, headers: axum::http::HeaderMap| {
                let seen = seen_for_route.clone();
                async move {
                    let query = params
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("&");
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    seen.lock().unwrap().push((query, auth));
                    let offset: usize = params
                        .get("offset")
                        .and_then(|o| o.parse().ok())
                        .unwrap_or(0);
                    let data = if offset == 0 {
                        serde_json::json!([{"id": "acme/test-model"}, {"id": "acme/other-model"}])
                    } else {
                        serde_json::json!([{"id": "acme/third-model"}])
                    };
                    axum::Json(serde_json::json!({
                        "success": true,
                        "data": {"data": data, "total": 3, "limit": 500, "offset": offset},
                    }))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{address}/agent-integrations/openrouter");

    let ids = probe_models(
        &base,
        Some("th-not-a-real-key"),
        catalogue::AuthStyle::Bearer,
        LOCAL_OFFERED,
        catalogue::CatalogShape::PagedEnvelope,
    )
    .await
    .expect("the paged catalog reads");
    server.abort();

    assert_eq!(
        ids,
        vec!["acme/test-model", "acme/other-model", "acme/third-model"]
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "one request per page, got: {seen:?}");
    assert!(seen[0].0.contains("limit=500") && seen[0].0.contains("offset=0"));
    assert!(seen[1].0.contains("limit=500") && seen[1].0.contains("offset=2"));
    for (_, auth) in seen.iter() {
        assert_eq!(auth.as_deref(), Some("Bearer th-not-a-real-key"));
    }
}

/// A page that answers the OpenAI shape (not the envelope) classifies
/// `Unknown`, never `Auth` — a body that fails to parse says nothing about
/// the credential.
#[tokio::test]
async fn a_paged_probe_that_gets_an_openai_body_is_unknown_not_auth() {
    let app = axum::Router::new().route(
        "/agent-integrations/openrouter/models",
        axum::routing::get(|| async {
            axum::Json(serde_json::json!({"data": [{"id": "acme/test-model"}]}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{address}/agent-integrations/openrouter");

    let failure = probe_models(
        &base,
        Some("th-not-a-real-key"),
        catalogue::AuthStyle::Bearer,
        LOCAL_OFFERED,
        catalogue::CatalogShape::PagedEnvelope,
    )
    .await
    .expect_err("an OpenAI-shaped body is not the envelope");
    server.abort();

    assert_eq!(failure.class, ProbeClass::Unknown);
}

// ---- the catalog body cap (bug KR-L1-01, keys rework issue #2306) ------

/// The regression test for the bug itself: a realistic OpenRouter-sized
/// catalog (600 ids, comfortably past the old 64 KiB failure-body cap
/// this success read used to share) is read to the last id, not silently
/// truncated into an empty list.
#[tokio::test]
async fn a_realistic_sized_catalog_past_the_old_64kib_cap_is_read_in_full() {
    const COUNT: usize = 600;
    // Padding per entry so the whole body lands comfortably past 900 KB —
    // the measured size of OpenRouter's real `/models` response — while
    // staying far under `CATALOG_BODY_CAP` (16 MiB).
    let filler = "x".repeat(1600);
    let entries: Vec<_> = (0..COUNT)
        .map(|i| serde_json::json!({"id": format!("acme/test-model-{i}"), "description": filler}))
        .collect();
    let body = serde_json::json!({"data": entries}).to_string();
    assert!(
        body.len() > 900_000,
        "test fixture must exceed 900 KB to reproduce the bug: {} bytes",
        body.len()
    );

    let app = axum::Router::new().route(
        "/models",
        axum::routing::get(move || {
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

    let ids = probe_models(
        &format!("http://{address}"),
        Some("sk-not-a-real-key"),
        catalogue::AuthStyle::Bearer,
        LOCAL_OFFERED,
        catalogue::CatalogShape::OpenAi,
    )
    .await
    .expect("a large but valid catalog must be read in full, not truncated");
    server.abort();

    assert_eq!(ids.len(), COUNT, "every id must survive the read");
    assert_eq!(ids[0], "acme/test-model-0");
    assert_eq!(ids[COUNT - 1], format!("acme/test-model-{}", COUNT - 1));
}

/// A body that runs past `CATALOG_BODY_CAP` is refused outright, as a
/// classified `Unknown`/`truncated` failure — never silently parsed as an
/// empty catalog. This is the exact failure mode the bug report measured
/// against real OpenRouter: the body is cut mid-string, which as raw text
/// would be garbage JSON, and the fix is to never hand that text to the
/// parser at all.
#[tokio::test]
async fn a_catalog_body_over_the_cap_is_an_explicit_error_not_an_empty_list() {
    // One entry whose `description` alone is bigger than the whole cap —
    // the cheapest way to produce a real, over-the-wire body that exceeds
    // `CATALOG_BODY_CAP` without generating and comparing 16 MiB of
    // meaningful content.
    let oversized = "x".repeat(CATALOG_BODY_CAP + 1024);
    let body = format!(r#"{{"data":[{{"id":"acme/test-model","description":"{oversized}"#);

    let app = axum::Router::new().route(
        "/models",
        axum::routing::get(move || {
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

    let failure = probe_models(
        &format!("http://{address}"),
        Some("sk-not-a-real-key"),
        catalogue::AuthStyle::Bearer,
        LOCAL_OFFERED,
        catalogue::CatalogShape::OpenAi,
    )
    .await
    .expect_err("a body over the cap must be an explicit failure, not Ok(vec![])");
    server.abort();

    assert!(
        failure.truncated,
        "the failure must be flagged as a truncated catalog, not a generic Unknown"
    );
    assert_eq!(
        failure.class,
        ProbeClass::Unknown,
        "still non-destructive — a huge catalog says nothing about the credential"
    );
}
