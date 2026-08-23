//! Route tests for the company's Smithery directory credential (issue #1287).

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::company::CompanyManifest;
use crate::ports::types::{CompanyId, CompanyRecord};
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use crate::store::FsCompanyStore;
use crate::{AppConfig, AppState};

const PATH: &str = "/api/v1/company/mcp/registry/credential";

/// Long and opaque enough that a leak would be unmistakable in any body.
const KEY: &str = "smithery_directory_SECRET_do_not_echo_me";

const MANIFEST: &str = "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n";

fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("oc-smithery-key-")
        .tempdir()
        .expect("tempdir")
}

async fn state_for(home: &std::path::Path, company: &str) -> AppState {
    use crate::ports::CompanyStore;
    let manifest: CompanyManifest = toml::from_str(MANIFEST).unwrap();
    let store = FsCompanyStore::new(home.to_path_buf());
    let id = CompanyId::new(company);
    store
        .save(&CompanyRecord {
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
            overlay_desk_tools: Default::default(),
            disabled_workflows: Vec::new(),
            template_provenance: None,
            setup: None,
        })
        .await
        .unwrap();
    let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id.clone())
        .build()
        .await
        .unwrap();
    let state = AppState::new(AppConfig::default());
    state.registry().insert(id, std::sync::Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, company).await;
    state
}

async fn send_as(
    state: &AppState,
    method: &str,
    body: Option<Value>,
    cookie: String,
) -> (StatusCode, Value, String) {
    let request = Request::builder()
        .method(method)
        .uri(PATH)
        .header("cookie", cookie);
    let request = match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => request.body(Body::empty()).unwrap(),
    };
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let raw = String::from_utf8_lossy(&bytes).to_string();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value, raw)
}

async fn send(
    state: &AppState,
    company: &str,
    method: &str,
    body: Option<Value>,
) -> (StatusCode, Value, String) {
    send_as(
        state,
        method,
        body,
        crate::server::test_support::fixed_cookie(company),
    )
    .await
}

/// The core round trip: an admin sets the key, the read plane reports the
/// company tier, and the value never comes back out of any response.
#[tokio::test]
async fn the_key_round_trips_write_only_and_reports_the_company_tier() {
    let home_dir = home();
    let state = state_for(home_dir.path(), "acme").await;

    let (status, dto, raw) = send(&state, "acme", "GET", None).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(dto["configured"], false);
    assert_eq!(dto["source"], "none");
    assert!(
        dto["notice"]
            .as_str()
            .unwrap_or_default()
            .contains("open registry"),
        "the degraded state has to say what the operator still gets: {dto}"
    );

    let (status, resp, raw) = send(&state, "acme", "PUT", Some(json!({ "key": KEY }))).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["status"]["configured"], true);
    assert_eq!(resp["status"]["source"], "company");
    assert!(!raw.contains(KEY), "the write must not echo the key: {raw}");

    let (_, dto, raw) = send(&state, "acme", "GET", None).await;
    assert_eq!(dto["configured"], true);
    assert_eq!(dto["source"], "company");
    assert!(!raw.contains(KEY), "the read must not carry the key: {raw}");
    assert!(dto.get("key").is_none(), "status must never carry the key");
}

/// Clearing withdraws the company's key and says so, rather than leaving a
/// stale `configured: true` behind.
#[tokio::test]
async fn clearing_reports_the_degraded_state() {
    let home_dir = home();
    let state = state_for(home_dir.path(), "acme").await;
    send(&state, "acme", "PUT", Some(json!({ "key": KEY }))).await;

    let (status, resp, raw) = send(&state, "acme", "PUT", Some(json!({ "key": "" }))).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["status"]["configured"], false);
    assert_eq!(resp["status"]["source"], "none");
}

/// The notice must not frighten an operator out of rotating. This key is used
/// to browse and install; an installed server connects with its own stored
/// credentials, so a rotation disconnects nothing — and the copy has to say so,
/// because the opposite is the natural assumption.
#[tokio::test]
async fn the_notice_says_a_rotation_does_not_disconnect_installed_servers() {
    let home_dir = home();
    let state = state_for(home_dir.path(), "acme").await;
    send(&state, "acme", "PUT", Some(json!({ "key": KEY }))).await;

    let (_, dto, _) = send(&state, "acme", "GET", None).await;
    let notice = dto["notice"].as_str().unwrap_or_default();
    assert!(
        notice.contains("does not disconnect"),
        "an operator must be told a rotation is safe: {notice}"
    );
}

/// Admin-only, for the reason every other credential write is: this decides
/// which Smithery account the company browses and bills through.
#[tokio::test]
async fn a_member_cannot_set_the_directory_credential() {
    let home_dir = home();
    let state = state_for(home_dir.path(), "acme").await;
    let member =
        crate::server::test_support::seed_session(&state, "acme", crate::ports::UserRole::Member)
            .await;

    let (status, body, raw) =
        send_as(&state, "PUT", Some(json!({ "key": KEY })), member.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{raw}");
    assert!(
        body["error"].as_str().unwrap_or_default().contains("admin"),
        "the refusal has to say why: {body}"
    );

    // The refusal is real, not merely a different status code.
    let (_, dto, _) = send(&state, "acme", "GET", None).await;
    assert_eq!(
        dto["configured"], false,
        "a refused write must not have stored anything: {dto}"
    );

    // …but a member may still READ it. Knowing the directory is thin because no
    // key is set is what stops them filing "search is broken".
    let (status, dto, raw) = send_as(&state, "GET", None, member).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(dto["source"], "none");
}

/// Both writes are journaled with an actor, and a clear is told apart from a
/// set — the two have opposite blast radii.
#[tokio::test]
async fn setting_and_clearing_are_journaled_with_an_actor() {
    use crate::ports::types::CompanyEvent;

    let home_dir = home();
    let state = state_for(home_dir.path(), "audited").await;
    send(&state, "audited", "PUT", Some(json!({ "key": KEY }))).await;
    send(&state, "audited", "PUT", Some(json!({ "key": "" }))).await;

    let id = CompanyId::new("audited");
    let runtime = state.registry().get(&id).expect("registered");
    let events = runtime
        .events()
        .read_from(&id, crate::ports::types::EventSeq::new(0), 200)
        .await
        .expect("events");
    let changes: Vec<(String, bool)> = events
        .iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::ToolAccessChanged { change, by, .. } => {
                Some((change.clone(), by.is_some()))
            }
            _ => None,
        })
        .collect();
    assert!(
        changes.contains(&("smithery_key_set".to_string(), true)),
        "a set must be journaled with who did it: {changes:?}"
    );
    assert!(
        changes.contains(&("smithery_key_cleared".to_string(), true)),
        "a clear must be told apart from a set: {changes:?}"
    );
    // Every credential route appends `ToolAccessChanged` to one log. Borrowing a
    // sibling's vocabulary would make an auditor unable to tell which credential
    // moved.
    assert!(
        !changes
            .iter()
            .any(|(change, _)| change.starts_with("company_key_")
                || change == "credential_set"
                || change == "credential_cleared"),
        "no other credential was written here, so no other audit word may appear: {changes:?}"
    );
}

/// The whole point of the tier split: a company that set nothing on a host that
/// has a key is a *working* directory, and the copy must say it is shared
/// rather than claim the company owns it.
#[tokio::test]
async fn the_shared_host_tier_is_reported_as_shared() {
    use crate::app::config::MapEnv;
    use crate::company::smithery::{API_KEY_ENV, DirectoryKeySource, resolve};

    let home_dir = home();
    let state = state_for(home_dir.path(), "acme").await;
    let id = CompanyId::new("acme");
    let runtime = state.registry().get(&id).expect("registered");

    // Exercised through the resolver rather than the route: setting a real
    // process env var would leak across the test binary's other threads.
    let resolved = resolve(
        runtime.id(),
        runtime.secrets().as_ref(),
        &MapEnv::new([(API_KEY_ENV, "host-wide-key")]),
    )
    .await
    .expect("resolve");
    assert_eq!(resolved.source(), DirectoryKeySource::Environment);
    assert!(
        !crate::company::smithery::key_configured(runtime.id(), runtime.secrets().as_ref())
            .await
            .expect("configured"),
        "the host's key is not this company's own, and the two must not be conflated"
    );
}
