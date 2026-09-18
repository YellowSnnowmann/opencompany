use super::*;

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use tower::ServiceExt;

use crate::ports::types::CompanyId;
use crate::{AppConfig, AppState};

#[tokio::test]
async fn start_returns_an_expiring_structured_retirement_error() {
    let response = native_oauth_start_retired();
    assert_eq!(response.status(), StatusCode::GONE);
    assert_eq!(
        response.headers().get("deprecation").unwrap(),
        "true",
        "a caller must learn this is a bounded compatibility response"
    );
    assert_eq!(
        response.headers().get("sunset").unwrap(),
        NATIVE_OAUTH_SUNSET,
        "the response itself names when the bridge is removed"
    );

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["code"], "native_oauth_retired");
    assert_eq!(body["removalAfter"], NATIVE_OAUTH_REMOVAL_DATE);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("not reachable by agents"),
        "{body}"
    );
}

#[tokio::test]
async fn callback_ends_an_inflight_flow_without_accepting_its_code() {
    let response = router()
        .with_state(AppState::new(AppConfig::default()))
        .oneshot(
            Request::get("/api/v1/oauth/callback?code=CANARY-authz-code&state=CANARY-signed-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::GONE);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    assert_eq!(
        response.headers().get("sunset").unwrap(),
        NATIVE_OAUTH_SUNSET
    );
    assert!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );

    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("Native OAuth connection is no longer available"));
    assert!(body.contains("Nothing was saved from this authorization"));
    assert!(body.contains(NATIVE_OAUTH_REMOVAL_DATE));
    assert!(
        !body.contains("CANARY"),
        "the callback neither accepts nor reflects its former OAuth inputs: {body}"
    );
}

// ---- disconnect / best-effort revoke ----------------------------------

use crate::company::CompanyManifest;
use crate::company::runtime::CompanyRuntime;
use crate::runtime::RuntimeBuilder;

/// Builds an isolated in-memory company runtime for disconnect tests.
///
/// The caller must hold the returned handle for the life of the test: it
/// owns the runtime's home directory and removes it on drop.
async fn test_runtime() -> (Arc<CompanyRuntime>, tempfile::TempDir) {
    let home = tempfile::Builder::new()
        .prefix("oc-disc-")
        .tempdir()
        .expect("tempdir");
    let manifest: CompanyManifest =
        toml::from_str("[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n").unwrap();
    let runtime = RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(CompanyId::new("acme"))
        .build()
        .await
        .unwrap();
    (Arc::new(runtime), home)
}

/// A process-unique provider id (env-key safe) so the env vars each test
/// sets never collide with a sibling test running in parallel.
fn unique_provider() -> String {
    format!("revtest{}", crate::ports::generate_id().replace('-', ""))
}

async fn store_token(runtime: &CompanyRuntime, provider: &str, access_token: &str) {
    runtime
        .secrets()
        .set(
            runtime.id(),
            &oauth_key(provider),
            SecretValue(
                json!({ "token": { "access_token": access_token }, "account": "acc" }).to_string(),
            ),
        )
        .await
        .unwrap();
}

async fn is_blanked(runtime: &CompanyRuntime, provider: &str) -> bool {
    runtime
        .secrets()
        .get(runtime.id(), &oauth_key(provider))
        .await
        .unwrap()
        .map(|v| v.expose().trim().is_empty())
        .unwrap_or(true)
}

fn revoke_env(provider: &str, url: String) -> crate::app::config::MapEnv {
    let key = provider.to_ascii_uppercase();
    crate::app::config::MapEnv::new([
        (format!("OPENCOMPANY_OAUTH_{key}_ID"), "cid".to_string()),
        (
            format!("OPENCOMPANY_OAUTH_{key}_SECRET"),
            "csec".to_string(),
        ),
        (
            format!("OPENCOMPANY_OAUTH_{key}_AUTHORIZE_URL"),
            "http://x/a".to_string(),
        ),
        (
            format!("OPENCOMPANY_OAUTH_{key}_TOKEN_URL"),
            "http://x/t".to_string(),
        ),
        (format!("OPENCOMPANY_OAUTH_{key}_REVOKE_URL"), url),
    ])
}

/// No app credentials configured → provider_config is `None`, so there is no
/// remote to revoke, but the disconnect must still blank the local secret.
#[tokio::test]
async fn disconnect_blanks_secret_without_revoke_config() {
    let (runtime, _home) = test_runtime().await;
    let provider = unique_provider();
    store_token(&runtime, &provider, "CANARY-should-never-leak").await;

    let resp = do_disconnect(runtime.clone(), &provider).await.unwrap();
    assert_eq!(resp.0["connected"], false);
    assert!(is_blanked(&runtime, &provider).await, "secret not blanked");
}

/// The best-effort revoke path is invoked: with app credentials + a revoke
/// URL configured, disconnect POSTs the token to the provider endpoint AND
/// blanks the local secret.
#[tokio::test]
async fn disconnect_invokes_provider_revoke() {
    use axum::extract::State;
    use axum::routing::post;

    let hits: Arc<tokio::sync::Mutex<Vec<String>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/revoke",
            post(|State(hits): State<Arc<tokio::sync::Mutex<Vec<String>>>>, body: String| async move {
                hits.lock().await.push(body);
                "ok"
            }),
        )
        .with_state(hits.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let (runtime, _home) = test_runtime().await;
    let provider = unique_provider();
    let env = revoke_env(&provider, format!("http://{addr}/revoke"));

    store_token(&runtime, &provider, "CANARY-revoke-me").await;
    let _ = do_disconnect_from(runtime.clone(), &provider, &env)
        .await
        .unwrap();

    let received = hits.lock().await;
    assert_eq!(received.len(), 1, "revoke endpoint was not called");
    assert!(
        received[0].contains("CANARY-revoke-me"),
        "revoke request did not carry the stored token"
    );
    drop(received);
    assert!(is_blanked(&runtime, &provider).await, "secret not blanked");
}

/// A revoke endpoint that refuses the connection must not fail the
/// disconnect: the local secret is still blanked.
#[tokio::test]
async fn disconnect_blanks_secret_when_revoke_fails() {
    // Bind then drop to obtain a port with nothing listening on it.
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_addr = dead.local_addr().unwrap();
    drop(dead);

    let (runtime, _home) = test_runtime().await;
    let provider = unique_provider();
    let env = revoke_env(&provider, format!("http://{dead_addr}/revoke"));

    store_token(&runtime, &provider, "CANARY-unreachable").await;
    // Must still succeed even though the revoke POST cannot connect.
    let _ = do_disconnect_from(runtime.clone(), &provider, &env)
        .await
        .unwrap();
    assert!(is_blanked(&runtime, &provider).await, "secret not blanked");
}

/// `disconnect` is declared `AdminScopedCompany` in its signature, but every
/// other test above calls [`do_disconnect`]/[`do_disconnect_from`] directly,
/// bypassing the extractor entirely. Route through the real router with a
/// member session so the scope is the thing under test.
#[tokio::test]
async fn disconnect_refuses_a_member_session_with_403() {
    let (runtime, _home) = test_runtime().await;
    let provider = unique_provider();
    store_token(&runtime, &provider, "CANARY-member-must-not-disconnect").await;

    let state = AppState::new(AppConfig::default());
    state
        .registry()
        .insert(runtime.id().clone(), runtime.clone());
    let member = crate::server::test_support::seed_session(
        &state,
        runtime.id().as_ref(),
        crate::ports::users::UserRole::Member,
    )
    .await;

    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/companies/{}/connections/{provider}/disconnect",
            runtime.id().as_ref()
        ))
        .header("cookie", member)
        .body(Body::empty())
        .unwrap();

    let response = crate::server::router(state).oneshot(request).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a member reached an admin-scoped disconnect route"
    );
    assert!(
        !is_blanked(&runtime, &provider).await,
        "a refused member must not blank the stored credential"
    );
}
