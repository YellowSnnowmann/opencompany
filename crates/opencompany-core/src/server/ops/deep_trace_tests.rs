use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::company::CompanyManifest;
use crate::ports::deep_trace::{RunStepDetailRecord, TurnStepDetail};
use crate::ports::types::CompanyId;
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use crate::{AppConfig, AppState};

const MANIFEST: &str = "[company]\nname = \"Provisional Co\"\n\
     [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n";

fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("oc-deep-trace-")
        .tempdir()
        .expect("tempdir")
}

async fn state(home: &std::path::Path) -> AppState {
    let manifest: CompanyManifest = toml::from_str(MANIFEST).unwrap();
    let id = CompanyId::new("acme");
    let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id.clone())
        .build()
        .await
        .unwrap();
    let state = AppState::new(AppConfig::default());
    state.registry().insert(id, std::sync::Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, "acme").await;
    state
}

async fn delete_request(state: &AppState, path: &str) -> StatusCode {
    let request = Request::builder()
        .method("DELETE")
        .uri(path)
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .body(Body::empty())
        .unwrap();
    router(state.clone())
        .oneshot(request)
        .await
        .unwrap()
        .status()
}

/// The store layer already proves `purge_deep_trace` is idempotent
/// (`store/conformance.rs`: purging a run twice returns `1` then `0`,
/// never an error). What that leaves open is whether the real HTTP
/// handler — auth extraction, path parsing, the route itself — actually
/// reaches that behaviour, or whether something in the handler (an
/// unwrap, a not-found mapped to an error status) turns a second purge
/// into something other than a quiet no-op. Drives `DELETE
/// …/deep-trace/{run_id}` through the real router twice.
#[tokio::test]
async fn purge_run_through_the_route_is_idempotent() {
    let dir = home();
    let state = state(dir.path()).await;
    let id = CompanyId::new("acme");
    let runtime = state.registry().get(&id).unwrap();

    runtime
        .deep_trace()
        .append_step_detail(
            &id,
            &RunStepDetailRecord {
                run_id: "r1".to_string(),
                step_seq: 0,
                at_millis: 1,
                detail: TurnStepDetail {
                    reasoning: Some("why".to_string()),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        runtime
            .deep_trace()
            .list_step_details(&id, "r1")
            .await
            .unwrap()
            .len(),
        1,
        "fixture did not seed the record the test purges"
    );

    let first = delete_request(&state, "/api/v1/company/deep-trace/r1").await;
    assert_eq!(
        first,
        StatusCode::NO_CONTENT,
        "first purge destroys the row"
    );
    assert!(
        runtime
            .deep_trace()
            .list_step_details(&id, "r1")
            .await
            .unwrap()
            .is_empty(),
        "the route did not actually purge the record"
    );

    let second = delete_request(&state, "/api/v1/company/deep-trace/r1").await;
    assert_eq!(
        second,
        StatusCode::NO_CONTENT,
        "purging an already-purged run through the route must stay a quiet \
         no-op, not surface an error status"
    );
}

/// Same shape for the whole-company purge: through the real route, twice
/// in a row, on a company with nothing left to destroy the second time.
#[tokio::test]
async fn purge_all_through_the_route_is_idempotent() {
    let dir = home();
    let state = state(dir.path()).await;
    let id = CompanyId::new("acme");
    let runtime = state.registry().get(&id).unwrap();

    for (run_id, step_seq) in [("r1", 0u32), ("r2", 0u32)] {
        runtime
            .deep_trace()
            .append_step_detail(
                &id,
                &RunStepDetailRecord {
                    run_id: run_id.to_string(),
                    step_seq,
                    at_millis: 1,
                    detail: TurnStepDetail {
                        output: Some("out".to_string()),
                        ..Default::default()
                    },
                },
            )
            .await
            .unwrap();
    }

    let first = delete_request(&state, "/api/v1/company/deep-trace").await;
    assert_eq!(first, StatusCode::NO_CONTENT);
    assert!(
        runtime
            .deep_trace()
            .list_step_details(&id, "r1")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        runtime
            .deep_trace()
            .list_step_details(&id, "r2")
            .await
            .unwrap()
            .is_empty()
    );

    let second = delete_request(&state, "/api/v1/company/deep-trace").await;
    assert_eq!(
        second,
        StatusCode::NO_CONTENT,
        "purging an already-empty deep-trace store through the route must \
         stay a quiet no-op"
    );
}

/// The extractor guards this surface: a plain member (not an admin) must
/// not reach the purge at all. Pins `AdminScopedCompany` actually being
/// wired on this route rather than the ordinary member-scope extractor.
#[tokio::test]
async fn a_member_may_not_purge_deep_trace() {
    let dir = home();
    let state = state(dir.path()).await;
    crate::server::test_support::seed_fixed_member(&state, "acme").await;

    let request = Request::builder()
        .method("DELETE")
        .uri("/api/v1/company/deep-trace")
        .header("cookie", crate::server::test_support::member_cookie("acme"))
        .body(Body::empty())
        .unwrap();
    let status = router(state.clone())
        .oneshot(request)
        .await
        .unwrap()
        .status();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a member cookie must not be able to purge unredacted deep-trace bodies"
    );
}
