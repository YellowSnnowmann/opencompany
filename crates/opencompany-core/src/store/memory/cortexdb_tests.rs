//! Offline tests for the CortexDB adapter, against an in-process axum mock
//! speaking the wire shapes this driver relies on — no real CortexDB
//! instance, no network.

use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tinymemory_api::traits::Memory;
use tinymemory_api::types::{MemoryCategory, RecallOpts};

use super::{CORTEXDB_DRIVER_ID, CortexdbMemory};

/// One event the mock accepted, keyed the same way CortexDB's own scope
/// grammar would file it.
#[derive(Clone)]
struct StoredEvent {
    scope: String,
    id: String,
    content: Value,
    observed_at: String,
    /// A relevance stand-in `recall` reports as `confidence`, distinct from
    /// insertion recency: earlier-stored events score higher by default, so
    /// a test can construct a case where the highest-scored hit is *not* the
    /// most recent one, and assert `recall` keeps it under a tight `limit`
    /// rather than the timestamp-freshest hit.
    score: f64,
}

/// State shared by the mock's handlers.
#[derive(Default)]
pub(super) struct MockState {
    events: Mutex<Vec<StoredEvent>>,
    /// The only (token, actor) pair the mock accepts; anything else is a 401,
    /// exactly like a real CortexDB instance.
    valid_token: String,
    valid_actor: String,
    next_id: Mutex<u64>,
    /// Simulates ranked-recall's own indexing lag, distinct from
    /// `/v1/events` listing: while positive, `/v1/recall` reports no hits at
    /// all (as if the write had not indexed into ranked recall yet) and
    /// decrements by one per call, regardless of query or scope.
    recall_lag_calls: Mutex<u32>,
}

pub(super) const TOKEN: &str = "test-token";
pub(super) const ACTOR: &str = "opencompany-test";

/// Whether the request carries the one accepted credential pair.
fn authorized(headers: &HeaderMap, state: &MockState) -> bool {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let actor = headers.get("x-cortex-actor").and_then(|v| v.to_str().ok());
    bearer == Some(state.valid_token.as_str()) && actor == Some(state.valid_actor.as_str())
}

async fn ready(headers: HeaderMap, State(state): State<Arc<MockState>>) -> StatusCode {
    if authorized(&headers, &state) {
        StatusCode::OK
    } else {
        StatusCode::UNAUTHORIZED
    }
}

async fn experience(
    headers: HeaderMap,
    State(state): State<Arc<MockState>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers, &state) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        );
    }
    let scope = body
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let content = body.get("content").cloned().unwrap_or(Value::Null);
    let observed_at = body
        .pointer("/context/observed_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut id_guard = state.next_id.lock().unwrap();
    *id_guard += 1;
    let id = format!("evt_{}", *id_guard);
    drop(id_guard);
    // A relevance stand-in independent of insertion order or recency: a
    // marker in the stored text, not when it was written. This lets a test
    // construct a hit that is simultaneously older (loses on recency) and
    // more relevant (should win on score).
    let score = if content
        .pointer("/data/content")
        .and_then(Value::as_str)
        .is_some_and(|text| text.contains("HIGH_RELEVANCE_MARKER"))
    {
        1.0
    } else {
        0.1
    };
    state.events.lock().unwrap().push(StoredEvent {
        scope,
        id: id.clone(),
        content,
        observed_at,
        score,
    });
    (
        StatusCode::OK,
        Json(json!({ "event_id": id, "duplicate": false })),
    )
}

async fn recall(
    headers: HeaderMap,
    State(state): State<Arc<MockState>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers, &state) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        );
    }
    {
        let mut lag = state.recall_lag_calls.lock().unwrap();
        if *lag > 0 {
            *lag -= 1;
            return (StatusCode::OK, Json(json!({ "layers": { "events": [] } })));
        }
    }
    let scope = body
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let events = state.events.lock().unwrap();
    let items: Vec<Value> = events
        .iter()
        .filter(|event| event.scope == scope)
        // A rough stand-in for CortexDB's BM25 ranking: an empty query
        // matches everything (as the module docs describe), a non-empty one
        // only matches events whose stored content contains it. This is
        // enough to reproduce the real failure mode this driver has to
        // correct for — a stale event that still matches `query` even though
        // a newer, non-matching event has since superseded it.
        .filter(|event| {
            query.is_empty()
                || event
                    .content
                    .pointer("/data/content")
                    .and_then(Value::as_str)
                    .is_some_and(|text| {
                        text.to_ascii_lowercase()
                            .contains(&query.to_ascii_lowercase())
                    })
        })
        .map(|event| {
            json!({
                "id": event.id,
                "content": event.content,
                "observed_at": event.observed_at,
                "confidence": event.score,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({ "layers": { "events": items } })),
    )
}

/// `GET /v1/events?scope=...&limit=...&cursor=...` — the paginated raw
/// listing this driver's exhaustive reads (`get`/`list`/`forget`/
/// `namespace_summaries`/recall's stale-hit correction) walk instead of
/// `/v1/recall`, which cannot page. The mock pages by a plain numeric offset
/// carried as the cursor — opaque to the driver, which only round-trips it.
async fn events_list(
    headers: HeaderMap,
    State(state): State<Arc<MockState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers, &state) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        );
    }
    let scope = params.get("scope").cloned().unwrap_or_default();
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let offset: usize = params
        .get("cursor")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let events = state.events.lock().unwrap();
    let matching: Vec<&StoredEvent> = events.iter().filter(|event| event.scope == scope).collect();
    let page: Vec<Value> = matching
        .iter()
        .skip(offset)
        .take(limit)
        .map(|event| {
            json!({
                "id": event.id,
                "content": event.content,
                "observed_at": event.observed_at,
            })
        })
        .collect();
    let next_offset = offset + page.len();
    let has_more = next_offset < matching.len();
    (
        StatusCode::OK,
        Json(json!({
            "items": page,
            "has_more": has_more,
            "next_cursor": if has_more { Some(next_offset.to_string()) } else { None },
        })),
    )
}

/// `GET /v1/scopes/list` — every scope this mock has ever accepted a write
/// under, unpaginated (the mock never holds enough scopes across a test to
/// need a second page).
async fn scopes_list(
    headers: HeaderMap,
    State(state): State<Arc<MockState>>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers, &state) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        );
    }
    let events = state.events.lock().unwrap();
    let mut scopes: Vec<String> = events.iter().map(|event| event.scope.clone()).collect();
    scopes.sort();
    scopes.dedup();
    let items: Vec<Value> = scopes
        .into_iter()
        .map(|scope| json!({ "path": scope }))
        .collect();
    (StatusCode::OK, Json(json!({ "items": items })))
}

async fn forget(
    headers: HeaderMap,
    State(state): State<Arc<MockState>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !authorized(&headers, &state) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        );
    }
    let ids: Vec<String> = body
        .pointer("/selector/memory_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let mut events = state.events.lock().unwrap();
    let before = events.len();
    events.retain(|event| !ids.contains(&event.id));
    let removed = before - events.len();
    (
        StatusCode::OK,
        Json(json!({ "deleted": { "events": removed }, "matched": removed })),
    )
}

/// Serves the mock on loopback and returns its base URL plus the shared state.
pub(super) async fn spawn_mock(valid_actor: &str) -> (String, Arc<MockState>) {
    let state = Arc::new(MockState {
        events: Mutex::new(Vec::new()),
        valid_token: TOKEN.to_string(),
        valid_actor: valid_actor.to_string(),
        next_id: Mutex::new(0),
        recall_lag_calls: Mutex::new(0),
    });
    let app = Router::new()
        .route("/v1/admin/ready", get(ready))
        .route("/v1/experience", post(experience))
        .route("/v1/recall", post(recall))
        .route("/v1/forget", post(forget))
        .route("/v1/events", get(events_list))
        .route("/v1/scopes/list", get(scopes_list))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), state)
}

pub(super) fn client(base_url: &str, actor: &str) -> CortexdbMemory {
    CortexdbMemory::new(base_url, TOKEN, actor).expect("valid config")
}

#[tokio::test]
async fn name_is_the_driver_id() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    assert_eq!(client(&base_url, ACTOR).name(), CORTEXDB_DRIVER_ID);
}

#[tokio::test]
async fn store_then_recall_round_trips() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    memory
        .store(
            "company-a",
            "greeting",
            "hello there",
            MemoryCategory::Core,
            None,
        )
        .await
        .expect("store succeeds");

    let hits = memory
        .recall(
            "hello",
            10,
            RecallOpts {
                namespace: Some("company-a"),
                ..Default::default()
            },
        )
        .await
        .expect("recall succeeds");

    assert_eq!(
        hits.len(),
        1,
        "expected exactly one recalled entry: {hits:?}"
    );
    assert_eq!(hits[0].key, "greeting");
    assert_eq!(hits[0].content, "hello there");
    assert_eq!(hits[0].category, MemoryCategory::Core);

    let fetched = memory
        .get("company-a", "greeting")
        .await
        .expect("get succeeds")
        .expect("entry exists");
    assert_eq!(fetched.content, "hello there");
}

/// Regression: `POST /v1/experience?wait=captured` only acknowledges
/// durability, and listing visibility (`GET /v1/events`) is a separate,
/// earlier indexing stage than ranked-recall visibility (`POST /v1/recall`)
/// against a live CortexDB instance. `store` must not return until both have
/// cleared — a caller that immediately calls `recall` after a successful
/// `store` must see its own write, not an empty or stale result.
#[tokio::test]
async fn store_waits_for_ranked_recall_visibility_not_only_listing() {
    let (base_url, state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    // The event is listable immediately (the mock's `/v1/events` has no lag
    // of its own), but `/v1/recall` reports nothing for the next two calls —
    // simulating the documented extra indexing delay into ranked recall.
    *state.recall_lag_calls.lock().unwrap() = 2;

    memory
        .store(
            "company-a",
            "greeting",
            "hello there",
            MemoryCategory::Core,
            None,
        )
        .await
        .expect("store succeeds once ranked-recall visibility clears");

    let hits = memory
        .recall(
            "hello",
            10,
            RecallOpts {
                namespace: Some("company-a"),
                ..Default::default()
            },
        )
        .await
        .expect("recall succeeds");
    assert_eq!(
        hits.len(),
        1,
        "store must not return before its own write is visible through /v1/recall: {hits:?}"
    );
    assert_eq!(hits[0].key, "greeting");
}

#[tokio::test]
async fn a_second_store_under_the_same_key_replaces_the_first_on_read() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    memory
        .store("company-a", "note", "first", MemoryCategory::Core, None)
        .await
        .expect("first store succeeds");
    // Ensure a distinct observed_at so "most recent" is unambiguous even at
    // millisecond resolution.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    memory
        .store("company-a", "note", "second", MemoryCategory::Core, None)
        .await
        .expect("second store succeeds");

    let fetched = memory
        .get("company-a", "note")
        .await
        .expect("get succeeds")
        .expect("entry exists");
    assert_eq!(fetched.content, "second");
}

#[tokio::test]
async fn two_companies_never_share_recall() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    memory
        .store(
            "company-a",
            "secret",
            "company A's secret",
            MemoryCategory::Core,
            None,
        )
        .await
        .expect("store for company-a succeeds");
    memory
        .store(
            "company-b",
            "secret",
            "company B's secret",
            MemoryCategory::Core,
            None,
        )
        .await
        .expect("store for company-b succeeds");

    let a_hits = memory
        .recall(
            "secret",
            10,
            RecallOpts {
                namespace: Some("company-a"),
                ..Default::default()
            },
        )
        .await
        .expect("recall succeeds");
    assert_eq!(a_hits.len(), 1);
    assert_eq!(a_hits[0].content, "company A's secret");

    let b_hits = memory
        .recall(
            "secret",
            10,
            RecallOpts {
                namespace: Some("company-b"),
                ..Default::default()
            },
        )
        .await
        .expect("recall succeeds");
    assert_eq!(b_hits.len(), 1);
    assert_eq!(b_hits[0].content, "company B's secret");
}

#[tokio::test]
async fn an_actor_mismatch_is_a_surfaced_error_not_a_silent_empty_result() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    // The mock only accepts `ACTOR`; this client claims a different one, so
    // every request is a 401.
    let memory = client(&base_url, "someone-else");

    let error = memory
        .store("company-a", "k", "v", MemoryCategory::Core, None)
        .await
        .expect_err("a 401 must surface as Err, not as a quiet no-op");
    let message = error.to_string();
    assert!(
        message.contains("actor") || message.to_ascii_lowercase().contains("unauthorized"),
        "error should name the auth failure: {message}"
    );

    // Read paths must not swallow it into "nothing found" either.
    let error = memory
        .get("company-a", "k")
        .await
        .expect_err("a 401 on recall must also surface as Err");
    assert!(!error.to_string().is_empty());
}

#[tokio::test]
async fn forget_removes_the_stored_event() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    memory
        .store(
            "company-a",
            "temp",
            "throwaway",
            MemoryCategory::Daily,
            None,
        )
        .await
        .expect("store succeeds");
    assert!(
        memory
            .get("company-a", "temp")
            .await
            .expect("get succeeds")
            .is_some()
    );

    let removed = memory
        .forget("company-a", "temp")
        .await
        .expect("forget succeeds");
    assert!(removed, "forget should report the record was removed");

    assert!(
        memory
            .get("company-a", "temp")
            .await
            .expect("get succeeds")
            .is_none()
    );
}

#[tokio::test]
async fn forget_of_an_absent_key_is_false_not_an_error() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);
    let removed = memory
        .forget("company-a", "never-stored")
        .await
        .expect("forget of an absent key does not error");
    assert!(!removed);
}
