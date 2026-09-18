/// The projection runs **before** the task-aware extractor and before the
/// artifact store, so anything it drops is gone for every consumer. A link
/// can be the answer — "list the issues with their browser links" — and the
/// first cut of this dropped `url`, `href` and every `*_url` on the premise
/// that a link never is (codex and CodeRabbit on
/// tinyhumansai/opencompany#2153).
#[test]
fn answering_links_survive_the_projection() {
    // Two records, not one: the projection declines an array shorter than
    // that, so a single-record payload would pass every assertion below
    // without the projection having run at all.
    let record = |n: u64| {
        serde_json::json!({
            "number": n,
            "title": "flaky login",
            "url": format!("https://api.github.com/repos/o/r/issues/{n}"),
            "html_url": format!("https://github.com/o/r/issues/{n}"),
            "href": format!("https://example.test/{n}"),
            "avatar_urls": ["https://example.test/a.png"],
            "comments_url": format!("https://api.github.com/repos/o/r/issues/{n}/comments"),
            "labels_url": format!("https://api.github.com/repos/o/r/issues/{n}/labels"),
            "user": { "login": "octocat", "id": 7 }
        })
    };
    // Over `MAX_BODY_BYTES`: the projection declines anything already small
    // enough, so a short payload passes every assertion below without it
    // having run. Two earlier versions of this test did exactly that.
    let records: Vec<serde_json::Value> = (1..=60).map(record).collect();
    let payload = serde_json::json!({ "data": records });
    assert!(
        serde_json::to_string(&payload).unwrap().len() > MAX_BODY_BYTES,
        "the fixture must exceed the projection threshold or nothing runs"
    );

    let projected = project_records_value(payload);
    let record = &projected["data"][0];

    // The fields a caller can actually have asked for.
    assert_eq!(record["html_url"], "https://github.com/o/r/issues/1");
    assert_eq!(record["url"], "https://api.github.com/repos/o/r/issues/1");
    assert_eq!(record["href"], "https://example.test/1");
    assert!(
        !record["avatar_urls"].is_null(),
        "a `*_urls` field is content, not API plumbing: {record}"
    );
    // Proof the projection actually ran. Without this the link assertions
    // above pass trivially on a payload the projection declined — which is
    // exactly what the first version of this test did.
    assert_eq!(
        record["user"], "octocat",
        "the nested object did not collapse, so the projection never ran: {record}"
    );
    assert_eq!(record["title"], "flaky login");
    // Only self-referential collection endpoints go.
    assert!(record["comments_url"].is_null(), "collection endpoint kept");
    assert!(record["labels_url"].is_null(), "collection endpoint kept");
}

use super::*;

/// The exact shape the live GitHub call returns: records two levels down,
/// under Composio's own `data` envelope.
#[test]
fn projects_records_nested_under_the_composio_envelope() {
    let body = serde_json::json!({
        "data": { "details": [
            { "number": 1, "title": "a", "url": "https://x", "html_url": "https://y",
              "user": { "login": "octocat", "avatar_url": "https://z", "id": 5 },
              "labels": [ { "name": "bug", "url": "https://l" } ] },
            { "number": 2, "title": "b", "url": "https://x2", "html_url": "https://y2",
              "user": { "login": "hubot", "avatar_url": "https://z2", "id": 6 },
              "labels": [ { "name": "p2", "url": "https://l2" } ] }
        ]}
    })
    .to_string();

    let projected = project_records(&body).expect("records nested under `data.details`");
    assert!(
        projected.len() < body.len(),
        "must shrink: {} -> {}",
        body.len(),
        projected.len()
    );
    // These used to assert that `avatar_url` and `html_url` were dropped.
    // They are answers a caller can ask for, and this projection runs ahead
    // of the task-aware extractor and the artifact store, so dropping them
    // was unrecoverable (codex and CodeRabbit on
    // tinyhumansai/opencompany#2153). What goes now is API plumbing only.
    assert!(
        projected.contains("html_url"),
        "a browser link can be the answer and must survive: {projected}"
    );
    assert!(
        !projected.contains("comments_url"),
        "self-referential collection endpoints still go: {projected}"
    );
    assert!(
        projected.contains("octocat"),
        "nested user collapses to its login: {projected}"
    );
    assert!(
        projected.contains("bug"),
        "label array collapses to names: {projected}"
    );
    assert!(
        projected.contains("\"title\""),
        "answering fields survive: {projected}"
    );
}
