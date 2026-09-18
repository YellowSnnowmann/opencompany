//! Text search over the company workspace (issue #607).
//!
//! Before this module the shared tree could be *listed* and *read* but not
//! searched, so finding "which note mentions the refund policy" cost one
//! `workspace_read` per candidate — one round trip and one full note body in
//! context per hop. That cost grows with exactly the agent-published content the
//! shared workspace exists to accumulate (issues #551, #552), so the discovery
//! step got more expensive as the workspace got more useful.
//!
//! # Why this is a company-layer helper and not a [`WorkspaceStore`] method
//!
//! A port method would need five implementations (`fs`, `sqlite`, `mongodb`, the
//! announcing decorator, the tools' test double) **and** one agreed definition of
//! *matching* across three engines. SQLite's FTS5 tokeniser, MongoDB's `$text`
//! stemming and a hand-rolled filesystem scan cannot be made to agree without
//! either forbidding the native indexes — all of the cost, none of the benefit —
//! or accepting that the same query answers differently depending on which
//! backend a tenant happens to be on. A search whose semantics depend on
//! deployment is not a search anybody can reason about.
//!
//! This helper is correct on all three backends *by construction*: it is written
//! against the port's existing [`tree`](WorkspaceStore::tree) and
//! [`read`](WorkspaceStore::read), so there is one implementation and one
//! definition of a match.
//!
//! **The cost that buys, stated plainly**: O(N) reads per query, N = the number
//! of prose notes in the company's tree. That is the same shape the workspace
//! already pays elsewhere — [`workspace_links`](crate::company::workspace_links)
//! scans every file's content to compute backlinks on *every file open*, in the
//! default build, and the MongoDB backend materialises the whole node set on
//! every `tree()` call. This module is the named place to add an index when that
//! stops being acceptable. Until then the worst case is a slow search, never a
//! wrong one.
//!
//! # What a match is
//!
//! Case-insensitive **substring**, Unicode-aware, against a node's name (folders
//! and files alike) and a text file's content. No tokenising, no stemming, no
//! ranking model. An operator who types `refund` finds `Refunds.md` and every
//! note containing `REFUND`, and can predict that from the rule alone — which is
//! the property a three-backend surface needs more than it needs relevance
//! scoring.
//!
//! # Binary nodes match by name only
//!
//! A text [`read`](WorkspaceStore::read) of a binary node returns an **empty
//! body** on all three backends (the port's stated answer to a prose-shaped read
//! of a payload), so a content scan built over `read` would find nothing in a PNG
//! whether or not it knew binaries existed. This module states the rule instead
//! of inheriting the silence: a binary node is matched on its name and never
//! content-scanned, and [`read_bytes`](WorkspaceStore::read_bytes) is never
//! called from a search path — it exists to serve a download, and reaching for it
//! here would turn one query into a bulk GridFS transfer.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use crate::Result;
use crate::company::workspace_paths::{render_path, split_logical_path};
use crate::company::workspace_scaffold::is_agent_hidden_path;
use crate::error::OpenCompanyError;
use crate::ports::types::CompanyId;
use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceStore};

/// Hard ceiling on hits any one search returns, whatever the caller asked for.
///
/// The limit argument is a [`NonZeroUsize`], so "zero results" is unrepresentable;
/// this is the other half — it makes "unlimited" unrepresentable too. Without it
/// a caller could ask for the whole tree and get it, which is the crawl this
/// module exists to replace rather than to automate.
pub const MAX_SEARCH_RESULTS: usize = 50;

/// The limit a caller that names none gets.
pub const DEFAULT_SEARCH_LIMIT: usize = 20;

/// Max bytes of query text accepted.
///
/// A query is caller-supplied and otherwise unbounded, and it is echoed back in
/// error messages and (on the agent surface) in a size-budgeted header. Bounding
/// it here means neither surface has to.
pub const MAX_QUERY_BYTES: usize = 256;

/// Target width of the excerpt rendered around a content match.
///
/// Wide enough to show the phrase in its sentence, narrow enough that a
/// full page of hits stays inside one tool result.
const EXCERPT_BYTES: usize = 160;

/// Which part of a node the query matched.
///
/// A node whose *name* matches is a stronger answer than one that merely
/// mentions the phrase, which is why this drives ordering and not just display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchKind {
    /// The node's name contains the query. Folders can only match this way.
    Name,
    /// A text file's body contains the query.
    Content,
}

impl MatchKind {
    /// The wire/agent-facing token — one spelling shared by every surface.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Content => "content",
        }
    }
}

/// One node the query matched.
#[derive(Clone, Debug)]
pub struct SearchHit {
    /// The matching node, carrying its id, kind, both origins and — for a binary
    /// node — its `mime` / `size` / `sha256`.
    pub node: WorkspaceNode,
    /// The node's rendered logical path, e.g. `standards/Engineering.md`.
    pub path: String,
    /// Whether the name or the body matched.
    pub matched: MatchKind,
    /// Text around the first body occurrence. `None` for a name match, for a
    /// folder, and for a binary node.
    ///
    /// **This is stored content**, and on the agent surface it must be fenced as
    /// untrusted reference material exactly as `workspace_read`'s body is — see
    /// the tool's `execute`. Search widens that exposure rather than repeating
    /// it: an agent that never opens a poisoned note still receives an excerpt of
    /// one here.
    pub excerpt: Option<String>,
}

/// A search's answer: the hits that fit, and how many there were.
///
/// `total` is separate from `hits.len()` so truncation is *visible*. A caller
/// that could only see the returned page would have no way to distinguish "these
/// are all of them" from "these are the first 20 of 400", and the second is
/// exactly when a human or an agent needs to narrow the query.
#[derive(Clone, Debug)]
pub struct SearchOutcome {
    /// The hits, ordered: name matches first, then content matches;
    /// most-recently-updated first within each group; path as the tie-break.
    pub hits: Vec<SearchHit>,
    /// How many nodes matched in total, before the limit was applied.
    pub total: usize,
}

impl SearchOutcome {
    /// How many matches are not in [`hits`](Self::hits).
    pub fn omitted(&self) -> usize {
        self.total.saturating_sub(self.hits.len())
    }
}

/// The effective limit for a caller-supplied one, clamped to
/// [`MAX_SEARCH_RESULTS`].
///
/// Clamping rather than refusing: an over-large limit is a caller asking for
/// everything, and the honest answer is a capped page plus a `total` that says
/// how much was left — not an error that returns nothing.
pub fn clamp_limit(limit: NonZeroUsize) -> usize {
    limit.get().min(MAX_SEARCH_RESULTS)
}

/// Search one company's workspace.
///
/// `prefix` scopes the search to a subtree by logical path (`Standards`,
/// `product/Specs`); `None` searches the whole tree. It is validated with the
/// same [`split_logical_path`] the agent tools use, so a traversal-shaped scope
/// is refused rather than normalised.
///
/// Errors on an empty/whitespace query and on one over [`MAX_QUERY_BYTES`] —
/// both are [`InvalidRequest`](OpenCompanyError::InvalidRequest), so the REST
/// surface answers 400 and says why. An empty query is not "match everything":
/// that is `workspace_list`, and answering it here would turn a mistyped search
/// into a full tree dump.
pub async fn search_workspace(
    store: &dyn WorkspaceStore,
    company: &CompanyId,
    query: &str,
    prefix: Option<&str>,
    limit: NonZeroUsize,
) -> Result<SearchOutcome> {
    search_workspace_with_visibility(store, company, query, prefix, limit, |_| true).await
}

/// Search the workspace subset exposed to agents.
///
/// Operator REST and GraphQL search use [`search_workspace`] and retain the
/// complete tree. Agent tools use this function so the operator-only
/// `secrets/` subtree is discarded before either names or note bodies are
/// considered.
pub async fn search_workspace_for_agent(
    store: &dyn WorkspaceStore,
    company: &CompanyId,
    query: &str,
    prefix: Option<&str>,
    limit: NonZeroUsize,
) -> Result<SearchOutcome> {
    search_workspace_with_visibility(store, company, query, prefix, limit, |path| {
        !is_agent_hidden_path(path)
    })
    .await
}

async fn search_workspace_with_visibility(
    store: &dyn WorkspaceStore,
    company: &CompanyId,
    query: &str,
    prefix: Option<&str>,
    limit: NonZeroUsize,
    visible: impl Fn(&str) -> bool,
) -> Result<SearchOutcome> {
    let needle = query.trim();
    if needle.is_empty() {
        return Err(OpenCompanyError::InvalidRequest(
            "a workspace search needs something to search for; `query` is empty".to_string(),
        ));
    }
    if needle.len() > MAX_QUERY_BYTES {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "`query` is {} bytes; workspace search accepts at most {MAX_QUERY_BYTES}",
            needle.len()
        )));
    }
    let scope = match prefix.map(str::trim).filter(|p| !p.is_empty()) {
        Some(prefix) => Some(
            split_logical_path(prefix)
                .map_err(OpenCompanyError::InvalidRequest)?
                .join("/"),
        ),
        None => None,
    };

    let needle_lower = needle.to_lowercase();
    let nodes = store.tree(company).await?;
    // Borrowed against `nodes` for the ancestor walk, exactly as the tools'
    // `PathIndex` does — one company-scoped read is the whole reachable set.
    let by_id: HashMap<&str, &WorkspaceNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let mut hits: Vec<SearchHit> = Vec::new();
    for node in &nodes {
        // A node with no renderable path is skipped outright. `workspace_read`
        // refuses it (it is absent from the tools' index by path *and* by id), so
        // offering it as a search hit would advertise something nothing can open.
        let Some(path) = render_path(node, &by_id) else {
            continue;
        };
        if !visible(&path) {
            continue;
        }
        if let Some(scope) = &scope
            && !(path == *scope || path.starts_with(&format!("{scope}/")))
        {
            continue;
        }

        if contains_lower(&node.name, &needle_lower) {
            hits.push(SearchHit {
                node: node.clone(),
                path,
                matched: MatchKind::Name,
                excerpt: None,
            });
            continue;
        }

        // Folders have no body, and a binary node's body is empty by the port's
        // definition — neither is worth a store round trip, and for a binary the
        // read would answer `""` and quietly imply "no match" rather than "not
        // searchable this way".
        if node.kind != NodeKind::File || node.is_binary() {
            continue;
        }
        let Some((_, content)) = store.read(company, &node.id).await? else {
            // Raced with a delete between the tree read and this one. A node that
            // no longer exists is not a result.
            continue;
        };
        if let Some(excerpt) = excerpt_around(&content, &needle_lower) {
            hits.push(SearchHit {
                node: node.clone(),
                path,
                matched: MatchKind::Content,
                excerpt: Some(excerpt),
            });
        }
    }

    // Name before content, then freshest, then path. Total order with no
    // reliance on the store's unspecified `tree()` ordering, so the same
    // workspace answers the same query identically on every backend and on
    // every call.
    hits.sort_by(|a, b| {
        (a.matched == MatchKind::Content)
            .cmp(&(b.matched == MatchKind::Content))
            .then(b.node.updated_at_millis.cmp(&a.node.updated_at_millis))
            .then(a.path.cmp(&b.path))
    });

    let total = hits.len();
    hits.truncate(clamp_limit(limit));
    Ok(SearchOutcome { hits, total })
}

/// Whether `haystack` contains `needle_lower`, compared case-insensitively.
///
/// `needle_lower` is already lowercased by the caller — once per search rather
/// than once per node.
fn contains_lower(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_lowercase().contains(needle_lower)
}

/// Text around the first case-insensitive occurrence of `needle_lower` in
/// `body`, or `None` when it does not occur.
///
/// # Why this does not slice `body` at an index found in `body.to_lowercase()`
///
/// Lowercasing is not length-preserving in Unicode: `İ` (U+0130, two bytes)
/// lowercases to `i̇` (three bytes), and there are several more like it. An
/// offset found in the lowercased copy therefore does not address the same
/// character in the original — it drifts by however much every preceding
/// expansion added. Applied to the original string that offset lands in the
/// wrong place at best and **mid-codepoint at worst, which panics**: the byte-slice
/// panic class this repo keeps rediscovering.
///
/// So the lowercased haystack is built alongside a map from each of its byte
/// offsets back to the original's, and the match offset is translated through it
/// before anything is sliced. Both window edges are then snapped outward-to-inward
/// onto char boundaries, so every returned excerpt is valid UTF-8 by construction.
fn excerpt_around(body: &str, needle_lower: &str) -> Option<String> {
    let (lowered, to_original) = lower_with_offsets(body);
    let found = lowered.find(needle_lower)?;
    // `to_original` carries one entry per byte of `lowered` plus a final
    // sentinel, so this index is always in range.
    let at = to_original[found];

    let mut start = at.saturating_sub(EXCERPT_BYTES / 2);
    while start > 0 && !body.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + EXCERPT_BYTES).min(body.len());
    while end > start && !body.is_char_boundary(end) {
        end -= 1;
    }

    let mut excerpt = String::new();
    if start > 0 {
        excerpt.push('…');
    }
    // Whitespace inside a note is layout, not content: collapsing it keeps one
    // hit to one line, so a page of results reads as a list rather than as
    // fragments of other people's formatting.
    excerpt.push_str(
        body[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim(),
    );
    if end < body.len() {
        excerpt.push('…');
    }
    Some(excerpt)
}

/// `body` lowercased, plus a map from every byte offset of the lowercased string
/// back to the byte offset of the original character that produced it.
///
/// The returned vector has `lowered.len() + 1` entries — one per byte plus a
/// sentinel for the end — so any index `find` can return is addressable.
fn lower_with_offsets(body: &str) -> (String, Vec<usize>) {
    let mut lowered = String::with_capacity(body.len());
    let mut map = Vec::with_capacity(body.len() + 1);
    for (offset, ch) in body.char_indices() {
        for lower in ch.to_lowercase() {
            lowered.push(lower);
        }
        // Every byte this character contributed maps back to where the character
        // started, so an offset landing anywhere inside an expansion still
        // resolves to a real boundary in the original.
        map.resize(lowered.len(), offset);
    }
    map.push(body.len());
    (lowered, map)
}

#[cfg(test)]
#[path = "workspace_search/workspace_search_tests.rs"]
mod tests;
