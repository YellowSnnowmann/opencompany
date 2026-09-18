//! **Server-only end-to-end drive of team orchestration.**
//!
//! A company template is written to disk, given *mock* credentials, booted on
//! the real Axum host, and then driven over HTTP exactly as the console would
//! drive it — no browser, no Playwright, no UI. What it proves is the thing
//! that is otherwise only visible by watching an operator console: that a
//! company's agents **coordinate**, and that the work they do on the way
//! reaches real skills and a real MCP server.
//!
//! ```bash
//! cargo run --example team_orchestration_e2e --features openhuman,mcp
//! ```
//!
//! Exits `0` when every claim below held, non-zero (with the failed claim
//! named) otherwise. It needs no network beyond loopback and no real key.
//!
//! # The chain it drives
//!
//! ```text
//!   operator ──chat──▶ ceo (orchestrator)
//!                        ├─ list_skills / describe_skill      (the skill tree)
//!                        ├─ delegate_to_desk("research") ─────▶ analyst
//!                        │                                       ├─ mcp_list_servers
//!                        │                                       ├─ mcp_call_tool ──▶ mock MCP
//!                        │                                       └─ delegate_to_teammate ──▶ writer
//!                        │                                                                    ├─ describe_skill
//!                        │                                                                    └─ workspace_create
//!                        └─ spawn_task("Publish…", assignee = writer)
//!
//!   operator ──PATCH column=in_progress──▶ the spawned card dispatches to writer
//!   operator ──PATCH column=done────────▶ the card completes
//! ```
//!
//! Three agents, three hand-offs, two delegation *kinds* (desk and teammate),
//! and a nested hand-off — the analyst's own `delegate_to_teammate` runs one
//! level below the CEO's, which is what makes this an orchestration test rather
//! than a single-agent turn with extra steps.
//!
//! # What is mocked, and what is therefore NOT proven
//!
//! Exactly two things are mocked, and both are mocked *at the wire*, not at a
//! seam inside the crate:
//!
//! * **Inference** — a scripted OpenAI-compatible `/chat/completions` endpoint
//!   on loopback, reached through the real `openai_compatible` provider with a
//!   real (mock-valued) credential read from the real `SecretStore`. Only the
//!   model's *choices* are scripted; the tool loop, the toolbelt assembly, the
//!   delegation queue, its drain, and every tool implementation are real.
//! * **The MCP server** — a real streamable-HTTP JSON-RPC MCP endpoint on
//!   loopback that **rejects an unauthenticated call with 401**. The bearer it
//!   accepts is written to the company's `SecretStore` under the canonical
//!   `mcp/<name>/auth` key, so the credential path is the shipped one.
//!
//! A green run therefore says nothing about any *particular* model choosing to
//! delegate — that is judgement, and no test can hold it fixed. It says that
//! when a model does choose to, the company executes the hand-off, runs the
//! delegate's turn, feeds it its skills and its MCP tools, and lands the result
//! where the operator can see it.
//!
//! # How each claim is evidenced
//!
//! Assertions are deliberately not made against console prose, which changes.
//! They are made against what the two mocks *observed*, which cannot be faked
//! by a passing-looking response:
//!
//! * an agent ran a turn ⟸ the inference mock was called with that agent's
//!   persona as its system prompt;
//! * a skill was really read ⟸ the SKILL.md body marker came back into that
//!   agent's transcript as a tool result on the following model call;
//! * the MCP server was really called ⟸ the MCP mock recorded a `tools/call`
//!   **with the bearer**, and its marker likewise reached the agent;
//! * the board moved ⟸ the REST board says so.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{Value, json};

use opencompany::company::CompanyManifest;
use opencompany::company::mcp::{AuthMaterial, store_auth};
use opencompany::ports::secrets::SecretStore;
use opencompany::ports::types::{CompanyId, SecretValue};
use opencompany::runtime::{RuntimeBuilder, company_id_from_name};
use opencompany::store::FsSecretStore;
use opencompany::{AppConfig, AppState};

// ---------------------------------------------------------------------------
// Markers
// ---------------------------------------------------------------------------

/// The operator identity in the template's `[users] admins`. Sign-in is the
/// real loopback magic-link flow.
const OPERATOR: &str = "operator@opencompany.local";

/// The mock inference key. Never a real credential; the scripted endpoint
/// checks it anyway, so a build that stopped sending the credential fails here
/// rather than silently working.
const MOCK_INFERENCE_KEY: &str = "mock-inference-key-do-not-use";

/// The mock MCP bearer, written to `mcp/factbook/auth` before boot.
const MOCK_MCP_BEARER: &str = "mock-mcp-bearer-do-not-use";

/// A string that exists **only** inside the template's `SKILL.md`. Its arrival
/// in an agent's transcript is the proof the skill tree was really read, as
/// opposed to a `describe_skill` frame that was emitted and errored.
const SKILL_MARKER: &str = "MARKET-SCAN-SKILL-BODY-MARKER";

/// The same idea for the MCP round trip: text only the mock server produces.
const MCP_MARKER: &str = "FACTBOOK-MCP-RESULT-MARKER";

/// Distinctive roles, because the scripted model identifies which agent it is
/// answering for by the persona in the system prompt (`persona_prompt` writes
/// "You are the {role} at {company}").
const ROLE_CEO: &str = "Chief Executive";
const ROLE_ANALYST: &str = "Research Analyst";
const ROLE_WRITER: &str = "Copywriter";

// ---------------------------------------------------------------------------
// The scripted model
// ---------------------------------------------------------------------------

/// One scripted model reply.
#[derive(Clone, Debug)]
enum Turn {
    /// Emit a native tool call with these literal arguments.
    Call(&'static str, Value),
    /// Finish the turn with plain assistant text.
    Say(String),
}

/// What the inference mock observed, which is what the assertions read.
#[derive(Default)]
struct Observed {
    /// How many model calls each agent's persona made.
    calls: BTreeMap<String, usize>,
    /// Every `tool` message body that came back into each agent's transcript.
    /// This is where a skill body and an MCP result actually land, so it is the
    /// only honest place to assert they arrived.
    tool_results: BTreeMap<String, Vec<String>>,
    /// Requests that arrived without the mock inference credential.
    unauthenticated: usize,
}

impl Observed {
    fn ran(&self, agent: &str) -> bool {
        self.calls.get(agent).copied().unwrap_or(0) > 0
    }

    fn transcript(&self, agent: &str) -> String {
        self.tool_results
            .get(agent)
            .map(|v| v.join("\n"))
            .unwrap_or_default()
    }
}

/// The scripted endpoint's state: a per-agent script, a per-agent cursor, and
/// what it saw.
struct Inference {
    scripts: BTreeMap<&'static str, Vec<Turn>>,
    cursor: Mutex<BTreeMap<String, usize>>,
    observed: Mutex<Observed>,
}

/// Which agent a request belongs to, read from its **system** message only.
///
/// Deliberately not a scan of the whole body: a delegation instruction quotes
/// the delegator's words into the delegate's *user* message, so a whole-body
/// scan would attribute the analyst's turn to the CEO the moment the CEO's
/// instruction mentioned it.
fn agent_of(body: &Value) -> String {
    let system = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|m| {
            m.iter()
                .find(|msg| msg.get("role").and_then(Value::as_str) == Some("system"))
        })
        .and_then(|msg| msg.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    for role in [ROLE_CEO, ROLE_ANALYST, ROLE_WRITER] {
        if system.contains(role) {
            return role.to_string();
        }
    }
    "unknown".to_string()
}

/// Every `tool`-role message body in the request — the tool results the agent
/// can actually read.
fn tool_results(body: &Value) -> Vec<String> {
    body.get("messages")
        .and_then(Value::as_array)
        .map(|msgs| {
            msgs.iter()
                .filter(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
                .filter_map(|m| m.get("content").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One assistant message carrying a native `tool_calls` array — the shape the
/// provider's tool-calling profile puts the turn loop on.
fn tool_call_message(tool: &str, args: &Value) -> Value {
    json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": format!("call-{tool}"),
            "type": "function",
            "function": { "name": tool, "arguments": args.to_string() }
        }]
    })
}

async fn spawn_inference(scripts: BTreeMap<&'static str, Vec<Turn>>) -> (String, Arc<Inference>) {
    let state = Arc::new(Inference {
        scripts,
        cursor: Mutex::new(BTreeMap::new()),
        observed: Mutex::new(Observed::default()),
    });
    let handle = Arc::clone(&state);
    let app = Router::new().route(
        "/chat/completions",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let state = Arc::clone(&handle);
            async move {
                let authenticated = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v == format!("Bearer {MOCK_INFERENCE_KEY}"));

                let agent = agent_of(&body);
                {
                    let mut observed = state.observed.lock().unwrap();
                    if !authenticated {
                        observed.unauthenticated += 1;
                    }
                    *observed.calls.entry(agent.clone()).or_default() += 1;
                    observed
                        .tool_results
                        .entry(agent.clone())
                        .or_default()
                        .extend(tool_results(&body));
                }

                // The next scripted step for THIS agent. Running off the end of
                // a script is not a hang and not an error: the turn simply ends
                // with text, and the observation-based assertions below are what
                // report a script that did not match the turn shape.
                let next = {
                    let mut cursor = state.cursor.lock().unwrap();
                    let at = cursor.entry(agent.clone()).or_insert(0);
                    let step = state
                        .scripts
                        .get(agent.as_str())
                        .and_then(|s| s.get(*at))
                        .cloned();
                    *at += 1;
                    step
                };
                // Running off the end repeats that agent's own closing line
                // rather than inventing a new one. A turn can make more model
                // calls than there are scripted steps for reasons that are not
                // the script's business — a tools-disabled wrap-up call, a
                // second turn on the same agent — and a generic filler would
                // then become the operator-visible reply, which reads as the
                // agent forgetting what it just did.
                let next = next.unwrap_or_else(|| {
                    state
                        .scripts
                        .get(agent.as_str())
                        .and_then(|s| s.iter().rev().find(|t| matches!(t, Turn::Say(_))).cloned())
                        .unwrap_or_else(|| Turn::Say("Understood.".to_string()))
                });
                let message = match next {
                    Turn::Say(text) => json!({ "role": "assistant", "content": text }),
                    Turn::Call(tool, args) => tool_call_message(tool, &args),
                };
                Json(json!({
                    "choices": [{ "index": 0, "message": message, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 24, "completion_tokens": 8 }
                }))
            }
        }),
    );
    (serve(app).await, state)
}

// ---------------------------------------------------------------------------
// The mock MCP server
// ---------------------------------------------------------------------------

/// The MCP protocol revision the vendored client negotiates.
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// What the MCP mock observed.
#[derive(Default)]
struct McpObserved {
    /// `(method, tool name)` for every JSON-RPC request that carried the bearer.
    authenticated_calls: Vec<(String, String)>,
    /// Requests that arrived with no bearer or the wrong one. Any of these is a
    /// failure: the credential path is under test, not incidental.
    rejected: usize,
}

async fn spawn_mcp() -> (String, Arc<Mutex<McpObserved>>) {
    let observed = Arc::new(Mutex::new(McpObserved::default()));
    let handle = Arc::clone(&observed);
    let app = Router::new().route(
        "/mcp",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let observed = Arc::clone(&handle);
            async move {
                let authenticated = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v == format!("Bearer {MOCK_MCP_BEARER}"));
                if !authenticated {
                    observed.lock().unwrap().rejected += 1;
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({ "error": "missing or wrong bearer" })),
                    )
                        .into_response();
                }

                let id = body.get("id").cloned().unwrap_or(Value::Null);
                let method = body
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let tool = body
                    .get("params")
                    .and_then(|p| p.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                observed
                    .lock()
                    .unwrap()
                    .authenticated_calls
                    .push((method.clone(), tool.clone()));

                let result = match method.as_str() {
                    "initialize" => json!({
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "factbook-mock", "version": "0.1.0" }
                    }),
                    "tools/list" => json!({
                        "tools": [{
                            "name": "lookup_market",
                            "description": "Look up a short market summary for a topic.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "topic": { "type": "string", "description": "The market to summarize." }
                                },
                                "required": ["topic"]
                            }
                        }]
                    }),
                    "tools/call" => json!({
                        "content": [{
                            "type": "text",
                            "text": format!(
                                "{MCP_MARKER}: the reference-management market grew 18% year over year; \
                                 three vendors hold 60% of it."
                            )
                        }],
                        "isError": false
                    }),
                    // A notification (`notifications/initialized`) carries no id
                    // and wants no result; an empty object is a valid reply the
                    // client discards.
                    _ => json!({}),
                };
                Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
            }
        }),
    );
    (format!("{}/mcp", serve(app).await), observed)
}

/// Serves `app` on an ephemeral loopback port and returns its base URL.
async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

// ---------------------------------------------------------------------------
// The company template
// ---------------------------------------------------------------------------

/// Writes the template this run drives: a `company.toml`, one committed skill,
/// and nothing else. It is an ordinary company directory — the same thing
/// `opencompany serve --company <dir>` takes — so nothing here is a test-only
/// shape the product does not have.
///
/// The roster is the smallest one that can show orchestration rather than
/// dispatch: an orchestrator, a desk it can hand work to, and a second desk the
/// *first desk's member* can hand work to in turn. Two desks, because a
/// teammate hand-off that stayed inside one desk would not distinguish "the
/// company routed this" from "the same agent kept working".
fn write_template(dir: &Path, inference_url: &str, mcp_endpoint: &str) {
    std::fs::create_dir_all(dir.join("skills/market-scan")).unwrap();

    std::fs::write(
        dir.join("company.toml"),
        format!(
            r#"
[company]
name = "Orchestration E2E Co"
output = "Market briefs and the copy that goes with them"
human_role = "Asking for the work and approving what comes back"

[policy]
# `full` lets an ordinary turn run without an approval prompt. The gate is not
# what this script is about, and parking every hand-off behind one would turn a
# coordination test into an approval test.
mode = "full"

[users]
admins = ["{OPERATOR}"]

[tools]
# `mcp:*` is what puts the bridge tools on a belt at all; `workspace` and
# `workspace.*` are both needed because the grant list matches with exact/glob
# semantics and an agent asking for the bare word is not covered by the glob.
allow = ["mcp:*", "workspace", "workspace.*"]

# Bring-your-own-key inference pointed at the scripted loopback endpoint. The
# credential is NOT here — `api_key_secret` names a SecretStore key, and the
# value is written before boot, which is the shipped shape.
[inference]
provider = "openai_compatible"
base_url = "{inference_url}"
api_key_secret = "inference/key"

[inference.models]
"chat-v1" = "mock-model"
"agentic-v1" = "mock-model"

[[mcp_server]]
name = "factbook"
endpoint = "{mcp_endpoint}"
description = "A market reference the analyst can query."
# Declared read-only by the operator, so a call to it is not priced as a change
# to the outside world. Without this the bridge call gates under stricter
# policies — see `read_only_tools` in the manifest docs.
read_only_tools = ["lookup_market"]

[[group_chat]]
id = "research"
name = "Research desk"
description = "Market and competitor questions."
members = ["analyst"]

[[group_chat]]
id = "content"
name = "Content desk"
description = "Written drafts and copy."
members = ["writer"]

[[agent]]
id = "ceo"
role = "{ROLE_CEO}"
description = "Sets direction and hands work to the desk that should do it."
tier = "orchestrator"
tools = ["mcp:*", "workspace.read"]

[[agent]]
id = "analyst"
role = "{ROLE_ANALYST}"
description = "Answers market questions from the company's reference sources."
# Opting in at all is what wires the hand-off tools; naming `content` is what
# lets this desk reach the writer.
delegates_to = ["content"]
tools = ["mcp:*", "workspace"]

[[agent]]
id = "writer"
role = "{ROLE_WRITER}"
description = "Turns findings into short written copy."
tools = ["workspace"]
"#
        ),
    )
    .unwrap();

    std::fs::write(
        dir.join("skills/market-scan/SKILL.md"),
        format!(
            r#"---
name: Market Scan
description: Summarize a market in one page so the operator can decide quickly.
category: Research
version: 1.0.0
---

# Market Scan

{SKILL_MARKER}

## Steps

1. **Name** the market and the question being asked of it.
2. **Pull** the reference numbers from the company's sources.
3. **Say** what the numbers mean for us, in two sentences.
"#
        ),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// The scripts
// ---------------------------------------------------------------------------

/// What each agent's model does, in order, across every turn it runs.
///
/// A cursor per agent rather than one global queue: the turns interleave (the
/// analyst's turn runs *inside* the CEO's drain, and the writer's inside the
/// analyst's), so a single queue would hand the analyst the CEO's next step.
fn scripts() -> BTreeMap<&'static str, Vec<Turn>> {
    let mut scripts = BTreeMap::new();

    // The orchestrator: look at what the company knows how to do, hand the
    // research out, and open a card for the follow-up it is NOT doing now.
    scripts.insert(
        ROLE_CEO,
        vec![
            Turn::Call("list_skills", json!({})),
            Turn::Call("describe_skill", json!({ "skill_id": "market-scan" })),
            Turn::Call(
                "delegate_to_desk",
                json!({
                    "desk": "research",
                    "instruction": "Size the reference-management market and hand the finding \
                                    to the content desk as a two-sentence summary."
                }),
            ),
            Turn::Call(
                "spawn_task",
                json!({
                    "title": "Publish the market brief",
                    "assignee": "writer",
                    "note": "Take the analyst's summary and publish it as a short brief."
                }),
            ),
            Turn::Say(
                "I asked the research desk to size the market and opened a card for the writer \
                 to publish the brief."
                    .to_string(),
            ),
        ],
    );

    // The research desk lead: reach the MCP server, then hand the finding on.
    // The hand-off runs one level below the CEO's, which is the nesting.
    scripts.insert(
        ROLE_ANALYST,
        vec![
            Turn::Call("mcp_list_servers", json!({})),
            Turn::Call(
                "mcp_call_tool",
                json!({
                    "server": "factbook",
                    "tool": "lookup_market",
                    "arguments": { "topic": "reference management" }
                }),
            ),
            Turn::Call(
                "delegate_to_teammate",
                json!({
                    "teammate": "writer",
                    "instruction": "Write two sentences of copy from the market figures in the \
                                    factbook result."
                }),
            ),
            Turn::Say(
                "The market grew 18% year over year and the writer has the copy.".to_string(),
            ),
        ],
    );

    // The writer, twice: once as the analyst's delegate, once as the assignee of
    // the dispatched card. One flat script, because the cursor carries across
    // both turns.
    scripts.insert(
        ROLE_WRITER,
        vec![
            // Turn one — the teammate hand-off.
            Turn::Call("describe_skill", json!({ "skill_id": "market-scan" })),
            Turn::Call(
                "workspace_create",
                json!({
                    "path": "Agents/writer/Market copy.md",
                    "kind": "file",
                    "content": "The reference-management market grew 18% last year. \
                                Three vendors hold 60% of it."
                }),
            ),
            Turn::Say("Copy drafted and filed in my workspace folder.".to_string()),
            // Turn two — the dispatched card.
            Turn::Call(
                "workspace_create",
                json!({
                    "path": "Agents/writer/Market brief.md",
                    "kind": "file",
                    "content": "# Market brief\n\nThe reference-management market grew 18% \
                                year over year; three vendors hold 60% of it."
                }),
            ),
            Turn::Say("The brief is published and ready for review.".to_string()),
        ],
    );

    scripts
}

// ---------------------------------------------------------------------------
// Booting the company
// ---------------------------------------------------------------------------

/// Seeds the two mock credentials, then boots the template on loopback.
///
/// Both credentials go in through the **real** `SecretStore` under the **real**
/// canonical keys, before the runtime resolves them — the same order the hosted
/// path uses. Nothing is injected past a seam.
async fn boot(home: &Path, template: &Path) -> (SocketAddr, CompanyId) {
    let manifest = CompanyManifest::from_path(template).expect("the template's manifest parses");
    let company_id = company_id_from_name(&manifest.company.name);

    let mut manifest = manifest;
    manifest.apply_globals();
    let problems = manifest.validate();
    assert!(
        problems.is_empty(),
        "the template must be valid: {problems:?}"
    );

    let secrets: Arc<dyn SecretStore> = Arc::new(FsSecretStore::new(home.to_path_buf()));
    secrets
        .set(
            &company_id,
            "inference/key",
            SecretValue(MOCK_INFERENCE_KEY.to_string()),
        )
        .await
        .expect("the mock inference key is stored");
    store_auth(
        &company_id,
        "factbook",
        &AuthMaterial::Bearer(MOCK_MCP_BEARER.to_string()),
        secrets.as_ref(),
    )
    .await
    .expect("the mock MCP bearer is stored");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = AppState::new(AppConfig {
        bind: address.to_string(),
        ..AppConfig::default()
    })
    .with_home(home.to_path_buf());
    let runtime = RuntimeBuilder::new(state.home().to_path_buf(), manifest)
        .with_id(company_id.clone())
        // The seed dir is what lets the harness find the template's committed
        // `skills/` tree — without it `describe_skill` answers about the global
        // baseline only and the skill marker never appears.
        .with_seed_dir(template.to_path_buf())
        .with_secrets(Arc::clone(&secrets))
        // The agent pool. Without it `dispatch_task` is a documented no-op and
        // no agent ever runs, so every claim below would fail for one reason.
        .with_harness(Arc::new(opencompany::harness::HarnessPool::new()))
        .build()
        .await
        .expect("the template boots");
    state
        .registry()
        .insert(company_id.clone(), Arc::new(runtime));
    tokio::spawn(async move {
        let _ = opencompany::server::serve_on(listener, state).await;
    });
    (address, company_id)
}

// ---------------------------------------------------------------------------
// The HTTP client
// ---------------------------------------------------------------------------

/// A cookie-carrying HTTP helper. `reqwest`'s cookie store is behind a feature
/// this crate does not enable, so the session cookie is carried by hand.
struct Client {
    inner: reqwest::Client,
    base: String,
    cookie: Mutex<Option<String>>,
}

impl Client {
    fn new(address: SocketAddr) -> Self {
        Self {
            inner: reqwest::Client::builder()
                // Generous: a chat turn here runs three agents' turns, nested,
                // before it answers.
                .timeout(Duration::from_secs(180))
                .build()
                .unwrap(),
            base: format!("http://{address}"),
            cookie: Mutex::new(None),
        }
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self.inner.request(method, format!("{}{path}", self.base));
        if let Some(cookie) = self.cookie.lock().unwrap().clone() {
            request = request.header(reqwest::header::COOKIE, cookie);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("the loopback host answers");
        let status = response.status().as_u16();
        if let Some(set) = response.headers().get(reqwest::header::SET_COOKIE)
            && let Ok(value) = set.to_str()
            && let Some((pair, _)) = value.split_once(';')
        {
            *self.cookie.lock().unwrap() = Some(pair.to_string());
        }
        let text = response.text().await.unwrap_or_default();
        let json = serde_json::from_str(&text).unwrap_or(Value::String(text));
        (status, json)
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        self.send(reqwest::Method::GET, path, None).await
    }
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }
    async fn patch(&self, path: &str, body: Value) -> (u16, Value) {
        self.send(reqwest::Method::PATCH, path, Some(body)).await
    }

    /// Signs in over the loopback magic-link flow, which echoes the code rather
    /// than mailing it — there is no mail transport here, and on a routable host
    /// it would not echo at all.
    async fn sign_in(&self, email: &str) {
        let (status, body) = self
            .post("/api/v1/company/auth/request", json!({ "email": email }))
            .await;
        assert_eq!(status, 200, "sign-in request refused: {body}");
        let code = body["dev_code"]
            .as_str()
            .unwrap_or_else(|| panic!("no dev_code came back, so no session can be minted: {body}"))
            .to_string();
        let (status, body) = self
            .post("/api/v1/company/auth/verify", json!({ "code": code }))
            .await;
        assert_eq!(status, 200, "the login code was refused: {body}");
    }

    /// The session cookie, for the SSE reader (which is a separate request).
    fn cookie(&self) -> Option<String> {
        self.cookie.lock().unwrap().clone()
    }
}

// ---------------------------------------------------------------------------
// The live trace
// ---------------------------------------------------------------------------

/// Streams `GET /events` and prints every tool call as it happens, so a person
/// running this **watches** the coordination rather than reading a verdict.
///
/// The stream is also the only surface that attributes a step to the agent that
/// took it while the turn is still running; the durable record is folded per
/// turn and arrives afterwards.
///
/// **Act two's steps do not appear here, and that is correct.** A dispatched
/// card runs un-streamed on purpose — its transient frames would otherwise
/// misattribute onto whichever chat thread the console happens to be watching —
/// so the live feed goes quiet once the card is dragged, and the card's own
/// timeline is where those steps are read back. A reader who expects the writer's
/// second turn to scroll past here would otherwise conclude it never ran; the
/// claim ledger below is what says it did.
fn spawn_trace(address: SocketAddr, cookie: Option<String>) -> Arc<AtomicUsize> {
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let mut request = client.get(format!("http://{address}/api/v1/company/events"));
        if let Some(cookie) = cookie {
            request = request.header(reqwest::header::COOKIE, cookie);
        }
        let Ok(response) = request.send().await else {
            return;
        };
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        while let Some(Ok(chunk)) = stream.next().await {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(at) = buffer.find('\n') {
                let line: String = buffer.drain(..=at).collect();
                let Some(data) = line.trim().strip_prefix("data:") else {
                    continue;
                };
                let Ok(frame) = serde_json::from_str::<Value>(data.trim()) else {
                    continue;
                };
                let kind = frame.get("type").and_then(Value::as_str).unwrap_or("");
                if kind != "tool_call" && kind != "tool_result" {
                    continue;
                }
                counter.fetch_add(1, Ordering::Relaxed);
                let agent = frame.get("agentId").and_then(Value::as_str).unwrap_or("?");
                let label = frame.get("label").and_then(Value::as_str).unwrap_or("?");
                let status = frame.get("status").and_then(Value::as_str).unwrap_or("");
                let detail = frame.get("detail").and_then(Value::as_str).unwrap_or("");
                let arrow = if kind == "tool_call" { "→" } else { "←" };
                println!("   {arrow} {agent:<10} {label}  [{status}] {detail}");
                // A refused step is the interesting one — say why, in the
                // tool's own words, rather than leaving a bare `[error]`.
                if status == "error"
                    && let Some(result) = frame.get("result").and_then(Value::as_str)
                {
                    println!("     ⤷ {result}");
                }
            }
        }
    });
    seen
}

// ---------------------------------------------------------------------------
// Claims
// ---------------------------------------------------------------------------

/// One thing this run set out to prove, and whether it held.
struct Claims {
    rows: Vec<(String, bool)>,
}

impl Claims {
    fn new() -> Self {
        Self { rows: Vec::new() }
    }

    fn check(&mut self, what: &str, held: bool) {
        self.rows.push((what.to_string(), held));
    }

    /// Prints the ledger and returns the process exit code.
    fn report(&self) -> i32 {
        println!("\n── claims ──────────────────────────────────────────────");
        for (what, held) in &self.rows {
            println!("   {} {what}", if *held { "PASS" } else { "FAIL" });
        }
        let failed = self.rows.iter().filter(|(_, held)| !held).count();
        if failed == 0 {
            println!("\n{} claims held.\n", self.rows.len());
            0
        } else {
            println!("\n{failed} of {} claims FAILED.\n", self.rows.len());
            1
        }
    }
}

/// Polls `GET /tasks/{id}` until its column is one of `wanted`, or gives up.
async fn wait_for_column(client: &Client, task_id: &str, wanted: &[&str]) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        let (status, body) = client
            .get(&format!("/api/v1/company/tasks/{task_id}"))
            .await;
        if status == 200 {
            let column = body
                .get("task")
                .and_then(|task| task.get("column"))
                .or_else(|| body.get("column"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if wanted.contains(&column.as_str()) {
                return column;
            }
            last = column;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("card never reached {wanted:?}; it is sitting in `{last}`");
}

/// Every card on the board. The route answers a bare JSON **array** of cards
/// (not an envelope), newest-updated first; the caller matches on title rather
/// than on position.
async fn tasks(client: &Client) -> Vec<Value> {
    let (_, body) = client.get("/api/v1/company/tasks").await;
    body.as_array().cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The drive
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;
    let template = tempfile::tempdir()?;

    println!("\n▸ standing up the mocks");
    let (mcp_endpoint, mcp) = spawn_mcp().await;
    let (inference_url, inference) = spawn_inference(scripts()).await;
    println!("   inference  {inference_url}");
    println!("   mcp        {mcp_endpoint}");

    println!("\n▸ writing the company template");
    write_template(template.path(), &inference_url, &mcp_endpoint);
    println!("   {}", template.path().display());

    println!("\n▸ booting the company (mock credentials in the SecretStore)");
    let (address, company) = boot(home.path(), template.path()).await;
    let client = Client::new(address);
    let (status, _) = client.get("/healthz").await;
    assert_eq!(status, 200, "the host serves /healthz");
    println!("   company    {company}");
    println!("   host       http://{address}");

    client.sign_in(OPERATOR).await;
    println!("   signed in  {OPERATOR}");

    // Subscribe before the first turn, or the trace misses it.
    let frames = spawn_trace(address, client.cookie());
    tokio::time::sleep(Duration::from_millis(250)).await;

    // -- Act one: one operator message, three agents ------------------------
    println!("\n▸ act one — the operator asks the company for a market brief");
    let (status, reply) = client
        .post(
            "/api/v1/company/chat",
            json!({
                // Phrased as an **instruction**, not a question, and that is
                // load-bearing rather than stylistic. The lexical triage in
                // `company::task_intent` reads a leading interrogative as
                // `Answer`, and an `Answer` turn holds only an *answering*
                // claim on the delegation queue — under which `delegate_to_desk`
                // still works (routing a question to a desk that can answer it
                // is why that claim exists) but the three pure board writes,
                // `spawn_task` among them, are refused at the tool boundary.
                // Asked as "how big is the market, and can we get a brief?",
                // this same script runs the whole hand-off chain and silently
                // opens no card.
                "text": "Write a short brief on the size of the reference-management market.",
                // A request for work, done once — the historical default, stated
                // rather than implied.
                "deliverable": "once"
            }),
        )
        .await;
    assert_eq!(status, 200, "the chat turn was refused: {reply}");
    // Every bubble the cycle produced, not just the CEO's. A desk hand-off
    // surfaces the delegate's own reply as its own bubble, so this is where the
    // coordination becomes readable as a conversation rather than as a step
    // trail.
    println!();
    for bubble in reply
        .get("responses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let channel = bubble.get("channel").and_then(Value::as_str).unwrap_or("?");
        let text = bubble.get("text").and_then(Value::as_str).unwrap_or("");
        println!("   [{channel}] {text}");
    }

    // Give the drain's trailing frames a moment to reach the SSE reader.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // -- Act two: the card the CEO opened, dispatched -----------------------
    println!("\n▸ act two — dispatching the card the CEO opened");
    let board = tasks(&client).await;
    for card in &board {
        println!(
            "   board: {:<40} [{}] → {}",
            card.get("title").and_then(Value::as_str).unwrap_or("?"),
            card.get("column").and_then(Value::as_str).unwrap_or("?"),
            card.get("assignee").and_then(Value::as_str).unwrap_or("-")
        );
    }
    let brief = board
        .iter()
        .find(|t| {
            t.get("title")
                .and_then(Value::as_str)
                .is_some_and(|title| title.contains("Publish the market brief"))
        })
        .cloned();

    let mut settled = String::new();
    if let Some(card) = &brief {
        let id = card["id"].as_str().unwrap().to_string();
        let (status, body) = client
            .patch(
                &format!("/api/v1/company/tasks/{id}"),
                json!({ "column": "in_progress" }),
            )
            .await;
        assert_eq!(status, 200, "dispatch refused: {body}");
        settled = wait_for_column(&client, &id, &["in_review", "done", "paused"]).await;
        println!("   the card settled in `{settled}`");

        // `done` has exactly one route and it runs through a person — an agent
        // takes a card as far as In Review and no further.
        if settled == "in_review" {
            let (status, body) = client
                .patch(
                    &format!("/api/v1/company/tasks/{id}"),
                    json!({ "column": "done" }),
                )
                .await;
            assert_eq!(status, 200, "the operator's approval was refused: {body}");
            settled = wait_for_column(&client, &id, &["done"]).await;
            println!("   the operator approved it to `{settled}`");
        }
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // -- The verdict --------------------------------------------------------
    let observed = inference.observed.lock().unwrap();
    let mcp = mcp.lock().unwrap();
    let mut claims = Claims::new();

    claims.check("the orchestrator ran a turn", observed.ran(ROLE_CEO));
    claims.check(
        "the CEO's desk hand-off ran the research desk's lead (delegate_to_desk)",
        observed.ran(ROLE_ANALYST),
    );
    claims.check(
        "the analyst's nested hand-off ran the writer (delegate_to_teammate)",
        observed.ran(ROLE_WRITER),
    );
    claims.check(
        "the writer ran twice — once as a delegate, once as a card assignee",
        observed.calls.get(ROLE_WRITER).copied().unwrap_or(0) > 3,
    );
    claims.check(
        "every inference call carried the mock credential",
        observed.unauthenticated == 0,
    );
    claims.check(
        "the template's committed skill reached the orchestrator's context",
        observed.transcript(ROLE_CEO).contains(SKILL_MARKER),
    );
    claims.check(
        "and the writer's",
        observed.transcript(ROLE_WRITER).contains(SKILL_MARKER),
    );
    claims.check(
        "the MCP server was called with the mock bearer",
        mcp.authenticated_calls
            .iter()
            .any(|(method, tool)| method == "tools/call" && tool == "lookup_market"),
    );
    claims.check(
        "no MCP call was ever made unauthenticated",
        mcp.rejected == 0,
    );
    claims.check(
        "the MCP result reached the analyst's context",
        observed.transcript(ROLE_ANALYST).contains(MCP_MARKER),
    );
    claims.check(
        "the orchestrator's spawn_task opened a card on the board",
        brief.is_some(),
    );
    claims.check(
        "that card ran to done through a dispatched agent turn and an operator approval",
        settled == "done",
    );
    claims.check(
        "the operator's live event feed carried the tool steps",
        frames.load(Ordering::Relaxed) > 0,
    );

    println!("\n── what the mocks saw ──────────────────────────────────");
    for (agent, calls) in &observed.calls {
        // `unknown` is not a miss: the card the chat handler opened runs a
        // tool-less planning pass whose system prompt is the planning desk's,
        // not any teammate's persona. It is named here so a reader does not go
        // looking for a fourth agent.
        let note = if agent == "unknown" {
            "  (the card's tool-less planning pass — no persona)"
        } else {
            ""
        };
        println!("   {agent:<20} {calls} model call(s){note}");
    }
    for (method, tool) in &mcp.authenticated_calls {
        println!("   mcp {method:<16} {tool}");
    }
    println!(
        "   {} live tool frames streamed",
        frames.load(Ordering::Relaxed)
    );

    std::process::exit(claims.report());
}
