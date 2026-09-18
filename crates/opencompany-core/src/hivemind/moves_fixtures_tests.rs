//! Tests for the per-member move grammar, desk memory, and speaker diversity.
//!
//! Everything here is scripted through [`HiveTurnRunner`]: no model, no store,
//! no provider. A room whose members are handed exact lines is the only way to
//! assert that a *barred* move is corrected and then demoted — a live room
//! would be asserting the model's compliance rather than this host's
//! enforcement.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::memory::{HiveMemory, HiveMemoryHit, HiveMemoryNote};
use super::test::MemoryLog;
use super::*;
use crate::Result;
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, EventSeq};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A room whose members answer from a per-agent queue, and which may be told to
/// fail one member's turn a fixed number of times.
pub(super) struct Runner {
    lines: Mutex<Vec<(String, String)>>,
    asked: Mutex<Vec<(String, String)>>,
    /// Agent id → how many of its next turns must fail.
    fail: Mutex<BTreeMap<String, usize>>,
}

impl Runner {
    pub(super) fn new(lines: &[(&str, &str)]) -> Self {
        Self {
            lines: Mutex::new(
                lines
                    .iter()
                    .map(|(id, line)| ((*id).to_owned(), (*line).to_owned()))
                    .collect(),
            ),
            asked: Mutex::new(Vec::new()),
            fail: Mutex::new(BTreeMap::new()),
        }
    }

    pub(super) fn failing(self, agent_id: &str, times: usize) -> Self {
        self.fail
            .lock()
            .expect("poisoned")
            .insert(agent_id.to_owned(), times);
        self
    }

    pub(super) fn asked(&self) -> Vec<(String, String)> {
        self.asked.lock().expect("poisoned").clone()
    }

    /// Every prompt `agent_id` was handed, in order.
    pub(super) fn prompts_for(&self, agent_id: &str) -> Vec<String> {
        self.asked()
            .into_iter()
            .filter(|(id, _)| id == agent_id)
            .map(|(_, prompt)| prompt)
            .collect()
    }
}

#[async_trait]
impl HiveTurnRunner for Runner {
    async fn speak(&self, agent_id: &str, prompt: &str) -> Result<String> {
        self.asked
            .lock()
            .expect("poisoned")
            .push((agent_id.to_owned(), prompt.to_owned()));
        {
            let mut fail = self.fail.lock().expect("poisoned");
            if let Some(left) = fail.get_mut(agent_id)
                && *left > 0
            {
                *left -= 1;
                return Err(crate::error::OpenCompanyError::Config(
                    "turn for 'verifier' hit the harness's per-turn wall-clock ceiling after \
                     10m 00s"
                        .to_owned(),
                ));
            }
        }
        let mut lines = self.lines.lock().expect("poisoned");
        let at = lines.iter().position(|(id, _)| id == agent_id);
        Ok(match at {
            Some(at) => lines.remove(at).1,
            None => format!("!question {agent_id} has nothing further."),
        })
    }
}

/// A memory that answers a fixed set of hits and records every note written.
#[derive(Default)]
pub(super) struct ScriptedMemory {
    pub(super) hits: Vec<String>,
    pub(super) notes: Mutex<Vec<HiveMemoryNote>>,
    /// When set, both halves fail — the best-effort contract under test.
    pub(super) broken: bool,
}

impl ScriptedMemory {
    pub(super) fn with_hits(hits: &[&str]) -> Self {
        Self {
            hits: hits.iter().map(|hit| (*hit).to_owned()).collect(),
            ..Self::default()
        }
    }

    pub(super) fn notes(&self) -> Vec<HiveMemoryNote> {
        self.notes.lock().expect("poisoned").clone()
    }
}

#[async_trait]
impl HiveMemory for ScriptedMemory {
    async fn recall(&self, _query: &str, limit: usize) -> Result<Vec<HiveMemoryHit>> {
        if self.broken {
            return Err(crate::error::OpenCompanyError::Store("no store".to_owned()));
        }
        Ok(self
            .hits
            .iter()
            .take(limit)
            .map(|snippet| HiveMemoryHit {
                snippet: snippet.clone(),
            })
            .collect())
    }

    async fn remember(&self, note: HiveMemoryNote) -> Result<()> {
        if self.broken {
            return Err(crate::error::OpenCompanyError::Store("no store".to_owned()));
        }
        self.notes.lock().expect("poisoned").push(note);
        Ok(())
    }
}

/// Three teammates, with `moves` assigned per the caller's TOML fragment.
pub(super) fn manifest_with(hive: &str) -> String {
    format!(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"planner\"\nrole = \"Planner\"\n\
         [[agent]]\nid = \"scout\"\nrole = \"Scout\"\n\
         [[agent]]\nid = \"critic\"\nrole = \"Critic\"\n\
         [[group_chat]]\nid = \"eng\"\nname = \"Engineering\"\n\
         description = \"Ship the rollout\"\n\
         members = [\"planner\", \"scout\", \"critic\"]\n\
         {hive}\n"
    )
}

/// The operator's message, and the watermark the episode opens on.
pub(super) async fn open(log: &MemoryLog) -> EventSeq {
    log.append(
        &MemoryLog::company(),
        CompanyEvent::OperatorMessage {
            text: "Decide the rollout.".into(),
            by: None,
            chat: Some("eng".into()),
            parent: None,
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        },
    )
    .await
    .expect("the journal accepts the operator's message")
}
