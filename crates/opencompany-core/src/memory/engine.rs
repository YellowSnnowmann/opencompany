//! [`CompanyMemory`] over the process-wide OpenHuman runtime's memory facade
//! (`openhuman_embed::memory`), bound to the company's root.

use openhuman_embed::memory::api::{self as tm, ItemKind, SegmentKind};
use openhuman_embed::memory::{ItemsQuery, LearnParams, Memory, MemoryError};

use super::{
    BrainFiled, BrainSource, BrainSources, CompanyMemory, LearningKind, MemoryAgent, MemoryAgents,
    MemoryItem, MemoryItemKind, MemoryPage, MemoryQuery, MemoryStatus, RecallAnswer,
    RecallCitation,
};
use crate::error::OpenCompanyError;
use crate::harness::openhuman_runtime::{self, RuntimeBoot};

/// Longest title cut from an item's first line.
const TITLE_CHARS: usize = 120;

impl CompanyMemory {
    /// The facade bound to this company's root.
    async fn handle(&self) -> crate::Result<Memory> {
        let runtime = openhuman_runtime::global(RuntimeBoot::from_env()).await?;
        runtime.memory(&self.root).map_err(error)
    }

    /// Whether memory is on, and with what.
    pub async fn status(&self) -> MemoryStatus {
        match self.handle().await {
            Ok(memory) => {
                let status = memory.status();
                MemoryStatus {
                    root: status.root,
                    on: status.on,
                    engine: status.engine,
                    endpoint: status.endpoint,
                    reason: status.reason,
                }
            }
            Err(err) => MemoryStatus {
                root: self.root.clone(),
                on: false,
                engine: None,
                endpoint: None,
                reason: Some(err.to_string()),
            },
        }
    }

    /// A page of rows, newest first.
    pub async fn list(&self, query: MemoryQuery) -> crate::Result<MemoryPage> {
        let page = self
            .handle()
            .await?
            .list(ItemsQuery {
                agent_id: query.agent_id,
                kinds: query.kind.map(item_kind).into_iter().collect(),
                tags_any: query.tags_any,
                limit: query.limit,
                cursor: query.cursor,
            })
            .await
            .map_err(error)?;
        Ok(MemoryPage {
            items: page.items.into_iter().map(item).collect(),
            next_cursor: page.next_cursor,
        })
    }

    /// Ranked rows matching `query`, best first.
    pub async fn search(
        &self,
        query: &str,
        kind: Option<MemoryItemKind>,
        limit: usize,
    ) -> crate::Result<Vec<MemoryItem>> {
        let view = self
            .handle()
            .await?
            .fetch(query, kind.map(item_kind).into_iter().collect(), Some(limit))
            .await
            .map_err(error)?;
        Ok(view.hits.into_iter().map(item).collect())
    }

    /// The rows among `ids` this company holds.
    pub async fn get(&self, ids: Vec<String>) -> crate::Result<Vec<MemoryItem>> {
        let hits = self.handle().await?.get(ids).await.map_err(error)?;
        Ok(hits.into_iter().map(item).collect())
    }

    /// Stores a shared learning at the company root, tagged with `tags`.
    pub async fn learn(
        &self,
        text: &str,
        kind: LearningKind,
        tags: Vec<String>,
    ) -> crate::Result<MemoryItem> {
        let memory = self.handle().await?;
        let mut params: LearnParams = serde_json::from_value(serde_json::json!({
            "text": text,
            "kind": kind,
        }))?;
        params.meta = Some(tm::MemoryMeta {
            tags,
            ..tm::MemoryMeta::default()
        });
        let learned = memory.learn(params).await.map_err(error)?;
        let stored = memory
            .get(vec![learned.id.clone()])
            .await
            .map_err(error)?
            .into_iter()
            .next();
        tracing::debug!(root = %self.root, id = %learned.id, "[memory] learned");
        Ok(match stored {
            Some(hit) => item(hit),
            // An engine whose writes reach `get` only later: the row as stored.
            None => MemoryItem {
                id: learned.id,
                kind: MemoryItemKind::Learning,
                title: title_of(text),
                body: text.to_string(),
                agent_id: None,
                namespace: self.root.clone(),
                source: None,
                tags: Vec::new(),
                updated_at: tm::chrono::Utc::now().timestamp_millis(),
                score: 0.0,
                editable: true,
            },
        })
    }

    /// Forgets the rows among `ids` this company holds; returns how many.
    pub async fn forget(&self, ids: Vec<String>) -> crate::Result<usize> {
        let view = self.handle().await?.forget(ids).await.map_err(error)?;
        Ok(view.forgotten)
    }

    /// Forgets one teammate's logged turns and the beliefs built from them.
    pub async fn forget_agent(&self, agent_id: &str) -> crate::Result<usize> {
        self.handle()
            .await?
            .forget_agent(agent_id)
            .await
            .map_err(error)
    }

    /// The teammates with logged turns, most first.
    pub async fn agents(&self) -> crate::Result<MemoryAgents> {
        let view = self.handle().await?.agents().await.map_err(error)?;
        Ok(MemoryAgents {
            root: view.root,
            agents: view
                .agents
                .into_iter()
                .map(|agent| MemoryAgent {
                    agent_id: agent.agent_id,
                    turns: agent.turns,
                })
                .collect(),
        })
    }

    /// Answers `question` from the company's memory, or from one teammate's
    /// turns plus the shared learnings.
    pub async fn recall(
        &self,
        question: &str,
        agent_id: Option<&str>,
    ) -> crate::Result<RecallAnswer> {
        let view = self
            .handle()
            .await?
            .recall(question, agent_id, None)
            .await
            .map_err(error)?;
        Ok(RecallAnswer {
            answer: view.answer,
            citations: view
                .citations
                .into_iter()
                .map(|citation| RecallCitation {
                    id: citation.id.0,
                    kind: kind_of(citation.kind),
                    text: citation.snippet,
                    score: citation.score,
                })
                .collect(),
        })
    }

    /// The brain's sources and their sizes.
    pub async fn brain_sources(&self) -> crate::Result<BrainSources> {
        let view = self.handle().await?.brain_sources().await.map_err(error)?;
        Ok(BrainSources {
            root: view.root,
            sources: view
                .sources
                .into_iter()
                .map(|source| BrainSource {
                    source: source.source,
                    documents: source.documents,
                })
                .collect(),
            unfiled: view.unfiled,
        })
    }

    /// Files `text` in the brain under `source` (`pdf`, `markdown`, `web`, …).
    pub async fn brain_file(
        &self,
        title: &str,
        source: &str,
        text: &str,
    ) -> crate::Result<BrainFiled> {
        let params = serde_json::from_value(serde_json::json!({
            "text": text,
            "source": source,
            "title": title,
        }))?;
        let view = self
            .handle()
            .await?
            .brain_ingest(params)
            .await
            .map_err(error)?;
        tracing::debug!(root = %self.root, source = %view.source, "[memory] brain document filed");
        Ok(BrainFiled {
            id: view.id,
            source: view.source,
            replayed: view.replayed,
        })
    }

    /// Forgets every brain document of one source; returns how many.
    pub async fn brain_forget(&self, source: &str) -> crate::Result<usize> {
        let view = self
            .handle()
            .await?
            .brain_forget(source)
            .await
            .map_err(error)?;
        Ok(view.forgotten)
    }
}

/// The host error for a memory failure: off is a configuration the operator
/// can reach, a bad request is theirs, anything else is the store's.
fn error(err: MemoryError) -> OpenCompanyError {
    match err {
        MemoryError::Off(reason) => {
            OpenCompanyError::NotConfigured(format!("company memory is off: {reason}"))
        }
        MemoryError::InvalidRequest(reason) => OpenCompanyError::InvalidRequest(reason),
        other => OpenCompanyError::Store(format!("company memory: {other}")),
    }
}

fn item_kind(kind: MemoryItemKind) -> ItemKind {
    match kind {
        MemoryItemKind::Learning => ItemKind::Learning,
        MemoryItemKind::Conversation => ItemKind::Conversation,
        MemoryItemKind::Document => ItemKind::Document,
    }
}

fn kind_of(kind: ItemKind) -> MemoryItemKind {
    match kind {
        ItemKind::Learning => MemoryItemKind::Learning,
        ItemKind::Conversation => MemoryItemKind::Conversation,
        ItemKind::Document => MemoryItemKind::Document,
    }
}

/// The id of the last segment of `kind` in `namespace`.
fn segment(namespace: &tm::Namespace, kind: SegmentKind) -> Option<String> {
    namespace
        .segments()
        .iter()
        .rev()
        .find(|segment| segment.kind() == kind)
        .map(|segment| segment.id().to_string())
}

fn title_of(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty());
    line.unwrap_or_default().chars().take(TITLE_CHARS).collect()
}

/// A host row from an engine hit.
fn item(hit: tm::Hit) -> MemoryItem {
    let namespace = &hit.meta.namespace;
    let agent_id = hit
        .meta
        .agent_id
        .clone()
        .or_else(|| segment(namespace, SegmentKind::Agent));
    let source = match hit.kind {
        ItemKind::Document => segment(namespace, SegmentKind::Source),
        _ => None,
    };
    MemoryItem {
        id: hit.id.0,
        kind: kind_of(hit.kind),
        title: title_of(&hit.text),
        body: hit.text,
        agent_id,
        namespace: namespace.to_string(),
        source,
        tags: hit.meta.tags,
        updated_at: hit
            .meta
            .observed_at
            .map(|at| at.timestamp_millis())
            .unwrap_or(0),
        score: hit.score,
        editable: true,
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
