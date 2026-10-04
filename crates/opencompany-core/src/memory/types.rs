//! The host's view of a company's memory: the shapes the console, the brain
//! device tools and the orchestrator read, independent of the engine types
//! OpenHuman speaks.

use serde::{Deserialize, Serialize};

/// What sort of item a memory row is — TinyMemory's three item kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryItemKind {
    /// A statement: an operator fact, or something an agent learned.
    Learning,
    /// One logged agent turn.
    Conversation,
    /// A brain document: a dropped file or a remembered link.
    Document,
}

impl MemoryItemKind {
    /// Parses the wire spelling (`learning`, `conversation`, `document`).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "learning" => Some(Self::Learning),
            "conversation" => Some(Self::Conversation),
            "document" => Some(Self::Document),
            _ => None,
        }
    }
}

/// What kind of statement a learning is — TinyMemory's `LearningKind`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningKind {
    /// How the company likes things done.
    Preference,
    /// Something true about the company or the world.
    #[default]
    Fact,
    /// How to do something.
    Procedure,
    /// A correction of an earlier mistake.
    Correction,
    /// Anything else.
    Other,
}

/// One memory row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItem {
    /// The engine's item id.
    pub id: String,
    /// The item kind.
    pub kind: MemoryItemKind,
    /// A short heading: the first line of the text.
    pub title: String,
    /// The item's text.
    pub body: String,
    /// The memory agent whose node holds it, for an agent's item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The namespace node it lives at (`team:acme/agent:ceo`).
    pub namespace: String,
    /// For a brain document, the source it is filed under (`pdf`, `web`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Free-form tags.
    pub tags: Vec<String>,
    /// Epoch millis of when it was observed; `0` when the engine has no stamp.
    pub updated_at: i64,
    /// Relevance, for a ranked match; `0` in a listing.
    pub score: f32,
    /// Whether the operator may forget it. Every item: memory has no
    /// read-only rows.
    pub editable: bool,
}

/// What [`CompanyMemory::list`](super::CompanyMemory::list) pages through.
#[derive(Debug, Clone, Default)]
pub struct MemoryQuery {
    /// One teammate's node; `None` is the whole company.
    pub agent_id: Option<String>,
    /// One kind; `None` is every kind.
    pub kind: Option<MemoryItemKind>,
    /// Items carrying any one of these tags; empty is no constraint.
    pub tags_any: Vec<String>,
    /// Page size.
    pub limit: Option<usize>,
    /// The engine cursor of the next page.
    pub cursor: Option<String>,
}

/// One page of memory rows.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPage {
    /// The rows, newest first.
    pub items: Vec<MemoryItem>,
    /// The cursor of the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Whether a company's memory is on, and with which engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatus {
    /// The company's layout root.
    pub root: String,
    /// Whether an engine is bound.
    pub on: bool,
    /// The engine id, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// The engine endpoint, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Why memory is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One teammate with logged turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAgent {
    /// The memory agent id: the teammate's manifest id.
    pub agent_id: String,
    /// Logged turns.
    pub turns: u64,
}

/// The teammates with memory under a company's root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAgents {
    /// The company's layout root.
    pub root: String,
    /// Every teammate with logged turns, most first.
    pub agents: Vec<MemoryAgent>,
}

/// One item a recall answer rests on.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallCitation {
    /// The cited item's id.
    pub id: String,
    /// The cited item's kind.
    pub kind: MemoryItemKind,
    /// The relevant excerpt.
    pub text: String,
    /// Relevance, when the engine scores.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

/// A synthesised answer from memory.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallAnswer {
    /// The answer.
    pub answer: String,
    /// What it rests on.
    pub citations: Vec<RecallCitation>,
}

/// One brain source and its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrainSource {
    /// The source id (`pdf`, `markdown`, `web`, …).
    pub source: String,
    /// Documents filed under it.
    pub documents: u64,
}

/// The brain's sources under a company's root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrainSources {
    /// The company's layout root.
    pub root: String,
    /// Each source with documents, most first.
    pub sources: Vec<BrainSource>,
    /// Documents outside any source node.
    pub unfiled: u64,
}

/// A document filed in the brain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrainFiled {
    /// The stored document's id.
    pub id: String,
    /// The source it was filed under.
    pub source: String,
    /// Whether an identical document was already stored.
    pub replayed: bool,
}
