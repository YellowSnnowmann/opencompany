use crate::ports::types::CompanyId;
use crate::ports::{UserRole, UserStatus};
use crate::server::graphql::auth::{GqlAuth, resolve_principal};
use crate::server::router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::auth_test_support_1::*;

#[tokio::test]
async fn a_session_header_cannot_reach_the_platform_write_plane() {
    // THE load-bearing property. `resolve_claims` cannot return a human, so
    // provisioning and suspension are unreachable by any human credential. A
    // new carrier is exactly the kind of change that could quietly route around
    // that, so it is asserted against the real router rather than the resolver.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with(&home, &["acme"]).await;
    let token = seed_session(&state, "acme", UserRole::Admin, UserStatus::Active).await;

    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/companies")
                .header("content-type", "application/toml")
                .header(
                    crate::server::users::cookie::SESSION_HEADER,
                    session_header_value("acme", &token),
                )
                .body(Body::from("[company]\nname = \"Pwned\"\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "an admin's session header must not provision companies"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/companies/acme/suspend")
                .header(
                    crate::server::users::cookie::SESSION_HEADER,
                    session_header_value("acme", &token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "an admin's session header must not suspend a company"
    );
}

#[tokio::test]
async fn a_malformed_session_header_resolves_no_user() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with(&home, &["acme"]).await;
    let token = seed_session(&state, "acme", UserRole::Member, UserStatus::Active).await;
    let acme = CompanyId::new("acme");

    for hostile in [
        // No separator at all: the whole value would otherwise be read as a
        // company with no token, or a token with no company.
        token.clone(),
        "acme".to_string(),
        // Empty on either side of the separator.
        format!(".{token}"),
        "acme.".to_string(),
        ".".to_string(),
        String::new(),
        // A company id that could not name a cookie must not be able to name a
        // header either — otherwise the two carriers disagree about which
        // companies can hold a session at all.
        format!("ac.me.{token}"),
        format!("evil;Path=/.{token}"),
    ] {
        let mut headers = axum::http::HeaderMap::new();
        let Ok(value) = hostile.parse() else {
            continue; // Unrepresentable as a header value; nothing to test.
        };
        headers.insert(crate::server::users::cookie::SESSION_HEADER, value);
        assert!(
            resolve_principal(&headers, &state, Some(&acme), None)
                .await
                .is_err(),
            "{hostile:?} must not authenticate"
        );
    }
}

#[tokio::test]
async fn a_header_naming_another_company_falls_through_to_a_valid_cookie() {
    // Same degrade-rather-than-fail policy the cookie path has: a credential
    // for somewhere else must not brick a request that carried a good one.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with(&home, &["acme", "globex"]).await;
    let acme_token = seed_session(&state, "acme", UserRole::Member, UserStatus::Active).await;

    let acme = CompanyId::new("acme");
    let mut headers = headers_with_session_header("globex", "irrelevant");
    headers.insert(
        axum::http::header::COOKIE,
        cookie_header("acme", &acme_token).parse().unwrap(),
    );

    let auth = resolve_principal(&headers, &state, Some(&acme), None)
        .await
        .unwrap();
    match auth {
        GqlAuth::User(u) => assert_eq!(u.company, acme),
        other => panic!("expected the cookie to still win, got {other:?}"),
    }
}
