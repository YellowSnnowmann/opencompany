//! A company hive on a real company, for integration targets that drive a
//! desk through the chat route and read what its agents did (OC-2).
//!
//! [`Room::boot`] stands up the company behind the production router with a
//! platform credential, so a message can be sent the way a machine client
//! sends it. An operator line is handed to the company hive's Coordinator and
//! answered on the hive's own task, so a test waits on the journal
//! ([`Room::wait_for`]) rather than on the chat route's answer.
//!
//! The scripted model reads each coordinator turn off the prompt the
//! OpenHuman host renders for it — `Incoming attributed Hivemind context
//! (JSON).` followed by the `TurnRequest` — with [`hive_turn`], and
//! [`hive_script`] hands that to a test's closure.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use opencompany::CompanyRuntime;
use opencompany::app::{AppConfig, AppState};
use opencompany::company::CompanyManifest;
use opencompany::ports::types::{CompanyEvent, CompanyId, EventSeq, StoredEvent};
use opencompany::runtime::RuntimeBuilder;
use opencompany::server::platform_auth::{PlatformAuthConfig, StaticPlatformVerifier};

use super::script_model::{Ask, Reply, Responder};

/// The platform credential every request carries.
pub const TOKEN: &str = "room-platform-token";

/// The line the OpenHuman host opens every coordinator turn's prompt with.
const HIVE_CONTEXT: &str = "Incoming attributed Hivemind context (JSON).";

/// One coordinator turn, read off a request.
#[derive(Clone, Debug)]
pub struct HiveTurn {
    /// The manifest agent id the turn runs as.
    pub agent: String,
    /// The episode the turn was assigned in, absent for a direct message.
    pub episode: Option<String>,
    /// The hive (desk) that episode is in.
    pub hive: Option<String>,
    /// The attributed messages delivered to the turn: `(sender, body)`, the
    /// sender a manifest agent id or `"host"` for an operator line.
    pub messages: Vec<(String, String)>,
    /// The host's release note, when the turn follows an approval decision.
    pub resumption: Option<String>,
    /// The tools this turn has already called, in order.
    pub called: Vec<String>,
    /// What those calls answered, in order.
    pub results: Vec<String>,
}

impl HiveTurn {
    /// Whether the turn has made any tool call yet.
    pub fn acted(&self) -> bool {
        !self.called.is_empty()
    }

    /// The newest delivered message's body.
    pub fn last_body(&self) -> &str {
        self.messages
            .last()
            .map(|(_, body)| body.as_str())
            .unwrap_or_default()
    }
}

/// The manifest id behind a coordinator agent id (`{company}--{agent}`).
pub fn manifest_id(coordinator_id: &str) -> String {
    coordinator_id
        .rsplit("--")
        .next()
        .unwrap_or(coordinator_id)
        .to_string()
}

/// The coordinator turn `ask` is, when it is one.
pub fn hive_turn(ask: &Ask) -> Option<HiveTurn> {
    let opened = ask.messages.iter().rposition(|message| {
        message.get("role").and_then(Value::as_str) == Some("user")
            && message
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains(HIVE_CONTEXT))
    })?;
    let text = ask.messages[opened]
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let after = &text[text.find(HIVE_CONTEXT)? + HIVE_CONTEXT.len()..];
    let json_start = after.find('{')?;
    let request: Value = serde_json::from_str(after[json_start..].trim()).ok()?;
    let agent = manifest_id(request.get("agent_id")?.as_str()?);
    let episode = request.get("episode").filter(|e| !e.is_null());
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    let sender = message["sender"].as_str().unwrap_or_default();
                    let sender = if sender.contains("--") {
                        manifest_id(sender)
                    } else {
                        "host".to_string()
                    };
                    (sender, message["body"].as_str().unwrap_or_default().to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    let mut called = Vec::new();
    let mut results = Vec::new();
    for message in &ask.messages[opened + 1..] {
        match message.get("role").and_then(Value::as_str) {
            Some("assistant") => {
                for call in message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(name) = call["function"]["name"].as_str() {
                        called.push(name.to_string());
                    }
                }
            }
            Some("tool") => results.push(
                message
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            _ => {}
        }
    }
    Some(HiveTurn {
        agent,
        episode: episode.and_then(|e| e["episode_id"].as_str().map(str::to_string)),
        hive: episode.and_then(|e| e["hive_id"].as_str().map(str::to_string)),
        messages,
        resumption: request
            .get("resumption")
            .and_then(Value::as_str)
            .map(str::to_string),
        called,
        results,
    })
}

/// Completes the turn's episode with `body`.
pub fn complete(turn: &HiveTurn, body: impl Into<String>) -> Reply {
    Reply::Call {
        tool: "hivemind_complete",
        args: json!({
            "episode_id": turn.episode.clone().unwrap_or_default(),
            "body": body.into(),
        }),
    }
}

/// Sends `body` to teammate `agent` directly.
pub fn send_agent(agent: &str, message_id: &str, body: impl Into<String>) -> Reply {
    Reply::Call {
        tool: "hivemind_send_agent",
        args: json!({ "agent_id": agent, "message_id": message_id, "body": body.into() }),
    }
}

/// One call to a tool on the agent's own belt.
pub fn call(tool: &'static str, args: Value) -> Reply {
    Reply::Call { tool, args }
}

/// A script over coordinator turns: `act` decides every model call a turn
/// makes, and a request that is no coordinator turn (a triage or a title, say)
/// answers `"Noted."`.
pub fn hive_script(act: impl Fn(&HiveTurn) -> Reply + Send + Sync + 'static) -> Responder {
    Arc::new(move |request: &Ask| {
        let Some(turn) = hive_turn(request) else {
            return Reply::Say("Noted.".to_string());
        };
        if std::env::var_os("ROOM_TURNS").is_some() {
            eprintln!(
                "[turn] {} episode={:?} hive={:?} messages={:?} called={:?} results={:?}",
                turn.agent, turn.episode, turn.hive, turn.messages, turn.called, turn.results
            );
        }
        act(&turn)
    })
}

/// The ordinary turn: complete an episode with `answer` and then say so, or
/// answer a direct message with `answer` itself.
pub fn answer(turn: &HiveTurn, answer: impl Into<String>) -> Reply {
    let answer = answer.into();
    match (&turn.episode, turn.acted()) {
        (Some(_), false) => complete(turn, answer),
        (Some(_), true) => Reply::Say("Done.".to_string()),
        (None, _) => Reply::Say(answer),
    }
}

/// A company running on loopback behind the production router.
pub struct Room {
    pub base: String,
    pub company: String,
    pub runtime: Arc<CompanyRuntime>,
    client: reqwest::Client,
}

impl Room {
    /// Boots `manifest` under `company_id` with its data under `home`.
    pub async fn boot(home: &Path, company_id: &str, manifest: &str) -> Self {
        let manifest =
            CompanyManifest::from_stored_toml(manifest).expect("the room's manifest parses");
        Self::boot_manifest(home, company_id, manifest).await
    }

    /// Boots an already parsed `manifest` under `company_id`.
    pub async fn boot_manifest(
        home: &Path,
        company_id: &str,
        mut manifest: CompanyManifest,
    ) -> Self {
        manifest.apply_globals();
        let problems = manifest.validate();
        assert!(problems.is_empty(), "the manifest is valid: {problems:?}");
        let runtime = Arc::new(
            RuntimeBuilder::new(home.to_path_buf(), manifest)
                .with_id(CompanyId::new(company_id))
                .with_harness(Arc::new(opencompany::harness::HarnessPool::new()))
                .build()
                .await
                .expect("the company builds"),
        );
        let state = AppState::new(AppConfig {
            bind: "127.0.0.1:0".to_string(),
            ..AppConfig::default()
        })
        .with_home(home.to_path_buf())
        .with_platform_auth(PlatformAuthConfig::new(Arc::new(
            StaticPlatformVerifier::new(TOKEN),
        )));
        state
            .registry()
            .insert(CompanyId::new(company_id), Arc::clone(&runtime));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, opencompany::server::router(state)).await;
        });
        Self {
            base: format!("http://{address}"),
            company: company_id.to_string(),
            runtime,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .unwrap(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1/companies/{}{path}", self.base, self.company)
    }

    /// Posts `body` to `path`, returning the status and the JSON answer.
    pub async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self
            .client
            .post(self.url(path))
            .bearer_auth(TOKEN)
            .json(&body)
            .send()
            .await
            .expect("the loopback host answers");
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    /// Reads `path`, returning the status and the JSON answer.
    pub async fn get(&self, path: &str) -> (u16, Value) {
        let response = self
            .client
            .get(self.url(path))
            .bearer_auth(TOKEN)
            .send()
            .await
            .expect("the loopback host answers");
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    /// Sends `text` to `chat` (a desk id, `general`, or a teammate's DM).
    pub async fn say(&self, chat: &str, text: &str) -> Value {
        let (status, answer) = self
            .post("/chat", json!({ "text": text, "chat": chat }))
            .await;
        assert_eq!(status, 200, "chat refused: {answer}");
        answer
    }

    /// The board as the console reads it.
    pub async fn cards(&self) -> Vec<Value> {
        let (status, body) = self.get("/tasks").await;
        assert_eq!(status, 200, "{body}");
        body.as_array().cloned().unwrap_or_default()
    }

    /// Every journal row.
    pub async fn journal(&self) -> Vec<StoredEvent> {
        self.runtime
            .events()
            .read_from(self.runtime.id(), EventSeq::new(0), 100_000)
            .await
            .expect("the journal reads back")
    }

    /// Polls the journal until `done` holds, or fails after `timeout`.
    pub async fn wait_for(
        &self,
        what: &str,
        timeout: Duration,
        done: impl Fn(&[StoredEvent]) -> bool,
    ) -> Vec<StoredEvent> {
        let started = Instant::now();
        loop {
            let rows = self.journal().await;
            if done(&rows) {
                return rows;
            }
            if started.elapsed() >= timeout {
                if std::env::var_os("ROOM_DUMP").is_some() {
                    for row in &rows {
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
                }
                panic!(
                    "timed out waiting for {what}; journal kinds: {:?}",
                    rows.iter().map(|row| row.event.kind()).collect::<Vec<_>>()
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Waits until `n` hive episodes have settled.
    pub async fn episodes_settled(&self, n: usize, timeout: Duration) -> Vec<StoredEvent> {
        self.wait_for("the episodes to settle", timeout, |rows| {
            settled(rows).len() >= n
        })
        .await
    }
}

/// Every settled episode: `(hive_id, failure)`.
pub fn settled(rows: &[StoredEvent]) -> Vec<(String, Option<String>)> {
    rows.iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::HiveEpisodeSettled {
                hive_id, failure, ..
            } => Some((hive_id.clone(), failure.clone())),
            _ => None,
        })
        .collect()
}

/// The sequence of the operator's newest message on `chat`.
pub fn operator_message(rows: &[StoredEvent], chat: &str) -> Option<u64> {
    rows.iter()
        .rev()
        .find(|row| {
            matches!(&row.event, CompanyEvent::OperatorMessage { chat: Some(c), .. } if c == chat)
        })
        .map(|row| row.seq.value())
}
