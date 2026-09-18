use crate::company::CompanyManifest;
use crate::runtime::RuntimeBuilder;
use crate::server::graphql::auth::GqlAuth;
use crate::server::platform_auth::PlatformClaims;
use axum::http::StatusCode;
use std::collections::HashSet;
use std::sync::Arc;
use tower::ServiceExt;

use super::provision_test_support_1::*;

#[tokio::test]
async fn a_member_may_not_resume_the_company() {
    let home_dir = home();
    let (app, cookie) = company_with_session(home_dir.path(), crate::ports::UserRole::Member).await;

    app.clone()
        .oneshot(post_req(
            "/api/v1/companies/acme/pause",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();

    let denied = app
        .clone()
        .oneshot(post_req_as("/api/v1/companies/acme/resume", &cookie))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let status = app
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(json_body(status).await["lifecycle"], "paused");
}

/// The same gap on the kill switch. The confirmation phrase is a step-up
/// against a stray click, never a role check — a member knows it as well as an
/// admin does, and `emergency-resume`'s stronger confirmation is the company's
/// own id, which a member necessarily knows.
#[tokio::test]
async fn a_member_may_not_engage_the_emergency_stop() {
    let home_dir = home();
    let (app, cookie) = company_with_session(home_dir.path(), crate::ports::UserRole::Member).await;

    let denied = app
        .clone()
        .oneshot(json_post_req_as(
            "/api/v1/companies/acme/emergency-pause",
            &cookie,
            serde_json::json!({ "confirm": super::PAUSE_CONFIRMATION }),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let status = app
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(json_body(status).await["emergency_paused"], false);
}

#[tokio::test]
async fn a_member_may_not_release_the_emergency_stop() {
    let home_dir = home();
    let (app, cookie) = company_with_session(home_dir.path(), crate::ports::UserRole::Member).await;

    app.clone()
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-pause",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": super::PAUSE_CONFIRMATION }),
        ))
        .await
        .unwrap();

    let denied = app
        .clone()
        .oneshot(json_post_req_as(
            "/api/v1/companies/acme/emergency-resume",
            &cookie,
            serde_json::json!({ "confirm": "acme" }),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    // A refused release must leave the stop engaged.
    let status = app
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(json_body(status).await["emergency_paused"], true);
}

/// The other half of the guard: it must refuse a member without also refusing
/// the admin the routes exist for.
#[tokio::test]
async fn an_admin_may_still_pause_and_resume() {
    let home_dir = home();
    let (app, cookie) = company_with_session(home_dir.path(), crate::ports::UserRole::Admin).await;

    let paused = app
        .clone()
        .oneshot(post_req_as("/api/v1/companies/acme/pause", &cookie))
        .await
        .unwrap();
    assert_eq!(paused.status(), StatusCode::OK);
    assert_eq!(json_body(paused).await["lifecycle"], "paused");

    let resumed = app
        .oneshot(post_req_as("/api/v1/companies/acme/resume", &cookie))
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    assert_eq!(json_body(resumed).await["lifecycle"], "running");
}

#[tokio::test]
async fn an_admin_may_still_work_the_emergency_stop() {
    let home_dir = home();
    let (app, cookie) = company_with_session(home_dir.path(), crate::ports::UserRole::Admin).await;

    let stopped = app
        .clone()
        .oneshot(json_post_req_as(
            "/api/v1/companies/acme/emergency-pause",
            &cookie,
            serde_json::json!({ "confirm": super::PAUSE_CONFIRMATION }),
        ))
        .await
        .unwrap();
    assert_eq!(stopped.status(), StatusCode::OK);
    assert_eq!(json_body(stopped).await["emergency_paused"], true);

    let released = app
        .oneshot(json_post_req_as(
            "/api/v1/companies/acme/emergency-resume",
            &cookie,
            serde_json::json!({ "confirm": "acme" }),
        ))
        .await
        .unwrap();
    assert_eq!(released.status(), StatusCode::OK);
    assert_eq!(json_body(released).await["emergency_paused"], false);
}

/// No credential at all is `401`, not `403` — the guard must not turn an
/// anonymous request into a role decision.
#[tokio::test]
async fn an_unauthenticated_caller_cannot_reach_any_lifecycle_route() {
    let home_dir = home();
    let (app, _cookie) = company_with_session(home_dir.path(), crate::ports::UserRole::Admin).await;

    for uri in [
        "/api/v1/companies/acme/pause",
        "/api/v1/companies/acme/resume",
        "/api/v1/companies/acme/emergency-pause",
        "/api/v1/companies/acme/emergency-resume",
    ] {
        let denied = app.clone().oneshot(post_req(uri, None)).await.unwrap();
        assert_eq!(
            denied.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} answered an anonymous caller with {}",
            denied.status()
        );
    }
}

/// The narrower platform rule the admin guard must not swallow: a company's own
/// admin still cannot lift a platform-forced suspension.
#[tokio::test]
async fn an_admin_may_not_resume_a_platform_suspended_company() {
    let home_dir = home();
    let (app, cookie) = company_with_session(home_dir.path(), crate::ports::UserRole::Admin).await;

    let suspended = app
        .clone()
        .oneshot(post_req(
            "/api/v1/companies/acme/suspend",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(suspended.status(), StatusCode::OK);

    let denied = app
        .clone()
        .oneshot(post_req_as("/api/v1/companies/acme/resume", &cookie))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let status = app
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(json_body(status).await["lifecycle"], "suspended");
}

/// The runtime handle `AdminScopedCompany` hands `pause`/`resume` must be the
/// one that actually moves — not whatever `CompanyRegistry::get` returns for
/// the same id at the moment the handler body runs. `CompanyRegistry::insert`
/// replaces an occupied slot unconditionally ("a rebuild swap is the
/// production case"), so a second, independent lookup by id can return a
/// runtime nobody authorized this request against.
///
/// A live request race through the router can't be driven deterministically —
/// nothing suspends a request mid-flight between the extractor resolving
/// `admin.runtime` and the handler body running. This pins the invariant the
/// same way `eviction_preserves_a_runtime_that_replaced_the_one_confirmed_archived`
/// does above: construct the swap directly, and prove the function lifecycle
/// routes now call acts only on the runtime it was handed and never reaches
/// back into the registry for one sharing its id.
#[tokio::test]
async fn transition_runtime_ignores_a_registry_entry_that_replaced_it() {
    let id = CompanyId::new("acme");
    // Each runtime needs its own store that outlives the build call — unlike
    // `evict_test_runtime`, whose caller never round-trips through the store
    // and so never notices its `home` directory going away with it.
    let authorized_home = home();
    let manifest: CompanyManifest = toml::from_str(ACME_TOML).unwrap();
    let authorized = Arc::new(
        RuntimeBuilder::new(authorized_home.path().to_path_buf(), manifest.clone())
            .with_id(id.clone())
            .build()
            .await
            .expect("runtime builds"),
    );
    let swapped_in_home = home();
    let swapped_in = Arc::new(
        RuntimeBuilder::new(swapped_in_home.path().to_path_buf(), manifest)
            .with_id(id.clone())
            .build()
            .await
            .expect("runtime builds"),
    );
    assert!(
        !Arc::ptr_eq(&authorized, &swapped_in),
        "sanity: these must be two distinct runtime instances"
    );

    // As if a rebuild landed a fresh runtime under the same id after
    // `AdminScopedCompany` resolved and authorized `authorized`.
    let registry = crate::runtime::CompanyRegistry::new();
    registry.insert(id.clone(), swapped_in.clone());

    let auth = GqlAuth::Platform(PlatformClaims {
        tenant: "tenant:acme".to_string(),
        scopes: HashSet::new(),
        companies: None,
    });
    let response = super::transition_runtime(&authorized, &auth, "paused").await;
    assert_eq!(response.status(), StatusCode::OK);

    let authorized_status = authorized.status().await.expect("status reads");
    assert_eq!(
        authorized_status.lifecycle, "paused",
        "the runtime the caller was actually authorized against must be the one that moved"
    );

    let swapped_in_status = swapped_in.status().await.expect("status reads");
    assert_eq!(
        swapped_in_status.lifecycle, "running",
        "the runtime that merely shares the id — and replaced `authorized` in the registry \
         after authorization — must be left alone: a lifecycle route may not silently act on \
         whatever a registry lookup happens to return"
    );
}
