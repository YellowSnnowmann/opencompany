//! End-to-end tests for `POST /api/v1/admin/set-password`.
//!
//! Driven through the whole router. The value of this surface is the
//! composition: a valid platform token sets the standing admin's password on the
//! live host, and — the security crux — a token minted for SSO login is refused
//! here while a set-password token is refused at `/sso/redeem`, because the two
//! are signed with domain-separated keys derived from the same secret.

#![cfg(feature = "platform-jwt")]

use super::derive_key;
use crate::company::CompanyManifest;
use crate::ports::CompanyStore;
use crate::ports::types::{CompanyId, CompanyRecord, SecretValue};
use crate::runtime::RuntimeBuilder;
use crate::server::ops::ConnectionsRuntime;
use crate::server::router;
use crate::{AppConfig, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

const SSO_SECRET: &str = "sso-signing-secret";
const ADMIN: &str = "Ada@Example.com";
/// ≥ `MIN_PASSWORD_LEN` (12) and carries no part of the admin address, so it
/// passes `password::validate` and only the auth path can refuse it.
const PASSWORD: &str = "correct horse battery staple";

fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("oc-setpw-")
        .tempdir()
        .expect("tempdir")
}

/// A manifest whose `[users] admins` bootstraps `ada` — capitals on purpose so
/// normalization is exercised end to end.
fn manifest() -> CompanyManifest {
    toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [users]\nadmins = [\"Ada@Example.com\"]\n",
    )
    .unwrap()
}

/// State over a manifest and config, with the company registered under `id`.
async fn state_from_id(
    home: &std::path::Path,
    manifest: CompanyManifest,
    config: AppConfig,
    id: CompanyId,
) -> AppState {
    let store = crate::store::FsCompanyStore::new(home.to_path_buf());
    store
        .save(&CompanyRecord {
            general_channel: Default::default(),
            overlay_desk_hive: Vec::new(),
            overlay_retired_agents: Vec::new(),
            overlay_agent_edits: Vec::new(),
            id: id.clone(),
            manifest: manifest.clone(),
            ledger: Vec::new(),
            lifecycle: "running".to_string(),
            overlay_agents: Vec::new(),
            overlay_desk_members: Vec::new(),
            overlay_desk_order: Vec::new(),
            overlay_desks: Vec::new(),
            overlay_workflows: Vec::new(),
            overlay_budgets: Vec::new(),
            overlay_policy: None,
            overlay_tool_grants: None,
            overlay_desk_tools: Default::default(),
            disabled_workflows: Vec::new(),
            template_provenance: None,
            setup: None,
            name_confirmed: false,
            activation_completed_at: None,
            created_at_millis: None,
        })
        .await
        .unwrap();
    let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id.clone())
        .build()
        .await
        .unwrap();
    let state = AppState::new(config)
        .with_home(home.to_path_buf())
        .with_connections(ConnectionsRuntime::new());
    state.registry().insert(id, Arc::new(runtime));
    state
}

/// State with the SSO secret configured (the enabled path), company `acme`.
async fn enabled_state(home: &std::path::Path) -> AppState {
    state_from_id(
        home,
        manifest(),
        AppConfig {
            sso_secret: Some(SecretValue(SSO_SECRET.to_string())),
            ..AppConfig::default()
        },
        CompanyId::new("acme"),
    )
    .await
}

fn far_future() -> u64 {
    // ~2050, comfortably beyond any test run.
    2_524_608_000
}

/// Signs an HS256 token over the claim shape both token families share, with an
/// explicit key — so a test can sign with the derived set-password key or the
/// raw SSO key and prove the two do not cross-verify.
fn sign(key: &[u8], slug: &str, sub: &str, jti: &str, exp: u64) -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    encode(
        &Header::new(Algorithm::HS256),
        &serde_json::json!({
            "sub": sub,
            "slug": slug,
            "jti": jti,
            "iat": 1_700_000_000u64,
            "exp": exp,
        }),
        &EncodingKey::from_secret(key),
    )
    .expect("sign")
}

/// A well-formed set-password token: signed with the derived key, for `acme`/`ada`.
fn set_password_token() -> String {
    sign(
        &derive_key(SSO_SECRET),
        "acme",
        ADMIN,
        "jti-1",
        far_future(),
    )
}

async fn call(state: &AppState, token: &str, password: &str) -> axum::response::Response {
    router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/set-password")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "token": token, "password": password }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn admin_has_password(state: &AppState) -> bool {
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    runtime
        .users()
        .list_users(runtime.id())
        .await
        .unwrap()
        .iter()
        .any(|u| u.password_hash.is_some())
}

// ---------------------------------------------------------------------------
// Success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_valid_token_sets_the_admin_password_without_forcing_a_change() {
    let home = home();
    let state = enabled_state(home.path()).await;

    let response = call(&state, &set_password_token(), PASSWORD).await;

    assert_eq!(response.status(), StatusCode::OK);
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    let users = runtime.users().list_users(runtime.id()).await.unwrap();
    assert_eq!(users.len(), 1, "the admin is materialized exactly once");
    assert_eq!(users[0].email, "ada@example.com");
    assert!(
        users[0].password_hash.is_some(),
        "the admin now holds a password hash"
    );
    assert!(
        !users[0].must_change_password,
        "a dashboard-managed password never forces a change"
    );
}

// ---------------------------------------------------------------------------
// Domain separation — the security crux, tested both directions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_sso_login_token_cannot_set_a_password() {
    // A token signed with the RAW secret (what `/sso/redeem` accepts) must not
    // authorize a password change here — otherwise an intercepted login token
    // becomes an account-takeover vector.
    let home = home();
    let state = enabled_state(home.path()).await;

    let sso_login_token = sign(
        SSO_SECRET.as_bytes(),
        "acme",
        ADMIN,
        "jti-sso",
        far_future(),
    );
    let response = call(&state, &sso_login_token, PASSWORD).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !admin_has_password(&state).await,
        "a refused token sets no password"
    );
}

#[tokio::test]
async fn a_set_password_token_is_refused_by_the_sso_route() {
    // The other direction: a set-password token (signed with the derived key)
    // must not be redeemable for a session at `/sso/redeem`.
    let home = home();
    let state = enabled_state(home.path()).await;

    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/companies/acme/sso/redeem")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "token": set_password_token() }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Rejections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_bad_signature_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;

    // Derived from the wrong secret, so the signature does not verify.
    let forged = sign(
        &derive_key("the-wrong-secret"),
        "acme",
        ADMIN,
        "jti-1",
        far_future(),
    );
    let response = call(&state, &forged, PASSWORD).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!admin_has_password(&state).await);
}

#[tokio::test]
async fn an_expired_token_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;

    let expired = sign(
        &derive_key(SSO_SECRET),
        "acme",
        ADMIN,
        "jti-1",
        1_000_000_000,
    );
    let response = call(&state, &expired, PASSWORD).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_token_for_another_company_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;

    let other = sign(
        &derive_key(SSO_SECRET),
        "not-acme",
        ADMIN,
        "jti-1",
        far_future(),
    );
    let response = call(&state, &other, PASSWORD).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!admin_has_password(&state).await);
}

#[tokio::test]
async fn a_non_admin_subject_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;

    let stranger = sign(
        &derive_key(SSO_SECRET),
        "acme",
        "intruder@evil.test",
        "jti-1",
        far_future(),
    );
    let response = call(&state, &stranger, PASSWORD).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!admin_has_password(&state).await);
}

#[tokio::test]
async fn a_replayed_token_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;

    assert_eq!(
        call(&state, &set_password_token(), PASSWORD).await.status(),
        StatusCode::OK
    );
    // Same jti, a different password — single use refuses the replay so it cannot
    // overwrite the credential the first call set.
    let response = call(&state, &set_password_token(), "another valid passphrase").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_endpoint_is_disabled_without_a_secret() {
    let home = home();
    let state = state_from_id(
        home.path(),
        manifest(),
        AppConfig::default(),
        CompanyId::new("acme"),
    )
    .await;

    let response = call(&state, &set_password_token(), PASSWORD).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_password_below_the_minimum_length_is_rejected() {
    let home = home();
    let state = enabled_state(home.path()).await;

    // A valid, unexpired, single-use token — only the password is bad, so the
    // refusal is `password::validate`'s, surfaced as a 400 rather than a 401.
    let response = call(&state, &set_password_token(), "short").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
