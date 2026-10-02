//! The wire form is the contract: these pin the field names the platform
//! backend actually sends, because a rename here is a runtime decode error and
//! nothing else would catch it.

use super::{SearchResponse, SearchResultItem};

#[test]
fn reads_the_backend_envelope() {
    let response: SearchResponse = serde_json::from_value(serde_json::json!({
        "searchId": "req-1",
        "costUsd": 0.012,
        "results": [{
            "url": "https://example.com/a",
            "title": "A",
            "publish_date": "2026-01-02",
            "excerpts": ["one", "two"],
        }],
    }))
    .expect("the managed envelope");
    assert_eq!(response.search_id, "req-1");
    assert!((response.cost_usd - 0.012).abs() < f64::EPSILON);
    assert_eq!(response.provider, None);
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].url, "https://example.com/a");
    assert_eq!(response.results[0].excerpts, vec!["one", "two"]);
}

#[test]
fn a_result_needs_only_its_url() {
    let item: SearchResultItem =
        serde_json::from_value(serde_json::json!({ "url": "https://example.com/b" }))
            .expect("a bare result");
    assert_eq!(item.title, "");
    assert_eq!(item.publish_date, None);
    assert!(item.excerpts.is_empty());
}

#[test]
fn the_resolved_engine_arrives_under_any_of_its_names() {
    for field in ["provider", "resolvedProvider", "searchProvider"] {
        let response: SearchResponse = serde_json::from_value(serde_json::json!({
            "searchId": "req-2",
            "costUsd": 0.0,
            "results": [],
            field: "Exa",
        }))
        .expect("the envelope");
        assert_eq!(response.provider.as_deref(), Some("Exa"), "under {field}");
    }
}

#[test]
fn a_missing_search_id_is_a_decode_error() {
    let decoded: Result<SearchResponse, _> = serde_json::from_value(serde_json::json!({
        "costUsd": 0.0,
        "results": [],
    }));
    assert!(decoded.is_err(), "searchId is not optional");
}
