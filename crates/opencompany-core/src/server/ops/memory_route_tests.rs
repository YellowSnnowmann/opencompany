//! The Brain page's routes over the company's OpenHuman memory.
//!
//! Under the `openhuman` feature the test binary's process-wide runtime binds
//! TinyMemory's in-memory reference engine (`harness::openhuman_runtime`), so
//! these drive real memory. Each test registers its own company id, because
//! that engine is shared by the whole binary and companies stay apart only by
//! root — which is also what the cross-company test proves.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::company::CompanyManifest;
use crate::ports::types::{CompanyEvent, CompanyId, CompressedTrace, EventSeq};
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use crate::{AppConfig, AppState};

/// A fresh company id, so no other test's memory is in view.
fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", crate::ports::generate_id())
}

/// A state serving `ids`, each a registered company with a seeded admin.
async fn state_over(home: &std::path::Path, ids: &[&str]) -> AppState {
    let state = AppState::new(AppConfig::default());
    for id in ids {
        let manifest: CompanyManifest =
            toml::from_str("[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n").unwrap();
        let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest)
            .with_id(CompanyId::new(*id))
            .build()
            .await
            .unwrap();
        state.registry().insert(CompanyId::new(*id), Arc::new(runtime));
        crate::server::test_support::seed_fixed_admin(&state, id).await;
    }
    state
}

async fn call(
    state: &AppState,
    company: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1/companies/{company}{path}"))
        .header("cookie", crate::server::test_support::fixed_cookie(company));
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn the_traces_route_serializes_the_retained_window_camelcase() {
    let home = tempfile::tempdir().unwrap();
    let id = unique("traces");
    let state = state_over(home.path(), &[&id]).await;
    let runtime = state.registry().get(&CompanyId::new(&id)).unwrap();
    runtime
        .traces()
        .save_trace(
            &CompanyId::new(&id),
            CompressedTrace {
                cycle_id: "cyc-1".into(),
                summary: "one cycle".into(),
                at_millis: 7,
            },
        )
        .await
        .unwrap();
    let (status, body) = call(&state, &id, "GET", "/memory/traces", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body.as_array().expect("an array");
    let last = rows.last().expect("the saved trace");
    assert_eq!(last["cycleId"], "cyc-1");
    assert_eq!(last["atMillis"], 7);
}

#[tokio::test]
async fn a_bad_kind_is_a_bad_request() {
    let home = tempfile::tempdir().unwrap();
    let id = unique("kind");
    let state = state_over(home.path(), &[&id]).await;
    let (status, _) = call(&state, &id, "GET", "/memory?kind=fact", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[cfg(not(feature = "openhuman"))]
#[tokio::test]
async fn without_the_runtime_status_reports_off_and_reads_refuse() {
    let home = tempfile::tempdir().unwrap();
    let id = unique("off");
    let state = state_over(home.path(), &[&id]).await;
    let (status, body) = call(&state, &id, "GET", "/memory/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["on"], false);
    assert_eq!(body["root"], format!("team:{id}"));
    let (status, _) = call(&state, &id, "GET", "/memory", None).await;
    assert!(status.is_client_error() || status.is_server_error(), "{status}");
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_operator_learning_is_listed_searched_and_forgotten() {
    let home = tempfile::tempdir().unwrap();
    let id = unique("learn");
    let state = state_over(home.path(), &[&id]).await;

    let (status, body) = call(&state, &id, "GET", "/memory/status", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["on"], true, "{body}");
    assert_eq!(body["root"], format!("team:{id}"));

    let (status, _) = call(&state, &id, "POST", "/memory", Some(json!({ "text": "  " }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "blank text is refused");

    let (status, created) = call(
        &state,
        &id,
        "POST",
        "/memory",
        Some(json!({ "text": "Invoices go out on the first", "kind": "procedure" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["kind"], "learning");
    assert_eq!(created["namespace"], format!("team:{id}"));
    assert_eq!(created["editable"], true);
    let item_id = created["id"].as_str().unwrap().to_string();

    let (status, page) = call(&state, &id, "GET", "/memory?kind=learning", None).await;
    assert_eq!(status, StatusCode::OK);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], item_id.as_str());
    assert!(items[0]["tags"].as_array().unwrap().contains(&json!("operator")));

    let (status, found) = call(&state, &id, "GET", "/memory?query=invoices", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(found.get("nextCursor").is_none());

    let (status, _) = call(&state, &id, "DELETE", &format!("/memory/{item_id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&state, &id, "DELETE", &format!("/memory/{item_id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a second forget finds nothing");
    let (_, page) = call(&state, &id, "GET", "/memory", None).await;
    assert!(page["items"].as_array().unwrap().is_empty());

    // The forget is journaled for the audit trail.
    let runtime = state.registry().get(&CompanyId::new(&id)).unwrap();
    let journaled = runtime
        .events()
        .read_from(&CompanyId::new(&id), EventSeq::new(0), usize::MAX)
        .await
        .unwrap()
        .into_iter()
        .any(|stored| {
            matches!(&stored.event, CompanyEvent::MemoryFactDeleted { fact_id } if *fact_id == item_id)
        });
    assert!(journaled);
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn one_company_cannot_read_or_forget_anothers_memory() {
    let home = tempfile::tempdir().unwrap();
    let acme = unique("acme");
    let globex = unique("globex");
    let state = state_over(home.path(), &[&acme, &globex]).await;

    let (_, created) = call(
        &state,
        &acme,
        "POST",
        "/memory",
        Some(json!({ "text": "Acme's secret sauce" })),
    )
    .await;
    let item_id = created["id"].as_str().unwrap().to_string();

    let (_, page) = call(&state, &globex, "GET", "/memory", None).await;
    assert!(page["items"].as_array().unwrap().is_empty());
    let (status, _) = call(&state, &globex, "DELETE", &format!("/memory/{item_id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, page) = call(&state, &acme, "GET", "/memory", None).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "acme keeps it");
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn the_agents_brain_and_recall_routes_answer() {
    let home = tempfile::tempdir().unwrap();
    let id = unique("views");
    let state = state_over(home.path(), &[&id]).await;

    let (status, agents) = call(&state, &id, "GET", "/memory/agents", None).await;
    assert_eq!(status, StatusCode::OK, "{agents}");
    assert_eq!(agents["root"], format!("team:{id}"));
    assert!(agents["agents"].as_array().unwrap().is_empty());

    let (status, forgotten) = call(&state, &id, "DELETE", "/memory/agents/ceo", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(forgotten["forgotten"], 0);

    let (status, brain) = call(&state, &id, "GET", "/memory/brain", None).await;
    assert_eq!(status, StatusCode::OK, "{brain}");
    assert!(brain["sources"].as_array().unwrap().is_empty());

    let (status, _) = call(&state, &id, "POST", "/memory/recall", Some(json!({ "question": " " }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
