//! A HiveMind room on a real company, for integration targets that drive a
//! desk through the chat route and read what its seats did.
//!
//! [`Room::boot`] stands up the company behind the production router with a
//! platform credential, so a message can be sent the way a machine client
//! sends it (a card the chat handler opens then lands in To-do). The scripted
//! model reads each seat turn off the episode brief: [`seat_of`] says who is
//! speaking, in which thread, what the brief showed them and what the turn
//! has already called, and [`room_script`] hands that to a test's closure.
//!
//! The seat is identified by the persona's `You are the <role> at` line, so a
//! test passes the `(id, role)` pairs its manifest declares.

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

/// The line the episode brief ends a seat turn with, naming its desk.
const FENCE: &str = "Every tool call must carry \"chat\": \"";
/// The rest of that line, naming the thread.
const FENCE_PARENT: &str = "and \"parent\": ";
/// The brief's heading over what the seat has not seen yet.
const DESK_MESSAGES: &str = "## New desk messages\n";
/// A line only a settled episode's closing turn is given.
const CLOSING: &str = "Your one job is to say what it all adds up to";

/// One seat turn, read off a request.
#[derive(Clone, Debug)]
pub struct Seat {
    /// The desk the turn runs on.
    pub desk: String,
    /// The thread the turn speaks in; `None` on the desk itself.
    pub parent: Option<u64>,
    /// Whose turn it is.
    pub speaker: String,
    /// The tools this turn already called, oldest first.
    pub calls: Vec<String>,
    /// Their results, in the same order.
    pub results: Vec<String>,
    /// The brief, verbatim.
    pub prompt: String,
    /// The `## New desk messages` block alone.
    pub assignment: String,
    /// Whether this is a settled episode's closing turn.
    pub closing: bool,
}

impl Seat {
    /// Whether `tool` was called this turn.
    pub fn called(&self, tool: &str) -> bool {
        self.calls.iter().any(|call| call == tool)
    }

    /// How many times `tool` was called this turn.
    pub fn times(&self, tool: &str) -> usize {
        self.calls.iter().filter(|call| *call == tool).count()
    }

    /// The results `tool` returned this turn.
    pub fn results_of(&self, tool: &str) -> Vec<&str> {
        self.calls
            .iter()
            .zip(&self.results)
            .filter(|(call, _)| *call == tool)
            .map(|(_, result)| result.as_str())
            .collect()
    }

    /// The operator's newest words in the brief, or empty.
    pub fn operator_asked(&self) -> &str {
        self.assignment
            .lines()
            .filter_map(|line| line.strip_prefix("@operator: "))
            .next_back()
            .unwrap_or_default()
    }

    /// Whether the turn has recorded its part or opened a conversation.
    pub fn spoke(&self) -> bool {
        self.called("desk_complete_episode") || self.called("desk_ask")
    }
}

fn role(message: &Value) -> &str {
    message.get("role").and_then(Value::as_str).unwrap_or("")
}

fn content(message: &Value) -> &str {
    message.get("content").and_then(Value::as_str).unwrap_or("")
}

/// The seat turn `ask` is, when it is one.
pub fn seat_of(ask: &Ask, roles: &[(&str, &str)]) -> Option<Seat> {
    let last_user = ask
        .messages
        .iter()
        .rposition(|message| role(message) == "user")?;
    let prompt = content(&ask.messages[last_user]).to_string();
    let at = prompt.rfind(FENCE)?;
    let desk = prompt[at + FENCE.len()..].split_once('"')?.0.to_string();
    let parent = prompt.rfind(FENCE_PARENT).and_then(|at| {
        let rest = &prompt[at + FENCE_PARENT.len()..];
        rest.split_once('.')?
            .0
            .trim()
            .trim_matches('"')
            .parse()
            .ok()
    });
    let role_text = ask.messages.iter().rev().find_map(|message| {
        let (_, opening) = content(message).rsplit_once("You are the ")?;
        Some(opening.split_once(" at ")?.0.trim().to_string())
    })?;
    let speaker = roles
        .iter()
        .find(|(_, title)| *title == role_text)
        .map(|(id, _)| (*id).to_string())?;
    let closing = ask
        .messages
        .iter()
        .any(|message| role(message) == "system" && content(message).contains(CLOSING))
        || prompt.contains(CLOSING);
    let assignment = prompt
        .rsplit_once(DESK_MESSAGES)
        .map(|(_, rest)| rest.split("\n\n").next().unwrap_or(rest).to_string())
        .unwrap_or_default();
    let after = &ask.messages[last_user + 1..];
    let results = after
        .iter()
        .filter(|message| role(message) == "tool")
        .map(|message| content(message).to_string())
        .collect();
    let calls = after
        .iter()
        .filter(|message| role(message) == "assistant")
        .filter_map(|message| message.get("tool_calls").and_then(Value::as_array))
        .flatten()
        .filter_map(|call| Some(call.get("function")?.get("name")?.as_str()?.to_string()))
        .collect();
    Some(Seat {
        desk,
        parent,
        speaker,
        calls,
        results,
        prompt,
        assignment,
        closing,
    })
}

/// One speech act, carrying the desk and thread the fence named.
pub fn speech(tool: &str, seat: &Seat, mut arguments: Value) -> Reply {
    if let Some(object) = arguments.as_object_mut() {
        object.insert("chat".to_string(), json!(seat.desk));
        object.insert(
            "parent".to_string(),
            seat.parent
                .map_or(Value::Null, |seq| json!(seq.to_string())),
        );
    }
    Reply::Call {
        tool: Box::leak(format!("desk_{tool}").into_boxed_str()),
        args: arguments,
    }
}

/// Records the seat's part.
pub fn complete(seat: &Seat, message: impl Into<String>) -> Reply {
    speech(
        "complete_episode",
        seat,
        json!({ "message": message.into() }),
    )
}

/// Opens a conversation with `to`.
pub fn ask(seat: &Seat, to: &str, message: impl Into<String>) -> Reply {
    speech("ask", seat, json!({ "to": to, "message": message.into() }))
}

/// One call to a tool on the seat's own belt.
pub fn call(tool: &'static str, args: Value) -> Reply {
    Reply::Call { tool, args }
}

/// A script over seat turns: `act` decides what a seat turn that has not
/// spoken yet does; a turn that has spoken stops, and a request that is no
/// seat turn answers `"Noted."`.
pub fn room_script(
    roles: &'static [(&'static str, &'static str)],
    act: impl Fn(&Seat) -> Reply + Send + Sync + 'static,
) -> Responder {
    Arc::new(move |request: &Ask| {
        let Some(seat) = seat_of(request, roles) else {
            return Reply::Say("Noted.".to_string());
        };
        if std::env::var_os("ROOM_TURNS").is_some() {
            eprintln!(
                "[turn] {} parent={:?} closing={} calls={:?} results={:?}",
                seat.speaker, seat.parent, seat.closing, seat.calls, seat.results
            );
        }
        if seat.spoke() {
            return Reply::Say("done".to_string());
        }
        act(&seat)
    })
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
        let mut manifest =
            CompanyManifest::from_stored_toml(manifest).expect("the room's manifest parses");
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

    /// Sends `text` to `desk`.
    pub async fn say(&self, desk: &str, text: &str) -> Value {
        self.say_with(desk, text, json!({})).await
    }

    /// Sends `text` to `desk` with extra fields on the request.
    pub async fn say_with(&self, desk: &str, text: &str, extra: Value) -> Value {
        let mut body = json!({ "text": text, "chat": desk });
        if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                body.insert(key.clone(), value.clone());
            }
        }
        let (status, answer) = self.post("/chat", body).await;
        assert_eq!(status, 200, "chat refused: {answer}");
        answer
    }

    /// The board as the console reads it.
    pub async fn cards(&self) -> Vec<Value> {
        let (status, body) = self.get("/tasks").await;
        assert_eq!(status, 200, "{body}");
        body.as_array().cloned().unwrap_or_default()
    }

    /// The pending approvals.
    pub async fn approvals(&self) -> Vec<Value> {
        let (status, body) = self.get("/approvals").await;
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

    /// Waits until `n` episodes have completed.
    pub async fn episodes_completed(&self, n: usize, timeout: Duration) -> Vec<StoredEvent> {
        self.wait_for("the episodes to complete", timeout, |rows| {
            completed(rows) >= n
        })
        .await
    }
}

/// How many episodes the journal says completed.
pub fn completed(rows: &[StoredEvent]) -> usize {
    rows.iter()
        .filter(|row| matches!(row.event, CompanyEvent::EpisodeCompleted { .. }))
        .count()
}

/// The sequence of the operator's newest message on `desk`.
pub fn operator_message(rows: &[StoredEvent], desk: &str) -> Option<u64> {
    rows.iter()
        .rev()
        .find(|row| {
            matches!(&row.event, CompanyEvent::OperatorMessage { chat: Some(chat), .. } if chat == desk)
        })
        .map(|row| row.seq.value())
}
