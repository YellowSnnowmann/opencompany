use super::*;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[test]
fn a_v3_tool_keeps_the_schema_an_agent_needs_to_call_it() {
    // The whole reason this module restates the listing: `list_actions`
    // would drop `input_parameters`, and an agent without an action's
    // schema invents arguments.
    let body: V3Page<V3Tool> = serde_json::from_str(
        r#"{"items":[{"slug":"GMAIL_SEND_EMAIL","description":"Send",
             "input_parameters":{"type":"object"},"output_parameters":{"type":"object"}}]}"#,
    )
    .unwrap();
    assert_eq!(body.items.len(), 1);
    assert!(body.items[0].input_parameters.is_some());
    assert!(body.items[0].output_parameters.is_some());
}

#[test]
fn an_older_payload_spelling_parameters_still_carries_its_schema() {
    let body: V3Page<V3Tool> =
        serde_json::from_str(r#"{"items":[{"slug":"X","parameters":{"type":"object"}}]}"#).unwrap();
    assert!(body.items[0].input_parameters.is_some());
}

#[test]
fn categories_parse_whether_composio_sends_objects_or_strings() {
    let meta: V3ToolkitMeta = serde_json::from_str(
        r#"{"categories":[{"name":"Productivity","slug":"productivity"},"email",{"slug":"crm"}]}"#,
    )
    .unwrap();
    let names: Vec<String> = meta
        .categories
        .into_iter()
        .filter_map(V3Category::name)
        .collect();
    assert_eq!(names, vec!["Productivity", "email", "crm"]);
}

#[test]
fn a_toolkit_row_missing_everything_but_a_slug_is_still_connectable() {
    // A thin catalog row must not drop the provider — the console renders
    // its own typography for slug-only entries, and a missing row is a
    // provider the operator simply cannot connect.
    let body: V3Page<V3Toolkit> =
        serde_json::from_str(r#"{"items":[{"slug":"GMAIL"},{"slug":"  "}]}"#).unwrap();
    assert_eq!(body.items.len(), 2);
    let kept: Vec<String> = body
        .items
        .into_iter()
        .map(|item| item.slug.trim().to_ascii_lowercase())
        .filter(|slug| !slug.is_empty())
        .collect();
    assert_eq!(
        kept,
        vec!["gmail"],
        "a blank slug is dropped, a bare one is kept"
    );
}

/// A mock Composio v3 that answers the two listings this module states
/// itself, asserting the `x-api-key` header on the way through: the whole
/// point of BYOK is that the *company's* key, and no other credential,
/// reaches Composio.
/// The two endpoints answer to different limits, and shared one constant
/// until review caught it. The 1000 is justified for `/tools` only; the
/// catalogue is paged at what the backend proves against the same API.
#[test]
fn the_two_endpoints_do_not_share_a_page_limit() {
    assert_eq!(TOOLS_PAGE_LIMIT, "1000");
    assert_eq!(TOOLKITS_PAGE_LIMIT, "500");
    assert_ne!(
        TOOLS_PAGE_LIMIT, TOOLKITS_PAGE_LIMIT,
        "a tools-only justification must not govern the provider catalogue"
    );
}

/// A `/tools` server that records the query it was asked with.
///
/// The existing fixture asserts on counts, which cannot see a query
/// parameter at all — and `important=true` changes what the *server*
/// returns, so a count-based assertion would pass whether it was sent or
/// not (tinysweeper on tinyhumansai/opencompany#2153).
async fn spawn_query_recorder() -> (String, Arc<Mutex<Vec<HashMap<String, String>>>>) {
    use axum::extract::Query;
    use axum::routing::get;
    use axum::{Json, Router};

    let seen: Arc<Mutex<Vec<HashMap<String, String>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    // `/tools`, not `/api/v3/tools`: `with_v3_base_for_test` already carries
    // the API path, exactly as the fixtures below mount it.
    let app = Router::new().route(
        "/tools",
        get(move |Query(params): Query<HashMap<String, String>>| {
            let sink = sink.clone();
            async move {
                sink.lock().expect("query sink").push(params);
                Json(serde_json::json!({ "items": [] }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, seen)
}

/// An unnarrowed browse asks for the curated set; a search must reach the
/// long tail, so it must not.
#[tokio::test]
async fn important_is_sent_only_for_an_unnarrowed_browse() {
    let (base, seen) = spawn_query_recorder().await;
    let direct = DirectComposio::new("ak_live").with_v3_base_for_test(base);

    direct
        .list_tools(&["github".to_string()], None, None)
        .await
        .expect("browse");
    direct
        .list_tools(&["github".to_string()], Some("issue"), None)
        .await
        .expect("search");
    direct
        .list_tools(
            &["github".to_string()],
            None,
            Some(&["important".to_string()]),
        )
        .await
        .expect("tag-narrowed");

    let seen = seen.lock().expect("query sink");
    assert_eq!(seen.len(), 3, "three calls were made");
    assert_eq!(
        seen[0].get("important").map(String::as_str),
        Some("true"),
        "an unnarrowed browse asks for the curated set: {:?}",
        seen[0]
    );
    assert!(
        !seen[1].contains_key("important"),
        "a search must reach past the featured actions: {:?}",
        seen[1]
    );
    assert!(
        !seen[2].contains_key("important"),
        "a tag filter is a search too: {:?}",
        seen[2]
    );
    assert_eq!(
        seen[2].get("tags").map(String::as_str),
        Some("important"),
        "the tag itself still travels: {:?}",
        seen[2]
    );
    assert_eq!(
        seen[1].get("search").map(String::as_str),
        Some("issue"),
        "the search term travels server-side: {:?}",
        seen[1]
    );
}

async fn spawn_composio_v3() -> String {
    use axum::extract::Query;
    use axum::http::HeaderMap;
    use axum::routing::get;
    use axum::{Json, Router};
    use std::collections::HashMap;

    async fn toolkits(headers: HeaderMap) -> Json<serde_json::Value> {
        assert_eq!(
            headers.get("x-api-key").and_then(|v| v.to_str().ok()),
            Some("ak_live"),
            "a BYOK call must present the company's own key"
        );
        Json(serde_json::json!({"items": [
            {"slug": "GMAIL", "name": "Gmail",
             "meta": {"logo": "https://cdn/gmail.png", "description": " Email ",
                      "categories": [{"name": "Productivity"}, "email"]}},
            {"slug": "slack", "name": "Slack"}
        ]}))
    }

    async fn tools(
        headers: HeaderMap,
        Query(params): Query<HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        assert_eq!(
            headers.get("x-api-key").and_then(|v| v.to_str().ok()),
            Some("ak_live")
        );
        assert_eq!(
            params.get("toolkit_versions").map(String::as_str),
            Some("latest"),
            "without the pin, v3 answers from a snapshot listing nothing recent"
        );
        // `toolkits` is the parameter Composio v3 silently ignores; if it
        // ever comes back the listing stops being scoped at all.
        assert!(
            !params.contains_key("toolkits"),
            "`toolkits` scopes nothing on Composio v3 — `toolkit_slug` is the one it honours"
        );
        let scoped = params.get("toolkit_slug").cloned().unwrap_or_default();
        Json(serde_json::json!({"items": [
            {"slug": "GMAIL_SEND_EMAIL", "description": scoped,
             "input_parameters": {"type": "object"}}
        ]}))
    }

    let app = Router::new()
        .route("/toolkits", get(toolkits))
        .route("/tools", get(tools));
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn the_catalog_is_the_company_s_own_and_every_row_is_connectable() {
    // OpenHuman's direct branch answers this with an empty list, which
    // would leave the console's provider grid blank with nothing saying
    // why. A BYOK company has no gate above its own account, so what it is
    // offered is what that account lists.
    let base = spawn_composio_v3().await;
    let direct = DirectComposio::new("ak_live").with_v3_base_for_test(base);

    let resp = direct.list_toolkits().await.expect("catalog");
    assert_eq!(
        resp.toolkits,
        vec!["gmail", "slack"],
        "slugs are normalized"
    );
    assert!(
        resp.catalog.iter().all(|e| e.enabled == Some(true)),
        "nothing stands between a company and its own account"
    );
    let gmail = &resp.catalog[0];
    assert_eq!(gmail.name, "Gmail");
    assert_eq!(gmail.description.as_deref(), Some("Email"));
    assert_eq!(gmail.logo.as_deref(), Some("https://cdn/gmail.png"));
    assert_eq!(gmail.categories, vec!["Productivity", "email"]);
    // A row with no `meta` at all is still offered, described by its slug.
    assert_eq!(resp.catalog[1].slug, "slack");
    assert!(resp.catalog[1].description.is_none());
}

#[tokio::test]
async fn a_tool_listing_carries_the_schema_and_is_scoped_to_the_toolkits_asked_for() {
    let base = spawn_composio_v3().await;
    let direct = DirectComposio::new("ak_live").with_v3_base_for_test(base);

    let resp = direct
        .list_tools(
            &["gmail".to_string(), "  ".to_string(), "slack".to_string()],
            None,
            None,
        )
        .await
        .expect("tools");
    assert_eq!(resp.tools.len(), 1);
    let function = &resp.tools[0].function;
    assert_eq!(function.name, "GMAIL_SEND_EMAIL");
    assert!(
        function.parameters.is_some(),
        "an agent without the schema invents arguments"
    );
    assert_eq!(
        function.description.as_deref(),
        Some("gmail,slack"),
        "blank slugs are dropped, the rest are sent as one csv"
    );
}

/// Composio's listings are cursor-paginated and far larger than one page:
/// 1501 toolkits, and 52,268 tools unscoped. A single `limit=200` request
/// returns the first 200 and says nothing, which is indistinguishable from a
/// complete answer — so the listing follows the cursor, and reports what it
/// could not reach.
#[tokio::test]
async fn a_listing_follows_the_cursor_and_says_what_it_could_not_reach() {
    use axum::extract::Query;
    use axum::routing::get;
    use axum::{Json, Router};
    use std::collections::HashMap;

    // Five pages of one row each, against a three-page budget.
    async fn toolkits(Query(params): Query<HashMap<String, String>>) -> Json<serde_json::Value> {
        let page: usize = params
            .get("cursor")
            .and_then(|c| c.parse().ok())
            .unwrap_or(1);
        Json(serde_json::json!({
            "items": [{ "slug": format!("tk{page}"), "name": format!("Toolkit {page}") }],
            "next_cursor": (page < 5).then(|| (page + 1).to_string()),
            "total_items": 5,
        }))
    }

    let app = Router::new().route("/toolkits", get(toolkits));
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let direct = DirectComposio::new("ak_live").with_v3_base_for_test(format!("http://{addr}"));
    let resp = direct.list_toolkits().await.expect("catalog");
    assert_eq!(
        resp.toolkits,
        vec!["tk1", "tk2", "tk3"],
        "three pages are followed, not one"
    );
}

/// A listing that hits the page budget with no `total_items` on any page —
/// an older or degraded response shape — still must not report `dropped: 0`.
/// Reaching the end of the budget with a live cursor in hand is itself proof
/// that more exists; the exact count is merely unknown, not zero.
#[tokio::test]
async fn a_truncated_listing_with_no_total_still_reports_dropped() {
    use axum::extract::Query;
    use axum::routing::get;
    use axum::{Json, Router};
    use std::collections::HashMap;

    async fn toolkits(Query(params): Query<HashMap<String, String>>) -> Json<serde_json::Value> {
        let page: usize = params
            .get("cursor")
            .and_then(|c| c.parse().ok())
            .unwrap_or(1);
        // No `total_items` field at all on any page.
        Json(serde_json::json!({
            "items": [{ "slug": format!("tk{page}"), "name": format!("Toolkit {page}") }],
            "next_cursor": (page + 1).to_string(),
        }))
    }

    let app = Router::new().route("/toolkits", get(toolkits));
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let direct = DirectComposio::new("ak_live").with_v3_base_for_test(format!("http://{addr}"));
    let paged: Paged<V3Toolkit> = direct
        .get_paged("/toolkits", &[("limit", "200".to_string())])
        .await
        .expect("page");
    assert_eq!(
        paged.items.len(),
        3,
        "the page budget, not the never-ending cursor"
    );
    assert!(
        paged.dropped >= 1,
        "no total_items must not read as a complete listing: dropped={}",
        paged.dropped
    );
}

/// A listing that ends inside the budget is complete, and must not claim to
/// have dropped anything — the cursor running out is the authority, not the
/// `total_items` count beside it.
#[tokio::test]
async fn a_listing_that_fits_reports_nothing_dropped() {
    use axum::routing::get;
    use axum::{Json, Router};

    let app = Router::new().route(
        "/toolkits",
        get(|| async {
            Json(serde_json::json!({
                "items": [{ "slug": "gmail", "name": "Gmail" }],
                "next_cursor": null,
                // Deliberately inconsistent with `items`: an exhausted cursor
                // ends the listing whatever the count says.
                "total_items": 99,
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let direct = DirectComposio::new("ak_live").with_v3_base_for_test(format!("http://{addr}"));
    let paged: Paged<V3Toolkit> = direct
        .get_paged("/toolkits", &[("limit", "200".to_string())])
        .await
        .expect("page");
    assert_eq!(paged.items.len(), 1);
    assert_eq!(paged.dropped, 0, "an exhausted cursor means complete");
}

/// Revoking works on this route. It is worth a test precisely because it was
/// once assumed not to: the vendored client has no delete method, and that
/// was mistaken for Composio not having the endpoint. It has one — a no-auth
/// probe answers 401 on it and 404 on a route that does not exist.
#[tokio::test]
async fn an_account_can_be_revoked_on_this_route() {
    use axum::extract::Path;
    use axum::http::HeaderMap;
    use axum::routing::delete;
    use axum::{Json, Router};
    use std::sync::{Arc, Mutex};

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let app = Router::new().route(
        "/connected_accounts/{id}",
        delete(move |Path(id): Path<String>, headers: HeaderMap| {
            let sink = Arc::clone(&sink);
            async move {
                assert_eq!(
                    headers.get("x-api-key").and_then(|v| v.to_str().ok()),
                    Some("ak_live"),
                    "a revoke presents the company's own key, like every other BYOK call"
                );
                sink.lock().unwrap().push(id);
                Json(serde_json::json!({}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let direct = DirectComposio::new("ak_live").with_v3_base_for_test(format!("http://{addr}"));
    let resp = direct
        .delete_connection("ca_abc123")
        .await
        .expect("revoked");
    assert!(resp.deleted);
    assert_eq!(seen.lock().unwrap().as_slice(), ["ca_abc123"]);
}

/// An id that is not one cannot be interpolated into the path — a value
/// carrying a slash would address a different route entirely.
#[tokio::test]
async fn a_value_that_is_not_a_connection_id_is_refused_before_any_request() {
    let direct = DirectComposio::new("ak_live").with_v3_base_for_test("http://127.0.0.1:1");
    for bad in ["", "  ", "ca_1/../../toolkits", "ca_1?x=y"] {
        assert!(
            direct.delete_connection(bad).await.is_err(),
            "`{bad}` must not reach the wire"
        );
    }
}

/// Live end-to-end against a real Composio account, driving the exact calls
/// the agent's `composio_list_tools` and `composio_execute` make.
///
/// `#[ignore]`d: it needs a real `ak_…` and a live connected GitHub account,
/// so it cannot run in CI. Run it by hand against a rig:
///
/// ```text
/// COMPOSIO_LIVE_KEY=$(cat <data-dir>/companies/<id>/secrets/%k-composio%2Fapi%5Fkey) \
///   cargo test --features composio --lib live_byok -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "needs a real Composio key and a live GitHub connection"]
async fn live_byok_lists_and_executes_against_real_composio() {
    let key = std::env::var("COMPOSIO_LIVE_KEY").expect("COMPOSIO_LIVE_KEY");
    let owner = std::env::var("LIVE_OWNER").unwrap_or_else(|_| "tinyhumansai".into());
    let repo = std::env::var("LIVE_REPO").unwrap_or_else(|_| "opencompany".into());
    let direct = DirectComposio::new(&key);

    // 1. The listing the agent discovers actions through.
    let tools = direct
        .list_tools(&["github".to_string()], None, None)
        .await
        .expect("list_tools");
    let target = tools
        .tools
        .iter()
        .find(|t| t.function.name == "GITHUB_LIST_PULL_REQUESTS")
        .expect("GITHUB_LIST_PULL_REQUESTS is in the listing");
    assert!(
        target.function.parameters.is_some(),
        "the agent needs the schema to build arguments"
    );
    println!("listed {} github tools", tools.tools.len());

    // 2. The execute the agent then makes.
    let resp = direct
        .execute(
            "GITHUB_LIST_PULL_REQUESTS",
            Some(serde_json::json!({
                "owner": owner, "repo": repo,
                "state": "open", "sort": "created", "per_page": 5
            })),
            None,
        )
        .await
        .expect("execute");
    println!("successful={} error={:?}", resp.successful, resp.error);
    let items = resp
        .data
        .get("details")
        .or_else(|| resp.data.get("data"))
        .cloned()
        .unwrap_or(resp.data.clone());
    println!(
        "{}",
        serde_json::to_string_pretty(&items).unwrap_or_default()
    );
    assert!(resp.successful, "execute failed: {:?}", resp.error);
}

#[tokio::test]
async fn a_refused_key_is_reported_without_echoing_it() {
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;

    let app = Router::new().route(
        "/toolkits",
        get(|| async { (StatusCode::UNAUTHORIZED, "Invalid API key: ak_live") }),
    );
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let direct = DirectComposio::new("ak_live").with_v3_base_for_test(format!("http://{addr}"));
    let err = format!("{:#}", direct.list_toolkits().await.expect_err("401"));
    assert!(
        err.contains("rejected this company's API key"),
        "a rejected key is named as such, not left as a bare status: {err}"
    );
    assert!(
        !err.contains("ak_live"),
        "a body that echoes the key must never reach the caller: {err}"
    );
}

// ── The draft-key probe ──────────────────────────────────────────

/// A mock Composio v3 `/toolkits` that answers `status`, recording the
/// `x-api-key` it was handed. Returns the base and the recorder.
async fn spawn_probe_backend(
    status: axum::http::StatusCode,
    body: &'static str,
) -> (String, Arc<Mutex<Vec<String>>>) {
    use axum::extract::State;
    use axum::http::HeaderMap;

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let app = axum::Router::new()
        .route(
            "/toolkits",
            axum::routing::get(
                move |State(seen): State<Arc<Mutex<Vec<String>>>>, headers: HeaderMap| async move {
                    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
                        seen.lock().unwrap().push(key.to_string());
                    }
                    (status, body)
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

/// A key Composio accepts probes clean, and the key really is what was
/// presented — a probe that authenticated as something else would report on
/// a credential the operator is not about to store.
#[tokio::test]
async fn a_key_composio_accepts_probes_clean_and_is_the_key_presented() {
    let (base, seen) = spawn_probe_backend(axum::http::StatusCode::OK, r#"{"items":[]}"#).await;
    probe_at(&base, "ak_not_a_real_key_0123456789")
        .await
        .expect("a 200 is a clean probe");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["ak_not_a_real_key_0123456789"]
    );
}

/// A rejected key comes back as a status line that classifies `auth` — and
/// the raw reason never carries the key, even when the upstream body does.
#[tokio::test]
async fn a_rejected_key_classifies_auth_without_echoing_itself() {
    use crate::company::composio_probe::{ComposioProbeClass, classify};

    let (base, _) = spawn_probe_backend(
        axum::http::StatusCode::UNAUTHORIZED,
        r#"{"error":"invalid key ak_not_a_real_key_0123456789"}"#,
    )
    .await;
    let err = probe_at(&base, "ak_not_a_real_key_0123456789")
        .await
        .expect_err("a 401 is not a clean probe");
    assert_eq!(classify(&err), ComposioProbeClass::Auth, "{err}");
    assert!(
        !err.contains("ak_not_a_real_key"),
        "the probe must not carry the key back, even when the body echoes it: {err}"
    );
}

/// A gateway in front of Composio is NOT a statement about the key: the
/// classifier has to see a non-destructive class, or a corporate proxy
/// deletes a working credential.
#[tokio::test]
async fn a_gateway_failure_is_never_read_as_a_bad_key() {
    use crate::company::composio_probe::{ComposioProbeClass, classify};

    let (base, _) =
        spawn_probe_backend(axum::http::StatusCode::BAD_GATEWAY, "<html>proxy</html>").await;
    let err = probe_at(&base, "ak_not_a_real_key_0123456789")
        .await
        .expect_err("a 502 is not a clean probe");
    assert_eq!(classify(&err), ComposioProbeClass::Unknown, "{err}");
    assert!(!classify(&err).is_destructive());
}

/// Nothing listening classifies as a connection problem, not a credential
/// one. Port 0 in a URL is never bound, so this needs no server at all.
#[tokio::test]
async fn an_unreachable_host_is_a_connection_problem_not_a_credential_one() {
    use crate::company::composio_probe::classify;

    let err = probe_at("http://127.0.0.1:1/api/v3", "ak_not_a_real_key_0123456789")
        .await
        .expect_err("nothing is listening there");
    assert!(
        !classify(&err).is_destructive(),
        "an unreachable host must never take a key away: {err} -> {}",
        classify(&err)
    );
}
