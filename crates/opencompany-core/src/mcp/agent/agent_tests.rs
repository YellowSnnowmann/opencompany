use super::*;

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

#[test]
fn empty_decls_yield_no_registry() {
    assert!(registry_for_agent(&[], &grants(&["mcp:*"])).is_none());
}

#[test]
fn server_name_strips_markdown_fences() {
    // Models wrap identifiers in markdown when answering in prose style;
    // the fence characters belong to the answer, not the server name
    // (seen live: server="werkplaats`" -> "unknown mcp server `werkplaats``").
    let mk = |v: &str| serde_json::json!({ "server": v });
    let parsed = |v: &str| required_string_arg(&mk(v), "server").unwrap();
    assert_eq!(parsed("werkplaats"), "werkplaats");
    assert_eq!(parsed("werkplaats`"), "werkplaats");
    assert_eq!(parsed("`werkplaats`"), "werkplaats");
    assert_eq!(parsed("*werkplaats*"), "werkplaats");
    assert_eq!(parsed("werkplaats."), "werkplaats");
    assert_eq!(parsed("werk"), "werk");
    assert!(required_string_arg(&mk("```"), "server").is_err());
}

#[test]
fn ungranted_agent_gets_no_registry() {
    let decls = vec![decl("notion", "https://notion.example/mcp")];
    // No mcp grant at all.
    assert!(registry_for_agent(&decls, &grants(&["email.send"])).is_none());
}

#[test]
fn wildcard_grant_admits_all_enabled_servers() {
    let decls = vec![
        decl("notion", "https://notion.example/mcp"),
        decl("linear", "https://linear.example/mcp"),
    ];
    let reg = registry_for_agent(&decls, &grants(&["mcp:*"])).expect("registry");
    let mut names: Vec<&str> = reg.list().iter().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["linear", "notion"]);
}

#[test]
fn named_grant_scopes_to_that_server() {
    let decls = vec![
        decl("notion", "https://notion.example/mcp"),
        decl("linear", "https://linear.example/mcp"),
    ];
    let reg = registry_for_agent(&decls, &grants(&["mcp:notion"])).expect("registry");
    let names: Vec<&str> = reg.list().iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["notion"]);
}

#[test]
fn disabled_server_is_excluded() {
    let mut d = decl("notion", "https://notion.example/mcp");
    d.enabled = false;
    assert!(registry_for_agent(&[d], &grants(&["mcp:*"])).is_none());
}

#[test]
fn gitbooks_default_server_never_leaks_in() {
    // OpenHuman's Config::default seeds a `gitbooks` server; the registry we
    // build for a tenant agent must NOT contain it.
    let decls = vec![decl("notion", "https://notion.example/mcp")];
    let reg = registry_for_agent(&decls, &grants(&["mcp:*"])).expect("registry");
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
    let reg = registry_for_agent(&[d], &grants(&["mcp:*"])).expect("registry");
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
    use oh::security::SecurityPolicy;

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
    let registry = registry_for_agent(&[d], &grants(&["mcp:*"])).expect("registry");
    let tool = OcMcpCallTool::new(
        registry,
        Arc::new(SecurityPolicy::default()),
        vec!["sk-super-secret-xyz".into()],
        McpFailureQueue::default(),
        McpMetering::off(),
        Default::default(),
    );

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

/// An empty raw request inherits the company belt at the builder seam. The
/// scrubber must receive those effective grants too, or an MCP credential
/// echoed by a server can reach the agent-visible failure even though the
/// registry correctly wires that server.
#[test]
fn granted_secrets_follows_effective_grants() {
    let mut server = decl("fixture", "http://127.0.0.1:1/mcp");
    server.auth = AuthMaterial::Bearer("inherited-canary".into());
    let inherited = granted_secrets(std::slice::from_ref(&server), &grants(&["*", "mcp:*"]));
    assert_eq!(inherited, vec!["inherited-canary"]);

    let omitted = granted_secrets(std::slice::from_ref(&server), &grants(&["*"]));
    assert!(omitted.is_empty());
}

/// SECURITY CANARY: a server that **reflects the submitted credential** in a
/// non-401 error body must not leak it anywhere the `OcMcpCallTool` decorator
/// surfaces — not the agent-visible result, and not the drained failure. This
/// is the regression guard for leak vector #1 (upstream `MCP HTTP {status} —
/// {body}` echoing the body) driven through the REAL vendored transport.
#[tokio::test]
async fn oc_call_tool_scrubs_reflected_credential() {
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};
    use oh::security::SecurityPolicy;

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
    let secrets = granted_secrets(std::slice::from_ref(&d), &grants(&["mcp:*"]));
    let registry = registry_for_agent(&[d], &grants(&["mcp:*"])).expect("registry");

    let queue = McpFailureQueue::default();
    let tool = OcMcpCallTool::new(
        registry,
        Arc::new(SecurityPolicy::default()),
        secrets,
        queue.clone(),
        McpMetering::off(),
        Default::default(),
    );

    let result = tool
        .execute(json!({ "server": "fixture", "tool": "echo", "arguments": {} }))
        .await
        .expect("mcp_call_tool");

    // The agent-visible result is an error, but carries NO canary.
    assert!(result.is_error, "a failed call must be an error result");
    let out = serde_json::to_string(&result).unwrap();
    assert!(
        !out.contains(CANARY),
        "OcMcpCallTool result leaked the reflected credential: {out}"
    );

    // The drained failure is recorded, classified, and scrubbed.
    let failures = queue.drain();
    assert_eq!(failures.len(), 1, "the failure was queued");
    assert_eq!(failures[0].server, "fixture");
    assert_eq!(failures[0].status, "server_error");
    let serialized = format!("{:?}", failures[0]);
    assert!(
        !serialized.contains(CANARY),
        "the drained failure leaked the reflected credential: {serialized}"
    );
}

/// A *successful* response that reflects the credential is scrubbed too.
///
/// The failure path above was the only one scrubbed until tinymcp's
/// `SecretScrubber` was applied to the success branch: a server echoing the
/// bearer inside an ordinary `tools/call` result would otherwise hand it to the
/// agent verbatim.
#[tokio::test]
async fn oc_call_tool_scrubs_a_credential_reflected_in_a_successful_result() {
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};
    use oh::security::SecurityPolicy;

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
    let secrets = granted_secrets(std::slice::from_ref(&d), &grants(&["mcp:*"]));
    let registry = registry_for_agent(&[d], &grants(&["mcp:*"])).expect("registry");
    let tool = OcMcpCallTool::new(
        registry,
        Arc::new(SecurityPolicy::default()),
        secrets,
        McpFailureQueue::default(),
        McpMetering::off(),
        Default::default(),
    );

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

/// A completed MCP call is counted, and a failed one is not (issue #698).
///
/// The rule this exercises — `mcp:` namespacing — is unit-tested in
/// `crate::metering::oauth`. What only this test can reach is the wiring:
/// that the success branch calls the meter at all, that it passes *this*
/// company and agent rather than a default, and that the failure branch
/// stays silent. Deleting the `if let Some(meter)` block, moving it to the
/// `Err` arm, or threading the wrong field all pass every other test in the
/// tree.
///
/// Both outcomes are driven through one fixture whose `tools/call` succeeds
/// or fails on the tool name, because "counts a success" is only half the
/// contract: `connections` is the count of providers seen, so a metered
/// failure would mint a connection row for a server that never answered.
#[tokio::test]
async fn a_completed_mcp_call_is_metered_and_a_failed_one_is_not() {
    use axum::extract::State;
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::Mutex;

    use crate::ports::usage::{SampleKind, UsageMeter, UsageSample};

    #[derive(Default)]
    struct RecordingMeter {
        samples: Mutex<Vec<(String, UsageSample)>>,
    }

    #[async_trait]
    impl UsageMeter for RecordingMeter {
        async fn record(&self, company: &CompanyId, sample: &UsageSample) -> crate::Result<()> {
            self.samples
                .lock()
                .unwrap()
                .push((company.to_string(), sample.clone()));
            Ok(())
        }
        async fn query(
            &self,
            _company: &CompanyId,
            _since: u64,
        ) -> crate::Result<Vec<UsageSample>> {
            Ok(Vec::new())
        }
    }

    async fn handler(State(()): State<()>, Json(body): Json<Value>) -> axum::response::Response {
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
                "result": { "tools": [
                    { "name": "echo", "description": "e", "inputSchema": { "type": "object" } },
                    { "name": "boom", "description": "b", "inputSchema": { "type": "object" } }
                ] }
            }))
            .into_response(),
            "tools/call" => {
                let called = body
                    .get("params")
                    .and_then(|p| p.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if called == "boom" {
                    return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response();
                }
                Json(json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": { "content": [{ "type": "text", "text": "ok" }] }
                }))
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

    let endpoint = format!("http://{addr}/mcp");
    let registry =
        registry_for_agent(&[decl("fixture", &endpoint)], &grants(&["mcp:*"])).expect("registry");

    let meter = Arc::new(RecordingMeter::default());
    let tool = OcMcpCallTool::new(
        registry,
        Arc::new(SecurityPolicy::default()),
        Vec::new(),
        McpFailureQueue::default(),
        McpMetering {
            company: CompanyId::new("acme"),
            agent: "ceo".to_string(),
            meter: Some(meter.clone()),
        },
        Default::default(),
    );

    let ok = tool
        .execute(json!({ "server": "fixture", "tool": "echo", "arguments": {} }))
        .await
        .expect("mcp_call_tool");
    assert!(!ok.is_error, "the fixture's `echo` succeeds: {ok:?}");

    {
        let samples = meter.samples.lock().unwrap();
        assert_eq!(samples.len(), 1, "one completed call, one sample");
        let (company, sample) = &samples[0];
        assert_eq!(company, "acme", "the sample is scoped to this company");
        assert_eq!(sample.agent, "ceo", "attributed to the calling agent");
        assert_eq!(sample.kind, SampleKind::OauthCall);
        // Namespaced, so this row cannot merge with a Composio toolkit that
        // happens to share the server's name.
        assert_eq!(sample.provider, "mcp:fixture");
        assert_eq!(sample.input_tokens, 0);
        assert_eq!(sample.output_tokens, 0);
        assert_eq!(sample.cost_usd, 0.0);
    }

    let failed = tool
        .execute(json!({ "server": "fixture", "tool": "boom", "arguments": {} }))
        .await
        .expect("mcp_call_tool");
    assert!(failed.is_error, "the fixture's `boom` fails: {failed:?}");
    assert_eq!(
        meter.samples.lock().unwrap().len(),
        1,
        "a call that never reached the server must not mint a connection row"
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
    let registry = registry_for_agent(&[d], &grants(&["mcp:*"])).expect("registry");
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
