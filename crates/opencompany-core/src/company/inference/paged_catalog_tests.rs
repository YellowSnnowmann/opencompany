use super::*;

#[test]
fn the_page_path_asks_for_the_maximum_page() {
    assert_eq!(page_path(0), "/models?limit=500&offset=0");
    assert_eq!(page_path(500), "/models?limit=500&offset=500");
}

#[test]
fn a_page_is_unwrapped_and_unknown_fields_are_ignored() {
    let body = serde_json::json!({
        "success": true,
        "data": {
            "object": "list",
            "data": [
                {
                    "id": "acme/test-model",
                    "display_name": "Test Model",
                    "context_length": 8192,
                    "pricing": {"prompt": "0.0001"},
                    "supports_tools": true,
                    "input_modalities": ["text"],
                },
                {"id": "acme/other-model", "name": "Other Model"},
            ],
            "total": 2,
            "limit": 500,
            "offset": 0,
        },
    })
    .to_string();
    let page = parse_page(&body).expect("parses");
    assert_eq!(page.raw_len, 2);
    assert_eq!(page.total, Some(2));
    assert_eq!(
        page.entries,
        vec![
            CatalogEntry {
                id: "acme/test-model".to_string(),
                name: Some("Test Model".to_string()),
                context_length: Some(8192),
            },
            CatalogEntry {
                id: "acme/other-model".to_string(),
                name: Some("Other Model".to_string()),
                context_length: None,
            },
        ]
    );
}

#[test]
fn every_id_is_kept_as_given() {
    let body = serde_json::json!({
        "success": true,
        "data": {"data": [{"id": "acme/test-model"}, {"id": "x"}, {"id": "a:b:c"}]},
    })
    .to_string();
    let page = parse_page(&body).expect("parses");
    let ids: Vec<&str> = page.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["acme/test-model", "x", "a:b:c"]);
}

#[test]
fn an_openai_shaped_body_is_an_error_not_an_empty_page() {
    let body = serde_json::json!({"data": [{"id": "acme/test-model"}]}).to_string();
    let err = parse_page(&body).expect_err("not the envelope");
    assert!(err.contains("envelope"), "{err}");
}

#[test]
fn success_false_is_an_error_carrying_the_reason() {
    let body = serde_json::json!({"success": false, "error": "rate limited"}).to_string();
    let err = parse_page(&body).expect_err("failure");
    assert!(err.contains("rate limited"), "{err}");
}

#[test]
fn a_malformed_entry_is_dropped_but_still_advances_paging() {
    let body = serde_json::json!({
        "success": true,
        "data": {
            "data": [
                {"id": 42},
                {"id": "   "},
                {"id": "acme/test-model", "context_length": "not-a-number"},
            ],
            "total": 3,
        },
    })
    .to_string();
    let page = parse_page(&body).expect("parses");
    assert_eq!(
        page.raw_len, 3,
        "raw_len counts every entry, dropped or not"
    );
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].id, "acme/test-model");
    assert_eq!(page.entries[0].context_length, None);
}

#[test]
fn paging_follows_total_and_stops_there() {
    let mut collector = Collector::default();
    let first = parse_page(
        &serde_json::json!({"success": true, "data": {"data": [{"id": "a"}, {"id": "b"}], "total": 3}})
            .to_string(),
    )
    .unwrap();
    assert_eq!(collector.push(first), NextPage::At(2));
    let second = parse_page(
        &serde_json::json!({"success": true, "data": {"data": [{"id": "c"}], "total": 3}})
            .to_string(),
    )
    .unwrap();
    assert_eq!(collector.push(second), NextPage::Done);
    assert_eq!(collector.finish().len(), 3);
}

#[test]
fn a_clamped_limit_costs_requests_not_models() {
    let mut collector = Collector::default();
    let first = parse_page(
        &serde_json::json!({"success": true, "data": {"data": [{"id": "a"}], "total": 2}})
            .to_string(),
    )
    .unwrap();
    assert_eq!(collector.push(first), NextPage::At(1));
    let second = parse_page(
        &serde_json::json!({"success": true, "data": {"data": [{"id": "b"}], "total": 2}})
            .to_string(),
    )
    .unwrap();
    assert_eq!(collector.push(second), NextPage::Done);
}

#[test]
fn an_empty_page_ends_a_read_whose_total_is_never_reached() {
    let mut collector = Collector::default();
    let page = parse_page(
        &serde_json::json!({"success": true, "data": {"data": [], "total": 100}}).to_string(),
    )
    .unwrap();
    assert_eq!(collector.push(page), NextPage::Done);
}

#[test]
fn a_page_with_no_total_is_the_whole_answer() {
    let mut collector = Collector::default();
    let page = parse_page(
        &serde_json::json!({"success": true, "data": {"data": [{"id": "a"}]}}).to_string(),
    )
    .unwrap();
    assert_eq!(collector.push(page), NextPage::Done);
}

#[test]
fn duplicates_across_pages_are_kept_once() {
    let mut collector = Collector::default();
    let first = parse_page(
        &serde_json::json!({"success": true, "data": {"data": [{"id": "a"}], "total": 2}})
            .to_string(),
    )
    .unwrap();
    collector.push(first);
    let second = parse_page(
        &serde_json::json!({"success": true, "data": {"data": [{"id": "a"}], "total": 2}})
            .to_string(),
    )
    .unwrap();
    collector.push(second);
    let ids: Vec<String> = collector.finish().into_iter().map(|e| e.id).collect();
    assert_eq!(ids, vec!["a".to_string()]);
}

#[test]
fn the_page_bound_reports_truncation_instead_of_looping() {
    let mut collector = Collector::default();
    let mut outcome = NextPage::Done;
    for i in 0..20 {
        let page = parse_page(
            &serde_json::json!({
                "success": true,
                "data": {"data": [{"id": format!("m{i}")}], "total": 1_000_000},
            })
            .to_string(),
        )
        .unwrap();
        outcome = collector.push(page);
    }
    assert_eq!(
        outcome,
        NextPage::Truncated {
            read: 20,
            total: 1_000_000
        }
    );
}

/// Moved from `server::ops::inference::providers` alongside
/// `catalogue_offer` itself (keys rework #2306, P3-7 review).
#[test]
fn the_offered_catalogue_is_sorted_and_deduplicated() {
    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    assert_eq!(
        catalogue_offer(&ids(&["b", "a", "b"])),
        ids(&["a", "b"]),
        "a catalog's own order is whatever the endpoint felt like"
    );
}

/// Round-3a review P2-5: a catalog larger than the old 500-id cap must be
/// offered in full, not truncated to an alphabetical prefix.
#[test]
fn a_catalogue_larger_than_the_old_cap_is_offered_in_full() {
    let many: Vec<String> = (0..600).map(|n| format!("model-{n:04}")).collect();
    let offered = catalogue_offer(&many);
    assert_eq!(offered.len(), 600);
    // Not just a count: the tail past the old 500-entry cap is present.
    assert!(offered.contains(&"model-0599".to_string()));
}
