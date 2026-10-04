//! The company-brain memory read: `Company.memory` over the company's
//! OpenHuman memory ([`crate::memory`]).

use std::sync::Arc;

use async_graphql::{Enum, ID, SimpleObject};

use super::pagination::Page;
use crate::company::runtime::CompanyRuntime;
use crate::memory::{MemoryItem, MemoryItemKind, MemoryQuery};
use crate::ports::iso8601;

/// Most items one `Company.memory` read pages over.
const MAX_ITEMS: usize = 200;

/// The kind of a memory item: TinyMemory's three item kinds.
#[derive(Enum, Copy, Clone, Eq, PartialEq)]
#[graphql(name = "MemoryKind")]
pub enum MemoryKindGql {
    /// A statement: an operator fact, or something an agent learned.
    Learning,
    /// One logged agent turn.
    Conversation,
    /// A brain document.
    Document,
}

impl From<MemoryItemKind> for MemoryKindGql {
    fn from(kind: MemoryItemKind) -> Self {
        match kind {
            MemoryItemKind::Learning => Self::Learning,
            MemoryItemKind::Conversation => Self::Conversation,
            MemoryItemKind::Document => Self::Document,
        }
    }
}

impl From<MemoryKindGql> for MemoryItemKind {
    fn from(kind: MemoryKindGql) -> Self {
        match kind {
            MemoryKindGql::Learning => Self::Learning,
            MemoryKindGql::Conversation => Self::Conversation,
            MemoryKindGql::Document => Self::Document,
        }
    }
}

/// One memory item. Mirrors [`MemoryItem`].
#[derive(SimpleObject)]
#[graphql(name = "MemoryItem")]
pub struct MemoryItemGql {
    /// The item id.
    pub id: ID,
    /// The item's kind.
    pub kind: MemoryKindGql,
    /// A short title: the first line.
    pub title: String,
    /// The item's text.
    pub body: String,
    /// The teammate whose node holds it, for a teammate's item.
    pub agent_id: Option<String>,
    /// The namespace node it lives at.
    pub namespace: String,
    /// For a brain document, the source it is filed under.
    pub source: Option<String>,
    /// When it was observed, ISO-8601 UTC; absent when the engine has no stamp.
    pub updated_at: Option<String>,
}

impl From<MemoryItem> for MemoryItemGql {
    fn from(item: MemoryItem) -> Self {
        Self {
            id: ID(item.id),
            kind: item.kind.into(),
            title: item.title,
            body: item.body,
            agent_id: item.agent_id,
            namespace: item.namespace,
            source: item.source,
            updated_at: u64::try_from(item.updated_at)
                .ok()
                .filter(|millis| *millis > 0)
                .map(iso8601),
        }
    }
}

/// Resolves `Company.memory(query, kind, first, offset)`: ranked matches for
/// `query`, else the newest items.
pub(crate) async fn resolve(
    runtime: &Arc<CompanyRuntime>,
    query: Option<String>,
    kind: Option<MemoryKindGql>,
    first: i32,
    offset: i32,
) -> async_graphql::Result<Page<MemoryItemGql>> {
    let memory = runtime.memory();
    let kind = kind.map(MemoryItemKind::from);
    let rows = match query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        Some(query) => memory.search(query, kind, MAX_ITEMS).await?,
        None => {
            memory
                .list(MemoryQuery {
                    kind,
                    limit: Some(MAX_ITEMS),
                    ..MemoryQuery::default()
                })
                .await?
                .items
        }
    };
    let items: Vec<MemoryItemGql> = rows.into_iter().map(MemoryItemGql::from).collect();
    Ok(Page::slice(
        items,
        offset.max(0) as usize,
        first.max(0) as usize,
    ))
}
