//! Tests for the MCP call observer, driven by real `mcp_call_tool` results
//! from tinymcp's bridge against loopback servers, so each case reads the
//! outcome the bridge actually attaches.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse as _;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use tinymcp::tools::{ActGate, McpCallTool};
use tinytools::{Tool, ToolResult};

use super::*;
use crate::company::mcp::{AuthMaterial, McpSource};
use crate::mcp::agent::registry_from_decls;
use crate::ports::{SampleKind, UsageSample};

const SERVER: &str = "fixture";
const TOKEN: &str = "observe-Secret_42";

#[derive(Default)]
struct RecordingMeter {
    samples: StdMutex<Vec<UsageSample>>,
}

#[async_trait]
impl UsageMeter for RecordingMeter {
    async fn record(&self, _company: &CompanyId, sample: &UsageSample) -> crate::Result<()> {
        self.samples.lock().unwrap().push(sample.clone());
        Ok(())
    }
    async fn query(&self, _company: &CompanyId, _since: u64) -> crate::Result<Vec<UsageSample>> {
        Ok(self.samples.lock().unwrap().clone())
    }
}

fn decl(endpoint: &str, auth: AuthMaterial, disallowed: &[&str]) -> McpServerDecl {
    McpServerDecl {
        name: SERVER.to_string(),
        endpoint: endpoint.to_string(),
        description: None,
        allowed_tools: Vec::new(),
        disallowed_tools: disallowed.iter().map(|tool| tool.to_string()).collect(),
        read_only_tools: Vec::new(),
        timeout_secs: 5,
        enabled: true,
        source: McpSource::Runtime,
        auth,
        tool_policies: Default::default(),
        tool_inventory: Default::default(),
    }
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/mcp")
}

/// A server that answers `ok` and `remote_error`, and rejects `rpc` with a
/// JSON-RPC error.
async fn answering_server() -> String {
    async fn handle(Json(body): Json<Value>) -> axum::response::Response {
        let id = body["id"].clone();
        let result = match body["method"].as_str().unwrap_or_default() {
            "initialize" => json!({
                "protocolVersion": "2025-11-25",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fixture", "version": "1" },
            }),
            "notifications/initialized" => return StatusCode::ACCEPTED.into_response(),
            "tools/list" => json!({ "tools": [
                { "name": "ok", "inputSchema": { "type": "object" } },
                { "name": "remote_error", "inputSchema": { "type": "object" } },
                { "name": "rpc", "inputSchema": { "type": "object" } },
                { "name": "blocked", "inputSchema": { "type": "object" } }
            ]}),
            "tools/call" => match body["params"]["name"].as_str().unwrap_or_default() {
                "remote_error" => json!({
                    "content": [{ "type": "text", "text": "no such page" }],
                    "isError": true,
                }),
                "rpc" => {
                    return Json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32602, "message": "bad cursor" }
                    }))
                    .into_response();
                }
                _ => json!({ "content": [{ "type": "text", "text": "fine" }] }),
            },
            _ => json!({}),
        };
        Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
    }
    serve(Router::new().route("/mcp", post(handle))).await
}

async fn unauthorized_server(resource_metadata: Option<&'static str>) -> String {
    let app = Router::new().route(
        "/mcp",
        post(move || async move {
            let challenge = resource_metadata.map_or_else(
                || "Bearer realm=\"mcp\"".to_string(),
                |url| format!("Bearer resource_metadata=\"{url}\""),
            );
            let mut headers = HeaderMap::new();
            headers.insert(
                "www-authenticate",
                HeaderValue::from_str(&challenge).unwrap(),
            );
            (StatusCode::UNAUTHORIZED, headers, "").into_response()
        }),
    );
    serve(app).await
}

async fn closed_endpoint() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}/mcp")
}

async fn call(decl: &McpServerDecl, tool: &str) -> ToolResult {
    let gate: ActGate = Arc::new(|_| Ok(()));
    let bridge = McpCallTool::new(
        Arc::new(registry_from_decls(std::slice::from_ref(decl))),
        gate,
    );
    bridge
        .execute(json!({ "server": SERVER, "tool": tool, "arguments": {} }))
        .await
        .expect("the bridge returns a result")
}

fn completed(result: &ToolResult) -> AgentProgress {
    let output = result.output();
    AgentProgress::ToolCallCompleted {
        call_id: "call-1".to_string(),
        tool_name: "mcp_call_tool".to_string(),
        success: !result.is_error,
        output_chars: output.chars().count(),
        output,
        arguments: None,
        elapsed_ms: 1,
        iteration: 1,
        failure: None,
        display_label: None,
        display_detail: None,
        structured: result.metadata.clone(),
    }
}

struct Fixture {
    sink: McpCallObserver,
    meter: Arc<RecordingMeter>,
    observer: AgentMcpObserver,
}

fn fixture(decl: &McpServerDecl) -> Fixture {
    let sink = McpCallObserver::default();
    let meter = Arc::new(RecordingMeter::default());
    let observer = sink.for_agent(
        CompanyId::new("acme"),
        "writer",
        Some(meter.clone() as Arc<dyn UsageMeter>),
        vec![decl.clone()],
    );
    Fixture {
        sink,
        meter,
        observer,
    }
}

async fn observe_call(decl: &McpServerDecl, tool: &str) -> (Fixture, Vec<McpFailure>) {
    let result = call(decl, tool).await;
    let fixture = fixture(decl);
    let failures = fixture.observer.observe(&[completed(&result)]).await;
    (fixture, failures)
}

#[tokio::test]
async fn an_answered_call_is_metered_and_records_no_failure() {
    let decl = decl(&answering_server().await, AuthMaterial::None, &[]);
    let (fixture, failures) = observe_call(&decl, "ok").await;
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(fixture.sink.queued(), 0);
    let samples = fixture.meter.samples.lock().unwrap().clone();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].kind, SampleKind::OauthCall);
    assert_eq!(samples[0].agent, "writer");
    assert_eq!(samples[0].provider, crate::metering::mcp_provider(SERVER));
}

#[tokio::test]
async fn a_remote_tool_error_is_still_an_answered_call() {
    let decl = decl(&answering_server().await, AuthMaterial::None, &[]);
    let (fixture, failures) = observe_call(&decl, "remote_error").await;
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(fixture.meter.samples.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_blocked_tool_is_a_refusal_not_a_failure() {
    let decl = decl(&answering_server().await, AuthMaterial::None, &["blocked"]);
    let result = call(&decl, "blocked").await;
    let outcome = McpCallOutcome::from_metadata(result.metadata.as_ref().unwrap()).unwrap();
    assert_eq!(
        outcome.error.unwrap().code,
        tinymcp::tinymcp_bus::errors::TOOL_NOT_ALLOWED
    );
    let fixture = fixture(&decl);
    assert!(
        fixture
            .observer
            .observe(&[completed(&result)])
            .await
            .is_empty()
    );
    assert_eq!(fixture.sink.queued(), 0);
    assert!(fixture.meter.samples.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_401_with_no_credential_needs_one() {
    let decl = decl(&unauthorized_server(None).await, AuthMaterial::None, &[]);
    let (fixture, failures) = observe_call(&decl, "ok").await;
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].server, SERVER);
    assert_eq!(failures[0].tool, "ok");
    assert_eq!(failures[0].status, "credential_required");
    assert_eq!(failures[0].hint.as_deref(), Some("credential_required"));
    assert_eq!(fixture.sink.drain(), failures);
    assert!(fixture.meter.samples.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_401_against_a_stored_credential_rejects_it_without_echoing_it() {
    let decl = decl(
        &unauthorized_server(None).await,
        AuthMaterial::Bearer(TOKEN.to_string()),
        &[],
    );
    let (_fixture, failures) = observe_call(&decl, "ok").await;
    assert_eq!(failures[0].status, "token_rejected");
    assert!(!failures[0].scrubbed_message.contains(TOKEN));
}

#[tokio::test]
async fn a_401_advertising_oauth_asks_for_sign_in() {
    let endpoint = unauthorized_server(Some("https://auth.test/.well-known/res")).await;
    let decl = decl(&endpoint, AuthMaterial::None, &[]);
    let (_fixture, failures) = observe_call(&decl, "ok").await;
    assert_eq!(failures[0].status, "oauth_required");
    assert_eq!(failures[0].hint.as_deref(), Some("oauth_required"));
    assert!(failures[0].scrubbed_message.contains("Sign in"));
}

#[tokio::test]
async fn an_unreachable_server_is_a_transport_failure() {
    let decl = decl(&closed_endpoint().await, AuthMaterial::None, &[]);
    let (_fixture, failures) = observe_call(&decl, "ok").await;
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].status, "unreachable");
    assert_eq!(failures[0].hint, None);
}

#[tokio::test]
async fn a_json_rpc_error_is_a_rejected_call_naming_the_reason() {
    let decl = decl(&answering_server().await, AuthMaterial::None, &[]);
    let (_fixture, failures) = observe_call(&decl, "rpc").await;
    assert_eq!(failures[0].status, "tool_call_rejected");
    assert!(
        failures[0].scrubbed_message.contains("bad cursor"),
        "{}",
        failures[0].scrubbed_message
    );
}

#[tokio::test]
async fn a_failure_reflecting_the_credential_is_scrubbed() {
    let decl = decl(
        "https://unused.test/mcp",
        AuthMaterial::Bearer(TOKEN.to_string()),
        &[],
    );
    let outcome = McpCallOutcome::failed(
        SERVER,
        "ok",
        tinymcp::McpCallError::new(tinymcp::tinymcp_bus::errors::RPC),
    );
    let event = AgentProgress::ToolCallCompleted {
        call_id: "c".to_string(),
        tool_name: "mcp_call_tool".to_string(),
        success: false,
        output_chars: 0,
        output: format!("mcp_call_tool failed: mcp error response: token {TOKEN} refused"),
        arguments: None,
        elapsed_ms: 1,
        iteration: 1,
        failure: None,
        display_label: None,
        display_detail: None,
        structured: serde_json::to_value(&outcome).ok(),
    };
    let failures = fixture(&decl).observer.observe(&[event]).await;
    assert!(!failures[0].scrubbed_message.contains(TOKEN));
}

#[tokio::test]
async fn subagent_calls_are_read_and_other_events_ignored() {
    let decl = decl(&unauthorized_server(None).await, AuthMaterial::None, &[]);
    let result = call(&decl, "ok").await;
    let events = vec![
        AgentProgress::TextDelta {
            delta: "hello".to_string(),
            iteration: 1,
        },
        AgentProgress::ToolCallCompleted {
            call_id: "plain".to_string(),
            tool_name: "read".to_string(),
            success: true,
            output_chars: 2,
            output: "ok".to_string(),
            arguments: None,
            elapsed_ms: 1,
            iteration: 1,
            failure: None,
            display_label: None,
            display_detail: None,
            structured: Some(json!({ "kind": "web_search" })),
        },
        AgentProgress::SubagentToolCallCompleted {
            agent_id: "child".to_string(),
            task_id: "t".to_string(),
            call_id: "sub".to_string(),
            tool_name: "mcp_call_tool".to_string(),
            success: false,
            output_chars: 0,
            output: result.output(),
            arguments: None,
            elapsed_ms: 1,
            iteration: 1,
            failure: None,
            display_label: None,
            display_detail: None,
            structured: result.metadata.clone(),
        },
    ];
    let fixture = fixture(&decl);
    let failures = fixture.observer.observe(&events).await;
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].status, "credential_required");
}

#[tokio::test]
async fn every_agent_records_into_the_one_company_observer() {
    let decl = decl(&unauthorized_server(None).await, AuthMaterial::None, &[]);
    let result = call(&decl, "ok").await;
    let sink = McpCallObserver::default();
    let writer = sink.for_agent(CompanyId::new("acme"), "writer", None, vec![decl.clone()]);
    let engineer = sink.for_agent(CompanyId::new("acme"), "engineer", None, vec![decl]);
    writer.observe(&[completed(&result)]).await;
    engineer.observe(&[completed(&result)]).await;
    assert_eq!(sink.queued(), 2);
    sink.clear();
    assert!(sink.drain().is_empty());
}

#[tokio::test]
async fn an_off_observer_meters_and_records_nothing_visible() {
    let decl = decl(&unauthorized_server(None).await, AuthMaterial::None, &[]);
    let result = call(&decl, "ok").await;
    let failures = AgentMcpObserver::off().observe(&[completed(&result)]).await;
    assert_eq!(failures.len(), 1);
}

#[tokio::test]
async fn a_credential_typed_as_the_server_or_tool_name_is_scrubbed() {
    let decl = decl(
        "https://unused.test/mcp",
        AuthMaterial::Bearer(TOKEN.to_string()),
        &[],
    );
    let outcome = McpCallOutcome::failed(
        TOKEN,
        format!("{TOKEN}-tool"),
        tinymcp::McpCallError::new(tinymcp::tinymcp_bus::errors::RPC),
    );
    let event = AgentProgress::ToolCallCompleted {
        call_id: "c".to_string(),
        tool_name: "mcp_call_tool".to_string(),
        success: false,
        output_chars: 0,
        output: format!("mcp_call_tool failed: mcp error response: no server {TOKEN}"),
        arguments: None,
        elapsed_ms: 1,
        iteration: 1,
        failure: None,
        display_label: None,
        display_detail: None,
        structured: serde_json::to_value(&outcome).ok(),
    };
    let failures = fixture(&decl).observer.observe(&[event]).await;
    assert_eq!(failures.len(), 1);
    assert!(
        !failures[0].server.contains(TOKEN),
        "{}",
        failures[0].server
    );
    assert!(!failures[0].tool.contains(TOKEN), "{}", failures[0].tool);
    assert!(!failures[0].scrubbed_message.contains(TOKEN));
}
