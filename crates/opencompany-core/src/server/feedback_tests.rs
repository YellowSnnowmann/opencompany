use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::*;
use crate::company::CompanyManifest;
use crate::feedback::tinyhumans::IngestOutcome;
use crate::feedback::types::ConsentMode;
use crate::feedback::{MockGitHubClient, MockTinyHumansClient};
use crate::ports::SecretStore;
use crate::ports::types::SecretValue;
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use crate::store::FsSecretStore;
use crate::{AppConfig, AppState};

fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-feedback-")
        .tempdir()
        .expect("tempdir")
}

fn manifest() -> CompanyManifest {
    toml::from_str(
        r#"
        [company]
        name = "Acme"
        handle = "acme"
        [[agent]]
        id = "dana_roe"
        role = "Analyst"
        [policy]
        mode = "full"
        "#,
    )
    .unwrap()
}

/// Builds state with a company wired for `auto` consent and a mock GitHub
/// client, plus a seeded secret to exercise the scrub-abort path.
async fn state_with_company(home: &std::path::Path, github: Arc<MockGitHubClient>) -> AppState {
    state_with_clients(home, github, None).await
}

/// As [`state_with_company`], but optionally provisioned with a TinyHumans
/// hub — the "instance has a credential" case.
async fn state_with_clients(
    home: &std::path::Path,
    github: Arc<MockGitHubClient>,
    hub: Option<Arc<MockTinyHumansClient>>,
) -> AppState {
    let id = CompanyId::new("acme");
    // Seed a secret whose value the scrubber must abort on if it appears.
    let secrets = FsSecretStore::new(home.to_path_buf());
    secrets
        .set(&id, "github_token", SecretValue("ghp_LEAKEDSECRET".into()))
        .await
        .unwrap();

    let mut builder = RuntimeBuilder::new(home.to_path_buf(), manifest())
        .with_id(id.clone())
        .with_github(github)
        .with_feedback_consent(ConsentMode::Auto);
    if let Some(hub) = hub {
        builder = builder.with_tinyhumans_feedback(hub);
    }
    let runtime = builder.build().await.unwrap();
    let state = AppState::new(AppConfig::default());
    state.registry().insert(id, Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, "acme").await;
    state
}

async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

async fn post_json(app: &Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                // Every route needs a principal now; sign in as the
                // harness admin so these assert feedback behavior.
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, value)
}

// The offline end-to-end path: capture → scrub (secret aborts, email
// redacted) → preview → mock-file with dedupe.
#[tokio::test]
async fn capture_scrub_preview_file_and_dedupe() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let state = state_with_company(&home, github.clone()).await;
    let app = router(state);

    // 1. A report containing the seeded secret is blocked (scrub fail-closed)
    //    and nothing is filed.
    let (status, value) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"bug","note":"token ghp_LEAKEDSECRET broke the run"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["blocked"], true);
    assert_eq!(value["filed"], false);
    assert!(github.created().is_empty());

    // 2. A clean report with an email, in preview mode, returns the byte-exact
    //    final body with the email redacted and nothing filed.
    let (status, value) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"wrong-output","note":"email dana@acme.co bounced","preview":true}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["filed"], false);
    let preview = value["preview_body"].as_str().expect("preview body");
    assert!(preview.contains("⟨redacted:email⟩"), "got {preview}");
    assert!(!preview.contains("dana@acme.co"));
    // Signed with the company @handle for provenance.
    assert!(preview.contains("— filed by @acme"));
    assert!(github.created().is_empty());
    let preview_item = value["item_id"].as_str().expect("item id");

    // 3. Send confirms the PREVIEWED item by id — it must not capture a
    //    second item and must post the exact previewed bytes. Auto consent
    //    files one issue.
    let confirm_body = serde_json::json!({
        "category": "wrong-output",
        "note": "email dana@acme.co bounced",
        "item_id": preview_item,
    })
    .to_string();
    let (status, value) = post_json(&app, "/api/v1/company/feedback", &confirm_body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["filed"], true);
    assert_eq!(value["item_id"], preview_item);
    assert!(value["issue_url"].is_string());
    let created = github.created();
    assert_eq!(created.len(), 1);
    assert!(created[0].labels.contains(&"source/operator".to_string()));
    assert!(created[0].labels.contains(&"sev/annoyance".to_string()));
    assert!(created[0].labels.contains(&"type/wrong-output".to_string()));
    // The preview and the confirm are one item: the reports list shows the
    // step-1 blocked capture plus exactly one filed report — no duplicate
    // from previewing then sending.
    let (_, list) = get_json(&app, "/api/v1/company/feedback").await;
    let items = list.as_array().expect("an array");
    assert_eq!(items.len(), 2);
    assert_eq!(
        items.iter().filter(|i| i["issue_status"] == "open").count(),
        1,
        "exactly one filed report"
    );

    // 4. A fresh filing with the same title dedupes: it comments, does not
    //    create a duplicate.
    let (status, value) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"wrong-output","note":"email dana@acme.co bounced"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["deduped"], true);
    assert_eq!(github.created().len(), 1);
    assert_eq!(github.comments().len(), 1);
}

// A confirm-by-id of an item that was captured (the built-in feedback tool
// or the chat intent) but never previewed must be refused: its words are
// hidden from the reports list, so sending it would file a body nobody
// inspected.
#[tokio::test]
async fn confirm_of_unpreviewed_capture_is_blocked() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let state = state_with_company(&home, github.clone()).await;

    // Capture the item the way the built-in feedback tool / chat intent
    // does — persisted locally, never previewed, words never surfaced.
    let runtime = lookup(&state, "acme").expect("acme runtime");
    let captured = runtime
        .capture_feedback(crate::feedback::FeedbackInput {
            category: crate::feedback::FeedbackCategory::Bug,
            note: "the run crashed".into(),
            work_ref: None,
            template_name: None,
            template_version: None,
        })
        .await
        .expect("captured");

    let app = router(state);

    let confirm_body = serde_json::json!({
        "category": "bug",
        "note": "the run crashed",
        "item_id": captured.id,
    })
    .to_string();
    let (status, value) = post_json(&app, "/api/v1/company/feedback", &confirm_body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["blocked"], true);
    assert_eq!(value["destination"], "local");
    assert!(
        github.created().is_empty(),
        "an unpreviewed item must not be filed"
    );
    assert!(github.comments().is_empty());
}

// Re-confirming an already-filed item is idempotent: it returns the
// recorded result instead of filing or commenting a second time.
#[tokio::test]
async fn re_confirm_of_filed_item_returns_recorded_result() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let state = state_with_company(&home, github.clone()).await;
    let app = router(state);

    // Preview, then confirm — one issue is filed.
    let (_, preview) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"bug","note":"the run crashed","preview":true}"#,
    )
    .await;
    let item_id = preview["item_id"].as_str().expect("item id").to_string();
    let confirm_body = serde_json::json!({
        "category": "bug",
        "note": "the run crashed",
        "item_id": item_id,
    })
    .to_string();

    let (status, first) = post_json(&app, "/api/v1/company/feedback", &confirm_body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["filed"], true);
    assert_eq!(github.created().len(), 1);

    // Confirm the same item again: the recorded result returns and nothing
    // new is filed or commented.
    let (status, again) = post_json(&app, "/api/v1/company/feedback", &confirm_body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["filed"], true);
    assert_eq!(again["item_id"], item_id);
    assert_eq!(again["issue_url"], first["issue_url"]);
    assert_eq!(again["deduped"], false);
    assert_eq!(github.created().len(), 1, "no second issue");
    assert_eq!(github.comments().len(), 0, "no duplicate comment");
}

// Re-confirming an already-forwarded item returns the recorded result
// instead of ingesting the report into the hub a second time.
#[tokio::test]
async fn re_confirm_of_forwarded_item_does_not_ingest_twice() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let hub = Arc::new(MockTinyHumansClient::new());
    let state = state_with_clients(&home, github, Some(hub.clone())).await;
    let app = router(state);

    let (_, preview) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"bug","note":"the run crashed","preview":true}"#,
    )
    .await;
    let item_id = preview["item_id"].as_str().expect("item id").to_string();
    let confirm_body = serde_json::json!({
        "category": "bug",
        "note": "the run crashed",
        "item_id": item_id,
    })
    .to_string();

    let (_, first) = post_json(&app, "/api/v1/company/feedback", &confirm_body).await;
    assert_eq!(first["filed"], true);
    assert_eq!(first["destination"], "tinyhumans");
    assert_eq!(hub.forwarded().len(), 1);

    let (_, again) = post_json(&app, "/api/v1/company/feedback", &confirm_body).await;
    assert_eq!(again["filed"], true);
    assert_eq!(again["destination"], "tinyhumans");
    assert_eq!(again["item_id"], item_id);
    assert_eq!(hub.forwarded().len(), 1, "no second ingest");
}

// Two confirms of the same item fired concurrently must file only once:
// the per-item confirm lock serialises them, and the loser returns the
// winner's recorded result instead of filing a second issue.
#[tokio::test]
async fn concurrent_confirms_file_once() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let state = state_with_company(&home, github.clone()).await;
    let app = router(state);

    let (_, preview) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"bug","note":"the run crashed","preview":true}"#,
    )
    .await;
    let item_id = preview["item_id"].as_str().expect("item id").to_string();
    let confirm_body = serde_json::json!({
        "category": "bug",
        "note": "the run crashed",
        "item_id": item_id,
    })
    .to_string();

    let (a, b) = tokio::join!(
        post_json(&app, "/api/v1/company/feedback", &confirm_body),
        post_json(&app, "/api/v1/company/feedback", &confirm_body),
    );

    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    // Both report the outcome, but only one issue was filed and no
    // duplicate comment was added.
    assert_eq!(a.1["filed"], true);
    assert_eq!(b.1["filed"], true);
    assert_eq!(github.created().len(), 1, "only one issue filed");
    assert_eq!(github.comments().len(), 0, "no duplicate comment");
}

// A confirm of a nonexistent item id is a 404 and must not mint an entry
// in the process-wide confirm-lock registry: the id is caller-supplied and
// the registry is never evicted, so a bad id must fail before the lock is
// taken rather than growing the server heap forever.
#[tokio::test]
async fn confirm_of_nonexistent_item_does_not_mint_confirm_lock() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let state = state_with_company(&home, github.clone()).await;
    let app = router(state);

    // An id no stored item could ever carry: minted, never persisted.
    let bogus = "no-such-item-confirm-nonexistent".to_string();
    let confirm_body = serde_json::json!({
        "category": "bug",
        "note": "the run crashed",
        "item_id": bogus,
    })
    .to_string();

    let (status, _) = post_json(&app, "/api/v1/company/feedback", &confirm_body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        !crate::feedback::store::confirm_lock_holds(&bogus),
        "a nonexistent item id must not mint a confirm-lock entry"
    );
}

// A provisioned instance forwards to the hub instead of filing, and what
// crosses the boundary is the scrubbed body — not the operator's raw words.
#[tokio::test]
async fn provisioned_instance_forwards_scrubbed_body_and_files_nothing() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let hub = Arc::new(MockTinyHumansClient::new());
    let state = state_with_clients(&home, github.clone(), Some(hub.clone())).await;
    let app = router(state);

    let (status, value) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"wrong-output","note":"email dana@acme.co bounced"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["filed"], true);
    assert_eq!(value["destination"], "tinyhumans");
    // The hub decides whether an issue exists, so we report no URL.
    assert!(value["issue_url"].is_null());

    // Nothing was filed from here.
    assert!(github.created().is_empty());
    assert!(github.comments().is_empty());

    let forwarded = hub.forwarded();
    assert_eq!(forwarded.len(), 1);
    let sent = &forwarded[0];
    assert!(sent.body.contains("⟨redacted:email⟩"), "got {}", sent.body);
    assert!(!sent.body.contains("dana@acme.co"));
    assert!(sent.body.contains("— filed by @acme"));
    assert_eq!(sent.origin, "acme");
    assert_eq!(sent.wire_type(), "bug");
    assert!(!sent.external_ref.is_empty());
}

// The scrub gate runs before the destination choice, so a secret is blocked
// rather than forwarded.
#[tokio::test]
async fn scrub_abort_blocks_before_forwarding() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let hub = Arc::new(MockTinyHumansClient::new());
    let state = state_with_clients(&home, github, Some(hub.clone())).await;
    let app = router(state);

    let (status, value) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"bug","note":"token ghp_LEAKEDSECRET broke the run"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["blocked"], true);
    assert_eq!(value["destination"], "local");
    assert!(
        hub.forwarded().is_empty(),
        "a blocked report must not leave"
    );
}

// An unreachable hub is a degraded success: the note is already stored, so
// the operator gets a reason rather than a failed request.
#[tokio::test]
async fn unreachable_hub_degrades_to_local() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let hub = Arc::new(MockTinyHumansClient::new().with_failure("connection refused"));
    let state = state_with_clients(&home, github.clone(), Some(hub)).await;
    let app = router(state);

    let (status, value) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"bug","note":"the run crashed"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["filed"], false);
    assert_eq!(value["destination"], "local");
    assert!(value["reason"].is_string());
    // It must not silently fall back to filing an issue instead.
    assert!(github.created().is_empty());
}

// Moderation rejection is reported as such, not as a transport failure.
#[tokio::test]
async fn hub_moderation_rejection_is_reported() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let hub = Arc::new(
        MockTinyHumansClient::new().with_outcome(IngestOutcome::Rejected {
            reason: "off-topic".to_string(),
        }),
    );
    let state = state_with_clients(&home, github, Some(hub)).await;
    let app = router(state);

    let (status, value) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"docs","note":"the docs are thin"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["filed"], false);
    assert_eq!(value["destination"], "tinyhumans");
    assert_eq!(value["reason"], "off-topic");
}

// Without a credential the original GitHub path is untouched.
#[tokio::test]
async fn unprovisioned_instance_still_files_to_github() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let state = state_with_company(&home, github.clone()).await;
    let app = router(state);

    let (status, value) = post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"bug","note":"the run crashed"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["filed"], true);
    assert_eq!(value["destination"], "github");
    assert_eq!(github.created().len(), 1);
}

// The reports list shows what was reported and where it went, and never the
// operator's own words.
#[tokio::test]
async fn list_returns_summaries_without_operator_words() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let state = state_with_company(&home, github).await;
    let app = router(state);

    post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"bug","note":"the payroll run crashed","work_ref":"payroll.run"}"#,
    )
    .await;

    let (status, value) = get_json(&app, "/api/v1/company/feedback").await;
    assert_eq!(status, StatusCode::OK);
    let items = value.as_array().expect("an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["category"], "bug");
    assert_eq!(items[0]["work_item"], "payroll.run");
    assert!(items[0]["id"].is_string());
    assert!(items[0]["at_millis"].is_number());

    // The local-only fields must not be in the payload at all.
    let raw = value.to_string();
    assert!(!raw.contains("payroll run crashed"), "got {raw}");
    assert!(!raw.contains("operator_words"), "got {raw}");
    assert!(!raw.contains("context_excerpt"), "got {raw}");
}

// A forwarded report stores the hub's id, which is not a URL — the list must
// not offer it as a link.
#[tokio::test]
async fn list_omits_a_non_url_reference_for_forwarded_reports() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let hub = Arc::new(MockTinyHumansClient::new());
    let state = state_with_clients(&home, github, Some(hub)).await;
    let app = router(state);

    post_json(
        &app,
        "/api/v1/company/feedback",
        r#"{"category":"bug","note":"the payroll run crashed"}"#,
    )
    .await;

    let (_, value) = get_json(&app, "/api/v1/company/feedback").await;
    let items = value.as_array().expect("an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["issue_status"], "forwarded");
    assert!(items[0]["filed_issue_url"].is_null());
}

#[tokio::test]
async fn feedback_for_unknown_company_is_404() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let github = Arc::new(MockGitHubClient::new());
    let state = state_with_company(&home, github).await;
    let app = router(state);

    let (status, _value) = post_json(
        &app,
        "/api/v1/companies/ghost/feedback",
        r#"{"category":"bug","note":"x"}"#,
    )
    .await;
    // 401, not 404: authentication precedes existence, so an unauthenticated
    // caller cannot enumerate which companies this host runs.
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
