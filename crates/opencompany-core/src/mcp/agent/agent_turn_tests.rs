//! A per-agent block on a declared server's tool, proven on live harness
//! turns.
//!
//! Drives the real `HarnessPool`, `build_agent` and `HostedProvider` on the
//! native tool-calling path, against two loopback stubs: a scripted
//! OpenAI-compatible model and a streamable-HTTP MCP server that counts every
//! `tools/call` it receives. The agent reaches the server only through
//! OpenHuman's own `mcp_call_tool` over the server attached to its spec, so the
//! attached deny list is the whole enforcement.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::routing::post;
use serde_json::{Value, json};

use crate::company::CompanyManifest;
use crate::company::credentials::Credential;
use crate::harness::orchestrator::{DelegationQueue, WorkflowRunnerHandle};
use crate::harness::policy::ApprovalRequestQueue;
use crate::harness::provider::{HostedProvider, HostedProviderConfig};
use crate::harness::{HarnessDeps, HarnessPool};
use crate::mcp::policy::{
    AgentToolPolicies, ApprovalMode, McpToolInventory, McpToolPolicies, ToolPolicy, ToolTier,
    save_tool_inventory, save_tool_policies, tool_inventory_key, tool_policies_key,
};
use crate::mcp::probe::McpFailureQueue;
use crate::ports::SecretStore;
use crate::ports::types::{CompanyId, CompanyRecord, SecretValue};
use crate::runtime::delegation::ChatTarget;
use crate::store::FsCompanyStore;

const SERVER: &str = "fixture";
const BLOCKED: &str = "delete_page";
const ALLOWED: &str = "search_pages";

#[derive(Default)]
struct MemorySecrets {
    map: Mutex<HashMap<String, String>>,
}

#[async_trait::async_trait]
impl SecretStore for MemorySecrets {
    async fn get(&self, _c: &CompanyId, key: &str) -> crate::Result<Option<SecretValue>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(key)
            .map(|v| SecretValue(v.clone())))
    }
    async fn set(&self, _c: &CompanyId, key: &str, value: SecretValue) -> crate::Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.0);
        Ok(())
    }
}

/// The model: on a user line `CALL <tool>` it calls that tool on the fixture
/// server once, and after any tool result it ends the turn.
struct Script {
    seen: Mutex<Vec<Value>>,
}

fn last_message(body: &Value) -> Option<&Value> {
    body.get("messages").and_then(Value::as_array)?.last()
}

async fn spawn_model() -> (String, Arc<Script>) {
    let script = Arc::new(Script {
        seen: Mutex::new(Vec::new()),
    });
    let handle = Arc::clone(&script);
    let app = axum::Router::new().route(
        "/chat/completions",
        post(move |Json(body): Json<Value>| {
            let script = Arc::clone(&handle);
            async move {
                script.seen.lock().unwrap().push(body.clone());
                let last = last_message(&body).cloned().unwrap_or(Value::Null);
                let role = last.get("role").and_then(Value::as_str).unwrap_or("");
                let text = last.get("content").and_then(Value::as_str).unwrap_or("");
                let call = (role == "user")
                    .then(|| {
                        text.lines()
                            .find_map(|line| line.trim().strip_prefix("CALL "))
                    })
                    .flatten()
                    .map(str::trim);
                let message = match call {
                    Some(tool) => json!({
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": format!("call-{tool}"),
                            "type": "function",
                            "function": {
                                "name": "mcp_call_tool",
                                "arguments": json!({
                                    "server": SERVER,
                                    "tool": tool,
                                    "arguments": {}
                                }).to_string()
                            }
                        }]
                    }),
                    None => json!({ "role": "assistant", "content": "done" }),
                };
                Json(json!({
                    "choices": [{ "index": 0, "message": message }],
                    "usage": { "prompt_tokens": 12, "completion_tokens": 4 }
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), script)
}

/// Every `tools/call` the MCP server received, by tool name.
#[derive(Default)]
struct McpStub {
    calls: Mutex<Vec<String>>,
}

impl McpStub {
    fn calls_to(&self, tool: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|name| *name == tool)
            .count()
    }
}

async fn spawn_mcp_server() -> (String, Arc<McpStub>) {
    let stub = Arc::new(McpStub::default());
    let handle = Arc::clone(&stub);
    let app = axum::Router::new().route(
        "/mcp",
        post(move |Json(body): Json<Value>| {
            let stub = Arc::clone(&handle);
            async move {
                let id = body.get("id").cloned().unwrap_or(Value::Null);
                let result = match body.get("method").and_then(Value::as_str).unwrap_or("") {
                    "initialize" => json!({
                        "protocolVersion": "2025-11-25",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "fixture", "version": "1" },
                    }),
                    "tools/list" => json!({
                        "tools": [
                            { "name": ALLOWED, "description": "Search.", "inputSchema": { "type": "object" } },
                            { "name": BLOCKED, "description": "Delete.", "inputSchema": { "type": "object" } }
                        ]
                    }),
                    "tools/call" => {
                        let name = body["params"]["name"].as_str().unwrap_or("").to_string();
                        stub.calls.lock().unwrap().push(name.clone());
                        json!({ "content": [{ "type": "text", "text": format!("ok:{name}") }] })
                    }
                    _ => return Json(json!({ "jsonrpc": "2.0" })),
                };
                Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/mcp"), stub)
}

fn manifest(endpoint: &str) -> CompanyManifest {
    toml::from_str(&format!(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"

[tools]
allow = ["mcp:*"]

[[agent]]
id = "writer"
role = "Writer"

[[agent]]
id = "engineer"
role = "Engineer"

[[mcp_server]]
name = "{SERVER}"
endpoint = "{endpoint}"
"#
    ))
    .expect("manifest parses")
}

fn record(manifest: CompanyManifest) -> CompanyRecord {
    CompanyRecord {
        general_channel: Default::default(),
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        id: crate::test_support::per_test_company_id("acme"),
        manifest,
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        overlay_agents: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_tool_grants: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
        setup: None,
        name_confirmed: false,
        activation_completed_at: None,
        created_at_millis: None,
    }
}

fn deps(model_url: String, dir: &std::path::Path, secrets: Arc<MemorySecrets>) -> HarnessDeps {
    HarnessDeps {
        hive_store: None,
        emergency_gate: None,
        notifications: None,
        ledgers: None,
        ledger_registry: Default::default(),
        provider: Arc::new(HostedProvider::new(HostedProviderConfig {
            base_url: model_url,
            credential: Credential::from_value("stub-key"),
            extra_headers: Vec::new(),
        })),
        provider_slug: "managed".to_string(),
        serves: None,
        store: Arc::new(FsCompanyStore::new(dir)),
        meter: None,
        workspace_root: dir.to_path_buf(),
        mcp_home: None,
        workspace_git_enabled: false,
        audit_root: dir.to_path_buf(),
        model_override: Some("stub-model".to_string()),
        tasks: None,
        artifacts: None,
        skills: None,
        skills_source_dir: None,
        skills_registry: std::sync::Arc::from([]),
        default_mcp_servers: Vec::new(),
        mcp_servers: Vec::new(),
        events: None,
        delegations: DelegationQueue::default(),
        workflow_runner: WorkflowRunnerHandle::default(),
        mcp_failures: McpFailureQueue::default(),
        pending_publishes: crate::harness::publish::PendingPublishQueue::default(),
        workflow_refs: crate::harness::workflow_refs::WorkflowRefQueue::default(),
        run_outputs: crate::harness::orchestrator::RunOutputCache::default(),
        run_output_store: None,
        workflow_revisions: None,
        approval_requests: ApprovalRequestQueue::default(),
        approval_parker: None,
        secrets: Some(secrets),
        web_allowed_domains: Vec::new(),
        capabilities: crate::harness::toolbelt::CapabilityFilter::AllowAll,
        workflow_source_dir: None,
        plan: None,
        media: None,
        composio: None,
        #[cfg(feature = "chargebee")]
        chargebee: None,
        #[cfg(feature = "paypal")]
        paypal: None,
        hosting: None,
        steer: crate::company::steer::InflightRegistry::default(),
        run_supervisor: crate::runtime::RunSupervisor::default(),
        delivery: None,
        search: None,
        tenant_search: None,
        workspace: None,
        workflow_runs: None,
        deep_trace: None,
    }
}

fn writer_blocked_from(tool: &str) -> McpToolPolicies {
    let mut policies = McpToolPolicies::default();
    policies.agents.insert(
        "writer".to_string(),
        AgentToolPolicies {
            overrides: [(
                tool.to_string(),
                ToolPolicy {
                    tier: None,
                    mode: Some(ApprovalMode::Blocked),
                },
            )]
            .into_iter()
            .collect(),
        },
    );
    policies
}

fn inventory() -> McpToolInventory {
    McpToolInventory {
        tools: [
            (ALLOWED.to_string(), ToolTier::ReadOnly),
            (BLOCKED.to_string(), ToolTier::WriteDelete),
        ]
        .into_iter()
        .collect(),
        discovered_at_millis: 1,
    }
}

/// The tool results the model was handed after it asked for `tool`.
fn results_for(script: &Script, tool: &str) -> Vec<String> {
    let call_id = format!("call-{tool}");
    script
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|body| body.get("messages").and_then(Value::as_array).cloned())
        .flatten()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
        .filter(|m| m.get("tool_call_id").and_then(Value::as_str) == Some(call_id.as_str()))
        .filter_map(|m| m.get("content").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// Runs one turn in which `agent` calls `tool`, returning the turn's reply.
async fn turn(
    pool: &HarnessPool,
    record: &CompanyRecord,
    deps: &HarnessDeps,
    agent: &str,
    tool: &str,
) -> String {
    pool.ensure(record, deps).await.expect("pool ensures");
    pool.run(
        &record.id,
        agent,
        &format!("CALL {tool}"),
        deps,
        ChatTarget::default(),
    )
    .await
    .expect("turn runs")
    .reply
}

#[tokio::test]
async fn a_per_agent_block_refuses_on_a_live_turn_and_lifts_without_a_restart() {
    let (model_url, script) = spawn_model().await;
    let (endpoint, server) = spawn_mcp_server().await;
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::default());
    let record = record(manifest(&endpoint));
    let deps = deps(model_url, dir.path(), secrets.clone());

    save_tool_inventory(
        &record.id,
        secrets.as_ref(),
        &tool_inventory_key(SERVER),
        &inventory(),
    )
    .await
    .unwrap();
    save_tool_policies(
        &record.id,
        secrets.as_ref(),
        &tool_policies_key(SERVER),
        &writer_blocked_from(BLOCKED),
    )
    .await
    .unwrap();

    let pool = HarnessPool::new();

    let refused = turn(&pool, &record, &deps, "writer", BLOCKED).await;
    assert_eq!(
        server.calls_to(BLOCKED),
        0,
        "a blocked tool must never reach the server"
    );
    assert!(
        refused.contains(&format!(
            "tool `{BLOCKED}` is not permitted on server `{SERVER}`"
        )),
        "the turn must report the transport's refusal: {refused}"
    );

    script.seen.lock().unwrap().clear();
    turn(&pool, &record, &deps, "writer", ALLOWED).await;
    assert_eq!(server.calls_to(ALLOWED), 1, "an unblocked tool must dial");
    assert!(
        results_for(&script, ALLOWED)
            .join("\n")
            .contains(&format!("ok:{ALLOWED}")),
        "the unblocked result must reach the model"
    );

    script.seen.lock().unwrap().clear();
    turn(&pool, &record, &deps, "engineer", BLOCKED).await;
    assert_eq!(
        server.calls_to(BLOCKED),
        1,
        "a teammate the rule does not name must still reach the tool"
    );

    let blocked_fingerprint = pool.mcp_fingerprint_of(&record.id).await;
    save_tool_policies(
        &record.id,
        secrets.as_ref(),
        &tool_policies_key(SERVER),
        &McpToolPolicies::default(),
    )
    .await
    .unwrap();

    script.seen.lock().unwrap().clear();
    turn(&pool, &record, &deps, "writer", BLOCKED).await;
    assert_ne!(
        pool.mcp_fingerprint_of(&record.id).await,
        blocked_fingerprint,
        "clearing the block must move the MCP fingerprint"
    );
    assert_eq!(
        server.calls_to(BLOCKED),
        2,
        "with the block cleared the writer reaches the tool on the same pool"
    );
    assert!(
        results_for(&script, BLOCKED)
            .join("\n")
            .contains(&format!("ok:{BLOCKED}")),
        "the lifted call's result must reach the model"
    );
}
