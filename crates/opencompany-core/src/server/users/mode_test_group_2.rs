use crate::app::config::AuthMode;
use crate::company::CompanyManifest;
use crate::ports::types::CompanyId;
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::mode_test_support_1::*;

/// `none` mode's local-owner resolution is peer-gated everywhere `MaybePeer`
/// reaches a handler — `CompanyAuth`, the GraphQL handler, and every REST route
/// that resolves a principal through `current_user` or `chat_actor` — as a
/// second, independent check alongside the bind-time refusal, see
/// `crate::server::graphql::auth::local_owner`. A loopback peer, or no peer at
/// all (an embedded caller or a router exercised directly, as this test itself
/// does for everything else), still resolves the owner; a non-loopback peer
/// does not, even though the same company would otherwise admit it.
#[tokio::test]
async fn none_mode_local_owner_resolution_refuses_a_non_loopback_peer() {
    let dir = home();
    let state = state_in_mode(dir.path(), AuthMode::None, None).await;
    let app = router(state);

    // No peer info at all — an embedded caller, or this very test harness for
    // any route it does not explicitly wire below. Still resolves: this is not
    // itself a refusal, only a positive non-loopback finding is.
    let no_peer = app
        .clone()
        .oneshot(get("/api/v1/company/feedback"))
        .await
        .unwrap();
    assert_eq!(no_peer.status(), StatusCode::OK);

    // A loopback peer resolves the owner.
    let mut loopback_req = get("/api/v1/company/feedback");
    loopback_req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:54321".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let loopback = app.clone().oneshot(loopback_req).await.unwrap();
    assert_eq!(loopback.status(), StatusCode::OK);

    // A non-loopback peer does not, even for the same company and route.
    let mut remote_req = get("/api/v1/company/feedback");
    remote_req.extensions_mut().insert(ConnectInfo(
        "203.0.113.7:54321".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let remote = app.oneshot(remote_req).await.unwrap();
    assert_eq!(
        remote.status(),
        StatusCode::UNAUTHORIZED,
        "a non-loopback peer must not resolve the none-mode local owner"
    );
}

/// The peer gate applies to `current_user`'s REST call sites too, not just
/// `CompanyAuth`/GraphQL — `/auth/me` is the simplest of them.
#[tokio::test]
async fn none_mode_auth_me_refuses_a_non_loopback_peer() {
    let dir = home();
    let state = state_in_mode(dir.path(), AuthMode::None, None).await;
    let app = router(state);

    let mut req = get("/api/v1/company/auth/me");
    req.extensions_mut().insert(ConnectInfo(
        "203.0.113.7:1".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a non-loopback peer must not resolve the none-mode local owner through /auth/me either"
    );
}

/// A valid platform bearer is not a way past the peer/header gates on a
/// `none`-mode company: falling through to it once `local_owner` refuses
/// would make the gates decorative, since a bearer is just another credential
/// a remote caller could hold. The identical bearer must keep working
/// normally on an `email`-mode company — the refusal is specific to `none`
/// mode's local-only contract, not a blanket rule against platform auth.
#[tokio::test]
async fn none_mode_refuses_a_platform_bearer_from_a_non_loopback_peer() {
    use crate::server::platform_auth::{
        PlatformAuthConfig, PlatformClaims, UnsignedTenantVerifier,
    };

    let secret = "top-secret";
    let verifier = std::sync::Arc::new(UnsignedTenantVerifier::new(secret));
    let token = UnsignedTenantVerifier::tenant_token(&PlatformClaims {
        tenant: "tenant:acme".to_string(),
        scopes: std::collections::HashSet::from(["platform".to_string()]),
        companies: None,
    });
    let remote = ConnectInfo("203.0.113.7:1".parse::<std::net::SocketAddr>().unwrap());
    let auth_header = format!("Bearer {token}");

    let dir = home();
    let none_state = state_in_mode(dir.path(), AuthMode::None, None)
        .await
        .with_platform_auth(PlatformAuthConfig::new(verifier.clone()));
    let mut none_req = get("/api/v1/company/feedback");
    none_req.extensions_mut().insert(remote);
    none_req
        .headers_mut()
        .insert("authorization", auth_header.parse().unwrap());
    let none_response = router(none_state).oneshot(none_req).await.unwrap();
    assert_eq!(
        none_response.status(),
        StatusCode::UNAUTHORIZED,
        "a platform bearer must not stand in for the local owner on a none-mode company"
    );

    // The same bearer, unchanged, still works on an ordinary email-mode
    // company — this isn't a blanket refusal of platform auth.
    let email_state = state_in_mode(dir.path(), AuthMode::Email, None)
        .await
        .with_platform_auth(PlatformAuthConfig::new(verifier));
    let mut email_req = get("/api/v1/company/feedback");
    email_req.extensions_mut().insert(remote);
    email_req
        .headers_mut()
        .insert("authorization", auth_header.parse().unwrap());
    let email_response = router(email_state).oneshot(email_req).await.unwrap();
    assert_eq!(email_response.status(), StatusCode::OK);
}

/// A same-host reverse proxy connects to a loopback-bound listener over
/// loopback too, so the peer this process sees always reads as loopback
/// regardless of where the proxy's own caller actually was — the peer check
/// alone cannot see an undeclared proxy. A `Forwarded`/`X-Forwarded-*` header
/// is the signal that one is there, and `local_owner` must refuse on it even
/// when the peer itself looks perfectly local.
#[tokio::test]
async fn none_mode_local_owner_resolution_refuses_a_forwarded_request_even_from_a_loopback_peer() {
    let dir = home();
    let state = state_in_mode(dir.path(), AuthMode::None, None).await;
    let app = router(state);

    let mut req = get("/api/v1/company/feedback");
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:54321".parse::<std::net::SocketAddr>().unwrap(),
    ));
    req.headers_mut()
        .insert("x-forwarded-for", "203.0.113.7".parse().unwrap());
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a proxy-forwarding header must refuse the none-mode local owner even from a loopback peer"
    );
}

/// A session minted while a company was `email` mode must not still
/// authenticate after the company is rebuilt into `none` mode — nothing purges
/// a company's session store on a manifest edit, so without this the peer and
/// forwarding-header gates on `none` mode's implicit owner would be moot: a
/// caller who already held (or stole) an old session could fall through to it
/// instead.
#[tokio::test]
async fn a_session_from_before_a_mode_flip_does_not_survive_it() {
    let dir = home();
    let state = state_in_mode(dir.path(), AuthMode::Email, Some("ada@example.com")).await;
    let app = router(state.clone());

    let requested = body_json(
        app.clone()
            .oneshot(post(
                "/api/v1/company/auth/request",
                serde_json::json!({"email": "ada@example.com"}),
            ))
            .await
            .unwrap(),
    )
    .await;
    let code = requested["dev_code"]
        .as_str()
        .expect("a loopback host with no mail transport echoes the code");
    let verify = app
        .oneshot(post(
            "/api/v1/company/auth/verify",
            serde_json::json!({"code": code}),
        ))
        .await
        .unwrap();
    assert_eq!(verify.status(), StatusCode::OK);
    let cookie = session_cookie(&verify);

    // The manifest edit + rebuild a mode flip is: the same company id, now
    // built with `[users].mode = "none"`. The session store is untouched.
    let none_mode = state_in_mode(dir.path(), AuthMode::None, None).await;
    state
        .registry()
        .insert(CompanyId::new("acme"), none_mode.registry().sole().unwrap());

    // A non-loopback peer refuses the implicit local owner, so this exercises
    // the fallback path — and the old session must not be what it falls
    // through to.
    let mut req = get("/api/v1/company/feedback");
    req.extensions_mut().insert(ConnectInfo(
        "203.0.113.9:1".parse::<std::net::SocketAddr>().unwrap(),
    ));
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let response = router(state).oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "a session minted before the mode flip must not authenticate a none-mode company"
    );
}

/// The owner is one durable record, not a principal invented per request —
/// chat attribution and the task board key off the user id.
#[tokio::test]
async fn the_local_owner_is_the_same_person_on_every_request() {
    let dir = home();
    let state = state_in_mode(dir.path(), AuthMode::None, None).await;
    let app = router(state);

    let first = body_json(
        app.clone()
            .oneshot(get("/api/v1/company/auth/me"))
            .await
            .unwrap(),
    )
    .await;
    let second = body_json(app.oneshot(get("/api/v1/company/auth/me")).await.unwrap()).await;
    assert_eq!(first["id"], second["id"]);
    assert!(first["id"].as_str().is_some_and(|id| !id.is_empty()));
}

/// The owner of a company with no sign-in is still a person with a name and a
/// face — and on the desktop they are the *only* person, so if the profile route
/// did not serve `none` mode it would not serve the case it matters most in.
#[tokio::test]
async fn the_local_owner_can_name_themselves_and_pick_a_face() {
    let dir = home();
    let state = state_in_mode(dir.path(), AuthMode::None, None).await;
    let app = router(state);

    let request = Request::builder()
        .method("PATCH")
        .uri("/api/v1/company/auth/me")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({"displayName": "Steven", "avatar": "tiny:clay"}).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let saved = body_json(response).await;
    assert_eq!(saved["displayName"], "Steven", "{saved}");
    assert_eq!(saved["avatar"], "tiny:clay", "{saved}");

    // The same durable owner record, so the choice survives the next request
    // rather than living on a principal invented per call.
    let reread = body_json(app.oneshot(get("/api/v1/company/auth/me")).await.unwrap()).await;
    assert_eq!(reread["id"], saved["id"], "{reread}");
    assert_eq!(reread["avatar"], "tiny:clay", "{reread}");
}

/// `none` cannot add users. An invite would grant an account nobody could ever
/// reach, because there is no sign-in to reach it through.
#[tokio::test]
async fn none_mode_admits_nobody_else() {
    let dir = home();
    let state = state_in_mode(dir.path(), AuthMode::None, None).await;
    let app = router(state);

    let response = app
        .clone()
        .oneshot(post(
            "/api/v1/company/users/invites",
            serde_json::json!({"email": "ada@example.com", "role": "member"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = body_json(response).await;
    assert_eq!(body["code"], "auth_mode");
    assert_eq!(body["mode"], "none");

    // And no login route to reach such an account through, had one existed.
    for request in [
        post(
            "/api/v1/company/auth/request",
            serde_json::json!({"email": "ada@example.com"}),
        ),
        post(
            "/api/v1/company/auth/wallet/challenge",
            serde_json::json!({"address": address(&wallet(9))}),
        ),
        post("/api/v1/company/auth/logout", serde_json::json!({})),
    ] {
        let uri = request.uri().to_string();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT, "{uri}");
    }
}

/// The host's own answer beats the manifest's. A packaged desktop build and a
/// hosting platform both need to guarantee a mode whatever a company says.
#[tokio::test]
async fn the_host_override_beats_the_manifest() {
    let dir = home();
    let manifest: CompanyManifest =
        toml::from_str("[company]\nname = \"Acme\"\n[users]\nmode = \"email\"\n").unwrap();
    let runtime = RuntimeBuilder::new(dir.path().to_path_buf(), manifest)
        .with_id(CompanyId::new("acme"))
        .with_auth_mode_override(Some(AuthMode::None))
        .build()
        .await
        .unwrap();
    assert_eq!(runtime.auth_mode(), AuthMode::None);
}
