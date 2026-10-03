//! Typed facades over one [`MemoryEngine`].
//!
//! The three memory ports stay, because their types are the company's
//! vocabulary and every call site is written against them. What collapses is the
//! *backends*: instead of three independent stores, all three ports become thin
//! views onto a single bound engine.
//!
//! ## How a keyed record maps onto an item
//!
//! The ports are keyed (a fact id, a chunk address, a cycle id); the v2 contract
//! is not — `store` mints a content fingerprint and there is no `get(key)`. So
//! every record is one `Document` item whose metadata carries the addressing:
//!
//! - `workspace` = the record's [`Namespace`] (company root + scope), and
//! - `source.id` = the port's key.
//!
//! Those are exactly the two fields a hosted engine narrows on server-side
//! (CortexDB labels them), so a keyed read is one narrowed listing, not a walk.
//! A rewrite stores the new body first and forgets the old items second: a
//! crash in between leaves a duplicate the next read reconciles (newest
//! `observed_at` wins), never a lost record.
//!
//! ## Why the records are JSON, not item-native structure
//!
//! The port records are richer than any item kind (a trace, a fact with a
//! kind and a timestamp, a chunk with a label set), so the facade owns the
//! encoding and each one carries a round-trip test.
//!
//! ## Every read is re-checked against the namespace it asked for
//!
//! An engine is somebody else's code — increasingly, somebody else's *service*.
//! Asking for a workspace and trusting the answer to be within it is exactly
//! the assumption a hosted engine is in a position to violate, by bug or
//! otherwise. So every decode path drops items whose reported workspace is not
//! the one this facade owns. The filter should never fire; if it does, the
//! alternative was serving one tenant another's memory.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tinymemory::{
    DocumentBody, Error as EngineError, FetchMode, FetchRequest, ForgetTarget, Hit, ItemId,
    ItemKind, ListRequest, MemoryEngine, MemoryMeta, MetaFilter, SourceKind, SourceRef, StoreItem,
};

use super::namespace::{Namespace, Scope};
use crate::error::OpenCompanyError;
use crate::ports::{
    ChunkAddr, ChunkHit, ChunkMeta, CompanyId, CompressedTrace, ContextChunk, ContextStore,
    EvictionPolicy, FactKind, FactRecord, FactStore, MemoryStore, TaskResult,
};
use crate::runtime::maintenance::TRACE_RETENTION_LIMIT;
use crate::store::text::{ceil_boundary, slice_on_char_boundaries};
use crate::{Result, store::content_address};

/// Envelope version. Bumped only if the on-the-wire shape of a record changes
/// incompatibly; a decoder that meets a version it does not know refuses rather
/// than guessing, because a half-understood memory record is worse than a
/// missing one.
const ENVELOPE_VERSION: u8 = 1;

/// The tag every inbound-channel write carries, beside `SourceKind::Link`.
///
/// The v1 contract had a `MemoryTaint::ExternalSync` the engine stored; v2 has
/// no taint, so external provenance is metadata this host stamps. A tag (not
/// only the source kind) so an operator filtering an engine's own console by
/// tag sees it, and so a future reader can refuse it by `tags_any`.
pub const EXTERNAL_TAG: &str = "oc:provenance:external";

/// Page size for walking a partition. Large enough that a typical partition is
/// one round trip, small enough that one page is a bounded response.
const LIST_PAGE: usize = 200;

/// Hard ceiling on pages walked in one listing, so an engine that keeps handing
/// back a cursor cannot turn one port call into an unbounded walk.
const MAX_LIST_PAGES: usize = 500;

/// The MIME type stamped on every envelope, so an engine's own tooling (and an
/// operator reading it) can tell these are structured records, not prose.
const ENVELOPE_MIME: &str = "application/vnd.opencompany.memory+json";

/// The wire form of a typed port record inside an item's text.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope<T> {
    /// Format version — see [`ENVELOPE_VERSION`].
    v: u8,
    /// The port's own record, verbatim.
    record: T,
}

/// Characters a hosted engine removes from text, escaped on the way out.
///
/// Hosted engines have been measured stripping `U+FFFD` server-side
/// (tinymemory#80). An engine is within its rights to sanitise text it is
/// handed; what breaks is that this host does not hand it text, it hands it a
/// JSON envelope, and a character removed from the middle of that envelope
/// comes back as a record whose body is quietly one character shorter than it
/// was written.
///
/// `U+0000` is deliberately absent: RFC 8259 requires escaping `U+0000`
/// through `U+001F`, so `serde_json` already emits it as `\u0000`.
const CHARACTERS_ENGINES_STRIP: [char; 1] = ['\u{FFFD}'];

/// Encodes a typed record for an item's text.
///
/// Rewriting the serialized text is safe: JSON's structural characters are all
/// ASCII, so a character from [`CHARACTERS_ENGINES_STRIP`] can only ever occur
/// inside a string literal, and substituting its `\uXXXX` form yields an
/// equivalent document.
fn encode<T: Serialize>(record: &T) -> Result<String> {
    let json = serde_json::to_string(&Envelope {
        v: ENVELOPE_VERSION,
        record,
    })
    .map_err(|error| OpenCompanyError::Store(format!("could not encode memory record: {error}")))?;
    Ok(CHARACTERS_ENGINES_STRIP
        .iter()
        .fold(json, |text, character| {
            if text.contains(*character) {
                text.replace(*character, &format!("\\u{:04x}", *character as u32))
            } else {
                text
            }
        }))
}

/// Whether `hit` is inside `namespace` — the re-check every read applies.
fn owned_by(hit: &Hit, namespace: &Namespace) -> bool {
    let reported = hit.meta.workspace.as_deref().unwrap_or_default();
    if reported == namespace.as_str() {
        return true;
    }
    tracing::warn!(
        expected = namespace.as_str(),
        reported,
        "memory engine returned an item outside the requested namespace; dropping it"
    );
    false
}

/// Decodes one item, or `None` when it is not ours to read.
///
/// Returns `None` — rather than an error — for an item outside `namespace` or
/// written by a version we do not understand. A single unreadable row must not
/// fail a whole `list`: on a shared hosted engine the store may legitimately
/// hold rows this build did not write.
///
/// A row *inside* our namespace that fails to parse is different: nothing else
/// writes there, so it is a record this host stored and can no longer read — a
/// corrupted write, not foreign data. It is still skipped, but loudly (#1201).
fn decode<T: DeserializeOwned>(hit: &Hit, namespace: &Namespace) -> Option<T> {
    if !owned_by(hit, namespace) {
        return None;
    }
    let key = hit.meta.source.id.as_deref().unwrap_or_default();
    let corrupt = |error: &dyn std::fmt::Display| {
        tracing::warn!(
            namespace = namespace.as_str(),
            key,
            %error,
            "memory item in our namespace failed to decode; dropping it \
             (a record we wrote and can no longer read — see #1201)"
        );
    };
    let envelope: Envelope<serde_json::Value> = match serde_json::from_str(&hit.text) {
        Ok(envelope) => envelope,
        Err(error) => {
            corrupt(&error);
            return None;
        }
    };
    if envelope.v != ENVELOPE_VERSION {
        tracing::debug!(
            namespace = namespace.as_str(),
            key,
            version = envelope.v,
            "memory item has an envelope version this build does not understand; skipping it"
        );
        return None;
    }
    match serde_json::from_value(envelope.record) {
        Ok(record) => Some(record),
        Err(error) => {
            corrupt(&error);
            None
        }
    }
}

/// Maps an engine error onto the crate error type.
pub(super) fn store_error(error: EngineError) -> OpenCompanyError {
    match error {
        EngineError::NotFound(what) => OpenCompanyError::NotFound(what),
        EngineError::InvalidRequest(why) => OpenCompanyError::InvalidRequest(why),
        // Not `Unimplemented`: that variant means *this build* has no code for a
        // port. This means the operator bound an engine that cannot do what was
        // asked, which is a deployment fact they can act on, so the engine's own
        // words are worth keeping.
        EngineError::Unsupported(what) => OpenCompanyError::Store(format!(
            "the bound memory engine does not support this: {what}"
        )),
        other => OpenCompanyError::Store(other.to_string()),
    }
}

/// Where a facade's writes come from — the v2 stand-in for the v1 taint enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Provenance {
    /// The company writing about itself: operator facts, agent working-out.
    Internal,
    /// Content that arrived from an inbound channel or the web.
    External,
}

impl Provenance {
    /// The source kind stamped on every item this facade writes.
    fn source_kind(self) -> SourceKind {
        match self {
            Self::Internal => SourceKind::Agent,
            Self::External => SourceKind::Link,
        }
    }
}

/// Shared plumbing: an engine, and which partition of a company's memory this
/// facade addresses.
///
/// # Why the company is a per-call argument, not a field
///
/// One `MemoryOverlay` is opened per *process* and injected into every
/// company's runtime, so a facade instance is shared by every tenant this host
/// serves. A namespace fixed at construction would therefore be one company's
/// namespace serving all of them — a cross-tenant leak, and exactly the defect
/// this module exists to prevent.
///
/// So the namespace is derived on every call from the `&CompanyId` the port
/// method was given: it cannot be stale, cannot be mismatched with the caller's
/// intent, and cannot be set to a company the caller was not holding.
#[derive(Clone)]
pub(super) struct Bound {
    engine: Arc<dyn MemoryEngine>,
    scope: Scope,
    provenance: Provenance,
}

impl Bound {
    pub(super) fn new(engine: Arc<dyn MemoryEngine>, scope: Scope, provenance: Provenance) -> Self {
        Self {
            engine,
            scope,
            provenance,
        }
    }

    /// The namespace this facade addresses for `company`.
    fn namespace(&self, company: &CompanyId) -> Namespace {
        Namespace::company_root(company).child(&self.scope)
    }

    /// The filter every read of `namespace` carries, optionally narrowed to
    /// one key.
    fn filter(namespace: &Namespace, key: Option<&str>) -> MetaFilter {
        MetaFilter {
            workspace: Some(namespace.as_str().to_string()),
            source_id: key.map(str::to_string),
            kinds: vec![ItemKind::Document],
            ..MetaFilter::default()
        }
    }

    /// Every item in `namespace` (optionally one key), walked page by page and
    /// re-checked against the namespace.
    async fn items(&self, namespace: &Namespace, key: Option<&str>) -> Result<Vec<Hit>> {
        let mut request = ListRequest::new(Self::filter(namespace, key), LIST_PAGE);
        let mut out = Vec::new();
        for _ in 0..MAX_LIST_PAGES {
            let page = self
                .engine
                .list(request.clone())
                .await
                .map_err(store_error)?;
            out.extend(page.items.into_iter().filter(|hit| {
                owned_by(hit, namespace)
                    && key.is_none_or(|key| hit.meta.source.id.as_deref() == Some(key))
            }));
            match page.next_cursor {
                Some(cursor) => request.cursor = Some(cursor),
                None => return Ok(out),
            }
        }
        Err(OpenCompanyError::Store(format!(
            "memory engine kept paging past {MAX_LIST_PAGES} pages listing one partition; \
             refusing to walk further"
        )))
    }

    /// The live item for `key`: the newest when a crashed rewrite left two.
    async fn current(&self, namespace: &Namespace, key: &str) -> Result<Option<Hit>> {
        Ok(self
            .items(namespace, Some(key))
            .await?
            .into_iter()
            .max_by_key(|hit| hit.meta.observed_at))
    }

    /// Stores one typed record under `key`, replacing whatever was there.
    async fn put<T: Serialize + Sync>(
        &self,
        company: &CompanyId,
        key: &str,
        record: &T,
        tag: &str,
    ) -> Result<()> {
        let namespace = self.namespace(company);
        let previous = self.items(&namespace, Some(key)).await?;
        let mut tags = vec![format!("oc:{tag}")];
        if self.provenance == Provenance::External {
            tags.push(EXTERNAL_TAG.to_string());
        }
        let meta = MemoryMeta {
            workspace: Some(namespace.as_str().to_string()),
            source: SourceRef {
                kind: self.provenance.source_kind(),
                id: Some(key.to_string()),
            },
            tags,
            observed_at: Some(tinymemory::chrono::Utc::now()),
            ..MemoryMeta::default()
        };
        let receipt = self
            .engine
            .store(StoreItem::Document {
                title: None,
                body: DocumentBody::Text(encode(record)?),
                mime: Some(ENVELOPE_MIME.to_string()),
                meta,
            })
            .await
            .map_err(store_error)?;
        // Store first, forget second: a crash between the two leaves a
        // duplicate that `current` reconciles, never a lost record.
        let stale: Vec<ItemId> = previous
            .into_iter()
            .map(|hit| hit.id)
            .filter(|id| *id != receipt.id)
            .collect();
        if !stale.is_empty() {
            self.engine
                .forget(ForgetTarget::Ids(stale))
                .await
                .map_err(store_error)?;
        }
        Ok(())
    }

    /// Fetches one typed record by key.
    async fn get<T: DeserializeOwned>(&self, company: &CompanyId, key: &str) -> Result<Option<T>> {
        let namespace = self.namespace(company);
        Ok(self
            .current(&namespace, key)
            .await?
            .and_then(|hit| decode(&hit, &namespace)))
    }

    /// Whether the engine holds a record at `key` at all, **without decoding
    /// it**.
    ///
    /// [`Self::get`] answers `None` for two different facts: the engine has no
    /// such record, and the engine has one this build cannot read. A caller
    /// that reports "there was nothing there" to a user must not conflate them.
    async fn exists(&self, company: &CompanyId, key: &str) -> Result<bool> {
        let namespace = self.namespace(company);
        Ok(self.current(&namespace, key).await?.is_some())
    }

    /// Lists every typed record in this company's partition, one per key.
    async fn list<T: DeserializeOwned>(&self, company: &CompanyId) -> Result<Vec<T>> {
        let namespace = self.namespace(company);
        let mut newest: HashMap<String, Hit> = HashMap::new();
        for hit in self.items(&namespace, None).await? {
            let key = hit.meta.source.id.clone().unwrap_or_default();
            match newest.get(&key) {
                Some(held) if held.meta.observed_at >= hit.meta.observed_at => {}
                _ => {
                    newest.insert(key, hit);
                }
            }
        }
        Ok(newest
            .values()
            .filter_map(|hit| decode(hit, &namespace))
            .collect())
    }

    /// Deletes one record, reporting whether it existed.
    async fn forget(&self, company: &CompanyId, key: &str) -> Result<bool> {
        let namespace = self.namespace(company);
        let report = self
            .engine
            .forget(ForgetTarget::Filter(Self::filter(&namespace, Some(key))))
            .await
            .map_err(store_error)?;
        Ok(report.forgotten > 0)
    }

    /// Ranked retrieval, narrowed to this partition on the way in and
    /// re-checked on the way out.
    ///
    /// `fetch`, not `recall`: recall synthesises an answer over the engine's
    /// whole scope (derived facts included) and only filters its citations,
    /// while fetch returns the stored items themselves, ranked.
    async fn search(
        &self,
        company: &CompanyId,
        query: &str,
        limit: usize,
    ) -> Result<(Namespace, Vec<Hit>)> {
        let namespace = self.namespace(company);
        let descriptor = self.engine.descriptor();
        let Some(mode) = [FetchMode::Hybrid, FetchMode::Keyword, FetchMode::Vector]
            .into_iter()
            .find(|mode| descriptor.supports(*mode))
        else {
            return Ok((namespace, Vec::new()));
        };
        if query.trim().is_empty() || limit == 0 {
            return Ok((namespace, Vec::new()));
        }
        let mut request = FetchRequest::new(query, mode, limit);
        request.filter = Self::filter(&namespace, None);
        let hits = self
            .engine
            .fetch(request)
            .await
            .map_err(store_error)?
            .hits
            .into_iter()
            .filter(|hit| owned_by(hit, &namespace))
            .collect();
        Ok((namespace, hits))
    }
}

// ---------------------------------------------------------------------------
// FactStore
// ---------------------------------------------------------------------------

/// The operator's hand-curated facts.
///
/// The closest fit of the three ports: `list`/`upsert`/`delete` map onto
/// `list`/`store`/`forget` almost exactly, and `forget` already returns the
/// bool `delete` needs.
pub struct ProviderFactStore {
    bound: Bound,
}

impl ProviderFactStore {
    pub(super) fn new(bound: Bound) -> Self {
        Self { bound }
    }
}

#[async_trait]
impl FactStore for ProviderFactStore {
    async fn list(
        &self,
        company: &CompanyId,
        query: Option<&str>,
        kind: Option<FactKind>,
    ) -> Result<Vec<FactRecord>> {
        let mut facts: Vec<FactRecord> = self.bound.list(company).await?;
        if let Some(kind) = kind {
            facts.retain(|fact| fact.kind == kind);
        }
        if let Some(needle) = query.map(str::trim).filter(|q| !q.is_empty()) {
            let needle = needle.to_lowercase();
            facts.retain(|fact| {
                fact.title.to_lowercase().contains(&needle)
                    || fact.body.to_lowercase().contains(&needle)
            });
        }
        // Most-recently-updated first, with the id as a tiebreak so the order is
        // total: two facts saved in the same millisecond must not swap places
        // between calls, or the console list flickers.
        facts.sort_by(|a, b| {
            b.updated_at_millis
                .cmp(&a.updated_at_millis)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(facts)
    }

    async fn upsert(&self, company: &CompanyId, fact: &FactRecord) -> Result<()> {
        self.bound.put(company, &fact.id, fact, "fact").await
    }

    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        self.bound.forget(company, id).await
    }
}

// ---------------------------------------------------------------------------
// ContextStore
// ---------------------------------------------------------------------------

/// The RLM environment.
///
/// Two host-side gaps the contract does not cover, both called out in
/// `docs/spec/runtime/orchestration/memory.md`:
///
/// - **Ranged `peek`** becomes a slice after a whole-entry read. The contract
///   has no ranged accessor, and inventing one per driver would be worse than
///   reading a chunk that is already bounded by construction.
/// - **`list` by label prefix** is a host-side filter, for the same reason.
pub struct ProviderContextStore {
    bound: Bound,
    /// Serializes the label-set read-merge-writes (`put`, `delete_label`) on
    /// the stored envelope (#1300). The contract's `store` is a whole-value
    /// upsert with no compare-and-set, so two concurrent puts of one body
    /// under different labels would otherwise both read the same envelope and
    /// one label would silently lose — the same reasoning as the fs backend's
    /// per-path lock, and process-local for the same reason it is there: this
    /// facade is the company's only writer of its partition.
    label_lock: tokio::sync::Mutex<()>,
}

impl ProviderContextStore {
    pub(super) fn new(bound: Bound) -> Self {
        Self {
            bound,
            label_lock: tokio::sync::Mutex::new(()),
        }
    }
}

/// A stored chunk: the port's [`ContextChunk`] plus the metadata `list` reports.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredChunk {
    /// The first label to claim this address — kept meaningful on its own so
    /// an envelope written by (or later read by) a binary from before
    /// `labels` existed still carries a real claim.
    label: String,
    body: String,
    stored_at_millis: u64,
    /// Every label claiming this address (#1300); envelopes from before the
    /// field decode empty, and [`stored_labels`] unions the scalar back in.
    #[serde(default)]
    labels: Vec<String>,
}

/// Every label claiming `chunk`, deduped, scalar (first-stored) label first.
fn stored_labels(chunk: &StoredChunk) -> Vec<String> {
    let mut labels = vec![chunk.label.clone()];
    for label in &chunk.labels {
        if !labels.iter().any(|have| have == label) {
            labels.push(label.clone());
        }
    }
    labels
}

#[async_trait]
impl ContextStore for ProviderContextStore {
    async fn put(&self, company: &CompanyId, chunk: ContextChunk) -> Result<ChunkAddr> {
        // The shared content address, so this backend mints the same addr for
        // the same body as fs / sqlite / mongodb do.
        let addr = content_address(&chunk.body);
        // Under the label lock: the merge below is a read-merge-write over a
        // plain upsert (#1300).
        let _guard = self.label_lock.lock().await;
        // Chunks are append-only and never rewritten. `store` is an upsert, so
        // without this check a re-`put` of an identical body would restamp
        // `stored_at_millis` and move the Brain header's "last updated" backwards
        // in meaning — it would start reporting when a chunk was last *re-seen*
        // rather than when it was first written. sqlite and mongodb keep the
        // first write; match them. A new label on an existing body is folded
        // into the envelope's label set instead — one claim per (addr, label).
        if let Some(existing) = self.bound.get::<StoredChunk>(company, &addr).await? {
            // A hit is almost always the same body written twice. It can also be
            // a content-address collision: `content_address` is a 64-bit
            // non-cryptographic hash (`crate::store::content_address`, shared by
            // every backend), so two different bodies can mint one address. That
            // is a pre-existing property of the address scheme rather than
            // anything this facade introduces — sqlite and mongodb keep the
            // first write for a collision exactly as this does, and changing the
            // scheme would move every existing chunk's address on every backend.
            //
            // What is worth doing here is refusing to be *silent* about it. On a
            // collision `peek(addr)` returns a body the caller never wrote, and
            // an operator debugging that has no way to reach this conclusion
            // from the outside. So compare, and say so when they differ.
            if existing.body != chunk.body {
                tracing::error!(
                    addr = %addr,
                    label = %chunk.label,
                    existing_label = %existing.label,
                    "content-address collision: two different chunk bodies hashed to the same \
                     address. The first body is kept and this write is dropped, so reads of this \
                     address return the earlier chunk. See crate::store::content_address."
                );
                return Ok(ChunkAddr::new(addr));
            }
            let labels = stored_labels(&existing);
            if !labels.iter().any(|have| have == &chunk.label) {
                let mut updated = existing;
                updated.labels = labels;
                updated.labels.push(chunk.label);
                self.bound.put(company, &addr, &updated, "chunk").await?;
            }
            return Ok(ChunkAddr::new(addr));
        }
        let stored = StoredChunk {
            labels: vec![chunk.label.clone()],
            label: chunk.label,
            body: chunk.body,
            stored_at_millis: crate::ports::now_millis(),
        };
        self.bound.put(company, &addr, &stored, "chunk").await?;
        Ok(ChunkAddr::new(addr))
    }

    async fn list(&self, company: &CompanyId, prefix: &str) -> Result<Vec<ChunkMeta>> {
        let chunks: Vec<StoredChunk> = self.bound.list(company).await?;
        let mut metas: Vec<ChunkMeta> = chunks
            .into_iter()
            .flat_map(|chunk| {
                // One meta per label claiming the address (#1300); the stamp
                // is the address's first write, since the envelope is one
                // record however many labels claim it.
                let addr = content_address(&chunk.body);
                let len = chunk.body.len();
                let stored_at_millis = chunk.stored_at_millis;
                stored_labels(&chunk)
                    .into_iter()
                    .filter(|label| label.starts_with(prefix))
                    .map(move |label| ChunkMeta {
                        addr: ChunkAddr::new(addr.clone()),
                        label,
                        len,
                        stored_at_millis,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        metas.sort_by(|a, b| {
            a.stored_at_millis
                .cmp(&b.stored_at_millis)
                .then_with(|| a.addr.as_ref().cmp(b.addr.as_ref()))
                // The label completes the order: two labels claiming one
                // address (#1300) share a stamp and an addr, so without it
                // their relative order would rest on enumeration order alone.
                .then_with(|| a.label.cmp(&b.label))
        });
        Ok(metas)
    }

    async fn peek(
        &self,
        company: &CompanyId,
        addr: &ChunkAddr,
        range: Option<Range<usize>>,
    ) -> Result<String> {
        let chunk: StoredChunk =
            self.bound
                .get(company, addr.as_ref())
                .await?
                .ok_or_else(|| {
                    OpenCompanyError::NotFound(format!("context chunk {}", addr.as_ref()))
                })?;
        let Some(range) = range else {
            return Ok(chunk.body);
        };
        Ok(slice_on_char_boundaries(&chunk.body, range))
    }

    async fn peek_many(
        &self,
        company: &CompanyId,
        addrs: &[ChunkAddr],
    ) -> Result<Vec<Option<String>>> {
        // `bound.list` already decodes every body in the partition, so one
        // enumeration answers the whole batch — the default's per-addr `peek`
        // would walk the provider once per chunk for the same bytes.
        let chunks: Vec<StoredChunk> = self.bound.list(company).await?;
        let by_addr: HashMap<String, String> = chunks
            .into_iter()
            .map(|chunk| (content_address(&chunk.body), chunk.body))
            .collect();
        Ok(addrs
            .iter()
            .map(|addr| by_addr.get(addr.as_ref()).cloned())
            .collect())
    }

    async fn delete(&self, company: &CompanyId, addr: &ChunkAddr) -> Result<bool> {
        // Under the label lock so an interleaved `put`'s read-merge-write
        // cannot resurrect an envelope this is removing.
        let _guard = self.label_lock.lock().await;
        // The engine keys chunks by their content address (see `put`), so the
        // port's addr IS the engine key — the envelope goes with every label
        // claiming it. On an address collision (64-bit non-cryptographic hash
        // — see `put`'s comment) the single stored body goes, whichever writer
        // minted it first; that is the same first-write-wins property every
        // backend already has.
        self.bound.forget(company, addr.as_ref()).await
    }

    async fn delete_label(
        &self,
        company: &CompanyId,
        addr: &ChunkAddr,
        label: &str,
    ) -> Result<bool> {
        // Label-scoped (#1300): remove one claim from the envelope's label
        // set, and forget the envelope exactly when the last claim goes. The
        // read-merge-write and the reap decision sit under the same lock every
        // put holds, so a concurrent put of identical content under another
        // label either lands its claim before this read or re-creates the
        // envelope after the forget — never loses its claim in between.
        let _guard = self.label_lock.lock().await;
        let Some(existing) = self
            .bound
            .get::<StoredChunk>(company, addr.as_ref())
            .await?
        else {
            // `get` answering `None` is two different facts, and only one of
            // them is "nothing to forget". If the engine DOES hold a record
            // here, this build simply cannot read its envelope — and returning
            // `Ok(false)` would tell `memory_forget` to reply "already gone"
            // about a chunk recall keeps serving, with nothing anywhere saying
            // otherwise. Refuse instead, naming the address, so the operator
            // gets a report rather than a lie.
            //
            // Deliberately NOT a forget-by-key fallback: the envelope is what
            // says which labels claim this address, so an unreadable one means
            // an unknown claim set, and removing the record could take a label
            // this caller never owned.
            if self.bound.exists(company, addr.as_ref()).await? {
                return Err(OpenCompanyError::Store(format!(
                    "context chunk {} exists but its envelope could not be decoded, so its \
                     label claims are unknown and `{label}` cannot be removed safely; the \
                     record needs repair or an operator-level delete",
                    addr.as_ref()
                )));
            }
            return Ok(false);
        };
        let mut labels = stored_labels(&existing);
        let before = labels.len();
        labels.retain(|have| have != label);
        if labels.len() == before {
            return Ok(false);
        }
        if labels.is_empty() {
            // The claim existed and is gone either way: `forget` answering
            // false here means another writer (a second process on a remote
            // driver, outside this process-local lock) reaped the envelope
            // first, which is the same end state.
            self.bound.forget(company, addr.as_ref()).await?;
            return Ok(true);
        }
        let updated = StoredChunk {
            // The scalar stays a real label so an envelope read by a binary
            // from before `labels` still carries a live claim.
            label: labels[0].clone(),
            labels,
            body: existing.body,
            stored_at_millis: existing.stored_at_millis,
        };
        self.bound
            .put(company, addr.as_ref(), &updated, "chunk")
            .await?;
        Ok(true)
    }

    async fn search(
        &self,
        company: &CompanyId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ChunkHit>> {
        let (namespace, hits) = self.bound.search(company, query, limit).await?;
        Ok(hits
            .iter()
            .filter_map(|hit| {
                let chunk: StoredChunk = decode(hit, &namespace)?;
                Some(ChunkHit {
                    addr: ChunkAddr::new(content_address(&chunk.body)),
                    snippet: snippet(&chunk.body),
                    // The port promises `[0, 1]`; engines rank on their own
                    // scale, so clamp rather than trust it.
                    score: hit.score.clamp(0.0, 1.0),
                })
            })
            .take(limit)
            .collect())
    }
}

/// The leading window of a body, used as a search snippet.
fn snippet(body: &str) -> String {
    const MAX: usize = 200;
    if body.len() <= MAX {
        return body.to_string();
    }
    body[..ceil_boundary(body, MAX)].to_string()
}

// ---------------------------------------------------------------------------
// MemoryStore
// ---------------------------------------------------------------------------

/// The brain's compressed traces and task results.
///
/// The spec sequences this port last, and says it may reasonably never move:
/// append-only, eviction-driven trace rows are the shape the contract suits
/// least. It is here because leaving one port on a different backend would mean
/// the export bundle spans two engines.
///
/// The gap this closes is `evict`. The contract has no archive tier and no bulk
/// delete by predicate, so eviction is a **move** between two namespaces — see
/// [`ProviderMemoryStore::evict`].
pub struct ProviderMemoryStore {
    traces: Bound,
    archive: Bound,
    task_results: Bound,
}

impl ProviderMemoryStore {
    pub(super) fn new(traces: Bound, archive: Bound, task_results: Bound) -> Self {
        Self {
            traces,
            archive,
            task_results,
        }
    }

    /// Reads the live trace set, oldest first.
    async fn ordered_traces(&self, company: &CompanyId) -> Result<Vec<CompressedTrace>> {
        let mut traces: Vec<CompressedTrace> = self.traces.list(company).await?;
        // Total order, not just by timestamp: two traces stamped in the same
        // millisecond must not reorder between reads, or `recent_traces` returns
        // a different window each call and eviction evicts a different set.
        traces.sort_by(|a, b| {
            a.at_millis
                .cmp(&b.at_millis)
                .then_with(|| a.cycle_id.cmp(&b.cycle_id))
        });
        Ok(traces)
    }

    /// Reads the archived trace set, for the operator's inspection.
    ///
    /// The archive is a bounded recovery tier on this facade: eviction moves
    /// traces here rather than destroying them. The export path carries this
    /// tier separately from the live `GET /memory/traces` window — both read
    /// distinct namespaces. This accessor exists so the operator tier and the
    /// "archives rather than destroys" property tests can observe the tier itself.
    pub(super) async fn archived_traces(
        &self,
        company: &CompanyId,
    ) -> Result<Vec<CompressedTrace>> {
        self.archive.list(company).await
    }

    /// Restores traces directly into the archive tier.
    pub(super) async fn restore_archived_traces(
        &self,
        company: &CompanyId,
        traces: &[CompressedTrace],
    ) -> Result<()> {
        for trace in traces {
            self.archive
                .put(company, &trace.cycle_id, trace, "trace")
                .await?;
        }
        Ok(())
    }

    /// Bounds the archive tier to the newest `n` archived traces.
    ///
    /// Eviction moves traces OUT of the live window rather than destroying
    /// them; without a matching cap here the archive would retain every trace a
    /// company ever evicted, so the documented retention policy would bound the
    /// inspectable window but not storage. Keeping the newest `n` evicted
    /// traces bounds the tier at `n` and total trace storage at `2n` — the live
    /// window plus the eviction history nearest to it.
    async fn prune_archive(&self, id: &CompanyId, n: usize) -> Result<()> {
        if n == 0 {
            let archived = self.archive.list::<CompressedTrace>(id).await?;
            for trace in archived {
                self.archive.forget(id, &trace.cycle_id).await?;
            }
            return Ok(());
        }
        let mut archived = self.archive.list::<CompressedTrace>(id).await?;
        if archived.len() <= n {
            return Ok(());
        }
        // Same total order as the live set, so "newest" is unambiguous even
        // when two traces share a millisecond.
        archived.sort_by(|a, b| {
            a.at_millis
                .cmp(&b.at_millis)
                .then_with(|| a.cycle_id.cmp(&b.cycle_id))
        });
        let prune = archived.len() - n;
        for trace in archived.into_iter().take(prune) {
            self.archive.forget(id, &trace.cycle_id).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl MemoryStore for ProviderMemoryStore {
    async fn save_trace(&self, id: &CompanyId, trace: CompressedTrace) -> Result<()> {
        self.traces.put(id, &trace.cycle_id, &trace, "trace").await
    }

    async fn recent_traces(&self, id: &CompanyId, limit: usize) -> Result<Vec<CompressedTrace>> {
        // Avoid even touching the provider when the caller requests no rows.
        // This matters for the provider-backed facade because `list` has no
        // limit argument and otherwise decodes the entire trace partition.
        if limit == 0 {
            return Ok(Vec::new());
        }
        let traces = self.ordered_traces(id).await?;
        // Newest last, per the port contract, so the tail is the window.
        let skip = traces.len().saturating_sub(limit);
        Ok(traces.into_iter().skip(skip).collect())
    }

    async fn save_task_result(&self, id: &CompanyId, result: TaskResult) -> Result<()> {
        self.task_results
            .put(id, &result.task_id, &result, "task-result")
            .await
    }

    /// Evicts per `policy`, **archiving rather than destroying**.
    ///
    /// `docs/spec/company-brain/memory.md` makes this normative: "evicted traces
    /// are archived, not deleted, until retention policy or the Operator says
    /// otherwise". The contract offers no archive tier, so the behaviour lives
    /// here as a move between two namespaces.
    ///
    /// Order matters and is not arbitrary: the archive write happens **before**
    /// the live delete. There is no transaction spanning two provider calls, so
    /// one of the two orders has to be chosen for what it does when the process
    /// dies in between. Archive-then-delete leaves a trace in both places — a
    /// duplicate the next read reconciles. Delete-then-archive loses it. For a
    /// port whose whole promise is "not destroyed", that asymmetry decides it.
    ///
    /// The returned count is **traces this call removed from the live set**, not
    /// traces archived. Those differ when `forget` reports a key was already
    /// gone: the archive write has happened by then, so the archive can hold an
    /// entry this call did not remove. That is a concurrent eviction having got
    /// there first, and the entry is archived either way — which is the
    /// behaviour the port promises. Reporting it as removed *here* would be the
    /// lie, so the count stays narrow.
    ///
    /// The same asymmetry appears if a `put` or `forget` fails mid-loop: the
    /// error propagates and the traces already processed stay archived. That is
    /// the archive-then-delete order behaving as designed under partial failure
    /// — a duplicate the next read reconciles, never a loss. What must NOT be
    /// skipped on that path is the archive bound itself: traces already moved
    /// by the failed pass are still in the archive, so the prune below runs
    /// before the error propagates, keeping the tier at its limit even when a
    /// maintenance pass repeatedly fails partway.
    ///
    /// Every eviction additionally bounds the archive itself to the newest
    /// `n` evicted traces (see [`ProviderMemoryStore::prune_archive`]): a
    /// `KeepRecent { n }` eviction bounds it to `n`, and `OlderThan` — which
    /// has no `n` of its own — to the retention limit, so the policy that
    /// bounds the live window also bounds storage on every path: a company
    /// that runs for years does not accumulate every trace it ever evicted
    /// beside the 32 it keeps, and an operator-sized `OlderThan` sweep cannot
    /// grow the archive without bound. That bound is what keeps
    /// `GET /memory/archives` a bounded read by construction rather than a
    /// download of the whole archive followed by a discard.
    async fn evict(&self, id: &CompanyId, policy: EvictionPolicy) -> Result<u64> {
        let traces = self.ordered_traces(id).await?;
        let doomed: Vec<CompressedTrace> = match &policy {
            EvictionPolicy::KeepRecent { n } => {
                let keep_from = traces.len().saturating_sub(*n);
                traces.into_iter().take(keep_from).collect()
            }
            EvictionPolicy::OlderThan { before_millis } => traces
                .into_iter()
                .filter(|trace| trace.at_millis < *before_millis)
                .collect(),
        };
        let mut evicted = 0u64;
        let move_result = (async {
            for trace in doomed {
                self.archive
                    .put(id, &trace.cycle_id, &trace, "trace")
                    .await?;
                if self.traces.forget(id, &trace.cycle_id).await? {
                    evicted += 1;
                }
            }
            Ok::<(), OpenCompanyError>(())
        })
        .await;
        // Bound the archive on every eviction path — the partial-failure path
        // included. `KeepRecent` prunes to its own `n`; `OlderThan` has no `n`
        // to bound by, so it prunes to the retention limit — the same window
        // the live set is held to, which is what keeps the tier "the eviction
        // history nearest to the live window" and the archive read bounded for
        // any policy.
        let bound = match policy {
            EvictionPolicy::KeepRecent { n } => n,
            EvictionPolicy::OlderThan { .. } => TRACE_RETENTION_LIMIT,
        };
        if let Err(move_err) = move_result {
            // A provider failure mid-loop still leaves the traces already
            // moved sitting in the archive, and a maintenance pass that keeps
            // failing partway must not grow the tier past its bound across
            // retries. Prune best-effort, then report the failure that
            // actually happened.
            if let Err(prune_err) = self.prune_archive(id, bound).await {
                tracing::warn!(
                    error = %prune_err,
                    "archive prune failed after a partial eviction failure; the archive may exceed \
                     its retention bound"
                );
            }
            return Err(move_err);
        }
        self.prune_archive(id, bound).await?;
        Ok(evicted)
    }
}

#[cfg(test)]
#[path = "facades_tests.rs"]
mod tests;
