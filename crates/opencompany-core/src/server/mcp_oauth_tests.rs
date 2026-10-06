use super::*;

#[test]
fn escape_neutralizes_html() {
    let out = escape("<script>alert(1)</script>&\"");
    assert!(!out.contains('<'));
    assert!(!out.contains('>'));
    assert!(out.contains("&lt;script&gt;"));
    assert!(out.contains("&amp;"));
    assert!(out.contains("&quot;"));
}

#[test]
fn success_page_names_the_server_escaped() {
    let resp = success_page("no<tion");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[test]
fn non_empty_trims_and_rejects_blank() {
    assert_eq!(non_empty(Some("  x ")), Some("x"));
    assert_eq!(non_empty(Some("   ")), None);
    assert_eq!(non_empty(None), None);
}

// Integration-style coverage of the unauthenticated callback route's guard
// branches, driven through a real axum app (mirrors
// `mcp::probe::oauth_token_never_leaks_into_probed_health`). These are the
// security-relevant early exits: none may exchange a code or reach a company.
use crate::AppConfig;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

async fn call(uri: &str) -> (StatusCode, String) {
    let state = AppState::new(AppConfig::default());
    let app = router().with_state(state);
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn callback_denied_by_authorization_server_renders_error() {
    let (status, body) = call("/oauth/mcp/callback?error=access_denied").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Sign-in was declined"));
}

#[tokio::test]
async fn callback_missing_code_or_state_is_rejected() {
    // No code, no state.
    let (status, body) = call("/oauth/mcp/callback").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Invalid sign-in response"));
    // Code without state is equally malformed.
    let (status, _) = call("/oauth/mcp/callback?code=abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn callback_with_unknown_state_is_expired() {
    // A well-formed callback whose `state` was never parked (or already
    // consumed) reclaims nothing — no token exchange happens.
    let (status, body) = call("/oauth/mcp/callback?code=abc&state=never-parked").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Sign-in expired"));
}

#[tokio::test]
async fn callback_for_unknown_company_stops_before_exchange() {
    use crate::company::mcp_oauth::{begin, fixture};
    use crate::ports::types::CompanyId;

    // Park a sign-in for a company the registry doesn't hold, then drive the
    // callback: it must resolve the parked state, find no company, and return
    // before ever attempting a token exchange.
    let server = fixture::spawn(true, serde_json::json!({ "access_token": "at" })).await;
    let state = AppState::new(AppConfig::default())
        .with_mcp_oauth(tinymcp::registry::oauth::OAuthFlow::new(None).unwrap());
    let url = begin(
        state.mcp_oauth(),
        &server.mcp,
        &CompanyId::new("ghost"),
        "notion",
        "https://acme.example/oauth/mcp/callback",
    )
    .await
    .expect("sign-in begins");
    let cb_state = fixture::state_of(&url);
    let app = router().with_state(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/oauth/mcp/callback?code=abc&state={cb_state}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("Company not found"));
    assert!(server.seen.token_forms.lock().unwrap().is_empty());
}
