//! The brain device tools' `context_*` operations (`CycleHost::context_op`)
//! over a company's memory.
//!
//! The wire is unchanged — `context_put { label, body }`, `context_list
//! { prefix }`, `context_peek { addr }`, `context_search { query, limit }` —
//! because the hosted brain and the sidecar speak it. What backs it is the
//! company's OpenHuman memory: a put is a learning at the company root,
//! tagged [`CONTEXT_TAG`] and `label:<label>`, and its address is the item id.
//! A put from a cycle that outside content triggered also carries
//! [`INBOUND_TAG`], so what the outside world said stays distinguishable from
//! the company's own conclusions (issue #1113).

use std::ops::Range;

use super::{CompanyMemory, LearningKind, MemoryItem, MemoryQuery};
use crate::ports::types::{ChunkAddr, ChunkHit, ChunkMeta, ContextOp, ContextOpResult};

/// Tag on every item a `context_put` stored.
pub const CONTEXT_TAG: &str = "context";

/// Tag on a `context_put` from an externally triggered cycle.
pub const INBOUND_TAG: &str = "inbound";

/// Prefix of the tag carrying a put's label.
const LABEL_TAG: &str = "label:";

/// Most items a `context_list` reads.
const LIST_LIMIT: usize = 200;

impl CompanyMemory {
    /// Runs one brain `context_*` operation. `external` marks a cycle that
    /// content from outside the company triggered.
    pub async fn context_op(&self, op: ContextOp, external: bool) -> crate::Result<ContextOpResult> {
        match op {
            ContextOp::Put(chunk) => {
                let mut tags = vec![CONTEXT_TAG.to_string(), format!("{LABEL_TAG}{}", chunk.label)];
                if external {
                    tags.push(INBOUND_TAG.to_string());
                }
                let item = self.learn(&chunk.body, LearningKind::Fact, tags).await?;
                Ok(ContextOpResult::Addr(ChunkAddr::new(item.id)))
            }
            ContextOp::List { prefix } => {
                let page = self
                    .list(MemoryQuery {
                        tags_any: vec![CONTEXT_TAG.to_string()],
                        limit: Some(LIST_LIMIT),
                        ..MemoryQuery::default()
                    })
                    .await?;
                let metas = page
                    .items
                    .into_iter()
                    .filter_map(|item| {
                        let label = label_of(&item)?;
                        label.starts_with(&prefix).then(|| meta(&item, label))
                    })
                    .collect();
                Ok(ContextOpResult::Metas(metas))
            }
            ContextOp::Peek { addr, range } => {
                let item = self.get(vec![addr.as_ref().to_string()]).await?.into_iter().next();
                let text = item.map(|item| slice(&item.body, range)).unwrap_or_default();
                Ok(ContextOpResult::Text(text))
            }
            ContextOp::Search { query, limit } => {
                let hits = self
                    .search(&query, None, limit.max(1))
                    .await?
                    .into_iter()
                    .map(|item| ChunkHit {
                        addr: ChunkAddr::new(item.id.clone()),
                        snippet: item.title.clone(),
                        score: f64::from(item.score),
                    })
                    .collect();
                Ok(ContextOpResult::Hits(hits))
            }
        }
    }
}

/// The label a `context_put` stamped on `item`.
fn label_of(item: &MemoryItem) -> Option<String> {
    item.tags
        .iter()
        .find_map(|tag| tag.strip_prefix(LABEL_TAG))
        .map(str::to_string)
}

fn meta(item: &MemoryItem, label: String) -> ChunkMeta {
    ChunkMeta {
        addr: ChunkAddr::new(item.id.clone()),
        label,
        len: item.body.len(),
        stored_at_millis: u64::try_from(item.updated_at).unwrap_or(0),
    }
}

/// `text`, or the byte `range` of it widened to char boundaries.
fn slice(text: &str, range: Option<Range<usize>>) -> String {
    match range {
        Some(range) => crate::store::text::slice_on_char_boundaries(text, range),
        None => text.to_string(),
    }
}

#[cfg(test)]
#[path = "context_op_tests.rs"]
mod tests;
