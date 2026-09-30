//! End-to-end tests for `POST …/sso/redeem`.
//!
//! Driven through the whole router — the redeem handler verifies the token,
//! claims-or-logs-in the admin, and mints a session — because the value of this
//! surface is the *composition*: a valid token yields a session and a claimed
//! admin, and every way a token can be wrong yields one flat refusal.

#![cfg(feature = "platform-jwt")]

use crate::company::CompanyManifest;
use crate::ports::CompanyStore;
use crate::ports::types::{CompanyId, CompanyRecord, SecretValue};
use crate::ports::users::UserStatus;
use crate::runtime::RuntimeBuilder;
use crate::server::ops::ConnectionsRuntime;
use crate::server::router;
use crate::server::users::cookie::{SESSION_CARRIER_HEADER, SESSION_HEADER};
use crate::{AppConfig, AppState};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

const SSO_SECRET: &str = "sso-signing-secret";
const ADMIN: &str = "Ada@Example.com";

fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("oc-sso-")
        .tempdir()
        .expect("tempdir")
}

/// A manifest whose `[users] admins` bootstraps `ada` — spelled with capitals so
/// normalization is exercised end to end.
fn manifest() -> CompanyManifest {
    toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [users]\nadmins = [\"Ada@Example.com\"]\n",
    )
    .unwrap()
}

/// State over a manifest and config, with the company registered as `acme`.
async fn state_from(
    home: &std::path::Path,
    manifest: CompanyManifest,
    config: AppConfig,
) -> AppState {
    state_from_id(home, manifest, config, CompanyId::new("acme")).await
}

/// Like [`state_from`] but registers the company under an explicit `id` — used to
/// exercise shared-single-DB tenant namespacing, where `runtime.id()` is
/// `<tenant>--acme` while a platform token still carries the bare slug.
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

/// State with the SSO secret configured (the enabled path).
async fn enabled_state(home: &std::path::Path) -> AppState {
    state_from(
        home,
        manifest(),
        AppConfig {
            sso_secret: Some(SecretValue(SSO_SECRET.to_string())),
            ..AppConfig::default()
        },
    )
    .await
}

/// Signs a JWT with `secret` over the given claims.
fn sign(secret: &str, claims: &serde_json::Value) -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign")
}

/// A well-formed token for `acme`/`ada`, signed with the configured secret and
/// expiring far in the future.
fn valid_token() -> String {
    token_with(SSO_SECRET, "acme", ADMIN, "jti-1", far_future())
}

fn token_with(secret: &str, slug: &str, sub: &str, jti: &str, exp: u64) -> String {
    sign(
        secret,
        &serde_json::json!({
            "sub": sub,
            "slug": slug,
            "jti": jti,
            "iat": 1_700_000_000u64,
            "exp": exp,
        }),
    )
}

fn far_future() -> u64 {
    // ~2050, comfortably beyond any test run.
    2_524_608_000
}

fn post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn post_wanting_header_carrier(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header(SESSION_CARRIER_HEADER, "header")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn redeem(state: &AppState, token: &str) -> axum::response::Response {
    router(state.clone())
        .oneshot(post(
            "/api/v1/companies/acme/sso/redeem",
            serde_json::json!({ "token": token }),
        ))
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// Success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_valid_token_signs_the_admin_in_and_sets_a_cookie() {
    // Arrange
    let home = home();
    let state = enabled_state(home.path()).await;

    // Act
    let response = redeem(&state, &valid_token()).await;

    // Assert
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().get("set-cookie").is_some(),
        "a same-origin redemption must set a session cookie"
    );
    let json = body_json(response).await;
    assert_eq!(json["email"], "ada@example.com");
    assert_eq!(json["role"], "admin");
    assert_eq!(json["company"], "acme");
}

#[tokio::test]
async fn a_valid_token_claims_the_admin_on_first_use() {
    // The company starts with no users; a first redemption materializes the
    // standing admin as a passwordless account — the SSO half of the claim.
    let home = home();
    let state = enabled_state(home.path()).await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    assert!(
        runtime
            .users()
            .list_users(runtime.id())
            .await
            .unwrap()
            .is_empty(),
        "a fresh company must start with no users"
    );

    let response = redeem(&state, &valid_token()).await;
    assert_eq!(response.status(), StatusCode::OK);

    let users = runtime.users().list_users(runtime.id()).await.unwrap();
    assert_eq!(users.len(), 1, "the admin must be claimed exactly once");
    assert_eq!(users[0].email, "ada@example.com");
    assert!(
        users[0].password_hash.is_none(),
        "an SSO-claimed admin holds no password — the dashboard manages that"
    );
}

#[tokio::test]
async fn a_second_valid_token_logs_the_same_admin_in_without_a_second_account() {
    let home = home();
    let state = enabled_state(home.path()).await;

    // First redemption claims.
    assert_eq!(
        redeem(&state, &valid_token()).await.status(),
        StatusCode::OK
    );
    // A *different* jti (single use is per-token) for the same subject logs in.
    let second = token_with(SSO_SECRET, "acme", ADMIN, "jti-2", far_future());
    assert_eq!(redeem(&state, &second).await.status(), StatusCode::OK);

    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    let users = runtime.users().list_users(runtime.id()).await.unwrap();
    assert_eq!(
        users.len(),
        1,
        "logging in again must not mint a second account"
    );
}

#[tokio::test]
async fn the_header_carrier_returns_a_session_in_the_body() {
    // A cross-origin console asks for the header carrier and must receive the
    // ready-made session value rather than a cookie.
    let home = home();
    let state = enabled_state(home.path()).await;
    let response = router(state.clone())
        .oneshot(post_wanting_header_carrier(
            "/api/v1/companies/acme/sso/redeem",
            serde_json::json!({ "token": valid_token() }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().get("set-cookie").is_none(),
        "the header carrier must not also set a cookie"
    );
    let json = body_json(response).await;
    let session = json["session"].as_str().expect("a header-carrier session");
    assert!(
        session.starts_with("acme."),
        "the session value carries its company: {session}"
    );
    // And it is a usable session the header form resolves.
    let me = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/companies/acme/auth/me")
                .header(SESSION_HEADER, session)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Rejections — every one is the same flat 401
// ---------------------------------------------------------------------------

async fn assert_rejected(state: &AppState, token: &str, why: &str) {
    let response = redeem(state, token).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{why}");
    let json = body_json(response).await;
    assert_eq!(json["code"], "invalid_sso_token", "{why}");
}

#[tokio::test]
async fn an_expired_token_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;
    // 2001-09-09 — in the past whenever this runs.
    let expired = token_with(SSO_SECRET, "acme", ADMIN, "jti-1", 1_000_000_000);
    assert_rejected(&state, &expired, "an expired token").await;
}

#[tokio::test]
async fn a_bad_signature_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;
    let forged = token_with("the-wrong-secret", "acme", ADMIN, "jti-1", far_future());
    assert_rejected(&state, &forged, "a token signed with the wrong secret").await;
}

#[tokio::test]
async fn a_token_for_a_different_company_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;
    let other = token_with(SSO_SECRET, "not-acme", ADMIN, "jti-1", far_future());
    assert_rejected(&state, &other, "a token whose slug is a different company").await;
}

#[tokio::test]
async fn a_token_for_a_non_admin_subject_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;
    let stranger = token_with(
        SSO_SECRET,
        "acme",
        "stranger@example.com",
        "jti-1",
        far_future(),
    );
    assert_rejected(&state, &stranger, "a signed token naming a non-admin").await;

    // And no account was created for the stranger.
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    assert!(
        runtime
            .users()
            .list_users(runtime.id())
            .await
            .unwrap()
            .is_empty(),
        "a refused subject must not be materialized"
    );
}

#[tokio::test]
async fn a_replayed_jti_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;

    // First use of this exact token succeeds.
    assert_eq!(
        redeem(&state, &valid_token()).await.status(),
        StatusCode::OK
    );
    // Replaying the same token — same jti — is refused even though it is still
    // otherwise valid.
    assert_rejected(&state, &valid_token(), "a replay of a consumed jti").await;
}

#[tokio::test]
async fn a_malformed_token_is_refused() {
    let home = home();
    let state = enabled_state(home.path()).await;
    assert_rejected(&state, "not-a-jwt", "a token that is not a JWT at all").await;
}

// ---------------------------------------------------------------------------
// Off unless configured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_endpoint_is_disabled_without_a_secret() {
    // No `sso_secret` in the config: the route must answer 404, indistinguishable
    // from not existing — even for a token that would otherwise be valid.
    let home = home();
    let state = state_from(home.path(), manifest(), AppConfig::default()).await;
    let response = redeem(&state, &valid_token()).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["code"], "not_found");
}

#[tokio::test]
async fn a_blank_secret_disables_the_endpoint() {
    // A whitespace-only injected value reads as unset, so it cannot become an
    // accepted signing key — the same rule the platform credentials use.
    let home = home();
    let state = state_from(
        home.path(),
        manifest(),
        AppConfig {
            sso_secret: Some(SecretValue("   ".to_string())),
            ..AppConfig::default()
        },
    )
    .await;
    assert_eq!(
        redeem(&state, &valid_token()).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_suspended_admin_is_refused_even_with_a_valid_token() {
    // A first redemption claims ada as an active admin; suspending her must then
    // refuse a later valid, unexpired token rather than resurrecting the account —
    // the same status gate every other login honors before minting a session.
    let home = home();
    let state = enabled_state(home.path()).await;
    assert_eq!(
        redeem(&state, &valid_token()).await.status(),
        StatusCode::OK
    );

    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    let mut user = runtime
        .users()
        .list_users(runtime.id())
        .await
        .unwrap()
        .remove(0);
    user.status = UserStatus::Suspended;
    runtime
        .users()
        .upsert_user(runtime.id(), &user)
        .await
        .unwrap();

    // A fresh jti (single use is per-token) so only the status gate can refuse it.
    let again = token_with(SSO_SECRET, "acme", ADMIN, "jti-2", far_future());
    assert_rejected(&state, &again, "a suspended admin").await;
}

#[tokio::test]
async fn a_bare_slug_token_signs_in_under_tenant_namespacing() {
    // Shared-single-DB mode: `runtime.id()` is `<tenant>--acme`, but the platform
    // mints the token with the bare slug `acme`. The slug is namespaced the same
    // way before the scope check, so a bare-slug token is still accepted here.
    let home = home();
    let config = AppConfig {
        sso_secret: Some(SecretValue(SSO_SECRET.to_string())),
        tenant_namespace: Some("acmecorp".to_string()),
        ..AppConfig::default()
    };
    let id = config.namespaced_company_id(CompanyId::new("acme"));
    assert_ne!(
        id.as_ref(),
        "acme",
        "namespacing must actually prefix the id for this test to mean anything"
    );
    let state = state_from_id(home.path(), manifest(), config, id.clone()).await;

    // `valid_token()` carries the bare slug `acme`; the URL addresses the
    // namespaced id, as a hosted console would.
    let response = router(state.clone())
        .oneshot(post(
            &format!("/api/v1/companies/{}/sso/redeem", id.as_ref()),
            serde_json::json!({ "token": valid_token() }),
        ))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a bare-slug token must sign in where runtime.id() is tenant-namespaced"
    );
}
