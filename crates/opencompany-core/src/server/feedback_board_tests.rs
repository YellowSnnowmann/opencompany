use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::*;
use crate::company::CompanyManifest;
use crate::feedback::MockTinyHumansClient;
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use crate::{AppConfig, AppState};

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

/// Two board items so filtering, ordering and paging all have something to
/// bite on.
fn seeded_board() -> Vec<BoardItem> {
    vec![
        BoardItem {
            id: "one".to_string(),
            kind: BoardKind::Feature,
            title: "Weekly digest".to_string(),
            body: "Send one on Mondays".to_string(),
            status: BoardStatus::Open,
            author: Some("rin".to_string()),
            upvotes: 3,
            downvotes: 1,
            score: 2,
            comment_count: 0,
            my_vote: VoteValue::None,
            issue_url: None,
            created_at: "2026-01-02T00:00:00.000Z".to_string(),
        },
        BoardItem {
            id: "two".to_string(),
            kind: BoardKind::Bug,
            title: "Totals are wrong".to_string(),
            body: "The invoice doubles tax".to_string(),
            status: BoardStatus::Planned,
            author: None,
            upvotes: 9,
            downvotes: 0,
            score: 9,
            comment_count: 1,
            my_vote: VoteValue::Up,
            issue_url: Some("https://example.test/issues/7".to_string()),
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
        },
    ]
}

/// A single-company host, optionally provisioned with a hub serving `board`.
async fn state_with(home: &std::path::Path, hub: Option<Arc<MockTinyHumansClient>>) -> AppState {
    let id = CompanyId::new("acme");
    let mut builder = RuntimeBuilder::new(home.to_path_buf(), manifest()).with_id(id.clone());
    if let Some(hub) = hub {
        builder = builder.with_tinyhumans_feedback(hub);
    }
    let runtime = builder.build().await.unwrap();
    let state = AppState::new(AppConfig::default());
    state.registry().insert(id, Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, "acme").await;
    state
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("cookie", crate::server::test_support::fixed_cookie("acme"));
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(
                    body.map(|b| Body::from(b.to_string()))
                        .unwrap_or(Body::empty()),
                )
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-board-")
        .tempdir()
        .expect("tempdir")
}

// The board reaches the console filtered, ordered and paged — and a vote or
// a comment travels back to the hub and returns the updated row.
#[tokio::test]
async fn lists_filters_votes_and_comments() {
    let home_dir = home();
    let hub = Arc::new(MockTinyHumansClient::new().with_board(seeded_board()));
    let state = state_with(home_dir.path(), Some(hub)).await;
    let app = router(state);

    // Default board: both rows, highest score first.
    let (status, value) = call(&app, "GET", "/api/v1/company/feedback/board", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["total"], 2);
    assert_eq!(value["items"][0]["id"], "two");
    assert_eq!(value["items"][0]["my_vote"], 1);
    assert_eq!(value["items"][1]["id"], "one");

    // Filtered to features: one row, and the total follows the filter.
    let (_, value) = call(
        &app,
        "GET",
        "/api/v1/company/feedback/board?type=feature&sort=new",
        None,
    )
    .await;
    assert_eq!(value["total"], 1);
    assert_eq!(value["items"][0]["id"], "one");

    // Paging past the end is an empty page, not an error.
    let (status, value) = call(&app, "GET", "/api/v1/company/feedback/board?page=9", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["items"].as_array().unwrap().len(), 0);
    assert_eq!(value["total"], 2);

    // An upvote lands and comes back on the updated row.
    let (status, value) = call(
        &app,
        "POST",
        "/api/v1/company/feedback/board/one/vote",
        Some(r#"{"value":1}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["upvotes"], 4);
    assert_eq!(value["score"], 3);
    assert_eq!(value["my_vote"], 1);

    // Voting again does not double-count: the previous vote is retracted first.
    let (_, value) = call(
        &app,
        "POST",
        "/api/v1/company/feedback/board/one/vote",
        Some(r#"{"value":-1}"#),
    )
    .await;
    assert_eq!(value["upvotes"], 3);
    assert_eq!(value["downvotes"], 2);
    assert_eq!(value["my_vote"], -1);

    // A comment posts and shows up on the item detail with its count bumped.
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/company/feedback/board/one/comments",
        Some(r#"{"body":"yes please"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, value) = call(&app, "GET", "/api/v1/company/feedback/board/one", None).await;
    assert_eq!(value["item"]["comment_count"], 1);
    assert_eq!(value["comments"][0]["body"], "yes please");

    // An empty comment never reaches the hub.
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/company/feedback/board/one/comments",
        Some(r#"{"body":"   "}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // An unknown item is the hub's 404, surfaced as one.
    let (status, _) = call(&app, "GET", "/api/v1/company/feedback/board/nope", None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

// Without a TinyHumans credential there is no board at all — a 404 the
// console reads as "hide the surface", never an empty-looking board.
#[tokio::test]
async fn an_unprovisioned_instance_has_no_board() {
    let home_dir = home();
    let state = state_with(home_dir.path(), None).await;
    let app = router(state);

    let (status, value) = call(&app, "GET", "/api/v1/company/feedback/board", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(value["code"], "tinyhumans_no_board");

    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/company/feedback/board/one/vote",
        Some(r#"{"value":1}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// The per-company form addresses the same board as the single-company alias.
#[tokio::test]
async fn the_company_scoped_route_answers_too() {
    let home_dir = home();
    let hub = Arc::new(MockTinyHumansClient::new().with_board(seeded_board()));
    let state = state_with(home_dir.path(), Some(hub)).await;
    let app = router(state);

    let (status, value) = call(
        &app,
        "GET",
        "/api/v1/companies/acme/feedback/board?status=planned",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["total"], 1);
    assert_eq!(value["items"][0]["id"], "two");
}

#[test]
fn params_default_to_the_hot_board_and_the_console_page_size() {
    let query = BoardParams::default().to_query();
    assert_eq!(query.sort, BoardSort::Hot);
    assert_eq!(query.kind, None);
    assert_eq!(query.status, None);
    assert_eq!(query.page, 1);
    assert_eq!(query.limit, DEFAULT_LIMIT);
}

#[test]
fn params_read_every_filter_the_console_sends() {
    let params = BoardParams {
        sort: Some("new".to_string()),
        kind: Some("bug".to_string()),
        status: Some("planned".to_string()),
        page: Some(3),
        limit: Some(5),
    };
    let query = params.to_query();
    assert_eq!(query.sort, BoardSort::New);
    assert_eq!(query.kind, Some(BoardKind::Bug));
    assert_eq!(query.status, Some(BoardStatus::Planned));
    assert_eq!(query.page, 3);
    assert_eq!(query.limit, 5);
}

#[test]
fn nonsense_filters_are_ignored_rather_than_refused() {
    let params = BoardParams {
        sort: Some("sideways".to_string()),
        kind: Some("wish".to_string()),
        status: Some("someday".to_string()),
        page: Some(0),
        limit: Some(10_000),
    };
    let query = params.to_query();
    // Falls back to the default board rather than 400ing a hand-edited URL.
    assert_eq!(query.sort, BoardSort::Hot);
    assert_eq!(query.kind, None);
    assert_eq!(query.status, None);
    // And the page bounds are corrected into what the hub accepts.
    assert_eq!(query.page, 1);
    assert_eq!(query.limit, crate::feedback::board::MAX_LIMIT);
}

#[test]
fn an_empty_comment_never_reaches_the_hub() {
    assert!(checked_comment("   \n ").is_err());
    assert_eq!(checked_comment("  hi  ").unwrap(), "hi");
}
