//! The company hive's transcript, journaled as company events (OC-2).
//!
//! The Coordinator keeps its own durable transcript; the console, the history
//! projection, `opencompany measure` and every SSE client read the company
//! journal. The projector is the one bridge between them: it tails
//! [`Coordinator::read_transcript`] on every committed revision
//! ([`Coordinator::subscribe`]) and appends what each new row means:
//!
//! * a member's line on a hive, visible to every member → an
//!   [`AgentReply`](CompanyEvent::AgentReply) in that desk's chat, threaded
//!   under the line it answers and carrying a [`HiveRef`];
//! * a member's reply to an operator direct message (destination
//!   [`HOST_ID`]) → an `AgentReply` in that DM;
//! * a private (`only_for`) hive line, or a direct message between two
//!   agents → a [`HiveMessage`](CompanyEvent::HiveMessage);
//! * a host row (the operator's own line) → nothing: it is journaled already;
//!   the projector only learns which journal line it was, to thread replies.
//!
//! Then, from the Coordinator's own status reads, each finished episode once
//! as a [`HiveEpisodeSettled`](CompanyEvent::HiveEpisodeSettled) and each new
//! interruption as a [`HiveTurnInterrupted`](CompanyEvent::HiveTurnInterrupted).
//!
//! # Exactly once, across restarts
//!
//! The cursor is the highest hive sequence the journal already holds a row
//! for, recovered at boot by [`Projector::reconcile`] from the journal itself
//! — not kept beside it, so a crash between an append and a cursor write
//! cannot double a row. A failed append stops the batch; the next commit
//! retries from the last row that landed.
//!
//! What a turn produced beyond its text (its steps, published outputs, the
//! card it opened) is handed over by the turn hooks on a [`TurnMetaBoard`] and
//! attached to the next visible row that agent's turn commits.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, PoisonError, RwLock};

use tinyhivemind_hives::{Coordinator, Destination, EpisodePhase, HOST_ID, Message};

use crate::ports::events::EventLog;
use crate::ports::types::{
    ChatOutput, CompanyEvent, CompanyId, EventSeq, HiveDestination, HiveRef, TurnStep,
};

/// The roster as the projector reads it: Coordinator agent id → manifest id.
#[derive(Debug, Default)]
pub struct HiveRoster {
    agents: RwLock<HashMap<String, String>>,
}

impl HiveRoster {
    /// Replaces the map with the company's roster as it is now.
    pub fn install(&self, agents: HashMap<String, String>) {
        *self.agents.write().unwrap_or_else(PoisonError::into_inner) = agents;
    }

    /// The map as it stands.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, String> {
        self.agents
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The manifest id behind a Coordinator id. An id the roster no longer
    /// holds — a retired teammate's old row — loses its `{company}--` prefix
    /// when it has one, which is the id it was registered from.
    #[must_use]
    pub fn manifest_id(&self, company: &CompanyId, coordinator_id: &str) -> String {
        if let Some(id) = self
            .agents
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(coordinator_id)
        {
            return id.clone();
        }
        let prefix = format!("{company}--");
        coordinator_id
            .strip_prefix(&prefix)
            .unwrap_or(coordinator_id)
            .to_string()
    }

    /// The Coordinator id of a manifest agent, when it is on the roster.
    #[must_use]
    pub fn coordinator_id(&self, manifest_id: &str) -> Option<String> {
        self.agents
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(_, id)| id.as_str() == manifest_id)
            .map(|(coordinator, _)| coordinator.clone())
    }
}

/// What one coordinator turn produced beyond its text, for the row it
/// commits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnMeta {
    /// The tool-step timeline.
    pub steps: Vec<TurnStep>,
    /// The files the turn published.
    pub outputs: Vec<ChatOutput>,
    /// The card the turn opened or filed on.
    pub task_id: Option<String>,
}

impl TurnMeta {
    fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.outputs.is_empty() && self.task_id.is_none()
    }
}

/// Per-agent [`TurnMeta`] queues, filled by the turn hooks and drained by the
/// projector.
#[derive(Debug, Default)]
pub struct TurnMetaBoard {
    pending: std::sync::Mutex<HashMap<String, VecDeque<TurnMeta>>>,
}

impl TurnMetaBoard {
    /// Hands a finished turn's meta to the next row `agent` commits.
    pub fn push(&self, agent: &str, meta: TurnMeta) {
        if meta.is_empty() {
            return;
        }
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(agent.to_string())
            .or_default()
            .push_back(meta);
    }

    /// The oldest meta waiting for `agent`'s next row.
    pub fn take(&self, agent: &str) -> Option<TurnMeta> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(agent)
            .and_then(VecDeque::pop_front)
    }
}

/// Where one host or member row landed in the journal.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Landed {
    /// The journal line.
    seq: EventSeq,
    /// The console chat it is in.
    chat: String,
}

/// What the projector has already journaled.
#[derive(Debug, Default)]
pub struct Cursor {
    /// The highest hive sequence the journal holds a row for.
    sequence: Option<u64>,
    /// Hive sequence → the journal line it became, for threading.
    landed: HashMap<u64, Landed>,
    /// Coordinator agent id → the chat of the operator's latest DM to it.
    direct: HashMap<String, Landed>,
    /// Episodes already journaled as settled.
    settled: HashSet<String>,
    /// Interruptions already journaled.
    interruptions: usize,
}

impl Cursor {
    /// The highest projected hive sequence.
    #[must_use]
    pub fn sequence(&self) -> Option<u64> {
        self.sequence
    }

    fn advance(&mut self, sequence: u64) {
        self.sequence = Some(self.sequence.map_or(sequence, |held| held.max(sequence)));
    }
}

/// Journals one company's Coordinator transcript.
pub struct Projector {
    company: CompanyId,
    events: Arc<dyn EventLog>,
    coordinator: Coordinator,
    roster: Arc<HiveRoster>,
    meta: Arc<TurnMetaBoard>,
    cursor: tokio::sync::Mutex<Cursor>,
}

impl std::fmt::Debug for Projector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Projector")
            .field("company", &self.company)
            .finish_non_exhaustive()
    }
}

/// How many journal rows one reconcile page reads.
const RECONCILE_PAGE: usize = 2_000;

impl Projector {
    /// A projector with an empty cursor; call [`reconcile`](Self::reconcile)
    /// before the first [`project`](Self::project).
    #[must_use]
    pub fn new(
        company: CompanyId,
        events: Arc<dyn EventLog>,
        coordinator: Coordinator,
        roster: Arc<HiveRoster>,
        meta: Arc<TurnMetaBoard>,
    ) -> Self {
        Self {
            company,
            events,
            coordinator,
            roster,
            meta,
            cursor: tokio::sync::Mutex::new(Cursor::default()),
        }
    }

    /// Rebuilds the cursor from the journal: every row a previous process
    /// projected names the hive sequence it came from.
    pub async fn reconcile(&self) -> crate::Result<()> {
        let mut cursor = Cursor::default();
        let mut from = EventSeq::new(0);
        loop {
            let page = self
                .events
                .read_from(&self.company, from, RECONCILE_PAGE)
                .await?;
            let Some(last) = page.last().map(|row| row.seq) else {
                break;
            };
            for row in &page {
                fold(&mut cursor, row.seq, &row.event, &self.roster);
            }
            if page.len() < RECONCILE_PAGE {
                break;
            }
            from = EventSeq::new(last.value() + 1);
        }
        *self.cursor.lock().await = cursor;
        Ok(())
    }

    /// Records a host line the brain just sent, so a reply that is projected
    /// before its [`HiveAccepted`](CompanyEvent::HiveAccepted) row lands still
    /// threads under it. Idempotent with the journal fold.
    pub async fn note_host_line(
        &self,
        sequence: u64,
        chat: &str,
        source: Option<EventSeq>,
        direct_to: Option<&str>,
    ) {
        let mut cursor = self.cursor.lock().await;
        if let Some(seq) = source {
            let landed = Landed {
                seq,
                chat: chat.to_string(),
            };
            cursor.landed.insert(sequence, landed.clone());
            if let Some(agent) = direct_to {
                cursor.direct.insert(agent.to_string(), landed);
            }
        }
    }

    /// Projects everything the Coordinator committed since the cursor.
    /// Returns how many journal rows it appended.
    pub async fn project(&self) -> crate::Result<usize> {
        let mut cursor = self.cursor.lock().await;
        let rows = self
            .coordinator
            .read_transcript(cursor.sequence)
            .map_err(hive_error)?;
        let opened: HashMap<String, u64> = self
            .coordinator
            .episodes()
            .map_err(hive_error)?
            .iter()
            .map(|episode| (episode.episode_id.clone(), episode.opened_at))
            .collect();
        let mut appended = 0usize;
        for row in rows {
            let event = self.map_row(&mut cursor, &row, &opened);
            if let Some(event) = event {
                let chat = match &event {
                    CompanyEvent::AgentReply { chat_id, .. } => Some(chat_id.clone()),
                    _ => None,
                };
                let seq = self.events.append(&self.company, event).await?;
                appended += 1;
                if let Some(chat) = chat {
                    cursor.landed.insert(row.sequence, Landed { seq, chat });
                }
            }
            cursor.advance(row.sequence);
        }
        for episode in self.coordinator.episodes().map_err(hive_error)? {
            let failure = match episode.phase {
                EpisodePhase::Settled => None,
                EpisodePhase::Failed(reason) => Some(reason),
                EpisodePhase::Open | EpisodePhase::AwaitingRelease => continue,
            };
            if cursor.settled.contains(&episode.episode_id) {
                continue;
            }
            self.events
                .append(
                    &self.company,
                    CompanyEvent::HiveEpisodeSettled {
                        episode_id: episode.episode_id.clone(),
                        hive_id: episode.hive_id.clone(),
                        opened_at: episode.opened_at,
                        thread: episode.thread,
                        failure,
                    },
                )
                .await?;
            appended += 1;
            cursor.settled.insert(episode.episode_id);
        }
        let interruptions = self.coordinator.interruptions().map_err(hive_error)?;
        for interrupted in interruptions.iter().skip(cursor.interruptions) {
            self.events
                .append(
                    &self.company,
                    CompanyEvent::HiveTurnInterrupted {
                        agent_id: self.roster.manifest_id(&self.company, &interrupted.agent_id),
                        episode_id: interrupted.episode_id.clone(),
                        message_ids: interrupted.message_ids.clone(),
                        reason: interrupted.reason.clone(),
                    },
                )
                .await?;
            appended += 1;
            cursor.interruptions += 1;
        }
        Ok(appended)
    }

    /// What one transcript row becomes in the journal, if anything.
    fn map_row(
        &self,
        cursor: &mut Cursor,
        row: &Message,
        opened: &HashMap<String, u64>,
    ) -> Option<CompanyEvent> {
        if row.sender == HOST_ID {
            return None;
        }
        let sender = self.roster.manifest_id(&self.company, &row.sender);
        let hive = HiveRef {
            sequence: row.sequence,
            episode_id: row.episode_id.clone(),
            thread: row.thread,
        };
        match &row.destination {
            Destination::Agent(to) if to == HOST_ID => {
                let landed = cursor.direct.get(&row.sender).cloned();
                let chat = landed.map_or_else(
                    || format!("{}{sender}", crate::runtime::assignee::DM_PREFIX),
                    |landed| landed.chat,
                );
                Some(self.reply(chat, &row.sender, sender, &row.body, None, hive))
            }
            Destination::Agent(to) => Some(CompanyEvent::HiveMessage {
                sequence: row.sequence,
                sender,
                destination: HiveDestination::Agent(self.roster.manifest_id(&self.company, to)),
                text: row.body.clone(),
                thread: row.thread,
                episode_id: row.episode_id.clone(),
                only_for: Vec::new(),
            }),
            Destination::Hive(hive_id) if !row.only_for.is_empty() => Some(CompanyEvent::HiveMessage {
                sequence: row.sequence,
                sender,
                destination: HiveDestination::Hive(hive_id.clone()),
                text: row.body.clone(),
                thread: row.thread,
                episode_id: row.episode_id.clone(),
                only_for: row
                    .only_for
                    .iter()
                    .map(|reader| self.roster.manifest_id(&self.company, reader))
                    .collect(),
            }),
            Destination::Hive(hive_id) => {
                // A row answers its thread; an episode's unthreaded row
                // answers the message that opened the episode.
                let root = row.thread.or_else(|| {
                    row.episode_id
                        .as_ref()
                        .and_then(|episode| opened.get(episode).copied())
                });
                let parent = root
                    .and_then(|root| cursor.landed.get(&root))
                    .filter(|landed| &landed.chat == hive_id)
                    .map(|landed| landed.seq);
                Some(self.reply(hive_id.clone(), &row.sender, sender, &row.body, parent, hive))
            }
        }
    }

    fn reply(
        &self,
        chat_id: String,
        coordinator_id: &str,
        agent_id: String,
        text: &str,
        parent: Option<EventSeq>,
        hive: HiveRef,
    ) -> CompanyEvent {
        let meta = self.meta.take(coordinator_id).unwrap_or_default();
        CompanyEvent::AgentReply {
            chat_id,
            agent_id,
            text: text.to_string(),
            steps: meta.steps,
            outputs: meta.outputs,
            task_id: meta.task_id,
            parent,
            mentions: Vec::new(),
            mention_depth: 0,
            audience: Vec::new(),
            hive: Some(hive),
        }
    }

    /// The cursor as it stands, for tests and diagnostics.
    pub async fn cursor_sequence(&self) -> Option<u64> {
        self.cursor.lock().await.sequence()
    }
}

/// Folds one journal row into the cursor at reconcile.
fn fold(cursor: &mut Cursor, seq: EventSeq, event: &CompanyEvent, roster: &HiveRoster) {
    match event {
        CompanyEvent::HiveAccepted {
            sequence,
            chat_id,
            source,
            starters,
            ..
        } => {
            cursor.advance(*sequence);
            if let Some(source) = source {
                let landed = Landed {
                    seq: *source,
                    chat: chat_id.clone(),
                };
                cursor.landed.insert(*sequence, landed.clone());
                if starters.is_empty()
                    && let Some(agent) = crate::runtime::assignee::dm_key(chat_id)
                        .map(str::to_string)
                        .or_else(|| Some(chat_id.clone()))
                    && let Some(coordinator) = roster.coordinator_id(&agent)
                {
                    cursor.direct.insert(coordinator, landed);
                }
            }
        }
        CompanyEvent::AgentReply {
            chat_id,
            hive: Some(hive),
            ..
        } => {
            cursor.advance(hive.sequence);
            cursor.landed.insert(
                hive.sequence,
                Landed {
                    seq,
                    chat: chat_id.clone(),
                },
            );
        }
        CompanyEvent::HiveMessage { sequence, .. } => cursor.advance(*sequence),
        CompanyEvent::HiveEpisodeSettled { episode_id, .. } => {
            cursor.settled.insert(episode_id.clone());
        }
        CompanyEvent::HiveTurnInterrupted { .. } => cursor.interruptions += 1,
        _ => {}
    }
}

fn hive_error(error: tinyhivemind_hives::Error) -> crate::OpenCompanyError {
    crate::OpenCompanyError::Store(format!("company hive: {error}"))
}

#[cfg(test)]
#[path = "projector_tests.rs"]
mod tests;
