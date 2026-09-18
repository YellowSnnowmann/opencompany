//! Memory-fact reads + writes: `GET /memory`, `GET /memory/traces`,
//! `GET /memory/stats`, `GET /memory/archives`, `POST /memory`,
//! `DELETE /memory/{fact_id}` under both scope forms.
//!
//! Bodies mirror the console's `MemoryEntry` (`frontend/src/api/memory.ts`).
//! Facts land in the [`FactStore`](crate::ports::FactStore) — the console's
//! durable Memory/Brain view. A create *also* mirrors the fact into the
//! [`ContextStore`](crate::ports::ContextStore) the embedded agents recall from,
//! so an operator note is agent-recallable on the next turn (see
//! [`create_fact`]). A delete journals a [`CompanyEvent::MemoryFactDeleted`] to
//! the `EventLog` per the Operator-rights section of
//! `docs/spec/company-brain/memory.md`.
//!
//! ## The delete → reap seam
//!
//! Deleting a fact removes it from the `FactStore` AND reaps its mirrored
//! `operator-fact/{id}` context chunk — the delete port this comment once
//! said was missing landed with #1290, and leaving the reap unwired would
//! have kept showing the operator "deleted" while agents still recalled it.
//! The reap is label-scoped since #1300: chunks are content-addressed, so a
//! mirror whose byte-identical body is indexed under any OTHER label loses
//! exactly the mirror's own claim — the other label keeps the body, and the
//! body goes only with its last claim, atomically inside the port.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::OpenCompanyError;
use crate::ports::facts::{FactKind, FactRecord};
use crate::ports::types::{ChunkAddr, ChunkMeta, CompanyEvent, CompressedTrace, ContextChunk};
use crate::ports::{generate_id, now_millis};
use crate::runtime::maintenance::TRACE_RETENTION_LIMIT;
use crate::server::error::ApiError;
use crate::server::ops::{ScopedCompany, scoped};

/// The deliberate-memory label family's prefix, with its separator.
///
/// A LOCAL copy: the authoring module (`crate::harness::built_in::memory_tools`,
/// `AGENT_MEMORY_LABEL_PREFIX`) is `openhuman`-gated and this file is not, so
/// the constant cannot be imported on every shape. The gated test below pins
/// the two spellings together, which makes the lockstep compiler-checked on
/// exactly the lanes that build both.
fn const_format_prefix() -> &'static str {
    "agent-memory/"
}

#[cfg(all(test, feature = "openhuman"))]
mod label_lockstep_test {
    /// A rename of the tool module's prefix must break here, not silently
    /// mis-attribute every deliberate memory in the Brain view.
    #[test]
    fn the_brain_view_parses_the_prefix_the_tools_write() {
        assert_eq!(
            super::const_format_prefix(),
            format!(
                "{}/",
                crate::harness::built_in::memory_tools::AGENT_MEMORY_LABEL_PREFIX
            )
        );
    }
}

/// Label prefix for the [`ContextStore`](crate::ports::ContextStore) mirror of
/// an operator-authored fact. Keyed by fact id so [`reap_fact_mirror`] can
/// find and remove the mirror's claim when the fact is deleted.
const OPERATOR_FACT_PREFIX: &str = "operator-fact";

/// Label prefix under which the harness stores completed task outcomes.
///
/// Mirrors `harness::memory_loop::OUTCOME_LABEL_PREFIX`, duplicated here because
/// that module is `openhuman`-gated while this route is always compiled. Kept in
/// sync by the `outcome_prefix_matches_harness` test under the feature.
const OUTCOME_LABEL_PREFIX: &str = "task-outcome";

/// Builds the memory route fragment.
pub fn router() -> Router<AppState> {
    scoped("/memory", post(create_fact).get(list_facts))
        .merge(scoped("/memory/traces", get(list_traces)))
        .merge(scoped("/memory/stats", get(memory_stats)))
        .merge(scoped("/memory/archives", get(archived_traces)))
        .merge(scoped("/memory/{fact_id}", delete(delete_fact)))
}

/// Upper bound on context-store entries materialised into the list, so a company
/// with a very large learned-context store can't force an unbounded number of
/// chunk-body reads on a single `GET /memory`. The stats endpoint only counts
/// (no per-chunk read), so it stays unbounded; the list caps its reads here.
const MAX_CONTEXT_ENTRIES: usize = 500;

/// Upper bound on archived traces materialised by `GET /memory/archives`.
///
/// The facade's `evict` bounds the archive tier itself on every eviction path
/// (keep-recent to its `n`, older-than to [`TRACE_RETENTION_LIMIT`]; see
/// `prune_archive`), so the route's cap is defense-in-depth for archive rows
/// written before that bound applied, not the primary bound. Mirrors
/// `recent_traces`' newest-window semantics: same total order, tail of the cap.
const MAX_ARCHIVED_TRACES: usize = TRACE_RETENTION_LIMIT;

/// `GET /memory/archives` — traces preserved by a provider-backed engine when
/// it evicts its active trace window. The base and embedded engines have no
/// archive tier, so they answer a clear refusal instead of an empty list that
/// would falsely imply there are no archived traces.
///
/// Responses map through the same camelCase [`TraceEntry`] DTO as
/// [`list_traces`], so a client that reads `cycleId`/`atMillis` from one gets
/// them from the other.
async fn archived_traces(company: ScopedCompany) -> Result<Json<Vec<TraceEntry>>, ApiError> {
    let mut traces = company.runtime.archived_traces().await?.ok_or_else(|| {
        // The route is registered for every company, but only a provider-backed
        // engine has an archive tier. A 500 would read as a server fault (and
        // prompt retries) for a permanent capability refusal; 404 is the same
        // "missing surface" answer the feedback board gives without a
        // credential — the console can treat it as "this engine keeps no
        // archives" without special-casing an error status.
        OpenCompanyError::NotFound(
            "the selected memory engine does not provide archived traces; use a provider-backed memory engine to retain evicted traces".into(),
        )
    })?;
    // Newest-first, capped at the same window the archive tier itself keeps.
    // The provider read has no limit argument, so the sort-and-tail happens
    // here as well as in the facade.
    traces.sort_by(|a, b| {
        a.at_millis
            .cmp(&b.at_millis)
            .then_with(|| a.cycle_id.cmp(&b.cycle_id))
    });
    let skip = traces.len().saturating_sub(MAX_ARCHIVED_TRACES);
    Ok(Json(
        traces
            .into_iter()
            .skip(skip)
            .map(TraceEntry::from)
            .collect(),
    ))
}

/// Max characters kept for a context entry's synthesised title (its first line).
const CONTEXT_TITLE_MAX: usize = 120;

/// A persisted cycle trace as exposed to an operator.
///
/// The route returns the full live retention window: maintenance retains at
/// most [`TRACE_RETENTION_LIMIT`] traces, so this materialises no unbounded
/// store read.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceEntry {
    cycle_id: String,
    summary: String,
    at_millis: u64,
}

impl From<CompressedTrace> for TraceEntry {
    fn from(trace: CompressedTrace) -> Self {
        Self {
            cycle_id: trace.cycle_id,
            summary: trace.summary,
            at_millis: trace.at_millis,
        }
    }
}

/// Where a rendered [`MemoryEntry`] came from. The console keys "editable vs
/// read-only" and the source label off this: only [`Fact`](MemoryOrigin::Fact)
/// rows are operator-authored and therefore deletable; the two context-derived
/// origins are the agents' own runtime memory and are read-only.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum MemoryOrigin {
    /// A durable operator-authored fact (FactStore). Editable + deletable.
    Fact,
    /// A learned-context chunk the agents recall from (ContextStore). Read-only.
    AgentMemory,
    /// A stored task outcome the harness wrote (ContextStore). Read-only.
    TaskOutcome,
    /// A chunk of a document or link an operator dropped on the Brain page
    /// (`crate::server::ops::memory_ingest`). Its own origin rather than
    /// folded into [`AgentMemory`](MemoryOrigin::AgentMemory): an operator has
    /// to be able to see what their upload became, and rendering it as
    /// something a teammate learned would say the wrong thing about where the
    /// knowledge came from.
    Document,
}

/// A durable memory entry as the console renders it.
///
/// Carries entries from two backends: operator facts (FactStore) and the agents'
/// runtime context chunks (ContextStore). `origin` + `editable` let the console
/// tell them apart — facts are editable/deletable, context rows are read-only.
/// `kind` is only meaningful for facts, so it is omitted for context rows.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryEntry {
    id: String,
    /// The fact taxonomy — present only on `Fact` rows (omitted for context).
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<FactKind>,
    /// Which backend the row came from; drives editable-vs-read-only rendering.
    origin: MemoryOrigin,
    /// Whether the operator may delete this row: facts, and the documents
    /// they dropped on the Brain page. Never the agents' own memory.
    editable: bool,
    title: String,
    body: String,
    source: String,
    updated_at: u64,
}

impl From<FactRecord> for MemoryEntry {
    fn from(f: FactRecord) -> Self {
        Self {
            id: f.id,
            kind: Some(f.kind),
            origin: MemoryOrigin::Fact,
            editable: true,
            title: f.title,
            body: f.body,
            source: f.source,
            updated_at: f.updated_at_millis,
        }
    }
}

/// A context chunk pulled from the [`ContextStore`](crate::ports::ContextStore),
/// carrying its content address (for a stable row id), logical label (for origin
/// classification), and body (peeked). The input to [`context_entries`].
struct RawChunk {
    addr: String,
    label: String,
    body: String,
    /// Epoch-millis the chunk was first stored (`0` when the backend has no
    /// stamp for it — chunks written before backends began recording one).
    stored_at_millis: u64,
}

/// Truncates `s` to at most `max` characters on a char boundary, appending an
/// ellipsis when anything was dropped. Char-based (never byte-slices) so a
/// multibyte body can't panic mid-codepoint.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

/// Splits a chunk body into a short title (its first non-empty line, truncated)
/// and the remaining body. Used to render a context chunk as a titled card.
fn split_title_body(body: &str) -> (String, String) {
    let trimmed = body.trim();
    let mut parts = trimmed.splitn(2, '\n');
    let first = parts.next().unwrap_or("").trim();
    let rest = parts.next().unwrap_or("").trim();
    (truncate_chars(first, CONTEXT_TITLE_MAX), rest.to_string())
}

/// Turns peeked context chunks into read-only [`MemoryEntry`]s, newest first
/// across both origins. Drops operator-fact mirrors (they are already
/// represented by their FactStore row — never double-list them) and applies
/// `query` (case-insensitive substring over the chunk body) so the list's
/// free-text search reaches context rows too, matching fact search.
///
/// Ordering is by stamp, never by origin. Bucketing agent memories ahead of
/// task outcomes put *every* outcome behind *every* memory whatever their
/// stamps, so the newest memory in the company could render last — the #1488
/// symptom reached by a second route, and past the cap the sort in
/// [`capped_newest_first`] was the only thing keeping it out of the list at
/// all. The console filters by origin client-side, so the grouping bought
/// nothing. The `id` tie-break carries the addr AND the label, so chunks
/// sharing a millisecond — including two labels claiming one address (#1300)
/// — keep a total, call-stable order, as the cap's sort does.
fn context_entries(chunks: Vec<RawChunk>, query: Option<&str>) -> Vec<MemoryEntry> {
    let needle = query.map(|q| q.to_lowercase());
    let mirror_prefix = format!("{OPERATOR_FACT_PREFIX}/");
    let outcome_prefix = format!("{OUTCOME_LABEL_PREFIX}/");
    let document_prefix = format!("{}/", crate::ingest::DOCUMENT_LABEL_PREFIX);

    let mut entries: Vec<MemoryEntry> = Vec::new();

    for chunk in chunks {
        // The operator-fact mirror is the same knowledge as its FactStore row;
        // surfacing it here would double-list the operator's note.
        if chunk.label.starts_with(&mirror_prefix) {
            continue;
        }
        if let Some(ref q) = needle
            && !chunk.body.to_lowercase().contains(q)
        {
            continue;
        }

        let (origin, source): (MemoryOrigin, String) = if chunk.label.starts_with(&document_prefix)
        {
            // The document's own name, which `ingest::chunk_document`
            // writes as the chunk's first line for exactly this reason: a
            // label is slugged and truncated, so it cannot be rendered
            // back as the file the operator dropped.
            let named = chunk
                .body
                .lines()
                .next()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .unwrap_or("a document");
            (MemoryOrigin::Document, named.to_string())
        } else if let Some(agent) = chunk.label.strip_prefix(&outcome_prefix) {
            let who = if agent.is_empty() { "an agent" } else { agent };
            (MemoryOrigin::TaskOutcome, who.to_string())
        } else {
            // Deliberate memories live one segment deeper —
            // `agent-memory/<agent>/<slug>` — so the naive first-segment
            // parse attributed every one of them to the literal
            // "agent-memory" (the #1290 review's M2).
            let who = match chunk.label.strip_prefix(const_format_prefix()) {
                Some(rest) => rest.split('/').next().filter(|s| !s.is_empty()),
                None => chunk.label.split('/').next().filter(|s| !s.is_empty()),
            };
            (
                MemoryOrigin::AgentMemory,
                who.unwrap_or("an agent").to_string(),
            )
        };

        let (mut title, body) = split_title_body(&chunk.body);
        if title.is_empty() {
            title = match origin {
                MemoryOrigin::TaskOutcome => "Task outcome".to_string(),
                MemoryOrigin::Document => "Document".to_string(),
                _ => "Agent memory".to_string(),
            };
        }

        entries.push(MemoryEntry {
            // Prefix so a context row's id can never collide with a fact id
            // (delete targets fact ids only; this keeps React keys unique too).
            // The LABEL is part of the id, not just the address: chunks are
            // content-addressed and one address carries one row per label
            // claiming it (#1300), so two rows here can share an address —
            // byte-identical text two agents both remembered. Keyed by address
            // alone they would collide, and the console renders these by id.
            id: format!("ctx:{}:{}", chunk.addr, chunk.label),
            kind: None,
            origin,
            // A document is material the operator supplied, so they may take
            // it back — through `…/memory/document/{slug}`, which forgets the
            // whole document rather than this one chunk of it. The two agent
            // origins stay read-only: they are the record of what the company
            // did, not something anybody typed.
            editable: matches!(origin, MemoryOrigin::Document),
            title,
            body,
            source,
            // A chunk stored before backends began stamping reports `0`, which
            // the console still renders as `—`.
            updated_at: chunk.stored_at_millis,
        });
    }

    // The cap upstream sorted metas; re-sort here because the query filter and
    // the mirror drop above both reshape the set, and because a caller that
    // hands us chunks in any other order still gets newest-first.
    entries.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    entries
}

/// The create-fact body.
#[derive(Debug, Deserialize)]
struct CreateFact {
    kind: FactKind,
    title: String,
    body: String,
    #[serde(default)]
    source: Option<String>,
}

/// Query params for `GET /memory`: an optional free-text `query` and `kind`
/// filter, both applied by the [`FactStore`](crate::ports::FactStore).
#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    kind: Option<FactKind>,
}

/// The sub-resource path (`fact_id`).
#[derive(Debug, Deserialize)]
struct FactPath {
    fact_id: String,
}

/// The Brain-tab health snapshot: how much the company remembers, across the
/// operator's durable facts and the agents' runtime context chunks. Lets the
/// console prove the store is live (non-fake) at a glance.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryStats {
    /// Number of durable operator facts.
    facts: usize,
    /// The newest fact's last-updated epoch-millis (`0` when there are none).
    facts_updated_at_millis: u64,
    /// The newest epoch-millis across *every* memory source — operator facts
    /// and the agents' context chunks alike (`0` when the company remembers
    /// nothing yet).
    ///
    /// This, not [`Self::facts_updated_at_millis`], is what the console's
    /// "Last updated" stat renders. Agents only ever write to the
    /// `ContextStore`, so a facts-only figure sat at `0` — and the stat at "—"
    /// — for any company whose operator had not hand-authored a fact, however
    /// much memory the agents had accumulated.
    last_updated_at_millis: u64,
    /// Operator facts plus the non-mirrored context chunks displayed by the
    /// Brain. This stays authoritative even when the list caps context rows.
    total_items: usize,
    /// Context chunks written by teammates, excluding task outcomes, the
    /// mirrors of operator-authored facts, and operator-dropped documents.
    teammate_memory: usize,
    /// Context chunks produced by an operator-dropped document or link (the
    /// `document/…` prefix), disjoint from teammate memory — the console
    /// renders these as their own origin, and counting them as teammate memory
    /// would attribute operator-supplied knowledge to an agent.
    document_memory: usize,
    /// Stored task outcomes, excluding operator-fact mirrors.
    task_outcomes: usize,
}

/// `GET /memory` — the rows together with the context-truncation metadata for
/// the SAME read, so the console's "newest N of M" notice never compares the
/// capped rows against a count taken at a different moment. The metadata
/// describes the unqueried browse list; a `?query=` request returns search
/// matches, not "the newest N", so it reports the metadata as not applicable.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryList {
    items: Vec<MemoryEntry>,
    /// The non-mirror context chunk population before the 500-row display
    /// cap — the "M" in the console's "showing the newest N of M" notice.
    /// Facts are never capped, so they are not counted here. `0` for a
    /// `?query=` request, whose rows are search matches the metadata does not
    /// describe.
    total_context: usize,
    /// Whether the context rows dropped any to [`MAX_CONTEXT_ENTRIES`], from
    /// this same read. Always `false` for a `?query=` request.
    context_truncated: bool,
}

/// `GET /memory` — everything the company remembers, so the console lists what
/// the Brain header counts. Returns `items` (the rows) with the context
/// truncation metadata for the same read (`totalContext`, `contextTruncated`)
/// so the console's "newest N of M" notice never compares the capped rows
/// against a count taken at a different moment. Two sources, in this order:
///
/// 1. **Operator facts** (FactStore) — newest-first, editable/deletable.
/// 2. **Context rows** (ContextStore chunks that are not operator-fact
///    mirrors) — read-only, newest-first, agent memories and task outcomes
///    interleaved by stamp rather than grouped by origin, so the newest
///    memory in the company always heads the context half. The console's
///    type filter separates the two origins client-side.
///
/// `?query=` (case-insensitive substring over title + body) filters all three.
/// `?kind=` is a *fact* taxonomy filter, so when it is set the context sources
/// are omitted (they have no `FactKind`) and only matching facts are returned —
/// preserving the type-filter's original facts-only meaning while the console's
/// wider "agent memory / task outcome" filters run client-side.
async fn list_facts(
    company: ScopedCompany,
    Query(ListQuery { query, kind }): Query<ListQuery>,
) -> Result<Json<MemoryList>, ApiError> {
    let rows = company
        .runtime
        .facts()
        .list(company.id(), query.as_deref(), kind)
        .await?;
    let mut entries: Vec<MemoryEntry> = rows.into_iter().map(MemoryEntry::from).collect();

    // The non-mirror context chunk population BEFORE the display cap — the "M"
    // in the console's "newest N of M" notice. Facts are never capped, so this
    // excludes them; a `?kind=` filter omits context entirely and leaves it 0.
    let mut total_context = 0;

    // A fact-kind filter is inherently facts-only — context chunks carry no
    // `FactKind`, so skip them (and the reads) when one is set.
    if kind.is_none() {
        // `""` lists every chunk; drop the operator-fact mirrors before peeking
        // so we neither double-list them nor pay to read their bodies, then cap
        // the reads so a huge context store can't unbound this request.
        let mirror_prefix = format!("{OPERATOR_FACT_PREFIX}/");
        let metas = company.runtime.context.list(company.id(), "").await?;
        // The truncation metadata describes the unqueried browse list — the
        // "newest N of M" notice. With `?query=` the rows are search matches,
        // not "the newest N", so the metadata is not applicable and stays 0/
        // false rather than implying a search result was omitted by the cap.
        if query.is_none() {
            total_context = metas
                .iter()
                .filter(|m| !m.label.starts_with(&mirror_prefix))
                .count();
        }
        let metas = capped_newest_first(metas, &mirror_prefix, MAX_CONTEXT_ENTRIES);
        // One batched read for every surviving body — how few round trips
        // that really is, is the backend's business (see `peek_many`); what
        // matters here is that the route no longer peeks per chunk. A body
        // that cannot be read
        // degrades that row to empty rather than failing the whole list, but
        // log it so a real storage fault surfaces instead of silently
        // rendering blank cards.
        let addrs: Vec<ChunkAddr> = metas.iter().map(|m| m.addr.clone()).collect();
        let bodies = match company
            .runtime
            .context
            .peek_many(company.id(), &addrs)
            .await
        {
            Ok(bodies) => bodies,
            Err(err) => {
                tracing::warn!(
                    company = %company.id(),
                    error = %err,
                    "bulk context read failed; rendering empty bodies"
                );
                vec![None; metas.len()]
            }
        };
        let chunks = metas
            .into_iter()
            .zip(bodies)
            .map(|(meta, body)| {
                let body = body.unwrap_or_else(|| {
                    tracing::warn!(
                        company = %company.id(),
                        addr = %meta.addr,
                        "failed to peek context chunk; rendering empty body"
                    );
                    String::new()
                });
                RawChunk {
                    addr: meta.addr.to_string(),
                    label: meta.label,
                    body,
                    stored_at_millis: meta.stored_at_millis,
                }
            })
            .collect();
        entries.extend(context_entries(chunks, query.as_deref()));
    }

    Ok(Json(MemoryList {
        items: entries,
        total_context,
        context_truncated: total_context > MAX_CONTEXT_ENTRIES,
    }))
}

/// `GET /memory/traces` — the retained, newest-last cycle trace window.
///
/// Trace summaries are intentionally not injected into a cycle: current
/// producers emit placeholders, and real compression/consumption needs its
/// own design. This inspection surface makes the durable record visible while
/// retention keeps the read bounded.
async fn list_traces(company: ScopedCompany) -> Result<Json<Vec<TraceEntry>>, ApiError> {
    let traces = company
        .runtime
        .memory
        .recent_traces(company.id(), TRACE_RETENTION_LIMIT)
        .await?;
    Ok(Json(traces.into_iter().map(TraceEntry::from).collect()))
}

/// Drops the operator-fact mirrors, then keeps the newest `cap` chunks.
///
/// Every backend lists oldest-first, so capping the head would pin the Brain
/// view to the oldest `cap` chunks forever — once a company crossed the cap, a
/// new memory could never appear again. Sort newest-first BEFORE capping; the
/// (addr, label) tie-break keeps the order total, so chunks stamped in the
/// same millisecond cannot swap places between calls. The label is part of
/// that tie-break because one address carries one row per label claiming it
/// (#1300), so the addr alone no longer separates two rows.
fn capped_newest_first(metas: Vec<ChunkMeta>, mirror_prefix: &str, cap: usize) -> Vec<ChunkMeta> {
    let mut metas: Vec<ChunkMeta> = metas
        .into_iter()
        .filter(|m| !m.label.starts_with(mirror_prefix))
        .collect();
    metas.sort_by(|a, b| {
        b.stored_at_millis
            .cmp(&a.stored_at_millis)
            .then_with(|| a.addr.as_ref().cmp(b.addr.as_ref()))
            .then_with(|| a.label.cmp(&b.label))
    });
    metas.truncate(cap);
    metas
}

/// `GET /memory/stats` — counts across the fact store and the agents' context
/// store, so the console's Brain health strip reflects the real backend.
async fn memory_stats(company: ScopedCompany) -> Result<Json<MemoryStats>, ApiError> {
    let facts = company
        .runtime
        .facts()
        .list(company.id(), None, None)
        .await?;
    // `list` is newest-first, so the head carries the freshest timestamp.
    let facts_updated_at_millis = facts.first().map(|f| f.updated_at_millis).unwrap_or(0);
    // Count the same disjoint context populations as `context_entries` without
    // its display cap: operator-fact mirrors duplicate FactStore rows and must
    // not inflate teammate memory, task outcomes get their own bucket, and
    // document/link chunks are operator-supplied material with their own
    // origin (never something a teammate learned).
    let chunks = company.runtime.context.list(company.id(), "").await?;
    // Chunks list in insertion order, not freshness order, and a backend that
    // predates the stamp reports `0` — so take the max rather than the head.
    let chunks_stored_at_millis = chunks.iter().map(|m| m.stored_at_millis).max().unwrap_or(0);
    let mirror_prefix = format!("{OPERATOR_FACT_PREFIX}/");
    let outcome_prefix = format!("{OUTCOME_LABEL_PREFIX}/");
    let document_prefix = format!("{}/", crate::ingest::DOCUMENT_LABEL_PREFIX);
    let (teammate_memory, task_outcomes, document_memory) = chunks
        .iter()
        .filter(|chunk| !chunk.label.starts_with(&mirror_prefix))
        .fold(
            (0, 0, 0),
            |(teammate_memory, task_outcomes, document_memory), chunk| {
                if chunk.label.starts_with(&outcome_prefix) {
                    (teammate_memory, task_outcomes + 1, document_memory)
                } else if chunk.label.starts_with(&document_prefix) {
                    (teammate_memory, task_outcomes, document_memory + 1)
                } else {
                    (teammate_memory + 1, task_outcomes, document_memory)
                }
            },
        );
    Ok(Json(MemoryStats {
        facts: facts.len(),
        facts_updated_at_millis,
        last_updated_at_millis: facts_updated_at_millis.max(chunks_stored_at_millis),
        total_items: facts.len() + teammate_memory + task_outcomes + document_memory,
        teammate_memory,
        document_memory,
        task_outcomes,
    }))
}

async fn create_fact(
    company: ScopedCompany,
    Json(body): Json<CreateFact>,
) -> Result<Json<MemoryEntry>, ApiError> {
    let record = FactRecord {
        id: generate_id(),
        kind: body.kind,
        title: body.title,
        body: body.body,
        source: body.source.unwrap_or_else(|| "You".to_string()),
        updated_at_millis: now_millis(),
    };
    company
        .runtime
        .facts()
        .upsert(company.id(), &record)
        .await?;

    // Mirror the fact into the agents' ContextStore so an operator note becomes
    // recallable on the agent's next turn. The harness retrieve→inject step
    // searches the ContextStore (not the FactStore), so without this mirror an
    // operator-added fact would land in the console but never reach an agent —
    // the manual-ingest loop would stay open. Best-effort: the fact is already
    // durable, so a mirror failure degrades recall (logged) rather than failing
    // the operator's write. See the module doc for the append-only-delete seam.
    let chunk = ContextChunk {
        label: format!("{OPERATOR_FACT_PREFIX}/{}", record.id),
        body: format!("{}\n{}", record.title, record.body),
    };
    if let Err(err) = company.runtime.context.put(company.id(), chunk).await {
        tracing::warn!(
            company = %company.id(),
            fact = %record.id,
            error = %err,
            "operator-fact context mirror failed; fact saved but not agent-recallable"
        );
    }

    Ok(Json(record.into()))
}

async fn delete_fact(
    company: ScopedCompany,
    Path(FactPath { fact_id }): Path<FactPath>,
) -> Result<StatusCode, ApiError> {
    if company
        .runtime
        .facts()
        .delete(company.id(), &fact_id)
        .await?
    {
        // Reap the mirrored context chunk so recall stops serving a fact the
        // operator just deleted. Best-effort: the fact row is already gone,
        // and a reap failure leaves only the pre-#1290 status quo (a stale
        // mirror), which must not turn the successful delete into an error —
        // but it must be visible, because agents keep recalling the survivor
        // while the Brain view shows the fact gone.
        if let Err(err) =
            reap_fact_mirror(company.runtime.context.as_ref(), company.id(), &fact_id).await
        {
            tracing::warn!(
                fact_id = %fact_id,
                error = %err,
                "fact deleted but its context mirror could not be reaped; \
                 recall may keep serving it until a retry"
            );
        }
        // Journal the operator deletion to the event log (audit trail).
        company
            .runtime
            .events()
            .append(
                company.id(),
                CompanyEvent::MemoryFactDeleted {
                    fact_id: fact_id.clone(),
                },
            )
            .await?;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(OpenCompanyError::CompanyNotFound(format!(
            "fact {fact_id}"
        ))))
    }
}

/// Removes the `operator-fact/{fact_id}` mirror's claim on its chunk(s),
/// label-scoped (`ContextStore::delete_label`, issue #1300): exactly the
/// mirror's own index entry goes, and the body is reaped only when no other
/// label claims it — decided atomically inside the port, so a write of
/// byte-identical content landing mid-reap keeps its row by construction
/// (this function used to snapshot-check for shared labels and then delete
/// the whole address, which both raced that write and left a shared mirror's
/// row behind forever; now the shared case removes the mirror's claim and
/// the other label keeps the body).
pub(crate) async fn reap_fact_mirror(
    context: &dyn crate::ports::ContextStore,
    company: &crate::ports::CompanyId,
    fact_id: &str,
) -> crate::Result<()> {
    let mirror_label = format!("{OPERATOR_FACT_PREFIX}/{fact_id}");
    let all = context.list(company, "").await?;
    for meta in all.iter().filter(|m| m.label == mirror_label) {
        context
            .delete_label(company, &meta.addr, &mirror_label)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "memory_reap_tests.rs"]
mod reap_test;

#[cfg(test)]
#[path = "memory_combined_list_tests.rs"]
mod combined_list_tests;

#[cfg(test)]
#[path = "memory_route_tests.rs"]
mod route_tests;

#[cfg(all(test, feature = "openhuman"))]
mod tests {
    /// The local prefix constant must track the harness's, since the two label
    /// the same chunks from opposite sides of the `openhuman` feature gate.
    #[test]
    fn outcome_prefix_matches_harness() {
        assert_eq!(
            super::OUTCOME_LABEL_PREFIX,
            crate::harness::memory_loop::OUTCOME_LABEL_PREFIX
        );
    }
}
