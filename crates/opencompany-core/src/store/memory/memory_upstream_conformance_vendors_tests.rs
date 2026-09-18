use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::tests_upstream_conformance::{
    Backend, Row, Store, assert_retains_then_conforms, facade_round_trip, open, remote_config,
    serve,
};

mod mem0 {
    use super::*;

    async fn mem0_list(State(store): State<Store>) -> Json<Value> {
        let store = store.lock().expect("store lock");
        let results: Vec<Value> = store
            .rows
            .values()
            .map(|r| {
                json!({
                    "id": r.id,
                    "memory": r.content,
                    "metadata": r.metadata,
                    "created_at": "1970-01-01T00:00:00Z",
                })
            })
            .collect();
        Json(json!({ "results": results }))
    }

    async fn mem0_create(State(store): State<Store>, Json(body): Json<Value>) -> Json<Value> {
        let mut store = store.lock().expect("store lock");
        let id = store.fresh_id();
        let content = body["messages"][0]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let metadata = body["metadata"].clone();
        store.rows.insert(
            id.clone(),
            Row {
                id: id.clone(),
                content,
                metadata,
                tag: String::new(),
            },
        );
        Json(json!({ "results": [{ "id": id }] }))
    }

    async fn mem0_update(
        State(store): State<Store>,
        Path(id): Path<String>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let mut store = store.lock().expect("store lock");
        if let Some(row) = store.rows.get_mut(&id) {
            if let Some(text) = body["text"].as_str() {
                row.content = text.to_owned();
            }
            if !body["metadata"].is_null() {
                row.metadata = body["metadata"].clone();
            }
        }
        Json(json!({ "id": id }))
    }

    async fn mem0_delete(State(store): State<Store>, Path(id): Path<String>) -> Json<Value> {
        store.lock().expect("store lock").rows.remove(&id);
        Json(json!({ "deleted": true }))
    }

    async fn mem0_search(State(store): State<Store>, Json(body): Json<Value>) -> Json<Value> {
        // Substring matching is enough: the suite asserts that recall *narrows*,
        // not that the backend ranks well.
        let needle = body["query"].as_str().unwrap_or_default().to_lowercase();
        let limit = body["top_k"].as_u64().unwrap_or(100) as usize;
        let store = store.lock().expect("store lock");
        let results: Vec<Value> = store
            .rows
            .values()
            .filter(|r| r.content.to_lowercase().contains(&needle))
            .take(limit)
            .map(|r| {
                json!({
                    "id": r.id,
                    "memory": r.content,
                    "metadata": r.metadata,
                    "score": 0.9,
                })
            })
            .collect();
        Json(json!({ "results": results }))
    }

    async fn mem0_backend() -> String {
        let store: Store = Arc::new(Mutex::new(Backend::default()));
        let app = Router::new()
            .route("/memories", get(mem0_list).post(mem0_create))
            .route("/memories/{id}", put(mem0_update).delete(mem0_delete))
            .route("/search", post(mem0_search))
            .with_state(store);
        serve(app).await
    }

    #[tokio::test]
    async fn the_mem0_driver_this_host_binds_upholds_the_contract() {
        let endpoint = mem0_backend().await;
        let (provider, _) = open(&remote_config("mem0", &endpoint));
        assert_retains_then_conforms(provider).await;
    }

    #[tokio::test]
    async fn the_mem0_driver_round_trips_the_facades() {
        let endpoint = mem0_backend().await;
        let (provider, class) = open(&remote_config("mem0", &endpoint));
        facade_round_trip(provider, class).await;
    }
}

/// Supermemory's native shapes: container tags are its namespace equivalent,
/// so the double keeps a tag per row and the tag listing reflects real writes.
mod supermemory {
    use super::*;

    fn tag_of(row: &Row) -> String {
        row.tag.clone()
    }

    async fn sm_tags(State(store): State<Store>) -> Json<Value> {
        let store = store.lock().expect("store lock");
        let mut tags: Vec<String> = store
            .rows
            .values()
            .map(tag_of)
            .filter(|tag| !tag.is_empty())
            .collect();
        tags.sort();
        tags.dedup();
        Json(Value::Array(
            tags.into_iter()
                .map(|t| json!({ "containerTag": t }))
                .collect(),
        ))
    }

    async fn sm_list(State(store): State<Store>, Json(body): Json<Value>) -> Json<Value> {
        // The adapter pages until a short page comes back. One page, then empty.
        let page = body["page"].as_u64().unwrap_or(1);
        let wanted = body["containerTags"][0].as_str().unwrap_or_default();
        let store = store.lock().expect("store lock");
        let entries: Vec<Value> = if page > 1 {
            Vec::new()
        } else {
            store
                .rows
                .values()
                .filter(|r| tag_of(r) == wanted)
                .map(|r| {
                    json!({
                        "id": r.id,
                        "content": r.content,
                        "metadata": r.metadata,
                        "createdAt": "1970-01-01T00:00:00Z",
                        "isLatest": true,
                        "isForgotten": false,
                    })
                })
                .collect()
        };
        Json(json!({ "memoryEntries": entries }))
    }

    async fn sm_create(
        State(store): State<Store>,
        Json(body): Json<Value>,
    ) -> Result<Json<Value>, axum::http::StatusCode> {
        // The real v4 API requires `containerTag`; filing a malformed create under
        // "" would hide an adapter regression.
        let Some(tag) = body["containerTag"].as_str().filter(|tag| !tag.is_empty()) else {
            return Err(axum::http::StatusCode::BAD_REQUEST);
        };
        let mut store = store.lock().expect("store lock");
        let id = store.fresh_id();
        let first = &body["memories"][0];
        store.rows.insert(
            id.clone(),
            Row {
                id: id.clone(),
                content: first["content"].as_str().unwrap_or_default().to_owned(),
                metadata: first["metadata"].clone(),
                tag: tag.to_owned(),
            },
        );
        Ok(Json(json!({ "memories": [{ "id": id }] })))
    }

    async fn sm_update(State(store): State<Store>, Json(body): Json<Value>) -> Json<Value> {
        let mut store = store.lock().expect("store lock");
        let id = body["id"].as_str().unwrap_or_default().to_owned();
        if let Some(row) = store.rows.get_mut(&id) {
            if let Some(text) = body["newContent"].as_str() {
                row.content = text.to_owned();
            }
            if !body["metadata"].is_null() {
                row.metadata = body["metadata"].clone();
            }
        }
        Json(json!({ "id": id }))
    }

    async fn sm_delete(State(store): State<Store>, Json(body): Json<Value>) -> Json<Value> {
        let id = body["id"].as_str().unwrap_or_default();
        store.lock().expect("store lock").rows.remove(id);
        Json(json!({ "deleted": true }))
    }

    async fn sm_search(State(store): State<Store>, Json(body): Json<Value>) -> Json<Value> {
        let needle = body["q"]
            .as_str()
            .or_else(|| body["query"].as_str())
            .unwrap_or_default()
            .to_lowercase();
        let limit = body["limit"].as_u64().unwrap_or(100) as usize;
        let tag = body["containerTag"].as_str();
        let store = store.lock().expect("store lock");
        let results: Vec<Value> = store
            .rows
            .values()
            .filter(|r| tag.is_none_or(|t| tag_of(r) == t))
            .filter(|r| r.content.to_lowercase().contains(&needle))
            .take(limit)
            .map(|r| {
                json!({
                    "id": r.id,
                    "content": r.content,
                    "metadata": r.metadata,
                    "score": 0.9,
                })
            })
            .collect();
        Json(json!({ "results": results }))
    }

    async fn supermemory_backend() -> String {
        let store: Store = Arc::new(Mutex::new(Backend::default()));
        let app = Router::new()
            .route("/v3/container-tags/list", get(sm_tags))
            .route("/v4/memories/list", post(sm_list))
            .route(
                "/v4/memories",
                post(sm_create).patch(sm_update).delete(sm_delete),
            )
            .route("/v4/search", post(sm_search))
            .with_state(store);
        serve(app).await
    }

    #[tokio::test]
    async fn the_supermemory_driver_this_host_binds_upholds_the_contract() {
        let endpoint = supermemory_backend().await;
        let (provider, _) = open(&remote_config("supermemory", &endpoint));
        assert_retains_then_conforms(provider).await;
    }

    #[tokio::test]
    async fn the_supermemory_driver_round_trips_the_facades() {
        let endpoint = supermemory_backend().await;
        let (provider, class) = open(&remote_config("supermemory", &endpoint));
        facade_round_trip(provider, class).await;
    }
}

/// Cognee's native shapes — the odd one out: no per-record API. The adapter
/// uploads each record as a JSON *file* into a per-namespace dataset and reads
/// it back through `/raw`, so the double stores the uploaded bytes verbatim
/// and serves them unchanged.
mod cognee {
    use super::*;

    type Datasets = Arc<Mutex<BTreeMap<String, BTreeMap<String, String>>>>;

    async fn cg_datasets(State(sets): State<Datasets>) -> Json<Value> {
        let sets = sets.lock().expect("store lock");
        Json(Value::Array(
            sets.keys()
                .map(|name| json!({ "id": name, "name": name }))
                .collect(),
        ))
    }

    async fn cg_data(State(sets): State<Datasets>, Path(dataset): Path<String>) -> Json<Value> {
        let sets = sets.lock().expect("store lock");
        let ids: Vec<Value> = sets
            .get(&dataset)
            .map(|d| d.keys().map(|id| json!({ "id": id, "name": id })).collect())
            .unwrap_or_default();
        Json(Value::Array(ids))
    }

    async fn cg_raw(
        State(sets): State<Datasets>,
        Path((dataset, data_id)): Path<(String, String)>,
    ) -> String {
        sets.lock()
            .expect("store lock")
            .get(&dataset)
            .and_then(|d| d.get(&data_id))
            .cloned()
            .unwrap_or_default()
    }

    async fn cg_delete(
        State(sets): State<Datasets>,
        Path((dataset, data_id)): Path<(String, String)>,
    ) -> Json<Value> {
        if let Some(d) = sets.lock().expect("store lock").get_mut(&dataset) {
            d.remove(&data_id);
        }
        Json(json!({ "deleted": true }))
    }

    /// Pulls the uploaded envelope and the dataset name out of a multipart body.
    async fn multipart_parts(mut form: axum::extract::Multipart) -> (String, String, String) {
        let (mut body, mut dataset, mut filename) = (String::new(), String::new(), String::new());
        while let Ok(Some(field)) = form.next_field().await {
            match field.name().unwrap_or_default().to_owned().as_str() {
                "datasetName" => dataset = field.text().await.unwrap_or_default(),
                "data" | "file" | "files" => {
                    filename = field.file_name().unwrap_or_default().to_owned();
                    body = field.text().await.unwrap_or_default();
                }
                _ => {
                    let _ = field.bytes().await;
                }
            }
        }
        (body, dataset, filename)
    }

    async fn cg_remember(
        State(sets): State<Datasets>,
        form: axum::extract::Multipart,
    ) -> Json<Value> {
        let (body, dataset, filename) = multipart_parts(form).await;
        let mut sets = sets.lock().expect("store lock");
        sets.entry(dataset).or_default().insert(filename, body);
        Json(json!({ "status": "ok" }))
    }

    async fn cg_update(
        State(sets): State<Datasets>,
        Query(q): Query<BTreeMap<String, String>>,
        form: axum::extract::Multipart,
    ) -> Json<Value> {
        let (body, _, _) = multipart_parts(form).await;
        let dataset = q.get("dataset_id").cloned().unwrap_or_default();
        let data_id = q.get("data_id").cloned().unwrap_or_default();
        if let Some(d) = sets.lock().expect("store lock").get_mut(&dataset) {
            d.insert(data_id, body);
        }
        Json(json!({ "status": "ok" }))
    }

    async fn cg_recall(State(sets): State<Datasets>, Json(body): Json<Value>) -> Json<Value> {
        let needle = body["query"].as_str().unwrap_or_default().to_lowercase();
        let limit = body["top_k"].as_u64().unwrap_or(100) as usize;
        let wanted: Option<Vec<String>> = body["datasets"].as_array().map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        });
        let sets = sets.lock().expect("store lock");
        let hits: Vec<Value> = sets
            .iter()
            .filter(|(name, _)| wanted.as_ref().is_none_or(|w| w.contains(name)))
            .flat_map(|(_, d)| d.values())
            .filter(|raw| raw.to_lowercase().contains(&needle))
            .take(limit)
            .map(|raw| json!({ "text": raw, "score": 0.9 }))
            .collect();
        // A TOP-LEVEL array, which is what the adapter's `search` iterates
        // (`response.as_array()`, cognee.rs). This deliberately DIVERGES from the
        // vendored double, which wraps hits in `{"results": …}` — a shape the
        // adapter cannot see, so the upstream suite's recall coverage for Cognee
        // is vacuously green. Caught here because the facade round-trip requires
        // an actual hit; reported upstream rather than mirrored.
        Json(Value::Array(hits))
    }

    async fn cognee_backend() -> String {
        let sets: Datasets = Arc::new(Mutex::new(BTreeMap::new()));
        let app = Router::new()
            // The real API serves the collection at the slashed form and 307s the
            // bare one; the adapter asks for `/api/v1/datasets/` directly.
            .route("/api/v1/datasets/", get(cg_datasets))
            .route("/api/v1/datasets/{dataset}/data", get(cg_data))
            .route("/api/v1/datasets/{dataset}/data/{data_id}/raw", get(cg_raw))
            .route(
                "/api/v1/datasets/{dataset}/data/{data_id}",
                delete(cg_delete),
            )
            .route("/api/v1/remember", post(cg_remember))
            .route("/api/v1/update", axum::routing::patch(cg_update))
            .route("/api/v1/recall", post(cg_recall))
            .with_state(sets);
        serve(app).await
    }

    #[tokio::test]
    async fn the_cognee_driver_this_host_binds_upholds_the_contract() {
        let endpoint = cognee_backend().await;
        let (provider, _) = open(&remote_config("cognee", &endpoint));
        assert_retains_then_conforms(provider).await;
    }

    #[tokio::test]
    async fn the_cognee_driver_round_trips_the_facades() {
        let endpoint = cognee_backend().await;
        let (provider, class) = open(&remote_config("cognee", &endpoint));
        facade_round_trip(provider, class).await;
    }
}

/// CortexDB — an append-only event log, not a keyed store.
///
/// This double is deliberately thin. The fidelity double lives upstream in
/// `tinymemory-remote`'s own conformance tests, where it reproduces the
/// duplicated listing, the cursor paging and the forget interlocks; duplicating
/// that here would be two copies to keep honest. What this one has to get right
/// is the pair of behaviours the *binding* depends on:
///
/// - `/v1/experience` is accepted and immediately readable. The real engine
///   indexes asynchronously and the driver waits for its own write, so a double
///   that never becomes readable would hang that wait rather than fail it.
/// - `/v1/recall` renders content with the speaker prefixed while `/v1/events`
///   returns it as stored. A driver that parses only the stored form lists
///   correctly and searches to nothing, which is the failure this catches.
mod cortex {
    use super::*;

    type Log = Arc<Mutex<Vec<Value>>>;

    async fn cx_experience(State(log): State<Log>, Json(body): Json<Value>) -> Json<Value> {
        let mut log = log.lock().expect("store lock");
        let id = format!("evt_{}", log.len() + 1);
        let offset = (log.len() as u64 + 1) * 2;
        log.push(json!({
            "id": id,
            "scope": body.get("scope").cloned().unwrap_or(Value::Null),
            "wal_offset": offset,
            "content": { "text": body.pointer("/content/text").cloned().unwrap_or(Value::Null) },
            "context": { "recorded_at": "2026-09-04T00:00:00Z" },
        }));
        Json(json!({ "event_id": id, "status": "captured" }))
    }

    async fn cx_events(
        State(log): State<Log>,
        Query(params): Query<BTreeMap<String, String>>,
    ) -> Json<Value> {
        let log = log.lock().expect("store lock");
        let scope = params.get("scope").cloned().unwrap_or_default();
        let items: Vec<Value> = log
            .iter()
            .rev()
            .filter(|e| e.get("scope").and_then(Value::as_str) == Some(scope.as_str()))
            .cloned()
            .collect();
        Json(json!({ "items": items, "has_more": false, "next_cursor": Value::Null }))
    }

    async fn cx_recall(State(log): State<Log>, Json(body): Json<Value>) -> Json<Value> {
        let log = log.lock().expect("store lock");
        let scope = body
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let query = body
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let hits: Vec<Value> = log
            .iter()
            .filter(|e| e.get("scope").and_then(Value::as_str) == Some(scope))
            .filter(|e| {
                query.is_empty()
                    || e.pointer("/content/text")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t.to_lowercase().contains(&query.to_lowercase()))
            })
            .map(|e| {
                // The engine renders for a reader; the listing does not.
                let mut hit = e.clone();
                if let Some(text) = e.pointer("/content/text").and_then(Value::as_str) {
                    hit["content"]["text"] = json!(format!("[user] {text}"));
                }
                hit
            })
            .collect();
        Json(json!({ "layers": { "events": hits } }))
    }

    async fn cx_forget(State(log): State<Log>, Json(body): Json<Value>) -> Json<Value> {
        let mut log = log.lock().expect("store lock");
        let ids: Vec<String> = body
            .pointer("/selector/memory_ids")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let before = log.len();
        log.retain(|e| {
            !ids.contains(
                &e.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        });
        Json(json!({ "deleted": { "events": before - log.len() } }))
    }

    async fn cx_scopes(State(log): State<Log>) -> Json<Value> {
        let log = log.lock().expect("store lock");
        let mut paths: Vec<String> = log
            .iter()
            .filter_map(|e| e.get("scope").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        paths.sort();
        paths.dedup();
        Json(json!({
            "items": paths.into_iter().map(|p| json!({ "path": p })).collect::<Vec<_>>()
        }))
    }

    async fn cortex_backend() -> String {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/experience", post(cx_experience))
            .route("/v1/events", get(cx_events))
            .route("/v1/recall", post(cx_recall))
            .route("/v1/forget", post(cx_forget))
            .route("/v1/scopes/list", get(cx_scopes))
            .route(
                "/v1/admin/health",
                get(|| async { Json(json!({ "status": "healthy" })) }),
            )
            .with_state(log);
        serve(app).await
    }

    #[tokio::test]
    async fn the_cortex_driver_this_host_binds_upholds_the_contract() {
        let endpoint = cortex_backend().await;
        let (provider, _) = open(&remote_config("cortex", &endpoint));
        assert_retains_then_conforms(provider).await;
    }

    #[tokio::test]
    async fn the_cortex_driver_round_trips_the_facades() {
        let endpoint = cortex_backend().await;
        let (provider, class) = open(&remote_config("cortex", &endpoint));
        facade_round_trip(provider, class).await;
    }
}
