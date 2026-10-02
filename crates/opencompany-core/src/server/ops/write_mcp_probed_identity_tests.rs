//! The Connections surface's view of what an MCP server says about itself,
//! exercised end-to-end over the router against an in-process MCP server.

use axum::http::StatusCode;
use serde_json::json;

use super::write_test_support::*;

/// An in-process MCP server that reports a full `serverInfo` block, including an
/// icon. Returns the address it listens on.
async fn describing_server() -> std::net::SocketAddr {
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::Value;

    async fn handler(Json(body): Json<Value>) -> Json<Value> {
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let result = match body.get("method").and_then(Value::as_str).unwrap_or("") {
            "initialize" => json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "serverInfo": {
                    "name": "fixture",
                    "title": "Fixture Docs",
                    "description": "Up-to-date documentation for any library.",
                    "websiteUrl": "https://fixture.example",
                    "icons": [{ "src": "https://fixture.example/icon.png" }],
                },
            }),
            "tools/list" => json!({
                "tools": [{
                    "name": "search_pages",
                    "description": "Searches the docs.",
                    "inputSchema": { "type": "object" }
                }]
            }),
            _ => return Json(json!({ "jsonrpc": "2.0" })),
        };
        Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/mcp", post(handler)))
            .await
            .unwrap();
    });
    addr
}

/// Adding a server probes it, and the probed title, description and website
/// reach the console — while the description the operator declared is left
/// standing. The two are separate fields so the console can offer the server's
/// own words as a default rather than overwriting a declaration with them.
///
/// The icon this server advertises is not reachable in a test, so `iconUrl` is
/// absent and the console draws its letter tile. What must never appear is the
/// advertised URL itself: an `src=` the remote server chose is a beacon for
/// every operator who opens the page.
#[tokio::test]
async fn a_probed_server_description_reaches_the_console() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let addr = describing_server().await;

    let (status, added) = send(
        &state,
        "POST",
        "/api/v1/company/mcp/servers",
        Some(json!({
            "name": "fixture",
            "endpoint": format!("http://{addr}/mcp"),
            "description": "What the operator typed.",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(added["test"]["status"], "ok", "{added}");

    let server = &added["server"];
    assert_eq!(server["description"], "What the operator typed.");
    assert_eq!(server["probedTitle"], "Fixture Docs");
    assert_eq!(
        server["probedDescription"],
        "Up-to-date documentation for any library."
    );
    assert_eq!(server["websiteUrl"], "https://fixture.example");
    assert!(
        server.get("iconUrl").is_none(),
        "an unfetchable icon leaves the field absent: {server}"
    );

    let (status, list) = send(&state, "GET", "/api/v1/company/mcp/servers", None).await;
    assert_eq!(status, StatusCode::OK);
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "fixture")
        .expect("server present");
    assert_eq!(
        row["probedDescription"],
        "Up-to-date documentation for any library."
    );
    assert!(
        !serde_json::to_string(&list)
            .unwrap()
            .contains("fixture.example/icon.png"),
        "the advertised icon URL must never reach the console: {list}"
    );
}

/// A server that describes itself with nothing leaves every probed field absent
/// rather than resolving one to a placeholder.
#[tokio::test]
async fn a_server_that_says_nothing_reports_nothing() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, added) = send(
        &state,
        "POST",
        "/api/v1/company/mcp/servers",
        Some(json!({ "name": "dead", "endpoint": "http://127.0.0.1:1/mcp" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let server = &added["server"];
    for field in ["probedTitle", "probedDescription", "websiteUrl", "iconUrl"] {
        assert!(
            server.get(field).is_none(),
            "{field} must be absent on an unprobed server: {server}"
        );
    }
}
