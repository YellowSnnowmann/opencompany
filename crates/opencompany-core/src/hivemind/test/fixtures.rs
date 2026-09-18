//! Shared fixtures for the hive-mind desk-seam unit tests: an in-memory
//! journal, a scripted turn runner, and small manifest/desk builders.

use std::sync::Mutex;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};

use super::super::*;
use crate::Result;
use crate::ports::events::{EventLog, EventStreamItem};
use crate::ports::types::{CompanyEvent, CompanyId, CompanyRecord, EventSeq, StoredEvent};

/// An in-memory journal, the smallest thing that satisfies the port.
///
/// `read_before` is implemented directly rather than inherited from the port's
/// forward-scan default, because the session adapter's paging is one of the
/// things under test and a default that reads the whole log would hide a cursor
/// bug rather than expose it.
/// `pub(crate)` rather than `pub(super)`: `harness::built_in::brain` needs an
/// `EventLog` to exercise what a referred room carries home, and this is the
/// crate's in-memory one. Still `#[cfg(test)]`, so nothing ships.
#[derive(Default)]
pub(crate) struct MemoryLog {
    events: Mutex<Vec<StoredEvent>>,
}

impl MemoryLog {
    pub(crate) fn company() -> CompanyId {
        CompanyId::new("acme")
    }

    fn rows(&self) -> Vec<StoredEvent> {
        self.events.lock().expect("journal poisoned").clone()
    }

    /// Every `AgentReply` on `chat`, as `(author, text)` in journal order.
    pub(crate) fn replies(&self, chat: &str) -> Vec<(String, String)> {
        self.rows()
            .into_iter()
            .filter_map(|stored| match stored.event {
                CompanyEvent::AgentReply {
                    chat_id,
                    agent_id,
                    text,
                    ..
                } if chat_id == chat => Some((agent_id, text)),
                _ => None,
            })
            .collect()
    }

    /// Every reply on `chat` with the audience it was journaled under.
    ///
    /// Separate from [`Self::replies`] rather than a widening of it: most tests
    /// are not about audience and reading a three-tuple would make them say so.
    pub(crate) fn addressed_replies(&self, chat: &str) -> Vec<(String, String, Vec<String>)> {
        self.rows()
            .into_iter()
            .filter_map(|stored| match stored.event {
                CompanyEvent::AgentReply {
                    chat_id,
                    agent_id,
                    text,
                    audience,
                    ..
                } if chat_id == chat => Some((agent_id, text, audience)),
                _ => None,
            })
            .collect()
    }
}

#[async_trait]
impl EventLog for MemoryLog {
    async fn append(&self, _id: &CompanyId, event: CompanyEvent) -> Result<EventSeq> {
        let mut events = self.events.lock().expect("journal poisoned");
        let seq = EventSeq::new(events.len() as u64 + 1);
        events.push(StoredEvent {
            seq,
            company: MemoryLog::company(),
            event,
            at_millis: 0,
        });
        Ok(seq)
    }

    async fn read_from(
        &self,
        _id: &CompanyId,
        seq: EventSeq,
        limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        Ok(self
            .rows()
            .into_iter()
            .filter(|stored| stored.seq.value() >= seq.value())
            .take(limit)
            .collect())
    }

    async fn read_before(
        &self,
        _id: &CompanyId,
        before: Option<EventSeq>,
        limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        let mut rows: Vec<StoredEvent> = self
            .rows()
            .into_iter()
            .filter(|stored| before.is_none_or(|cursor| stored.seq.value() < cursor.value()))
            .collect();
        rows.reverse();
        rows.truncate(limit);
        Ok(rows)
    }

    fn subscribe(&self, _id: &CompanyId) -> BoxStream<'static, EventStreamItem> {
        Box::pin(stream::empty())
    }
}

/// A participant that answers from a script, keyed by agent id.
///
/// A queue per agent rather than one flat list: the library decides who speaks,
/// so a test that scripted a flat sequence would be asserting the bid order by
/// accident and would break for reasons that have nothing to do with what it
/// meant to check.
pub(crate) struct ScriptedRunner {
    lines: Mutex<Vec<(String, String)>>,
    asked: Mutex<Vec<(String, String)>>,
}

impl ScriptedRunner {
    pub(crate) fn new(lines: &[(&str, &str)]) -> Self {
        Self {
            lines: Mutex::new(
                lines
                    .iter()
                    .map(|(id, line)| ((*id).to_owned(), (*line).to_owned()))
                    .collect(),
            ),
            asked: Mutex::new(Vec::new()),
        }
    }

    /// Every `(agent, prompt)` the episode asked for, in order.
    pub(crate) fn asked(&self) -> Vec<(String, String)> {
        self.asked.lock().expect("script poisoned").clone()
    }
}

#[async_trait]
impl HiveTurnRunner for ScriptedRunner {
    async fn speak(&self, agent_id: &str, prompt: &str) -> Result<String> {
        self.asked
            .lock()
            .expect("script poisoned")
            .push((agent_id.to_owned(), prompt.to_owned()));
        let mut lines = self.lines.lock().expect("script poisoned");
        let at = lines.iter().position(|(id, _)| id == agent_id);
        Ok(match at {
            Some(at) => lines.remove(at).1,
            // A member with nothing scripted left still has to say something —
            // the episode is entitled to a reply for every turn it authorizes.
            None => format!("!question {agent_id} has nothing further."),
        })
    }
}

pub(crate) fn record(manifest: &str) -> CompanyRecord {
    let manifest: crate::company::CompanyManifest =
        toml::from_str(manifest).expect("test manifest parses");
    CompanyRecord {
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        id: MemoryLog::company(),
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

/// Three teammates on one desk, all seated.
pub(crate) fn three_member_manifest() -> String {
    "[company]\nname = \"Acme\"\n\
     [[agent]]\nid = \"planner\"\nrole = \"Planner\"\n\
     [[agent]]\nid = \"scout\"\nrole = \"Scout\"\n\
     [[agent]]\nid = \"critic\"\nrole = \"Critic\"\n\
     [[group_chat]]\nid = \"eng\"\nname = \"Engineering\"\n\
     description = \"Ship the rollout\"\n\
     members = [\"planner\", \"scout\", \"critic\"]\n"
        .to_string()
}

pub(crate) fn desk_of(manifest: &str, chat: &str) -> Option<HiveDesk> {
    desk_episode(&record(manifest), Some(chat))
}
