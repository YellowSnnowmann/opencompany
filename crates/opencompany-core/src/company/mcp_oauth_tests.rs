use serde_json::json;

use super::fixture;
use super::*;

const NOW_PLUS_HOUR: u64 = 3600;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn oauth(token_endpoint: &str, expires_at: u64, refresh: Option<&str>) -> AuthMaterial {
    AuthMaterial::OAuth {
        access_token: "at-stale".into(),
        refresh_token: refresh.map(str::to_string),
        client_id: "cid".into(),
        client_secret: Some("cs-secret".into()),
        token_endpoint: token_endpoint.into(),
        expires_at,
    }
}

fn unguarded() -> OAuthFlow {
    named_flow()
}

#[test]
fn callback_uri_appends_route_and_trims_trailing_slash() {
    assert_eq!(
        callback_redirect_uri("http://127.0.0.1:8080"),
        "http://127.0.0.1:8080/oauth/mcp/callback"
    );
    assert_eq!(
        callback_redirect_uri("https://acme.example/"),
        "https://acme.example/oauth/mcp/callback"
    );
}

#[test]
fn a_server_id_names_its_company_and_server() {
    let company = CompanyId::new("tenant--acme");
    let id = server_id(&company, "notion");
    assert_eq!(split_server_id(&id), Some((company, "notion".to_string())));
    assert_eq!(split_server_id("no-separator"), None);
    assert_eq!(split_server_id("acme/"), None);
}

#[test]
fn a_slash_in_either_half_survives_the_round_trip() {
    for (company, server) in [
        ("acme", "acme/docs"),
        ("tenant/acme", "docs"),
        ("tenant/acme", "a/b/c"),
        ("100%/x", "docs"),
        ("a%2Fb", "docs"),
    ] {
        let company = CompanyId::new(company);
        let id = server_id(&company, server);
        assert_eq!(
            split_server_id(&id),
            Some((company, server.to_string())),
            "{id}"
        );
    }
}

#[test]
fn oauth_material_secret_values_cover_every_token() {
    let material = AuthMaterial::OAuth {
        access_token: "at-secret".into(),
        refresh_token: Some("rt-secret".into()),
        client_id: "cid".into(),
        client_secret: Some("cs-secret".into()),
        token_endpoint: "https://as/token".into(),
        expires_at: 0,
    };
    let secrets = material.secret_values();
    assert!(secrets.contains(&"at-secret".to_string()));
    assert!(secrets.contains(&"rt-secret".to_string()));
    assert!(secrets.contains(&"cs-secret".to_string()));
    assert!(!secrets.contains(&"cid".to_string()));
    assert!(material.is_configured());
}

/// A token stored before the flow moved to tinymcp reads back unchanged
/// through the credential map the flow sees.
#[test]
fn stored_oauth_material_round_trips_through_the_flows_credentials() {
    let material = oauth("https://as.example/token", 1_700_000_000, Some("rt-1"));
    let credentials = credentials_of(&material);
    assert_eq!(credentials[ACCESS_TOKEN_KEY], "Bearer at-stale");
    let bundle: OAuthBundle = serde_json::from_str(&credentials[OAUTH_BUNDLE_KEY]).unwrap();
    assert_eq!(bundle.client_secret.as_deref(), Some("cs-secret"));
    assert_eq!(bundle.expires_at, 1_700_000_000);
    assert_eq!(material_of(&credentials), Some(material));
}

#[test]
fn a_non_oauth_credential_is_invisible_to_the_flow() {
    assert!(credentials_of(&AuthMaterial::Bearer("t".into())).is_empty());
    assert!(credentials_of(&AuthMaterial::None).is_empty());
    assert_eq!(material_of(&BTreeMap::new()), None);
}

#[tokio::test]
async fn refresh_mints_a_new_token_and_keeps_an_unrotated_refresh_token() {
    let server = fixture::spawn(
        true,
        json!({ "access_token": "at-fresh", "expires_in": 600 }),
    )
    .await;
    let material = oauth(&format!("{}/token", server.base), 0, Some("rt-1"));
    let refreshed = refresh_with(&unguarded(), &material)
        .await
        .expect("an expired token is refreshed");
    let AuthMaterial::OAuth {
        access_token,
        refresh_token,
        client_id,
        client_secret,
        expires_at,
        ..
    } = refreshed
    else {
        panic!("refresh returns OAuth material");
    };
    assert_eq!(access_token, "at-fresh");
    assert_eq!(refresh_token.as_deref(), Some("rt-1"));
    assert_eq!(client_id, "cid");
    assert_eq!(client_secret.as_deref(), Some("cs-secret"));
    assert!(expires_at > now());
    let forms = server.seen.token_forms.lock().unwrap().clone();
    assert_eq!(forms.len(), 1);
    assert!(forms[0].contains(&("grant_type".into(), "refresh_token".into())));
    assert!(forms[0].contains(&("refresh_token".into(), "rt-1".into())));
}

#[tokio::test]
async fn the_guarded_refresh_refuses_a_loopback_token_endpoint_and_keeps_the_token() {
    let server = fixture::spawn(true, json!({ "access_token": "at-fresh" })).await;
    let material = oauth(&format!("{}/token", server.base), 0, Some("rt-1"));
    assert_eq!(refresh(&material).await, None);
    assert!(server.seen.token_forms.lock().unwrap().is_empty());
}

#[tokio::test]
async fn refresh_leaves_a_fresh_token_or_one_without_a_refresh_token_alone() {
    let server = fixture::spawn(true, json!({ "access_token": "at-fresh" })).await;
    let token = format!("{}/token", server.base);
    let flow = unguarded();
    assert_eq!(
        refresh_with(&flow, &oauth(&token, now() + NOW_PLUS_HOUR, Some("rt-1"))).await,
        None
    );
    assert_eq!(refresh_with(&flow, &oauth(&token, 0, None)).await, None);
    assert_eq!(refresh_with(&flow, &oauth(&token, 0, Some(""))).await, None);
    assert_eq!(
        refresh_with(&flow, &AuthMaterial::Bearer("t".into())).await,
        None
    );
    assert!(server.seen.token_forms.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_full_sign_in_returns_oauth_material_and_consumes_the_state() {
    let server = fixture::spawn(
        true,
        json!({ "access_token": "at-new", "refresh_token": "rt-new", "expires_in": 900 }),
    )
    .await;
    let flow = unguarded();
    let company = CompanyId::new("acme");
    let redirect = "https://acme.example/oauth/mcp/callback";
    let url = begin(&flow, &server.mcp, &company, "notion", redirect)
        .await
        .expect("sign-in begins");
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains("client_id=cid-1"));
    let state = fixture::state_of(&url);
    assert_eq!(
        flow.pending_server(&state)
            .as_deref()
            .and_then(split_server_id),
        Some((company, "notion".to_string()))
    );

    let material = complete(&flow, &state, "code-1").await.expect("exchange");
    assert_eq!(
        material,
        AuthMaterial::OAuth {
            access_token: "at-new".into(),
            refresh_token: Some("rt-new".into()),
            client_id: "cid-1".into(),
            client_secret: Some("cs-1".into()),
            token_endpoint: format!("{}/token", server.base),
            expires_at: match &material {
                AuthMaterial::OAuth { expires_at, .. } => *expires_at,
                _ => 0,
            },
        }
    );
    let forms = server.seen.token_forms.lock().unwrap().clone();
    assert!(forms[0].contains(&("code".into(), "code-1".into())));
    assert!(forms[0].contains(&("redirect_uri".into(), redirect.into())));

    assert_eq!(flow.pending_server(&state), None);
    assert!(complete(&flow, &state, "code-1").await.is_err());
}

#[tokio::test]
async fn registration_names_the_client_opencompany() {
    let server = fixture::spawn(true, json!({ "access_token": "at" })).await;
    let flow = unguarded();
    begin(
        &flow,
        &server.mcp,
        &CompanyId::new("acme"),
        "notion",
        "https://acme.example/oauth/mcp/callback",
    )
    .await
    .expect("sign-in begins");

    let bodies = server.seen.registrations.lock().unwrap().clone();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["client_name"], json!("OpenCompany"));
    assert_eq!(console_flow().client_name(), "OpenCompany");
}

#[tokio::test]
async fn the_console_flow_refuses_a_loopback_authorization_server() {
    let server = fixture::spawn(true, json!({})).await;
    let error = begin(
        &console_flow(),
        &server.mcp,
        &CompanyId::new("acme"),
        "notion",
        "https://acme.example/oauth/mcp/callback",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, OpenCompanyError::InvalidRequest(_)),
        "{error:?}"
    );
    assert!(server.seen.registrations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_server_without_dynamic_registration_asks_for_a_static_token() {
    let server = fixture::spawn(false, json!({})).await;
    let error = begin(
        &unguarded(),
        &server.mcp,
        &CompanyId::new("acme"),
        "slack",
        "https://acme.example/oauth/mcp/callback",
    )
    .await
    .unwrap_err();
    let OpenCompanyError::InvalidRequest(message) = error else {
        panic!("a clean operator error: {error:?}");
    };
    assert!(message.contains("paste a static API token"), "{message}");
}

#[tokio::test]
async fn a_transient_metadata_failure_is_not_reported_as_missing_registration() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let challenge =
        format!("Bearer resource_metadata=\"http://{addr}/.well-known/oauth-protected-resource\"");
    let app = axum::Router::new()
        .route(
            "/mcp",
            axum::routing::post(move || {
                let challenge = challenge.clone();
                async move {
                    (
                        axum::http::StatusCode::UNAUTHORIZED,
                        [("www-authenticate", challenge)],
                        "",
                    )
                }
            }),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            axum::routing::get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let error = begin(
        &unguarded(),
        &format!("http://{addr}/mcp"),
        &CompanyId::new("acme"),
        "slack",
        "https://acme.example/oauth/mcp/callback",
    )
    .await
    .unwrap_err();
    assert!(matches!(error, OpenCompanyError::Harness(_)), "{error:?}");
}

#[tokio::test]
async fn a_server_that_needs_no_sign_in_says_so() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/mcp",
        axum::routing::post(|| async {
            axum::Json(json!({ "jsonrpc": "2.0", "id": 1, "result": {} }))
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let error = begin(
        &unguarded(),
        &format!("http://{addr}/mcp"),
        &CompanyId::new("acme"),
        "open",
        "https://acme.example/oauth/mcp/callback",
    )
    .await
    .unwrap_err();
    let OpenCompanyError::InvalidRequest(message) = error else {
        panic!("a clean operator error: {error:?}");
    };
    assert!(
        message.contains("does not require authorization"),
        "{message}"
    );
}

#[tokio::test]
async fn completing_an_unknown_state_is_an_invalid_request() {
    let error = complete(&unguarded(), "never-parked", "code")
        .await
        .unwrap_err();
    assert!(
        matches!(error, OpenCompanyError::InvalidRequest(_)),
        "{error:?}"
    );
}

#[tokio::test]
async fn console_oauth_support_reads_dynamic_registration() {
    let with = fixture::spawn(true, json!({})).await;
    let without = fixture::spawn(false, json!({})).await;
    assert!(supports_console_oauth(&with.mcp).await);
    assert!(!supports_console_oauth(&without.mcp).await);
    assert!(supports_console_oauth("http://127.0.0.1:1/mcp").await);
}

const TINYMCP_FLOW: &str = include_str!(
    "../../../../vendor/openhuman/vendor/tinymcp/crates/tinymcp/src/registry/oauth/flow.rs"
);
const TINYMCP_GUARD: &str = include_str!(
    "../../../../vendor/openhuman/vendor/tinymcp/crates/tinymcp/src/registry/oauth/endpoint_guard.rs"
);
const TINYMCP_HTTP: &str = include_str!(
    "../../../../vendor/openhuman/vendor/tinymcp/crates/tinymcp/src/transport/http/mod.rs"
);

#[test]
fn the_error_prose_this_module_matches_is_still_tinymcps() {
    for (source, prose) in [
        (TINYMCP_FLOW, NO_AUTH_REQUIRED),
        (TINYMCP_GUARD, ENDPOINT_REFUSED),
        (TINYMCP_GUARD, ENDPOINT_HAS_NO_HOST),
        (TINYMCP_GUARD, "invalid {what} url"),
        (TINYMCP_HTTP, METADATA_FETCH_FAILED),
        (TINYMCP_FLOW, UNKNOWN_STATE),
    ] {
        assert!(source.contains(prose), "tinymcp no longer says `{prose}`");
    }
    assert!("invalid {what} url".starts_with(INVALID_URL));
}
