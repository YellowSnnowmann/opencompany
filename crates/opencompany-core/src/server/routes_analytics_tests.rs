//! `/spec` serves the analytics status, unauthenticated, and never the client
//! id. Split out of `routes_tests.rs`, which is past the 750-line cap.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use super::*;
use crate::AppConfig;
use crate::analytics::config::{ClientCredentials, Decision};
use crate::analytics::{DeferredTracker, NullTracker};
use crate::app::deployment::Deployment;

/// An obviously-fake client id, searched for in the whole `/spec` body.
const FAKE_CLIENT_ID: &str = "fake-spec-client-id-3b9e";

async fn spec_text(state: AppState) -> String {
    let response = router(state)
        .oneshot(Request::builder().uri("/spec").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn analytics_spec_for_an_unwired_host_says_off_not_wired() {
    let text = spec_text(AppState::new(AppConfig::default())).await;
    let spec: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(spec["analytics"]["decision"], "off");
    assert_eq!(spec["analytics"]["reason"], "not wired");
    assert_eq!(spec["analytics"]["last_send"], "never");
    assert_eq!(spec["analytics"]["accepted"], 0);
}

#[tokio::test]
async fn analytics_spec_carries_the_status_and_never_the_client_id() {
    let handle = std::sync::Arc::new(DeferredTracker::new());
    let decision = Decision::Report {
        endpoint: "https://ops:proxy-secret@collector.example/track?key=q".into(),
        credentials: ClientCredentials::new(FAKE_CLIENT_ID),
    };
    handle.install_with_decision(
        std::sync::Arc::new(NullTracker),
        &decision,
        Deployment::HostedTenant,
    );
    let state = AppState::new(AppConfig::default()).with_analytics(handle);

    let text = spec_text(state).await;
    let spec: serde_json::Value = serde_json::from_str(&text).unwrap();
    let analytics = &spec["analytics"];

    for key in [
        "decision",
        "reason",
        "deployment",
        "endpoint",
        "in_build",
        "consent",
        "last_send",
        "last_status",
        "last_at",
        "accepted",
        "dropped",
    ] {
        assert!(
            analytics.get(key).is_some(),
            "missing analytics.{key}: {text}"
        );
    }
    assert_eq!(analytics["deployment"], "hosted-tenant");
    assert!(
        !text.contains(FAKE_CLIENT_ID),
        "the client id leaked into /spec"
    );
    assert!(
        !text.contains("proxy-secret") && !text.contains("key=q"),
        "the endpoint must be redacted: {text}"
    );
}
