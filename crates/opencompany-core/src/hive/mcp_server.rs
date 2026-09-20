//! The MCP server the company agents speak and reach OpenCompany's tools on
//! (plan `hive-desks`, Phase 3).
//!
//! # Why a server at all
//!
//! `openhuman_embed::Agent` has no seam for an in-process host tool: an agent's
//! tools are OpenHuman's own groups plus the `McpServer`s fixed on its spec.
//! So every tool this crate authors — ledger, tasks, pages, memory, workspace,
//! and the room's speech tools `post` / `broadcast` / `dm` /
//! `complete_episode` / `read` — is served here and reached by the agent as
//! `mcp_call_tool{server: "opencompany", tool, arguments}`.
//!
//! # The wire
//!
//! JSON-RPC 2.0 over Streamable HTTP, on a dedicated loopback listener
//! (`127.0.0.1:0`, see [`McpHost::serve_loopback`]), one route:
//!
//! `POST /internal/mcp/{company}/{runtime_agent_id}` with
//! `Authorization: Bearer <per-agent token>` and
//! `Accept: application/json, text/event-stream`. Methods: `initialize`
//! (answers a `Mcp-Session-Id`), `notifications/initialized` (202, empty),
//! `ping`, `tools/list`, `tools/call`. Every reply is a plain JSON body; the
//! server never opens an SSE stream, and `GET` answers 405 so a client that
//! probes for one learns there is none. The client this matches is
//! OpenHuman's own (`tinymcp::McpHttpClient`), which is also what the tests
//! drive it with.
//!
//! # Attribution and approvals
//!
//! The bearer names the agent; the agent's one in-flight turn
//! ([`InFlightRegistry`]) names the episode, round and conversation. A speech
//! call folds into that turn's outbox; an OpenCompany tool call is decided by
//! the agent's [`ApprovalPolicy`] — allow, deny, or park — and runs under an
//! [`InFlightContext`]. A parked call pushes onto the same
//! `ApprovalRequestQueue` the previous in-process dispatcher fed, so the chat
//! cycle's `park_approval_requests` still journals `ApprovalParked` and the
//! grant re-issue on `ApprovalResolved` still re-dispatches the message; the
//! seat is told "awaiting approval" and stops.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use openhuman_core::agent::tool_policy::{
    ToolCallContext, ToolPolicy, ToolPolicyDecision, ToolPolicyRequest,
};
use openhuman_embed::{AgentSpec, McpAuthConfig, McpServer};
use serde_json::{Value, json};
use tinyhivemind::{SessionAuthor, SessionLog};
use tinyhivemind_core::aside::Viewer;
use tinytools::Tool;

use super::tools::{
    InFlight, InFlightContext, InFlightRegistry, McpToolAdapter, SPEECH_TOOL_NAMES, Speech,
    ToolJob, is_speech_tool, speech_descriptor,
};
use crate::harness::policy::ApprovalPolicy;
use crate::ports::events::EventLog;
use crate::ports::types::CompanyId;

/// The server name the agents address it by (`mcp_call_tool{server}`), and
/// the mock brain's contract.
pub const SERVER_SLUG: &str = "opencompany";
/// The route prefix; the full path is `{prefix}/{company}/{runtime_agent_id}`.
pub const MCP_PATH_PREFIX: &str = "/internal/mcp";
/// The `Mcp-Session-Id` header, issued on `initialize`.
pub const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
/// Per-request timeout the attached `McpServer` is told to wait: an
/// OpenCompany tool can run a model pass of its own.
const CALL_TIMEOUT_SECS: u64 = 300;

/// One agent the server serves: who it is, what it may call, and what decides
/// its calls.
pub struct McpAgent {
    /// The company the agent belongs to.
    pub company: CompanyId,
    /// The manifest agent id (the speaker id).
    pub agent_id: String,
    /// The runtime agent id the route and the bearer resolve to.
    pub runtime_agent_id: String,
    bearer: String,
    /// The speech tools this agent may call. Every desk seat gets all five;
    /// a non-desk turn still lists them so the catalogue is stable across
    /// surfaces, and the handler refuses `dm` outside an episode.
    pub speech_tools: Vec<String>,
    tools: Vec<McpToolAdapter>,
    /// The approval policy that decides each OpenCompany tool call. `None`
    /// allows everything — for tests over a bare belt only.
    pub policy: Option<Arc<ApprovalPolicy>>,
    /// The agent workspace the served tools sandbox to.
    pub workspace: Option<PathBuf>,
    /// The company journal `read` is served from.
    pub events: Option<Arc<dyn EventLog>>,
}

impl fmt::Debug for McpAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpAgent")
            .field("company", &self.company)
            .field("agent_id", &self.agent_id)
            .field("runtime_agent_id", &self.runtime_agent_id)
            .field("tools", &self.tool_names())
            .finish_non_exhaustive()
    }
}

impl McpAgent {
    /// An agent with every speech tool and no OpenCompany tools yet.
    #[must_use]
    pub fn new(
        company: CompanyId,
        agent_id: impl Into<String>,
        runtime_agent_id: impl Into<String>,
        bearer: impl Into<String>,
    ) -> Self {
        Self {
            company,
            agent_id: agent_id.into(),
            runtime_agent_id: runtime_agent_id.into(),
            bearer: bearer.into(),
            speech_tools: SPEECH_TOOL_NAMES.iter().map(|s| (*s).to_string()).collect(),
            tools: Vec::new(),
            policy: None,
            workspace: None,
            events: None,
        }
    }

    /// Mints a fresh bearer: `ocm_` + a v4 UUID.
    #[must_use]
    pub fn mint_bearer() -> String {
        format!("ocm_{}", uuid::Uuid::new_v4().simple())
    }

    /// The OpenCompany tools to serve — the belt minus what OpenHuman runs
    /// natively, which the caller has already split off.
    #[must_use]
    pub fn tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.tools = tools.into_iter().map(McpToolAdapter::new).collect();
        self
    }

    /// Restricts the speech tools this agent may call.
    #[must_use]
    pub fn speech_tools<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.speech_tools = names.into_iter().map(Into::into).collect();
        self
    }

    /// The approval policy that decides this agent's tool calls.
    #[must_use]
    pub fn policy(mut self, policy: Arc<ApprovalPolicy>) -> Self {
        self.policy = Some(policy);
        self
    }

    /// The agent workspace.
    #[must_use]
    pub fn workspace(mut self, workspace: PathBuf) -> Self {
        self.workspace = Some(workspace);
        self
    }

    /// The journal `read` is served from.
    #[must_use]
    pub fn events(mut self, events: Arc<dyn EventLog>) -> Self {
        self.events = Some(events);
        self
    }

    /// The bearer this agent authenticates with.
    #[must_use]
    pub fn bearer(&self) -> &str {
        &self.bearer
    }

    /// The OpenCompany tool names served, in belt order.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect()
    }

    /// Every tool name the catalogue lists — the `allow_tools` of the
    /// attached `McpServer`.
    #[must_use]
    pub fn allow_tools(&self) -> Vec<String> {
        let mut names = self.speech_tools.clone();
        names.extend(self.tool_names());
        names
    }

    fn tool(&self, name: &str) -> Option<&McpToolAdapter> {
        self.tools.iter().find(|tool| tool.name() == name)
    }

    fn catalogue(&self) -> Vec<Value> {
        let mut tools: Vec<Value> = tinyhivemind::speech::tool_specs()
            .iter()
            .filter(|spec| self.speech_tools.iter().any(|name| name == spec.name))
            .map(speech_descriptor)
            .collect();
        tools.extend(self.tools.iter().map(McpToolAdapter::descriptor));
        tools
    }
}

/// The `dm` recipient rule Phase 4's `DeskHive` supplies (`hive.resolve_dm`).
///
/// TODO(Phase 4): install one on the host from `graph.rs`. Until then the
/// membership snapshot on [`HiveTurn`](super::tools::HiveTurn) is the rule.
pub trait DmResolver: Send + Sync {
    /// `Ok` when `speaker` may address `to` on `desk_id`; `Err(reason)` is
    /// rendered to the seat as `refused: <reason>`.
    fn resolve_dm(
        &self,
        company: &CompanyId,
        desk_id: &str,
        speaker: &str,
        to: &[String],
    ) -> Result<(), String>;
}

/// The server: the agents it serves, the turns in flight, and the listener.
pub struct McpHost {
    agents: RwLock<HashMap<String, Arc<McpAgent>>>,
    in_flight: Arc<InFlightRegistry>,
    addr: OnceLock<SocketAddr>,
    dm_resolver: RwLock<Option<Arc<dyn DmResolver>>>,
}

impl fmt::Debug for McpHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpHost")
            .field("agents", &self.agents.read().map(|a| a.len()).unwrap_or(0))
            .field("addr", &self.addr.get())
            .field("in_flight", &self.in_flight)
            .finish()
    }
}

impl Default for McpHost {
    fn default() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            in_flight: Arc::new(InFlightRegistry::new()),
            addr: OnceLock::new(),
            dm_resolver: RwLock::new(None),
        }
    }
}

/// The one host this process serves every company's agents on.
///
/// Process-wide for the same reason the OpenHuman runtime is
/// ([`openhuman_runtime::global`](crate::harness::openhuman_runtime::global)):
/// an agent is registered on the runtime once, its spec names one endpoint,
/// and every pool in the process — the serving one, a desktop-parity one, a
/// test's — must resolve that endpoint to the same registry of bearers and
/// in-flight turns. Bind it with [`McpHost::serve_loopback`] before the first
/// roster is built; until then [`McpHost::endpoint_for`] is `None` and an agent
/// spec carries no server.
static GLOBAL: OnceLock<Arc<McpHost>> = OnceLock::new();

/// The process-wide host (see [`GLOBAL`]).
#[must_use]
pub fn global() -> Arc<McpHost> {
    GLOBAL.get_or_init(McpHost::new).clone()
}

impl McpHost {
    /// An empty host with no listener.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The in-flight turn registry the driver registers turns on.
    #[must_use]
    pub fn in_flight(&self) -> &Arc<InFlightRegistry> {
        &self.in_flight
    }

    /// Registers (or replaces) an agent, keyed by runtime agent id.
    pub fn register(&self, agent: McpAgent) -> Arc<McpAgent> {
        let agent = Arc::new(agent);
        self.agents
            .write()
            .expect("mcp agents poisoned")
            .insert(agent.runtime_agent_id.clone(), agent.clone());
        agent
    }

    /// Forgets one agent; its bearer stops working at once.
    pub fn unregister(&self, runtime_agent_id: &str) {
        self.agents
            .write()
            .expect("mcp agents poisoned")
            .remove(runtime_agent_id);
    }

    /// Forgets the agent registered under `runtime_agent_id` only if it still
    /// holds `bearer` — a dropped roster entry must not evict the rebuilt one
    /// that has since taken its id.
    pub fn unregister_if_bearer(&self, runtime_agent_id: &str, bearer: &str) {
        let mut agents = self.agents.write().expect("mcp agents poisoned");
        if agents
            .get(runtime_agent_id)
            .is_some_and(|agent| constant_time_eq(agent.bearer.as_bytes(), bearer.as_bytes()))
        {
            agents.remove(runtime_agent_id);
        }
    }

    /// Forgets every agent of `company` — a roster rebuild.
    pub fn unregister_company(&self, company: &CompanyId) {
        self.agents
            .write()
            .expect("mcp agents poisoned")
            .retain(|_, agent| &agent.company != company);
    }

    /// The registered agent, if any.
    #[must_use]
    pub fn agent(&self, runtime_agent_id: &str) -> Option<Arc<McpAgent>> {
        self.agents
            .read()
            .expect("mcp agents poisoned")
            .get(runtime_agent_id)
            .cloned()
    }

    /// Installs Phase 4's `dm` rule.
    pub fn set_dm_resolver(&self, resolver: Arc<dyn DmResolver>) {
        *self.dm_resolver.write().expect("dm resolver poisoned") = Some(resolver);
    }

    /// The loopback address the listener is bound to, once it is.
    #[must_use]
    pub fn addr(&self) -> Option<SocketAddr> {
        self.addr.get().copied()
    }

    /// The endpoint an agent's `McpServer::http` is pointed at.
    #[must_use]
    pub fn endpoint_for(&self, company: &CompanyId, runtime_agent_id: &str) -> Option<String> {
        let addr = self.addr()?;
        Some(format!(
            "http://{addr}{MCP_PATH_PREFIX}/{company}/{runtime_agent_id}"
        ))
    }

    /// Binds `127.0.0.1:0` and serves the router on it. Idempotent: a second
    /// call returns the address the first bound.
    ///
    /// The listener runs on its own thread with its own tokio runtime rather
    /// than on the caller's: the host is process-wide and outlives any one
    /// runtime — a test binary boots one per `#[tokio::test]`, and a listener
    /// spawned on the first would die with it and leave every later turn
    /// dialling a closed port. The same reason the OpenHuman runtime is
    /// booted the way it is.
    pub async fn serve_loopback(self: &Arc<Self>) -> std::io::Result<SocketAddr> {
        self.serve_loopback_blocking()
    }

    /// [`serve_loopback`](Self::serve_loopback) for a caller with no runtime.
    pub fn serve_loopback_blocking(self: &Arc<Self>) -> std::io::Result<SocketAddr> {
        if let Some(addr) = self.addr() {
            return Ok(addr);
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        if let Err(bound) = self.addr.set(addr) {
            // Lost a race with another caller; their listener serves.
            drop(listener);
            return Ok(bound);
        }
        let host = self.clone();
        std::thread::Builder::new()
            .name("opencompany-mcp".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("opencompany-mcp-worker")
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::error!(
                            "[hive::mcp] MCP listener runtime failed to build: {error}"
                        );
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::from_std(listener) {
                        Ok(listener) => listener,
                        Err(error) => {
                            tracing::error!("[hive::mcp] MCP listener failed to register: {error}");
                            return;
                        }
                    };
                    if let Err(error) = axum::serve(listener, host.router()).await {
                        tracing::error!("[hive::mcp] loopback MCP listener stopped: {error}");
                    }
                });
            })?;
        tracing::info!(%addr, "[hive::mcp] serving the opencompany MCP server on loopback");
        Ok(addr)
    }

    /// The router serving [`MCP_PATH_PREFIX`]`/{company}/{runtime_agent_id}`.
    #[must_use]
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route(
                &format!("{MCP_PATH_PREFIX}/{{company}}/{{runtime_agent_id}}"),
                post(handle)
                    .get(|| async { StatusCode::METHOD_NOT_ALLOWED })
                    .delete(|| async { StatusCode::NO_CONTENT }),
            )
            .with_state(self)
    }
}

/// Mounts the MCP routes on an existing router (a test or an operator app
/// that serves everything on one listener). Production uses
/// [`McpHost::serve_loopback`] instead.
#[must_use]
pub fn mount(router: Router, host: Arc<McpHost>) -> Router {
    router.merge(host.router())
}

/// What an agent spec needs to reach this server.
#[derive(Clone, Debug)]
pub struct McpAttach {
    /// The agent's endpoint (`McpHost::endpoint_for`).
    pub endpoint: String,
    /// The agent's bearer.
    pub bearer: String,
    /// The tools the agent may see (`McpAgent::allow_tools`).
    pub allow_tools: Vec<String>,
}

impl McpAttach {
    /// The attachment for a registered agent, once the host has an address.
    #[must_use]
    pub fn for_agent(host: &McpHost, agent: &McpAgent) -> Option<Self> {
        Some(Self {
            endpoint: host.endpoint_for(&agent.company, &agent.runtime_agent_id)?,
            bearer: agent.bearer.clone(),
            allow_tools: agent.allow_tools(),
        })
    }
}

/// Attaches the `opencompany` MCP server to an agent spec. The Phase 2
/// `agent_spec_for` calls this once the pool has an [`McpAttach`] for the
/// agent; a spec without it has only OpenHuman's native tools.
#[must_use]
pub fn attach_opencompany_mcp(spec: AgentSpec, ctx: &McpAttach) -> AgentSpec {
    spec.mcp(opencompany_mcp_server(ctx))
}

/// The `McpServer` declaration [`attach_opencompany_mcp`] fixes on the spec:
/// slug [`SERVER_SLUG`], the agent's endpoint and bearer, its allow list.
#[must_use]
pub fn opencompany_mcp_server(ctx: &McpAttach) -> McpServer {
    McpServer::http(SERVER_SLUG, ctx.endpoint.clone())
        .auth(McpAuthConfig::BearerToken {
            token: ctx.bearer.clone(),
        })
        .allow_tools(ctx.allow_tools.clone())
        .timeout_secs(CALL_TIMEOUT_SECS)
        .description("OpenCompany: speak to the desk and use the company's tools")
}

// ---------------------------------------------------------------------------
// The handler
// ---------------------------------------------------------------------------

/// Byte-equality that does not stop at the first mismatch.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

fn bearer_of(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"opencompany\"")],
        "",
    )
        .into_response()
}

fn rpc_result(id: Value, result: Value) -> Response {
    axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Response {
    axum::Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    }))
    .into_response()
}

/// An MCP tool result: text content plus the `isError` flag the client
/// renders into `McpToolResult::is_error`.
fn tool_result(text: impl Into<String>, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": is_error })
}

async fn handle(
    State(host): State<Arc<McpHost>>,
    Path((company, runtime_agent_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(agent) = host.agent(&runtime_agent_id) else {
        return unauthorized();
    };
    let presented = bearer_of(&headers).unwrap_or_default();
    if !constant_time_eq(presented.as_bytes(), agent.bearer.as_bytes())
        || agent.company.to_string() != company
    {
        return unauthorized();
    }
    let accepts = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("application/json"));
    if !accepts {
        return (
            StatusCode::NOT_ACCEPTABLE,
            "Accept must include application/json",
        )
            .into_response();
    }
    let body: Value = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(error) => return rpc_error(Value::Null, -32700, format!("parse error: {error}")),
    };
    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let params = body.get("params").cloned().unwrap_or(Value::Null);
    if id.is_null() {
        // A notification: `notifications/initialized`, `notifications/cancelled`.
        return StatusCode::ACCEPTED.into_response();
    }
    match method.as_str() {
        "initialize" => initialize(id, &params),
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": agent.catalogue() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = call(&host, &agent, name, arguments).await;
            rpc_result(id, result)
        }
        other => rpc_error(id, -32601, format!("method not found: {other}")),
    }
}

fn initialize(id: Value, params: &Value) -> Response {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|v| tinymcp::tinymcp_bus::SUPPORTED_PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(tinymcp::tinymcp_bus::LATEST_PROTOCOL_VERSION);
    (
        [(HEADER_SESSION_ID, uuid::Uuid::new_v4().simple().to_string())],
        axum::Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": SERVER_SLUG,
                    "version": env!("CARGO_PKG_VERSION"),
                },
            },
        })),
    )
        .into_response()
}

async fn call(host: &McpHost, agent: &McpAgent, name: &str, arguments: Value) -> Value {
    if is_speech_tool(name) {
        if !agent.speech_tools.iter().any(|allowed| allowed == name) {
            return tool_result(format!("refused: '{name}' is not available to you"), true);
        }
        return call_speech(host, agent, name, &arguments).await;
    }
    if agent.tool(name).is_none() {
        return tool_result(format!("refused: unknown tool '{name}'"), true);
    }
    let turn = host.in_flight.snapshot(&agent.runtime_agent_id);
    // A turn that is running hands the call to its own task (see `ToolJob`);
    // anything else — a test over a bare agent, a registered turn nobody is
    // driving — is served here.
    if let Some(executor) = turn.as_ref().and_then(|turn| turn.executor.clone()) {
        let (reply, answer) = tokio::sync::oneshot::channel();
        let job = ToolJob {
            tool: name.to_string(),
            arguments: arguments.clone(),
            reply,
        };
        match executor.send(job).await {
            Ok(()) => {
                return answer.await.unwrap_or_else(|_| {
                    tool_result(
                        format!("'{name}' did not run: the turn ended before it could"),
                        true,
                    )
                });
            }
            Err(_) => {
                tracing::debug!(
                    agent = %agent.runtime_agent_id,
                    tool = name,
                    "[hive::mcp] the turn's executor is gone; serving the call on the server"
                );
            }
        }
    }
    agent.serve_call(name, arguments, turn).await
}

impl McpAgent {
    /// Decides `name` under this agent's [`ApprovalPolicy`] and, when allowed,
    /// runs it under `turn`'s context — returning the MCP tool result either
    /// way. Called on the turn's own task when one is running (so the policy's
    /// parks and the tool's own claims file where that turn reads them), else
    /// on the server's.
    pub async fn serve_call(&self, name: &str, arguments: Value, turn: Option<InFlight>) -> Value {
        let Some(tool) = self.tool(name) else {
            return tool_result(format!("refused: unknown tool '{name}'"), true);
        };
        if let Some(policy) = &self.policy {
            let channel = turn
                .as_ref()
                .map_or_else(|| "mcp".to_string(), |turn| turn.surface.id.clone());
            let request = ToolPolicyRequest::new(
                name,
                arguments.clone(),
                ToolCallContext::session(
                    self.runtime_agent_id.clone(),
                    channel,
                    self.agent_id.clone(),
                    uuid::Uuid::new_v4().simple().to_string(),
                    0,
                ),
            );
            match policy.check(&request).await {
                ToolPolicyDecision::Allow => {}
                ToolPolicyDecision::Deny { reason } => {
                    return tool_result(format!("refused: {reason}"), true);
                }
                ToolPolicyDecision::RequireApproval { reason } => {
                    return tool_result(
                        format!(
                            "awaiting approval: {reason}. The request has been parked for the \
                             operator; stop and wait for the decision."
                        ),
                        true,
                    );
                }
            }
        }
        let context = InFlightContext::new(turn, self.workspace.clone());
        let result = tool.execute(arguments, &context).await;
        let is_error = result.is_error;
        tool_result(result.output(), is_error)
    }
}

async fn call_speech(host: &McpHost, agent: &McpAgent, name: &str, arguments: &Value) -> Value {
    let Some(speech) = host
        .in_flight
        .with(&agent.runtime_agent_id, |turn| turn.speak(name, arguments))
    else {
        return tool_result(
            "refused: no turn is in flight for this agent, so nothing can be said",
            true,
        );
    };
    match speech {
        Speech::Recorded(receipt) => {
            // Phase 4's `resolve_dm` outranks the membership snapshot when it
            // is installed: a `dm` it refuses is unrecorded again.
            if name == "dm"
                && let Some(reason) = resolve_dm_via_host(host, agent, arguments)
            {
                host.in_flight
                    .with(&agent.runtime_agent_id, |turn| turn.outbox.clear());
                return tool_result(format!("refused: {reason}"), true);
            }
            tool_result(receipt, false)
        }
        Speech::Refused(text) => tool_result(text, true),
        Speech::Read { limit } => read(host, agent, limit).await,
    }
}

fn resolve_dm_via_host(host: &McpHost, agent: &McpAgent, arguments: &Value) -> Option<String> {
    let resolver = host
        .dm_resolver
        .read()
        .expect("dm resolver poisoned")
        .clone()?;
    let desk_id = host
        .in_flight
        .with(&agent.runtime_agent_id, |turn| {
            turn.hive.as_ref().map(|hive| hive.desk_id.clone())
        })
        .flatten()?;
    let to: Vec<String> = arguments
        .get("to")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|id| id.trim().trim_start_matches('@').to_string())
                .collect()
        })
        .unwrap_or_default();
    resolver
        .resolve_dm(&agent.company, &desk_id, &agent.agent_id, &to)
        .err()
}

/// Serves `read`: the conversation's recent rows, oldest first, narrowed to
/// what this agent may see.
async fn read(host: &McpHost, agent: &McpAgent, limit: usize) -> Value {
    let Some(events) = agent.events.clone() else {
        return tool_result("refused: this agent has no journal to read from", true);
    };
    let Some(surface) = host
        .in_flight
        .with(&agent.runtime_agent_id, |turn| turn.surface.clone())
    else {
        return tool_result("refused: no turn is in flight for this agent", true);
    };
    let log = crate::hive::session_log::EventLogSessionLog::new(
        events,
        agent.company.clone(),
        surface.id.clone(),
        surface.id.clone(),
    );
    let page = match log.read_before(None, limit).await {
        Ok(page) => page,
        Err(error) => {
            return tool_result(
                format!("this conversation could not be read: {error}"),
                true,
            );
        }
    };
    let viewer = Viewer::Agent {
        id: agent.agent_id.clone(),
    };
    let mut lines: Vec<String> = page
        .messages
        .iter()
        .filter(|message| {
            let author_id = match &message.author {
                SessionAuthor::Agent { id, .. } => Some(id.as_str()),
                _ => None,
            };
            message.audience.admits(&viewer, author_id)
        })
        .map(|message| {
            let author = match &message.author {
                SessionAuthor::Operator => "operator".to_string(),
                SessionAuthor::Person { label, .. } => label.clone(),
                SessionAuthor::Agent { id, .. } => id.clone(),
                SessionAuthor::System { kind, .. } => kind.clone(),
            };
            format!("[{}] {author}: {}", message.sequence.0, message.content)
        })
        .collect();
    lines.reverse();
    if lines.is_empty() {
        return tool_result("Nothing has been said in this conversation yet.", false);
    }
    let mut body = lines.join("\n");
    if page.next_before.is_some() {
        body.push_str(&format!(
            "\n\n(Showing the most recent {limit}. Older messages are not in this reply.)"
        ));
    }
    tool_result(body, false)
}

#[cfg(test)]
#[path = "mcp_server_tests.rs"]
mod tests;
