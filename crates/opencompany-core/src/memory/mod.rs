//! A company's memory: OpenHuman's memory engine, scoped to the company.
//!
//! OpenCompany has no memory engine of its own. Every company teammate is an
//! OpenHuman agent bound to memory with
//! `MemoryBinding::new(<teammate id>).root(<company root>)`
//! (`harness::built_in::build::agent_spec_for`), so its turns run TinyMemory's
//! lifecycle on their own: the pre-turn pack, the post-turn log, compaction
//! recall and the `memory` tool. Everything a company remembers lives in one
//! subtree:
//!
//! ```text
//! team:<company>                    learnings: operator facts, what agents learned
//! ├── source:<kind>                 the brain: dropped files, remembered links
//! └── agent:<teammate id>           one teammate's logged turns
//! ```
//!
//! [`CompanyMemory`] is the host's handle on that subtree — the console's
//! Brain page, the brain device tools (`context_*`), the orchestrator's
//! company query and the workflow judge all go through it. It wraps
//! `openhuman_embed::memory::Memory`, which keeps every read, forget and
//! learn inside the root, so one company never reaches another's memory.
//!
//! Without the `openhuman` feature there is no runtime to hold memory:
//! [`CompanyMemory::status`] reports off and every other call is
//! [`OpenCompanyError::NotInBuild`]. With it but no engine bound (no
//! TinyHumans credential, no CortexDB key) memory is off and calls are
//! [`OpenCompanyError::NotConfigured`].

mod types;

#[cfg(feature = "openhuman")]
mod engine;

pub use types::{
    BrainFiled, BrainSource, BrainSources, LearningKind, MemoryAgent, MemoryAgents, MemoryItem,
    MemoryItemKind, MemoryPage, MemoryQuery, MemoryStatus, RecallAnswer, RecallCitation,
};

use crate::error::OpenCompanyError;
use crate::ports::CompanyId;

/// The layout root a company's memory lives under: `team:<company>`, the
/// company id kept as is when it is a valid namespace id and otherwise
/// sanitized with a hash suffix so distinct ids stay distinct.
pub fn memory_root(company: &CompanyId) -> String {
    format!("team:{}", segment_id(company.as_str()))
}

/// A namespace segment id for `raw`: `[A-Za-z0-9_-]{1,128}` kept as is,
/// anything else folded to `-` with an 8-hex FNV-1a suffix of the original —
/// TinyMemory's own `Segment::sanitized` rule, restated here so the root is
/// known without the `openhuman` feature.
fn segment_id(raw: &str) -> String {
    const MAX: usize = 128;
    let valid = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if !raw.is_empty() && raw.len() <= MAX && raw.chars().all(valid) {
        return raw.to_string();
    }
    if raw.is_empty() {
        return "_".to_string();
    }
    let cleaned: String = raw
        .chars()
        .map(|c| if valid(c) { c } else { '-' })
        .take(MAX - 9)
        .collect();
    let mut hash: u32 = 0x811c_9dc5;
    for byte in raw.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{cleaned}-{hash:08x}")
}

/// A company's memory. Cheap to clone; resolves the process-wide OpenHuman
/// runtime on each call, so it can be built before the runtime exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyMemory {
    company: CompanyId,
    root: String,
}

impl CompanyMemory {
    /// The memory of `company`.
    pub fn new(company: &CompanyId) -> Self {
        Self {
            company: company.clone(),
            root: memory_root(company),
        }
    }

    /// The company this memory belongs to.
    pub fn company(&self) -> &CompanyId {
        &self.company
    }

    /// The company's layout root.
    pub fn root(&self) -> &str {
        &self.root
    }
}

#[cfg(not(feature = "openhuman"))]
impl CompanyMemory {
    fn not_in_build() -> OpenCompanyError {
        OpenCompanyError::NotInBuild(
            "company memory runs on the embedded OpenHuman runtime, which this build was \
             compiled without (the `openhuman` feature)"
                .to_string(),
        )
    }

    /// Whether memory is on: never, in this build.
    pub async fn status(&self) -> MemoryStatus {
        MemoryStatus {
            root: self.root.clone(),
            on: false,
            engine: None,
            endpoint: None,
            reason: Some(Self::not_in_build().to_string()),
        }
    }

    /// A page of rows.
    pub async fn list(&self, _query: MemoryQuery) -> crate::Result<MemoryPage> {
        Err(Self::not_in_build())
    }

    /// Ranked rows matching `query`.
    pub async fn search(
        &self,
        _query: &str,
        _kind: Option<MemoryItemKind>,
        _limit: usize,
    ) -> crate::Result<Vec<MemoryItem>> {
        Err(Self::not_in_build())
    }

    /// The rows among `ids` this company holds.
    pub async fn get(&self, _ids: Vec<String>) -> crate::Result<Vec<MemoryItem>> {
        Err(Self::not_in_build())
    }

    /// Stores a shared learning at the company root.
    pub async fn learn(
        &self,
        _text: &str,
        _kind: LearningKind,
        _tags: Vec<String>,
    ) -> crate::Result<MemoryItem> {
        Err(Self::not_in_build())
    }

    /// Forgets the rows among `ids` this company holds.
    pub async fn forget(&self, _ids: Vec<String>) -> crate::Result<usize> {
        Err(Self::not_in_build())
    }

    /// Forgets one teammate's logged turns.
    pub async fn forget_agent(&self, _agent_id: &str) -> crate::Result<usize> {
        Err(Self::not_in_build())
    }

    /// The teammates with logged turns.
    pub async fn agents(&self) -> crate::Result<MemoryAgents> {
        Err(Self::not_in_build())
    }

    /// Answers `question` from memory.
    pub async fn recall(
        &self,
        _question: &str,
        _agent_id: Option<&str>,
    ) -> crate::Result<RecallAnswer> {
        Err(Self::not_in_build())
    }

    /// The brain's sources.
    pub async fn brain_sources(&self) -> crate::Result<BrainSources> {
        Err(Self::not_in_build())
    }

    /// Files text in the brain.
    pub async fn brain_file(
        &self,
        _title: &str,
        _source: &str,
        _text: &str,
    ) -> crate::Result<BrainFiled> {
        Err(Self::not_in_build())
    }

    /// Forgets every brain document of one source.
    pub async fn brain_forget(&self, _source: &str) -> crate::Result<usize> {
        Err(Self::not_in_build())
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
