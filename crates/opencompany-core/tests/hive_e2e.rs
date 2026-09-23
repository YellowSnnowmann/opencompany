#![cfg(feature = "openhuman")]
//! **End-to-end: hive desks over the embedded OpenHuman runtime** (plan
//! hive-desks, Phase 8).
//!
//! The unit tests under `src/hive/` drive the episode host with a scripted
//! `SeatRunner`; they pin the fold and cannot tell you whether a *company*
//! runs a room: whether an operator message on a desk of two opens an
//! episode, whether each seat's turn goes through the ordinary harness turn
//! with its `opencompany` MCP server attached, whether the one speech tool
//! the seat calls lands as exactly one `AgentReply` with its episode
//! metadata, whether two desks sharing an agent run at once without that
//! agent ever running twice, and whether the journal that results is the
//! one `opencompany measure` and the console fold the same numbers from.
//!
//! So every test here boots a **real company** — `RuntimeBuilder`, the
//! process-wide `openhuman_embed::Runtime`, the filesystem store, the HTTP
//! surface, loopback magic-link sign-in — and drives it through
//! `POST /api/v1/company/chat`, the route the console posts to. Only the
//! model is scripted (`support::script_model`), and the script is the mock
//! brain's hive arm in Rust (`frontend/test/e2e/mock-brain.mjs`): it keys on
//! the seat sentinel `Hive turn: desk <deskId>, episode <episodeId>, round
//! <revision>.` and on how many turns the seat has already taken in that
//! episode, answers with one `mcp_call_tool{server: "opencompany", tool,
//! arguments}`, and ends the turn with a plain `stop` once the tool result
//! is back.
//!
//! # What each test proves
//!
//! | Test | Claim |
//! | --- | --- |
//! | `a_two_member_desk_completes_in_two_rounds` | `EpisodeOpened` with both seats, two rounds of two brackets, one reply per seat per round, `EpisodeCompleted{complete_episode}` |
//! | `a_broadcast_without_jev_falls_back_deterministically` | `BroadcastRouted{router: fallback}` names the desk lead, who is reopened in the next round |
//! | `a_dm_schedules_its_recipient_and_is_journaled_with_its_audience` | the `dm` row carries `audience` and `episode.to`; `DmDelivered` names the peer; the peer runs next |
//! | `a_single_member_desk_answers_with_one_ordinary_turn` | a desk of one is one reply with no episode frames and no sentinel |
//! | `a_cross_desk_referral_crosses_only_the_answer_back` | the question is seeded on the far desk under `hive-referral`, the answer alone comes home, neither desk's seats speak on the other |
//! | `a_shared_agent_on_two_desks_runs_both_rooms_without_running_twice` | `companies/hive_demo`: both episodes complete, brackets overlap across desks, never for the same agent |
//! | `a_checkpoint_replays_the_rows_after_it_as_a_no_op` | the round-0 checkpoint plus the rows after it fold to the final state; folding them again changes nothing |
//! | `a_desk_remembers_across_episodes_through_the_mcp_memory_tool` | `memory_store` over MCP in one episode, `memory_recall` in the next, the post cites what came back |
//!
//! Every company gets a unique id: the runtime keeps one `Agent` per
//! `(company, agent)` for the life of the process, so two tests naming the
//! same company would share transcripts.

mod support;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::script_model::{Ask, Reply, Responder, spawn_script_with_latency};

use opencompany::CompanyRuntime;
use opencompany::company::CompanyManifest;
use opencompany::hive::measure::{Report, Thresholds, measure};
use opencompany::hive::referral::HIVE_REFERRAL_AUTHOR;
use opencompany::hive::routing::Router;
use opencompany::hive::tools::via_opencompany_mcp;
use opencompany::ports::types::{
    CompanyEvent, CompanyId, EpisodeReason, EventSeq, StoredEvent, UtteranceKind,
};
use opencompany::runtime::RuntimeBuilder;
use opencompany::{AppConfig, AppState};

// ---------------------------------------------------------------------------
// The seat as the scripted model sees it
// ---------------------------------------------------------------------------

/// The sentinel line every seat turn opens with.
const SENTINEL: &str = "Hive turn: desk ";

/// One seat turn, read off a request: the sentinel's coordinates, who is
/// speaking, which of its turns in this episode the seat is on, and the
/// tool results this turn has already collected.
#[derive(Clone, Debug)]
struct Seat {
    desk: String,
    episode: String,
    revision: u64,
    speaker: String,
    /// 0 for the seat's first turn in the episode, 1 for its second, …: the
    /// distinct earlier revisions below this one in the transcript. A retry
    /// at the same revision is the same turn.
    stage: usize,
    /// Tool results after the sentinel — this turn's own, oldest first.
    turn_tools: Vec<String>,
    /// The tools this turn already called, oldest first — the bare served
    /// name (`post`, `memory_store`) read out of each `mcp_call_tool`.
    calls: Vec<String>,
    /// The sentinel message, verbatim.
    prompt: String,
    /// The `## This assignment` block of the turn's own prompt — the
    /// operator's message, the broadcast that reopened the seat, or the
    /// answer that came home — free of whatever the preamble quoted.
    assignment: String,
}

/// The sentinel in `text` — the **last** one: the harness's memory loop
/// prepends a `## Relevant prior work` preamble quoting earlier prompts,
/// sentinel and all, and the turn's own sentinel follows it.
fn parse_sentinel(text: &str) -> Option<(String, String, u64)> {
    let at = text.rfind(SENTINEL)?;
    let rest = &text[at + SENTINEL.len()..];
    let (desk, rest) = rest.split_once(", episode ")?;
    let (episode, rest) = rest.split_once(", round ")?;
    let (revision, _) = rest.split_once('.')?;
    Some((
        desk.to_string(),
        episode.to_string(),
        revision.parse().ok()?,
    ))
}

fn role(message: &Value) -> &str {
    message.get("role").and_then(Value::as_str).unwrap_or("")
}

fn content(message: &Value) -> &str {
    message.get("content").and_then(Value::as_str).unwrap_or("")
}

fn seat_of(ask: &Ask) -> Option<Seat> {
    let last_user = ask
        .messages
        .iter()
        .rposition(|message| role(message) == "user")?;
    let prompt = content(&ask.messages[last_user]).to_string();
    let (desk, episode, revision) = parse_sentinel(&prompt)?;
    // The last `You are @…` for the reason the last sentinel is the turn's:
    // the prior-work preamble may quote another seat's prompt.
    let speaker = prompt
        .rsplit_once("You are @")
        .and_then(|(_, rest)| rest.split_once(' '))
        .map(|(id, _)| id.trim().to_string())
        .unwrap_or_default();
    let assignment = prompt
        .rsplit_once("## This assignment\n")
        .map(|(_, rest)| rest.split("\n\n## ").next().unwrap_or(rest).to_string())
        .unwrap_or_default();
    let mut earlier = std::collections::BTreeSet::new();
    for message in &ask.messages[..last_user] {
        if role(message) != "user" {
            continue;
        }
        if let Some((_, other, earlier_revision)) = parse_sentinel(content(message))
            && other == episode
            && earlier_revision < revision
        {
            earlier.insert(earlier_revision);
        }
    }
    let after = &ask.messages[last_user + 1..];
    let turn_tools = after
        .iter()
        .filter(|message| role(message) == "tool")
        .map(|message| content(message).to_string())
        .collect();
    let calls = after
        .iter()
        .filter(|message| role(message) == "assistant")
        .filter_map(|message| message.get("tool_calls").and_then(Value::as_array))
        .flatten()
        .filter_map(|call| {
            let function = call.get("function")?;
            let name = function.get("name").and_then(Value::as_str)?;
            if name != "mcp_call_tool" {
                return Some(name.to_string());
            }
            let arguments = function.get("arguments")?;
            let parsed: Value = match arguments {
                Value::String(text) => serde_json::from_str(text).ok()?,
                other => other.clone(),
            };
            parsed
                .get("tool")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    Some(Seat {
        desk,
        episode,
        revision,
        speaker,
        stage: earlier.len(),
        turn_tools,
        calls,
        prompt,
        assignment,
    })
}

/// The speech tools a seat ends its turn with.
const SPEECH: [&str; 4] = ["post", "broadcast", "dm", "complete_episode"];

impl Seat {
    /// Whether a speech tool has been called and answered in this turn —
    /// the utterance is recorded and the turn is over. The n-th result
    /// answers the n-th call.
    fn spoke(&self) -> bool {
        self.calls
            .iter()
            .zip(self.turn_tools.iter())
            .any(|(call, output)| SPEECH.contains(&call.as_str()) && !is_refused(output))
    }

    /// The operator's own words, when the assignment is an operator message:
    /// the line under `The operator asked (^N):`. The assignment goes on to
    /// index the channel's other threads, which quote earlier messages.
    fn operator_asked(&self) -> &str {
        self.assignment
            .split_once("):\n")
            .and_then(|(_, rest)| rest.lines().next())
            .unwrap_or_default()
    }

    /// The result of the last non-speech tool this turn called, when it
    /// answered.
    fn last_tool_result(&self) -> Option<&str> {
        self.calls
            .iter()
            .zip(self.turn_tools.iter())
            .filter(|(call, _)| !SPEECH.contains(&call.as_str()))
            .map(|(_, output)| output.as_str())
            .next_back()
    }
}

/// Whether a tool result reads as the host refusing the call.
fn is_refused(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "refused",
        "error",
        "rejected",
        "invalid",
        "not a member",
        "cannot",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

/// One speech act, as the `mcp_call_tool` the seat's belt carries it as.
fn speech(tool: &str, arguments: Value) -> Reply {
    let (name, args) = via_opencompany_mcp(tool, arguments);
    assert_eq!(name, "mcp_call_tool", "speech is served over MCP");
    Reply::Call {
        tool: "mcp_call_tool",
        args,
    }
}

fn post(message: impl Into<String>) -> Reply {
    speech("post", json!({ "message": message.into() }))
}

fn complete(message: impl Into<String>) -> Reply {
    speech("complete_episode", json!({ "message": message.into() }))
}

/// The turn is over once the seat's one speech act is recorded.
const DONE: &str = "done";

/// A script over seats: `act` decides the speech act for a seat turn that
/// has not spoken yet; a turn that has spoken stops; a request that is no
/// seat turn at all is answered with `plain`.
fn seat_script(
    plain: &'static str,
    act: impl Fn(&Seat) -> Reply + Send + Sync + 'static,
) -> Responder {
    Arc::new(move |ask: &Ask| {
        let Some(seat) = seat_of(ask) else {
            if ask.pending_tool.is_some() {
                return Reply::Say(DONE.to_string());
            }
            return Reply::Say(plain.to_string());
        };
        if seat.spoke() {
            return Reply::Say(DONE.to_string());
        }
        act(&seat)
    })
}

/// The mock brain's own hive arm: post, then complete.
fn post_then_complete(seat: &Seat) -> Reply {
    let stamp = format!(
        "desk {} episode {} round {} by @{}",
        seat.desk, seat.episode, seat.revision, seat.speaker
    );
    if seat.stage == 0 {
        post(format!("{stamp}: opening post."))
    } else {
        complete(format!("{stamp}: done, nothing left open."))
    }
}

// ---------------------------------------------------------------------------
// The company
// ---------------------------------------------------------------------------

const ENGINEERING: &str = "engineering";
const CONTENT: &str = "content";
const FRONT: &str = "front";
const ENGINEER: &str = "engineer";
const WRITER: &str = "writer";
const CEO: &str = "ceo";
const GREETER: &str = "greeter";
const ADMIN: &str = "operator@opencompany.local";
/// The admin `companies/hive_demo` grants.
const DEMO_ADMIN: &str = "harness-e2e@tinyhumans.ai";

/// Two desks of two sharing the CEO — `companies/hive_demo`'s shape — plus a
/// desk of one, so one company covers every surface these tests need.
///
/// `[policy] mode = "full"` so no tool call parks: this file is about the
/// room, and a parked turn would hang an episode rather than fail it.
fn manifest(name: &str, base_url: &str) -> String {
    format!(
        r#"
[company]
name = "{name}"
summary = "Proves desks answer as rooms."

[inference]
provider = "ollama"
base_url = "{base_url}"

# Every tier the roster reaches: the CEO is an orchestrator and asks for
# `agentic-v1`; an unmapped tier is refused, not passed through.
[inference.models]
chat-v1 = "llama3"
reasoning-v1 = "llama3"
agentic-v1 = "llama3"

[policy]
mode = "full"

[tools]
allow = []

[users]
admins = ["{ADMIN}"]

[[agent]]
id = "{CEO}"
role = "Chief Executive"
tier = "orchestrator"

[[agent]]
id = "{ENGINEER}"
role = "Engineer"

[[agent]]
id = "{WRITER}"
role = "Writer"

[[agent]]
id = "{GREETER}"
role = "Front desk"

[[group_chat]]
id = "{ENGINEERING}"
name = "Engineering"
description = "How things are built."
members = ["{ENGINEER}", "{CEO}"]

[group_chat.routing]
round_width = 2

[group_chat.routing.referral]
enabled = true
max_hops = 1
returns = true

[[group_chat]]
id = "{CONTENT}"
name = "Content"
description = "Written drafts and copy."
members = ["{WRITER}", "{CEO}"]

[group_chat.routing]
round_width = 2

[group_chat.routing.referral]
enabled = true
max_hops = 1
returns = true

[[group_chat]]
id = "{FRONT}"
name = "Front"
members = ["{GREETER}"]
"#
    )
}

/// A company id no other test in this process uses.
fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

/// Boots `manifest` on loopback under a unique company id.
async fn boot(
    home: &Path,
    company_id: &str,
    mut manifest: CompanyManifest,
) -> (SocketAddr, Arc<CompanyRuntime>) {
    manifest.apply_globals();
    let problems = manifest.validate();
    assert!(problems.is_empty(), "the manifest is valid: {problems:?}");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let company_id = CompanyId::new(company_id);
    let state = AppState::new(AppConfig {
        bind: address.to_string(),
        ..AppConfig::default()
    })
    .with_home(home.to_path_buf());
    let runtime = Arc::new(
        RuntimeBuilder::new(state.home().to_path_buf(), manifest)
            .with_id(company_id.clone())
            .with_harness(Arc::new(opencompany::harness::HarnessPool::new()))
            .build()
            .await
            .expect("the company builds"),
    );
    state
        .registry()
        .insert(company_id.clone(), Arc::clone(&runtime));
    tokio::spawn(async move {
        let _ = opencompany::server::serve_on(listener, state).await;
    });
    (address, runtime)
}

/// Boots the in-test company.
async fn boot_lab(home: &Path, base_url: &str) -> (SocketAddr, Arc<CompanyRuntime>) {
    let id = unique("hive-lab");
    let manifest = CompanyManifest::from_stored_toml(&manifest(&id, base_url))
        .expect("the in-test manifest parses");
    boot(home, &id, manifest).await
}

/// Boots `companies/hive_demo` — the measurement's company — against the
/// scripted model.
async fn boot_demo(home: &Path, base_url: &str) -> (SocketAddr, Arc<CompanyRuntime>) {
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../companies/hive_demo");
    let mut manifest = CompanyManifest::from_path(&bundle).expect("companies/hive_demo parses");
    manifest.inference.provider = Some("ollama".into());
    manifest.inference.base_url = Some(base_url.into());
    for tier in ["chat-v1", "reasoning-v1", "agentic-v1"] {
        manifest
            .inference
            .models
            .insert(tier.into(), "llama3".into());
    }
    boot(home, &unique("hive-demo"), manifest).await
}

// ---------------------------------------------------------------------------
// The operator
// ---------------------------------------------------------------------------

/// A cookie-carrying HTTP client — `reqwest`'s own cookie store is behind a
/// feature this crate does not enable.
struct Client {
    inner: reqwest::Client,
    base: String,
    cookie: Mutex<Option<String>>,
}

impl Client {
    fn new(address: SocketAddr) -> Self {
        Self {
            inner: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .unwrap(),
            base: format!("http://{address}"),
            cookie: Mutex::new(None),
        }
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let mut request = self.inner.post(format!("{}{path}", self.base)).json(&body);
        if let Some(cookie) = self.cookie.lock().unwrap().clone() {
            request = request.header(reqwest::header::COOKIE, cookie);
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
        let mut request = self.inner.get(format!("{}{path}", self.base));
        if let Some(cookie) = self.cookie.lock().unwrap().clone() {
            request = request.header(reqwest::header::COOKIE, cookie);
        }
        let response = request.send().await.expect("the loopback host answers");
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let json = serde_json::from_str(&text).unwrap_or(Value::String(text));
        (status, json)
    }

    /// Signs in over the loopback magic-link flow, which echoes the code.
    async fn sign_in(&self, email: &str) {
        let (status, body) = self
            .post("/api/v1/company/auth/request", json!({ "email": email }))
            .await;
        assert_eq!(status, 200, "sign-in refused: {body}");
        let code = body["dev_code"]
            .as_str()
            .unwrap_or_else(|| panic!("no dev_code, so no session: {body}"))
            .to_string();
        let (status, body) = self
            .post("/api/v1/company/auth/verify", json!({ "code": code }))
            .await;
        assert_eq!(status, 200, "the login code was refused: {body}");
    }

    /// Posts one operator message to `desk`. A desk with a room answers
    /// through its episode, so this returns as soon as the message is
    /// journaled; a desk of one answers in the response.
    async fn say(&self, desk: &str, text: &str) -> Value {
        let (status, body) = self
            .post(
                "/api/v1/company/chat",
                json!({ "text": text, "chat": desk }),
            )
            .await;
        assert_eq!(status, 200, "chat refused: {body}");
        body
    }
}

// ---------------------------------------------------------------------------
// Reading the journal back
// ---------------------------------------------------------------------------

async fn journal(runtime: &Arc<CompanyRuntime>) -> Vec<StoredEvent> {
    runtime
        .events()
        .read_from(runtime.id(), EventSeq::new(0), 100_000)
        .await
        .expect("the journal reads back")
}

/// With `HIVE_E2E_DUMP` set, prints the journal and every request the
/// scripted model saw — the way to read a failing run.
fn dump(rows: &[StoredEvent], script: &support::script_model::Script) {
    if std::env::var_os("HIVE_E2E_DUMP").is_none() {
        return;
    }
    for row in rows {
        eprintln!(
            "[journal] {} {}",
            row.seq.value(),
            serde_json::to_string(&row.event)
                .unwrap_or_default()
                .chars()
                .take(400)
                .collect::<String>()
        );
    }
    for ask in script.asks() {
        eprintln!(
            "[ask] tools={:?} seat={:?} pending={:?}",
            ask.tools,
            seat_of(&ask).map(|seat| (
                seat.speaker,
                seat.revision,
                seat.stage,
                seat.assignment,
                seat.calls
            )),
            ask.pending_tool
        );
    }
}

/// Polls the journal until `done` holds, or fails after `timeout`.
async fn wait_for(
    runtime: &Arc<CompanyRuntime>,
    what: &str,
    timeout: Duration,
    done: impl Fn(&[StoredEvent]) -> bool,
) -> Vec<StoredEvent> {
    let started = Instant::now();
    loop {
        let rows = journal(runtime).await;
        if done(&rows) {
            return rows;
        }
        if started.elapsed() >= timeout {
            if std::env::var_os("HIVE_E2E_DUMP").is_some() {
                for row in &rows {
                    eprintln!(
                        "[journal] {} {}",
                        row.seq.value(),
                        serde_json::to_string(&row.event)
                            .unwrap_or_default()
                            .chars()
                            .take(600)
                            .collect::<String>()
                    );
                }
            }
            panic!(
                "timed out waiting for {what}; journal kinds: {:?}",
                rows.iter().map(|row| row.event.kind()).collect::<Vec<_>>()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Every `EpisodeCompleted`, as `(episode_id, chat_id, reason, rounds)`.
fn completions(rows: &[StoredEvent]) -> Vec<(String, String, EpisodeReason, u32)> {
    rows.iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::EpisodeCompleted {
                episode_id,
                chat_id,
                reason,
                rounds,
                ..
            } => Some((episode_id.clone(), chat_id.clone(), *reason, *rounds)),
            _ => None,
        })
        .collect()
}

/// `n` episodes have completed, and no seat bracket is still open.
fn completed(n: usize) -> impl Fn(&[StoredEvent]) -> bool {
    move |rows| {
        completions(rows).len() >= n && {
            let report = opencompany::hive::measure::measure_rows(
                &CompanyId::new("x"),
                EventSeq::new(0),
                rows,
            );
            report.open_turns == 0
        }
    }
}

/// One journaled reply.
#[derive(Clone, Debug)]
struct ReplyRow {
    seq: u64,
    agent: String,
    text: String,
    audience: Vec<String>,
    kind: Option<UtteranceKind>,
    to: Vec<String>,
    episode: Option<String>,
}

fn replies(rows: &[StoredEvent], chat: &str) -> Vec<ReplyRow> {
    rows.iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::AgentReply {
                chat_id,
                agent_id,
                text,
                audience,
                episode,
                ..
            } if chat_id == chat => Some(ReplyRow {
                seq: row.seq.value(),
                agent: agent_id.clone(),
                text: text.clone(),
                audience: audience.clone(),
                kind: episode.as_ref().map(|episode| episode.kind),
                to: episode
                    .as_ref()
                    .map(|episode| episode.to.clone())
                    .unwrap_or_default(),
                episode: episode.as_ref().map(|episode| episode.id.clone()),
            }),
            _ => None,
        })
        .collect()
}

/// Every `RoundStarted` on `chat`, as `(revision, seats)`.
fn rounds(rows: &[StoredEvent], chat: &str) -> Vec<(u64, Vec<String>)> {
    rows.iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::RoundStarted {
                chat_id,
                revision,
                agent_ids,
                ..
            } if chat_id == chat => Some((*revision, agent_ids.clone())),
            _ => None,
        })
        .collect()
}

/// The coordination report over the company's whole journal — what
/// `opencompany measure --company <id>` prints.
async fn report(runtime: &Arc<CompanyRuntime>) -> Report {
    measure(runtime.events().as_ref(), runtime.id(), EventSeq::new(0))
        .await
        .expect("the journal measures")
}

/// A generous bound: every seat turn here is a couple of loopback calls.
const EPISODE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// 1: a two-member desk completes in two rounds
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_two_member_desk_completes_in_two_rounds() {
    let home = tempfile::tempdir().unwrap();
    let (base_url, script) = spawn_script_with_latency(
        seat_script("Noted.", post_then_complete),
        Duration::from_millis(150),
    )
    .await;
    let (address, runtime) = boot_lab(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    let accepted = client
        .say(
            ENGINEERING,
            "Plan the staging rollout for the new checkout.",
        )
        .await;
    assert!(
        accepted["responses"].is_array(),
        "the message is accepted: {accepted}"
    );

    let rows = wait_for(&runtime, "the episode to complete", EPISODE, completed(1)).await;

    let opened: Vec<(String, Vec<String>, Router)> = rows
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::EpisodeOpened {
                chat_id,
                participants,
                plan,
                ..
            } => Some((chat_id.clone(), participants.clone(), plan.router())),
            _ => None,
        })
        .collect();
    assert_eq!(
        opened,
        vec![(
            ENGINEERING.to_string(),
            vec![ENGINEER.to_string(), CEO.to_string()],
            Router::Fallback
        )],
        "no Jev: the opening plan is the desk in order, up to the round width"
    );
    assert_eq!(
        rounds(&rows, ENGINEERING),
        vec![
            (0, vec![ENGINEER.to_string(), CEO.to_string()]),
            (2, vec![ENGINEER.to_string(), CEO.to_string()]),
        ],
        "two rounds of both seats; the revision counts committed utterances"
    );
    let done = completions(&rows);
    assert_eq!(done.len(), 1, "{done:?}");
    assert_eq!(done[0].1, ENGINEERING);
    assert_eq!(done[0].2, EpisodeReason::CompleteEpisode);
    assert_eq!(done[0].3, 2);

    let desk = replies(&rows, ENGINEERING);
    let kinds: Vec<(String, Option<UtteranceKind>)> = desk
        .iter()
        .map(|row| (row.agent.clone(), row.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (ENGINEER.to_string(), Some(UtteranceKind::Post)),
            (CEO.to_string(), Some(UtteranceKind::Post)),
            (ENGINEER.to_string(), Some(UtteranceKind::CompleteEpisode)),
            (CEO.to_string(), Some(UtteranceKind::CompleteEpisode)),
        ],
        "one reply per seat per round, in desk order: {desk:?}"
    );
    assert!(
        desk.iter()
            .all(|row| row.episode == Some(done[0].0.clone())),
        "every row names the episode: {desk:?}"
    );
    assert!(
        desk.iter().all(|row| row.text.contains("by @")),
        "the row text is the tool's `message`, verbatim: {desk:?}"
    );

    // The seats saw the sentinel with the round's revision, and were handed
    // the desk delta on their second turn.
    let seats: Vec<Seat> = script.asks().iter().filter_map(seat_of).collect();
    assert!(
        seats
            .iter()
            .any(|seat| seat.revision == 2 && seat.stage == 1),
        "the second round's sentinel reads round 2: {:?}",
        seats
            .iter()
            .map(|seat| (seat.speaker.clone(), seat.revision, seat.stage))
            .collect::<Vec<_>>()
    );
    let second = seats
        .iter()
        .find(|seat| seat.speaker == CEO && seat.revision == 2 && seat.turn_tools.is_empty())
        .expect("the CEO's second turn");
    assert!(
        second.prompt.contains("@engineer (^"),
        "the second turn is handed the engineer's post as an attributed delta: {}",
        second.prompt
    );

    let measured = report(&runtime).await;
    assert_eq!(measured.episodes_completed, 1);
    assert_eq!(measured.same_agent_overlaps, 0);
    assert!(
        measured.max_concurrent_turns >= 2,
        "both seats of a round run at once: {measured:?}"
    );
    assert!(
        script.peak_in_flight() >= 2,
        "the model itself saw two completions in flight: {}",
        script.peak_in_flight()
    );
    assert_eq!(measured.utterance_kinds["post"], 2);
    assert_eq!(measured.utterance_kinds["complete_episode"], 2);

    // The console's episode list agrees with the journal.
    let (status, episodes) = client.get("/api/v1/company/episodes").await;
    assert_eq!(status, 200, "{episodes}");
    assert_eq!(episodes[0]["id"], done[0].0);
    assert_eq!(episodes[0]["status"], "completed");
    assert_eq!(episodes[0]["reason"], "complete_episode");

    // Every seat turn is an attempt on `GET /runs`, with its episode.
    let (status, runs) = client.get("/api/v1/company/runs?limit=50").await;
    assert_eq!(status, 200, "{runs}");
    let seat_runs: Vec<&Value> = runs
        .as_array()
        .or_else(|| runs["runs"].as_array())
        .expect("run rows")
        .iter()
        .filter(|run| run["episodeId"] == done[0].0)
        .collect();
    assert_eq!(seat_runs.len(), 4, "{runs}");
    assert!(
        seat_runs
            .iter()
            .all(|run| run["status"] == "succeeded" && run["roundRevision"].is_u64()),
        "{seat_runs:?}"
    );
}

// ---------------------------------------------------------------------------
// 2: a broadcast without Jev falls back to the desk lead
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broadcast_without_jev_falls_back_deterministically() {
    let home = tempfile::tempdir().unwrap();
    let (base_url, script) = spawn_script_with_latency(
        seat_script("Noted.", |seat| {
            // The CEO hands the work on; the engineer just finishes.
            if seat.speaker == CEO && seat.stage == 1 {
                return speech(
                    "broadcast",
                    json!({ "message": "Whoever is best placed: size the rollout." }),
                );
            }
            post_then_complete(seat)
        }),
        Duration::from_millis(50),
    )
    .await;
    let (address, runtime) = boot_lab(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client.say(ENGINEERING, "Size the rollout.").await;
    let rows = wait_for(&runtime, "the episode to complete", EPISODE, completed(1)).await;

    let routed: Vec<(String, u64, Router, Vec<String>, u64)> = rows
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::BroadcastRouted {
                agent_id,
                revision,
                router,
                plan,
                message_seq,
                ..
            } => Some((
                agent_id.clone(),
                *revision,
                *router,
                plan.agent_ids(),
                *message_seq,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(routed.len(), 1, "{routed:?}");
    let (by, revision, router, targets, message_seq) = &routed[0];
    assert_eq!(by, CEO);
    assert_eq!(*revision, 2);
    assert_eq!(*router, Router::Fallback, "no TinyHumans key, no Jev");
    assert_eq!(
        targets,
        &vec![ENGINEER.to_string()],
        "the deterministic fallback is the desk lead"
    );
    let desk = replies(&rows, ENGINEERING);
    let broadcast = desk
        .iter()
        .find(|row| row.seq == *message_seq)
        .expect("the routed row is the broadcast's reply");
    assert_eq!(broadcast.kind, Some(UtteranceKind::Broadcast));
    assert_eq!(broadcast.agent, CEO);

    // The lead is reopened: a later round runs the engineer again, and the
    // engineer's next prompt says who handed it what.
    assert!(
        rounds(&rows, ENGINEERING)
            .iter()
            .any(|(rev, seats)| *rev > 2 && seats.contains(&ENGINEER.to_string())),
        "the broadcast reopens the lead: {:?}",
        rounds(&rows, ENGINEERING)
    );
    let reopened = script
        .asks()
        .iter()
        .filter_map(seat_of)
        .find(|seat| seat.speaker == ENGINEER && seat.revision > 2 && seat.turn_tools.is_empty())
        .expect("the engineer's reopened turn");
    assert!(
        reopened
            .prompt
            .contains("@ceo handed you this by broadcast"),
        "{}",
        reopened.prompt
    );
    let done = completions(&rows);
    assert_eq!(done[0].2, EpisodeReason::CompleteEpisode, "{done:?}");
    let measured = report(&runtime).await;
    assert_eq!(measured.broadcasts, 1);
    assert_eq!(measured.routers["fallback"], 1);
    assert!(
        measured.distinct_pairs.contains("ceo→engineer"),
        "{measured:?}"
    );
}

// ---------------------------------------------------------------------------
// 3: a dm schedules its recipient and is journaled with its audience
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dm_schedules_its_recipient_and_is_journaled_with_its_audience() {
    let home = tempfile::tempdir().unwrap();
    let (base_url, _script) = spawn_script_with_latency(
        seat_script("Noted.", |seat| {
            if seat.speaker == ENGINEER && seat.stage == 1 {
                return speech(
                    "dm",
                    json!({ "to": [CEO], "message": "Between us: the rollout needs a freeze." }),
                );
            }
            post_then_complete(seat)
        }),
        Duration::from_millis(50),
    )
    .await;
    let (address, runtime) = boot_lab(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client.say(ENGINEERING, "Decide on the freeze.").await;
    let rows = wait_for(&runtime, "the episode to complete", EPISODE, completed(1)).await;

    let delivered: Vec<(String, Vec<String>, u64)> = rows
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::DmDelivered {
                from,
                to,
                message_seq,
                ..
            } => Some((from.clone(), to.clone(), *message_seq)),
            _ => None,
        })
        .collect();
    assert_eq!(
        delivered,
        vec![(ENGINEER.to_string(), vec![CEO.to_string()], delivered[0].2)],
        "{delivered:?}"
    );
    let desk = replies(&rows, ENGINEERING);
    let dm = desk
        .iter()
        .find(|row| row.seq == delivered[0].2)
        .expect("the delivered row is the dm's reply");
    assert_eq!(dm.kind, Some(UtteranceKind::Dm));
    assert_eq!(
        dm.audience,
        vec![CEO.to_string()],
        "journaled with its audience"
    );
    assert_eq!(dm.to, vec![CEO.to_string()], "and on the episode metadata");
    assert!(
        desk.iter()
            .filter(|row| row.seq != dm.seq)
            .all(|row| row.audience.is_empty()),
        "every other row is for the whole desk: {desk:?}"
    );

    // The recipient is scheduled: a round after the dm runs the CEO.
    let dm_round = rounds(&rows, ENGINEERING)
        .iter()
        .find(|(_, seats)| seats.contains(&ENGINEER.to_string()))
        .map(|(rev, _)| *rev)
        .unwrap();
    assert!(
        rounds(&rows, ENGINEERING)
            .iter()
            .any(|(rev, seats)| *rev > dm_round + 1 && seats.contains(&CEO.to_string())),
        "{:?}",
        rounds(&rows, ENGINEERING)
    );
    let measured = report(&runtime).await;
    assert_eq!(measured.dms, 1);
    assert!(
        measured.distinct_pairs.contains("engineer→ceo"),
        "{measured:?}"
    );
    assert_eq!(completions(&rows)[0].2, EpisodeReason::CompleteEpisode);

    // The history projection carries the audience and the episode too.
    let (status, history) = client
        .get(&format!("/api/v1/company/chat/history?desk={ENGINEERING}"))
        .await;
    assert_eq!(status, 200, "{history}");
    let messages = history
        .as_array()
        .or_else(|| history["messages"].as_array())
        .expect("history rows");
    let row = messages
        .iter()
        .find(|message| message["id"].as_str() == Some(dm.seq.to_string().as_str()))
        .unwrap_or_else(|| panic!("the dm row is in history: {history}"));
    assert_eq!(row["audience"], json!([CEO]));
    assert_eq!(row["episode"]["kind"], "dm");
    assert_eq!(row["episode"]["to"], json!([CEO]));
    assert!(row.get("asideConversation").is_none());
}

// ---------------------------------------------------------------------------
// 4: a desk of one is one ordinary reply
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_single_member_desk_answers_with_one_ordinary_turn() {
    let home = tempfile::tempdir().unwrap();
    let (base_url, script) = spawn_script_with_latency(
        seat_script("Noted — the front desk has it.", |seat| {
            panic!("a desk of one must never be handed a seat turn: {seat:?}")
        }),
        Duration::ZERO,
    )
    .await;
    let (address, runtime) = boot_lab(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    let body = client.say(FRONT, "Anything waiting at the front?").await;
    assert_eq!(
        body["responses"].as_array().map(Vec::len),
        Some(1),
        "{body}"
    );

    let rows = journal(&runtime).await;
    let desk = replies(&rows, FRONT);
    assert_eq!(desk.len(), 1, "one ordinary reply: {desk:?}");
    assert_eq!(desk[0].agent, GREETER);
    assert_eq!(desk[0].kind, None, "no episode metadata on a plain reply");
    assert!(
        !rows.iter().any(|row| matches!(
            &row.event,
            CompanyEvent::EpisodeOpened { .. }
                | CompanyEvent::RoundStarted { .. }
                | CompanyEvent::RoundCommitted { .. }
                | CompanyEvent::EpisodeCompleted { .. }
        )),
        "no episode frames: {:?}",
        rows.iter().map(|row| row.event.kind()).collect::<Vec<_>>()
    );
    assert!(
        script.asks().iter().all(|ask| seat_of(ask).is_none()),
        "no sentinel reached the model"
    );
    assert_eq!(report(&runtime).await.episodes_opened, 0);
}

// ---------------------------------------------------------------------------
// 5: a cross-desk referral crosses only the answer back
// ---------------------------------------------------------------------------

/// `@#<desk>` is the desk-mention spelling the mention resolver reads
/// (`tinyhivemind_core::mention`); a bare `#content` is prose.
const QUESTION: &str = "Please ask @#content for the release-note tagline.";
const TAGLINE: &str = "Checkout, now with fewer steps.";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cross_desk_referral_crosses_only_the_answer_back() {
    let home = tempfile::tempdir().unwrap();
    let (base_url, script) = spawn_script_with_latency(
        seat_script("Noted.", |seat| {
            match (seat.desk.as_str(), seat.speaker.as_str(), seat.stage) {
                (ENGINEERING, ENGINEER, 0) => post(format!("Rollout plan drafted. {QUESTION}")),
                (CONTENT, WRITER, 0) => post("Drafting the tagline."),
                (CONTENT, WRITER, _) => complete(format!("Tagline: {TAGLINE}")),
                _ => post_then_complete(seat),
            }
        }),
        Duration::from_millis(50),
    )
    .await;
    let (address, runtime) = boot_lab(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client
        .say(
            ENGINEERING,
            "Plan the rollout and get the release note written.",
        )
        .await;
    // Both desks' episodes complete, and the answer has come home — the
    // return marker is journaled after the answer row, so it is the marker
    // that says the crossing is over. `completed(2)` counts the
    // engineering desk's *first* completion (round 2, before the answer
    // ever arrives — its own CEO and engineer both call `complete_episode`
    // independently of the referral) together with the content desk's
    // completion, so it and the return marker can both be true while
    // `deliver_answer`'s reopened round for the engineer is still only
    // spawned (`crossing.rs`'s `spawn_drive`, a bare `tokio::spawn` the
    // driver does not await) and has not yet reached the model. Wait for
    // that reopened ask to actually land on the script too, or the
    // assertions below race it.
    let rows = wait_for(
        &runtime,
        "both episodes, the answer, and the reopened turn",
        EPISODE,
        |rows| {
            completed(2)(rows)
                && rows.iter().any(|row| {
                    matches!(
                        &row.event,
                        CompanyEvent::ReferralEnqueued {
                            returning: true,
                            ..
                        }
                    )
                })
                && script.asks().iter().filter_map(seat_of).any(|seat| {
                    seat.speaker == ENGINEER
                        && seat.prompt.contains("answered the question you put to it")
                })
        },
    )
    .await;

    // The forward marker, and the return: `(from_desk, to_desk, returning,
    // episode_id, to_episode_id)`.
    type Marker = (String, String, bool, Option<String>, Option<String>);
    let markers: Vec<Marker> = rows
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::ReferralEnqueued {
                from_desk,
                to_desk,
                returning,
                episode_id,
                to_episode_id,
                ..
            } => Some((
                from_desk.clone(),
                to_desk.clone(),
                *returning,
                episode_id.clone(),
                to_episode_id.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(markers.len(), 2, "one crossing, one return: {markers:?}");
    assert_eq!(
        (markers[0].0.as_str(), markers[0].1.as_str(), markers[0].2),
        (ENGINEERING, CONTENT, false)
    );
    assert_eq!(
        (markers[1].0.as_str(), markers[1].1.as_str(), markers[1].2),
        (CONTENT, ENGINEERING, true)
    );
    let done = completions(&rows);
    let engineering_episode = done
        .iter()
        .find(|(_, chat, _, _)| chat == ENGINEERING)
        .map(|(id, ..)| id.clone())
        .expect("the engineering episode completed");
    let content_episode = done
        .iter()
        .find(|(_, chat, _, _)| chat == CONTENT)
        .map(|(id, ..)| id.clone())
        .expect("the content episode completed");
    // On both legs `episode_id` is the episode that ASKED and
    // `to_episode_id` the one that answered — the pair a console keys the
    // crossing on stays the same whichever way the frame is addressed.
    assert_eq!(markers[0].3.as_deref(), Some(engineering_episode.as_str()));
    assert_eq!(markers[1].3.as_deref(), Some(engineering_episode.as_str()));
    assert_eq!(markers[1].4.as_deref(), Some(content_episode.as_str()));
    let hops: Vec<(String, u32)> = rows
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::EpisodeOpened { chat_id, hop, .. } => Some((chat_id.clone(), *hop)),
            _ => None,
        })
        .collect();
    assert_eq!(
        hops,
        vec![(ENGINEERING.to_string(), 0), (CONTENT.to_string(), 1)]
    );

    // Only the question crossed, under the referral author; only the answer
    // came home, under the same author. No seat spoke on the other desk.
    let content = replies(&rows, CONTENT);
    let seed = content.first().expect("the far desk starts with the seed");
    assert_eq!(seed.agent, HIVE_REFERRAL_AUTHOR);
    assert!(seed.text.contains(QUESTION), "{}", seed.text);
    assert!(
        content
            .iter()
            .all(|row| [WRITER, CEO, HIVE_REFERRAL_AUTHOR].contains(&row.agent.as_str())),
        "{content:?}"
    );
    let engineering = replies(&rows, ENGINEERING);
    assert!(
        engineering
            .iter()
            .all(|row| [ENGINEER, CEO, HIVE_REFERRAL_AUTHOR].contains(&row.agent.as_str())),
        "{engineering:?}"
    );
    let answer = engineering
        .iter()
        .find(|row| row.agent == HIVE_REFERRAL_AUTHOR)
        .unwrap();
    assert!(answer.text.contains(TAGLINE), "{}", answer.text);
    assert!(
        !answer.text.contains("Drafting the tagline"),
        "the far desk's working rows stay there: {}",
        answer.text
    );

    // The asker was reopened with the answer.
    let reopened = script
        .asks()
        .iter()
        .filter_map(seat_of)
        .find(|seat| {
            seat.speaker == ENGINEER && seat.prompt.contains("answered the question you put to it")
        })
        .expect("the engineer's reopened turn");
    // The assignment cites the answer row; the row itself reaches the seat
    // in the desk delta, attributed to the referral author.
    let delta = reopened
        .prompt
        .rsplit_once("## New desk messages\n")
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default();
    assert!(
        delta.contains(TAGLINE) && delta.contains(HIVE_REFERRAL_AUTHOR),
        "{}",
        reopened.prompt
    );

    let measured = report(&runtime).await;
    assert_eq!(measured.cross_desk_referrals, 1);
    assert_eq!(measured.referral_pairs, vec!["engineering→content"]);
    assert_eq!(measured.episodes_completed, 2);
    assert_eq!(measured.same_agent_overlaps, 0);
}

// ---------------------------------------------------------------------------
// 6: a shared agent on two desks
// ---------------------------------------------------------------------------

/// Whether two open brackets on different desks ever coincided.
fn cross_desk_overlap(rows: &[StoredEvent]) -> bool {
    let mut open: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for row in rows {
        match &row.event {
            CompanyEvent::TurnStarted {
                turn_id,
                chat_id,
                episode_id: Some(_),
                ..
            } => {
                if open.values().any(|desk| desk != chat_id) {
                    return true;
                }
                open.insert(turn_id.clone(), chat_id.clone());
            }
            CompanyEvent::TurnSettled { turn_id, .. }
            | CompanyEvent::TurnFailed { turn_id, .. } => {
                open.remove(turn_id);
            }
            _ => {}
        }
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_agent_on_two_desks_runs_both_rooms_without_running_twice() {
    let home = tempfile::tempdir().unwrap();
    let (base_url, script) = spawn_script_with_latency(
        seat_script("Noted.", post_then_complete),
        Duration::from_millis(300),
    )
    .await;
    let (address, runtime) = boot_demo(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(DEMO_ADMIN).await;

    // Both desks at once; the CEO sits on both.
    let (first, second) = tokio::join!(
        client.say(ENGINEERING, "Plan the staging rollout."),
        client.say(CONTENT, "Draft the release note."),
    );
    assert!(first["responses"].is_array() && second["responses"].is_array());
    let rows = wait_for(&runtime, "both episodes to complete", EPISODE, completed(2)).await;

    let done = completions(&rows);
    assert_eq!(done.len(), 2, "{done:?}");
    assert!(
        done.iter()
            .all(|(_, _, reason, _)| *reason == EpisodeReason::CompleteEpisode),
        "{done:?}"
    );
    let desks: std::collections::BTreeSet<&str> =
        done.iter().map(|(_, chat, _, _)| chat.as_str()).collect();
    assert_eq!(desks, [ENGINEERING, CONTENT].into_iter().collect());

    // The brackets: overlap across desks, never for the CEO.
    assert!(
        cross_desk_overlap(&rows),
        "two desks' brackets never coincided: {:?}",
        rows.iter().map(|row| row.event.kind()).collect::<Vec<_>>()
    );
    let measured = report(&runtime).await;
    assert_eq!(
        measured.same_agent_overlaps, 0,
        "the CEO ran on two desks and never twice at once: {measured:?}"
    );
    assert!(measured.max_concurrent_turns >= 2, "{measured:?}");
    assert!(
        script.peak_in_flight() >= 2,
        "the model saw both desks at once: {}",
        script.peak_in_flight()
    );
    assert_eq!(measured.episodes_completed, 2);
    let ceo_turns = rows
        .iter()
        .filter(|row| {
            matches!(
                &row.event,
                CompanyEvent::TurnStarted { agent_id: Some(agent), .. } if agent == CEO
            )
        })
        .count();
    assert_eq!(ceo_turns, 4, "the CEO took two turns on each desk");
    let failures = measured.failures(&Thresholds {
        cross_desk_referrals: 0,
        agent_contacts: 0,
        distinct_pairs: 0,
        ..Thresholds::default()
    });
    assert!(failures.is_empty(), "{failures:?}");
}

// ---------------------------------------------------------------------------
// 7: a checkpoint replays the rows after it as a no-op
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkpoint_replays_the_rows_after_it_as_a_no_op() {
    use opencompany::hive::episode_store::{PersistedEpisode, latest_state, replies_after};
    use opencompany::hive::round::utterance_of;
    use tinyhivemind::Sequence;
    use tinyhivemind_driver::{CommittedUtterance, CompletionDriver, DriverState};

    let home = tempfile::tempdir().unwrap();
    let (base_url, _script) = spawn_script_with_latency(
        seat_script("Noted.", post_then_complete),
        Duration::from_millis(30),
    )
    .await;
    let (address, runtime) = boot_lab(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client.say(ENGINEERING, "Settle the rollout window.").await;
    let rows = wait_for(&runtime, "the episode to complete", EPISODE, completed(1)).await;
    let episode_id = completions(&rows)[0].0.clone();

    // Every checkpoint the host wrote, oldest first: one per committed round.
    let checkpoints: Vec<PersistedEpisode> = rows
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::EpisodeStateSaved {
                episode_id: id,
                desk,
                thread_root,
                revision,
                state,
                sharing,
                hop,
                origin,
            } if id == &episode_id => Some(PersistedEpisode {
                episode_id: id.clone(),
                desk: desk.clone(),
                thread_root: *thread_root,
                revision: *revision,
                state: state.clone(),
                sharing: sharing
                    .iter()
                    .filter_map(|(agent, value)| {
                        serde_json::from_value(value.clone())
                            .ok()
                            .map(|state| (agent.clone(), state))
                    })
                    .collect(),
                hop: *hop,
                origin: origin
                    .clone()
                    .and_then(|value| serde_json::from_value(value).ok()),
            }),
            _ => None,
        })
        .collect();
    assert_eq!(
        checkpoints.len(),
        2,
        "one per round: {:?}",
        checkpoints.len()
    );
    assert_eq!(checkpoints[0].revision, 2);
    assert_eq!(checkpoints[1].revision, 4);
    let latest = latest_state(runtime.events().as_ref(), runtime.id(), &episode_id)
        .await
        .unwrap()
        .expect("a checkpoint");
    assert_eq!(latest.revision, 4, "the store hands back the newest");

    // A host that died after the first round's commit finds the round-0
    // checkpoint and the rows the second round committed. Folding those
    // through the driver reaches the final state; folding them again
    // changes nothing.
    let record = runtime
        .store()
        .load(runtime.id())
        .await
        .unwrap()
        .expect("the record");
    let pool = runtime.harness().expect("the harness pool");
    let mut agents = std::collections::HashMap::new();
    for agent in [ENGINEER, CEO] {
        let live = pool
            .agent(runtime.id(), agent)
            .await
            .unwrap_or_else(|| panic!("{agent} is built"));
        agents.insert(agent.to_string(), live.runtime_agent().clone());
    }
    let (hives, errors) =
        opencompany::hive::graph::desk_hives(&record, 1, &|id| agents.get(id).cloned());
    assert!(errors.is_empty(), "{errors:?}");
    let desk = hives.get(ENGINEERING).expect("the engineering hive");
    let driver = CompletionDriver::new(&desk.hive, 2).unwrap();
    let mut state: DriverState = serde_json::from_value(checkpoints[0].state.clone()).unwrap();
    state = driver.resume(state).unwrap();
    assert_eq!(state.revision(), 2);
    let later = replies_after(
        runtime.events().as_ref(),
        runtime.id(),
        &episode_id,
        checkpoints[0].revision,
    )
    .await
    .unwrap();
    assert_eq!(later.len(), 2, "the second round's two rows: {later:?}");
    let committed: Vec<CommittedUtterance> = later
        .iter()
        .map(|reply| CommittedUtterance {
            author_id: reply.agent_id.clone(),
            sequence: Sequence(reply.seq.value()),
            utterance: utterance_of(&reply.episode, reply.text.clone()),
        })
        .collect();
    for event in &committed {
        state = driver
            .apply_committed(&state, event.clone(), None)
            .await
            .unwrap()
            .state;
    }
    let final_state: DriverState = serde_json::from_value(checkpoints[1].state.clone()).unwrap();
    assert_eq!(
        state, final_state,
        "the replay reaches the final checkpoint"
    );
    for event in &committed {
        let again = driver
            .apply_committed(&state, event.clone(), None)
            .await
            .unwrap();
        assert_eq!(again.state, state, "an exact replay is a no-op");
        assert!(again.actions.is_empty());
    }
    // And a checkpoint that already folded everything has nothing to replay.
    let nothing = replies_after(runtime.events().as_ref(), runtime.id(), &episode_id, 4)
        .await
        .unwrap();
    assert!(nothing.is_empty(), "{nothing:?}");
}

// ---------------------------------------------------------------------------
// 8: memory over the MCP memory tool
// ---------------------------------------------------------------------------

const FACT: &str = "The rollout window is Tuesday 09:00 UTC, agreed with support.";
const FACT_KEY: &str = "Tuesday 09:00 UTC";
const ASK_ONE: &str = "Fix the rollout window.";
const ASK_TWO: &str = "When is the rollout window?";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desk_remembers_across_episodes_through_the_mcp_memory_tool() {
    let home = tempfile::tempdir().unwrap();
    let (base_url, script) = spawn_script_with_latency(
        seat_script("Noted.", |seat| {
            if seat.speaker != ENGINEER || seat.stage != 0 {
                return post_then_complete(seat);
            }
            let memory = seat.last_tool_result();
            if seat.operator_asked() == ASK_ONE {
                return match memory {
                    None => Reply::Call {
                        tool: "mcp_call_tool",
                        args: via_opencompany_mcp(
                            "memory_store",
                            json!({ "title": "rollout window", "body": FACT }),
                        )
                        .1,
                    },
                    Some(_) => post("Window fixed and written down."),
                };
            }
            if seat.operator_asked() == ASK_TWO {
                return match memory {
                    None => Reply::Call {
                        tool: "mcp_call_tool",
                        args: via_opencompany_mcp(
                            "memory_recall",
                            json!({ "query": "rollout window" }),
                        )
                        .1,
                    },
                    // Cite what memory handed back, never a constant.
                    Some(recalled) if recalled.contains(FACT_KEY) => {
                        post(format!("From the desk's memory: {FACT_KEY}."))
                    }
                    Some(recalled) => post(format!("Memory had nothing: {recalled}")),
                };
            }
            post_then_complete(seat)
        }),
        Duration::from_millis(30),
    )
    .await;
    let (address, runtime) = boot_lab(home.path(), &base_url).await;
    let client = Client::new(address);
    client.sign_in(ADMIN).await;

    client.say(ENGINEERING, ASK_ONE).await;
    let rows = wait_for(&runtime, "the first episode", EPISODE, completed(1)).await;
    assert!(
        replies(&rows, ENGINEERING)
            .iter()
            .any(|row| row.agent == ENGINEER && row.text.contains("written down")),
        "{:?}",
        replies(&rows, ENGINEERING)
    );
    let stored = script
        .asks()
        .iter()
        .filter_map(seat_of)
        .filter(|seat| seat.speaker == ENGINEER && seat.operator_asked() == ASK_ONE)
        .flat_map(|seat| seat.turn_tools)
        .collect::<Vec<_>>();
    assert!(
        stored.iter().any(|output| !is_refused(output)),
        "memory_store answered over MCP: {stored:?}"
    );

    client.say(ENGINEERING, ASK_TWO).await;
    let rows = wait_for(&runtime, "the second episode", EPISODE, completed(2)).await;
    dump(&rows, &script);
    let cited = replies(&rows, ENGINEERING)
        .into_iter()
        .rev()
        .find(|row| row.agent == ENGINEER && row.kind == Some(UtteranceKind::Post))
        .expect("the engineer's second-episode post");
    assert!(
        cited.text.contains(FACT_KEY),
        "the post cites what memory_recall returned: {}",
        cited.text
    );
    let recalled = script
        .asks()
        .iter()
        .filter_map(seat_of)
        .filter(|seat| seat.speaker == ENGINEER && seat.operator_asked() == ASK_TWO)
        .flat_map(|seat| seat.turn_tools)
        .collect::<Vec<_>>();
    assert!(
        recalled.iter().any(|output| output.contains(FACT_KEY)),
        "memory_recall came back with the fact: {recalled:?}"
    );
    assert_eq!(report(&runtime).await.episodes_completed, 2);
}
