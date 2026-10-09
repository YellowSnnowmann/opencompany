use super::*;

use std::sync::Arc;

use serde_json::json;
use tinytools::Tool;

/// Shared with [`super::blocked_tests`], which builds the same declarations and
/// then attaches a policy to them.
pub(super) fn decl(name: &str, endpoint: &str) -> McpServerDecl {
    McpServerDecl {
        name: name.to_string(),
        endpoint: endpoint.to_string(),
        description: None,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        read_only_tools: Vec::new(),
        timeout_secs: 30,
        enabled: true,
        source: crate::company::mcp::McpSource::Runtime,
        auth: AuthMaterial::None,
        tool_policies: Default::default(),
        tool_inventory: Default::default(),
    }
}

pub(super) fn grants(g: &[&str]) -> Vec<String> {
    g.iter().map(|s| s.to_string()).collect()
}

/// The `mcp_call_tool` a company agent runs: upstream's own, over the declared
/// servers attached to its spec, with an act gate that admits every call.
fn native_call_tool(decls: &[McpServerDecl]) -> tinymcp::tools::McpCallTool {
    tinymcp::tools::McpCallTool::new(
        Arc::new(registry_from_decls(decls)),
        Arc::new(|_: &str| Ok(())),
    )
}

#[test]
fn gitbooks_default_server_never_leaks_in() {
    // OpenHuman's Config::default seeds a `gitbooks` server; the registry we
    // build for a tenant agent must NOT contain it.
    let decls = vec![decl("notion", "https://notion.example/mcp")];
    let reg = registry_from_decls(&decls);
    assert!(reg.get("notion").is_some());
    assert!(reg.get("gitbooks").is_none(), "gitbooks must not leak in");
}

#[test]
fn auth_material_maps_onto_transport_config() {
    let bearer = auth_config(&AuthMaterial::Bearer("tok".into()));
    assert!(matches!(bearer, McpAuthConfig::BearerToken { .. }));
    // An OAuth credential resolves to the same bearer path, carrying exactly
    // its (already-refreshed) access token and nothing else.
    let oauth = auth_config(&AuthMaterial::OAuth {
        access_token: "at".into(),
        refresh_token: None,
        client_id: "cid".into(),
        client_secret: None,
        token_endpoint: "https://as.example/token".into(),
        expires_at: 0,
    });
    assert!(matches!(oauth, McpAuthConfig::BearerToken { token } if token == "at"));
    let header = auth_config(&AuthMaterial::Header {
        name: "X-Key".into(),
        value: "v".into(),
    });
    assert!(matches!(header, McpAuthConfig::Header { .. }));
    let query = auth_config(&AuthMaterial::QueryParam {
        name: "apiKey".into(),
        value: "qp".into(),
    });
    assert!(matches!(query, McpAuthConfig::QueryParam { .. }));
    assert!(matches!(
        auth_config(&AuthMaterial::None),
        McpAuthConfig::None
    ));
}

#[tokio::test]
async fn upstream_list_servers_tool_never_emits_a_credential() {
    let mut d = decl("notion", "https://notion.example/mcp");
    d.auth = AuthMaterial::Bearer("sk-super-secret-token".into());
    let reg = Arc::new(registry_from_decls(&[d]));
    // Upstream's tool, which this host relies on rather than replacing: the
    // guarantee is still this host's to check.
    let tool = tinymcp::tools::McpListServersTool::new(reg);
    let result = tool.execute(json!({})).await.expect("execute");

    // The whole serialized result (JSON + markdown) must not carry the token.
    let json_out = serde_json::to_string(&result).unwrap();
    assert!(
        !json_out.contains("sk-super-secret-token"),
        "list-servers output leaked a credential: {json_out}"
    );
    // But it still reports the server + that auth is configured.
    assert!(json_out.contains("notion"));
    assert!(json_out.contains("auth_configured"));
}

/// End-to-end: drive `mcp_call_tool` against an in-process axum MCP server
/// (plain JSON `initialize` / `tools/list` / `tools/call`, no new deps). The
/// bearer token reaches the *server* over the wire (auth is wired), but the
/// agent-visible `ToolResult` never carries it. This is the regression guard
/// for the "credentials never surface to the agent" invariant.
#[tokio::test]
async fn call_tool_through_agent_path_never_leaks_bearer() {
    use std::sync::Mutex as StdMutex;

    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};

    #[derive(Default)]
    struct Seen {
        auth: StdMutex<Option<String>>,
    }

    async fn handler(
        State(seen): State<Arc<Seen>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
            *seen.auth.lock().unwrap() = Some(auth.to_string());
        }
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body.get("method").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "serverInfo": { "name": "fixture", "version": "0" }
            }),
            "tools/list" => json!({
                "tools": [{
                    "name": "echo",
                    "description": "Echoes input.",
                    "inputSchema": { "type": "object" }
                }]
            }),
            "tools/call" => json!({
                "content": [{ "type": "text", "text": "remote ran ok, no secrets here" }],
                "isError": false
            }),
            // A notification (e.g. notifications/initialized) — ack only.
            _ => return Json(json!({ "jsonrpc": "2.0" })),
        };
        Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    let seen = Arc::new(Seen::default());
    let app = Router::new()
        .route("/mcp", post(handler))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let endpoint = format!("http://{addr}/mcp");
    let mut d = decl("fixture", &endpoint);
    d.auth = AuthMaterial::Bearer("sk-super-secret-xyz".into());
    let tool = native_call_tool(&[d]);

    let result = tool
        .execute(json!({ "server": "fixture", "tool": "echo", "arguments": {} }))
        .await
        .expect("mcp_call_tool");

    // Auth WAS wired: the server received the bearer over the wire.
    assert_eq!(
        seen.auth.lock().unwrap().as_deref(),
        Some("Bearer sk-super-secret-xyz"),
        "the transport must send the configured bearer"
    );
    // But the agent-visible result never carries the token.
    let out = serde_json::to_string(&result).unwrap();
    assert!(
        !out.contains("sk-super-secret-xyz"),
        "mcp_call_tool result leaked a credential: {out}"
    );
    assert!(result.output().contains("remote ran ok"));
}

/// SECURITY CANARY: a server that **reflects the submitted credential** in a
/// non-401 error body must not leak it into the agent-visible result of the
/// `mcp_call_tool` an agent actually runs. This is the regression guard for leak vector #1 (upstream `MCP HTTP {status} —
/// {body}` echoing the body) driven through the REAL vendored transport.
#[tokio::test]
async fn native_call_tool_scrubs_reflected_credential() {
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};

    // On tools/call, reflect the Authorization header back in a 500 body — the
    // exact hostile shape that would leak the token through upstream's
    // `MCP HTTP 500 — {body}` surfacing.
    async fn handler(
        State(()): State<()>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body.get("method").and_then(Value::as_str).unwrap_or("");
        match method {
            "initialize" => Json(json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "protocolVersion": "2025-11-25", "capabilities": {},
                            "serverInfo": { "name": "fixture", "version": "0" } }
            }))
            .into_response(),
            "tools/list" => Json(json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "tools": [{ "name": "echo", "description": "e",
                                        "inputSchema": { "type": "object" } }] }
            }))
            .into_response(),
            "tools/call" => {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("boom — received {auth}"),
                )
                    .into_response()
            }
            _ => Json(json!({ "jsonrpc": "2.0" })).into_response(),
        }
    }

    let app = Router::new().route("/mcp", post(handler)).with_state(());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    const CANARY: &str = "sk-canary-REFLECTED-9999";
    let endpoint = format!("http://{addr}/mcp");
    let mut d = decl("fixture", &endpoint);
    d.auth = AuthMaterial::Bearer(CANARY.into());
    let tool = native_call_tool(&[d]);

    let result = tool
        .execute(json!({ "server": "fixture", "tool": "echo", "arguments": {} }))
        .await
        .expect("mcp_call_tool");

    // The agent-visible result is an error, but carries NO canary.
    assert!(result.is_error, "a failed call must be an error result");
    let out = serde_json::to_string(&result).unwrap();
    assert!(
        !out.contains(CANARY),
        "mcp_call_tool result leaked the reflected credential: {out}"
    );
}

/// A *successful* response that reflects the credential is scrubbed too.
///
/// A server echoing the bearer inside an ordinary `tools/call` result would
/// otherwise hand it to the agent verbatim.
#[tokio::test]
async fn native_call_tool_scrubs_a_credential_reflected_in_a_successful_result() {
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};

    async fn handler(headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body.get("method").and_then(Value::as_str).unwrap_or("");
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Json(match method {
            "initialize" => json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "protocolVersion": "2025-11-25", "capabilities": {},
                            "serverInfo": { "name": "fixture", "version": "0" } }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "tools": [{ "name": "echo", "description": "e",
                                        "inputSchema": { "type": "object" } }] }
            }),
            _ => json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "content": [{ "type": "text", "text": format!("ok — you sent {auth}") }] }
            }),
        })
    }

    let app = Router::new().route("/mcp", post(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    const CANARY: &str = "sk-canary-SUCCESS-4242";
    let mut d = decl("fixture", &format!("http://{addr}/mcp"));
    d.auth = AuthMaterial::Bearer(CANARY.into());
    let tool = native_call_tool(&[d]);

    let result = tool
        .execute(json!({ "server": "fixture", "tool": "echo", "arguments": {} }))
        .await
        .expect("mcp_call_tool");
    assert!(!result.is_error, "the call succeeded");
    let out = serde_json::to_string(&result).unwrap();
    assert!(
        out.contains("ok — you sent"),
        "the result still carries the reply: {out}"
    );
    assert!(
        !out.contains(CANARY),
        "a successful result leaked the credential: {out}"
    );
}

/// The query-parameter credential is **appended** to an endpoint that already
/// carries a (non-secret) query string, reaching the server on the wire —
/// the BrowserBase shape (`?projectId=…` in the URL, `apiKey` as the
/// credential). Proves the upstream `request.query()` path composes rather
/// than replaces, and that our mapping wires it.
#[tokio::test]
async fn query_param_auth_appends_to_existing_query_on_the_wire() {
    use std::sync::Mutex as StdMutex;

    use axum::extract::State;
    use axum::http::Uri;
    use axum::routing::post;
    use axum::{Json, Router};

    #[derive(Default)]
    struct Seen {
        query: StdMutex<Option<String>>,
    }

    async fn handler(
        State(seen): State<Arc<Seen>>,
        uri: Uri,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        if let Some(q) = uri.query() {
            *seen.query.lock().unwrap() = Some(q.to_string());
        }
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body.get("method").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2025-11-25", "capabilities": {},
                "serverInfo": { "name": "fixture", "version": "0" }
            }),
            "tools/list" => json!({ "tools": [] }),
            _ => return Json(json!({ "jsonrpc": "2.0" })),
        };
        Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    let seen = Arc::new(Seen::default());
    let app = Router::new()
        .route("/mcp", post(handler))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // The non-secret project id stays in the endpoint; the secret rides as a
    // query-parameter credential.
    let endpoint = format!("http://{addr}/mcp?projectId=pid-123");
    let mut d = decl("browserbase", &endpoint);
    d.auth = AuthMaterial::QueryParam {
        name: "apiKey".into(),
        value: "qp-secret-abc".into(),
    };
    let registry = registry_from_decls(&[d]);
    // list_tools drives initialize + tools/list over the wire.
    let _ = registry
        .list_tools("browserbase")
        .await
        .expect("list_tools");

    let query = seen
        .query
        .lock()
        .unwrap()
        .clone()
        .expect("server saw a query");
    assert!(
        query.contains("projectId=pid-123"),
        "kept the existing id: {query}"
    );
    assert!(
        query.contains("apiKey=qp-secret-abc"),
        "appended the credential: {query}"
    );
}

/// A declared server is inspected by name. No company agent is scoped to list
/// the configured servers, so the brief must not send one looking for that tool.
#[test]
fn a_declared_only_agent_is_pointed_at_the_declared_enumeration_tools() {
    let brief = capability_brief(true, false);
    assert!(brief.contains("mcp_list_tools"), "{brief}");
    assert!(
        !brief.contains("mcp_registry_installed_list"),
        "it holds no registry tool, so naming one sends it at a tool it cannot see: {brief}"
    );
}

/// The registry family inspects through different tools, and an agent holding
/// only that family used to be told nothing at all.
#[test]
fn a_registry_only_agent_is_pointed_at_the_registry_enumeration_tools() {
    let brief = capability_brief(false, true);
    assert!(
        !brief.is_empty(),
        "a registry-only agent still gets the brief"
    );
    assert!(brief.contains("mcp_registry_installed_list"), "{brief}");
    assert!(brief.contains("mcp_registry_list_tools"), "{brief}");
}

/// Both families wired names both inspection paths.
#[test]
fn an_agent_holding_both_families_is_pointed_at_both() {
    let brief = capability_brief(true, true);
    assert!(brief.contains("mcp_list_tools"), "{brief}");
    assert!(brief.contains("mcp_registry_installed_list"), "{brief}");
}

/// No MCP family wired means no brief, rather than one naming nothing.
#[test]
fn an_agent_with_no_mcp_family_gets_no_capability_brief() {
    assert_eq!(capability_brief(false, false), "");
}

/// `mcp_list_servers` answers with each server's credentials and is kept out of
/// every company agent's scope. Whichever families are wired, the brief must
/// never send an agent to it — the one assertion that has to hold for all four
/// combinations, so it is made for all four.
#[test]
fn no_wiring_combination_sends_an_agent_to_the_server_listing() {
    for (declared, registry) in [(true, true), (true, false), (false, true), (false, false)] {
        let brief = capability_brief(declared, registry);
        assert!(
            !brief.contains("mcp_list_servers"),
            "declared={declared} registry={registry}: {brief}"
        );
    }
}
