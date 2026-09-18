//! Where a company's connected search providers and their credentials live.
//!
//! # One credential slot per provider, and why that is the whole point
//!
//! Before this module there was one `search/api_key` for the company and a
//! separate `search/provider` field selecting which API it was presented to.
//! Switching provider without re-pasting the key left the old key
//! authenticating against the new provider, and every layer downstream — the
//! status route, the console badge, [`crate::harness::built_in::search_byo`] —
//! agreed the company was correctly configured until an agent's first search
//! came back 401. Keying the credential on the provider it authenticates makes
//! that unrepresentable.
//!
//! ```text
//!   SearchProvider record              SecretStore (per CompanyId)
//!   ┌────────────────────┐             ┌───────────────────────────────────────────┐
//!   │ slug               │             │ search/providers  index (+endpoint)       │
//!   │ enabled            │ ──names──▶  │ search/provider/<slug>/key                │
//!   │ endpoint           │             │ search/provider/<slug>/endpoint  fallback │
//!   │                    │             │ search/default          one slug          │
//!   │   NO KEY FIELD     │             │ ───────────────────────────────────────── │
//!   └────────────────────┘             │ search/provider   entry zero              │
//!                                      │ search/api_key    entry zero              │
//!                                      │ search/endpoint   entry zero              │
//!                                      └───────────────────────────────────────────┘
//! ```
//!
//! # Convergence, not migration
//!
//! The [`SecretStore`](crate::ports::SecretStore) port has **no rename and no
//! delete** — clearing is a write of the empty string that reads back as unset —
//! and a flag-day migration on a store with no transaction can leave a company
//! with neither configuration. So the flat keys stay exactly where they are and
//! *are* the first entry in the list:
//!
//! - [`list_providers`] synthesises a record from `search/provider` when that
//!   slug has no record of its own, with its credential still at the flat
//!   address.
//! - A write for **that** slug moves the credential to
//!   `search/provider/<slug>/key` and clears the flat one in the same operation.
//!   The clear must be *issued* rather than inferred, because "cleared" and
//!   "never set" are the same state here, and a key left behind at the old
//!   address after the new one is written is an orphaned secret.
//! - A write for any **other** slug leaves the flat keys alone. Clearing them
//!   would destroy the entry-zero provider's credential, which is the opposite
//!   of what convergence is for.
//!
//! Nothing existing moves, nothing is orphaned, and there is no migration to
//! half-complete. The cost is one special case in the reader, kept until nothing
//! reads the flat keys.

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::ports::SecretStore;
use crate::ports::types::{CompanyId, SecretValue};

use super::{API_KEY_SECRET, ENDPOINT_SECRET, PROVIDER_SECRET, provider_is_byo};

/// One lock per company, held across the provider index's read-modify-write.
///
/// # Why a lock and not a compare-and-swap
///
/// Every index mutation here is read-modify-write: list what is connected,
/// change one row, write the whole list back. Two of them interleaving lose one
/// of the two edits, and the loss is not cosmetic — two concurrent connects can
/// each store their credential and leave only one row in the index, orphaning a
/// secret at an address nothing reads; a remove racing a toggle can resurrect
/// the removed row. This is reachable: the console deliberately keeps every
/// other row live while one request is in flight, and the routes are a plain
/// HTTP API besides.
///
/// A compare-and-swap would be better and is not available. [`SecretStore`] is
/// `get` and `set` and nothing else — no CAS, no delete, no transaction — and
/// widening that port is a change to every backend behind it rather than a fix
/// to this surface.
///
/// # What this does and does not cover
///
/// It serialises the mutations **within one process**, which is the whole of a
/// deployment: the manager runs one container per tenant and a company's
/// requests all land in it. It is not a distributed lock and must not be read
/// as one — if this workload is ever replicated per tenant, the index needs the
/// port-level primitive rather than this.
///
/// The registry is keyed by company id and grows by one entry per company ever
/// touched, which is bounded by the tenancy.
static INDEX_LOCKS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
> = std::sync::LazyLock::new(std::sync::Mutex::default);

/// Takes this company's index lock, held until the returned guard is dropped.
///
/// The inner `std` mutex is held only long enough to clone an `Arc` — never
/// across an await — so a panicking writer cannot poison anything a later
/// request needs.
async fn index_guard(company: &CompanyId) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut locks = INDEX_LOCKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks
            .entry(company.as_ref().to_string())
            .or_default()
            .clone()
    };
    lock.lock_owned().await
}

/// Holds the JSON index of connected providers. Carries no credential.
pub const PROVIDER_INDEX_KEY: &str = "search/providers";

/// Holds the slug of the provider this company's agents search through.
///
/// A slot rather than a flag per record: two flags can both be true and then
/// need a tie-break rule, while a slot cannot, so there is no rule to write down
/// and no state to reconcile.
pub const DEFAULT_PROVIDER_KEY: &str = "search/default";

/// The credential slot for one provider.
pub fn provider_key_key(slug: &str) -> String {
    format!("search/provider/{slug}/key")
}

/// The per-slug instance-address slot. Read as a fallback when an index row
/// carries no `endpoint`, and still written alongside the row for one release
/// so a rolled-back binary finds the address (#2306). Not a secret.
///
/// DEPRECATED(keys-rework #2306): the pre-index per-slug endpoint fallback
/// address; replaced by [`IndexEntry`]'s `endpoint` field (the index row
/// carries the address directly now); removable when a later release stops
/// the one-release mirror write in [`put_provider_locked`]. Not
/// `#[deprecated]`: the read fallback and the mirror write are both live and
/// required until then, and `clippy -D warnings` would fail on that usage.
pub fn provider_endpoint_key(slug: &str) -> String {
    format!("search/provider/{slug}/endpoint")
}

/// One search provider this company has connected.
///
/// Derives no `Serialize`: there is no credential on this struct and there must
/// never be one. The wire shape is a separate DTO carrying `keyConfigured`.
///
/// The index blob it round-trips through is [`IndexEntry`], which is a private
/// storage detail rather than this type, so that adding a field to the record
/// cannot accidentally add one to something serialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchProvider {
    /// The catalogue slug. Identity and address at once.
    pub slug: String,
    /// Whether this provider is eligible to be the one agents search through.
    /// Distinct from "not connected": a disabled provider keeps its credential.
    pub enabled: bool,
    /// The instance URL. `Some` only for a self-hosted provider.
    pub endpoint: Option<String>,
}

/// The stored shape of one index row. Deliberately not [`SearchProvider`].
///
/// Never add `#[serde(deny_unknown_fields)]`: a binary one release older must
/// still parse a blob this one wrote (it ignores `endpoint`), and this one must
/// parse blobs written before `endpoint` existed (`serde(default)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    slug: String,
    #[serde(default = "yes")]
    enabled: bool,
    /// The instance URL, for a self-hosted provider. Not a secret. Absent from
    /// the blob (not `null`) when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    endpoint: Option<String>,
}

fn yes() -> bool {
    true
}

/// Trims, and treats blank as absent — the same rule [`read`] applies.
fn non_blank(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Reads a stored value, treating empty as absent — the port has no delete, so
/// a cleared value is the empty string.
async fn read(company: &CompanyId, secrets: &dyn SecretStore, key: &str) -> Result<Option<String>> {
    Ok(secrets
        .get(company, key)
        .await?
        .map(|value| value.expose().trim().to_string())
        .filter(|value| !value.is_empty()))
}

/// Writes a value, or clears it when `value` is empty.
async fn write(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    key: &str,
    value: &str,
) -> Result<()> {
    secrets
        .set(company, key, SecretValue(value.to_string()))
        .await
}

/// The slug the legacy flat keys describe, when it names a BYO provider.
///
/// `managed` and an unknown slug both answer `None`: neither is a connection,
/// and neither should be synthesised into a row.
async fn entry_zero_slug(company: &CompanyId, secrets: &dyn SecretStore) -> Result<Option<String>> {
    Ok(read(company, secrets, PROVIDER_SECRET)
        .await?
        .map(|slug| slug.to_ascii_lowercase())
        .filter(|slug| provider_is_byo(slug)))
}

/// Every provider this company has connected, entry zero first.
///
/// Entry zero is synthesised only when its slug has **no record of its own**, so
/// a company that converges by saving its legacy provider does not then see it
/// twice.
pub async fn list_providers(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<Vec<SearchProvider>> {
    let index: Vec<IndexEntry> = match read(company, secrets, PROVIDER_INDEX_KEY).await? {
        // A blob that will not parse is reported rather than silently treated as
        // an empty list: resolving to "no providers" would quietly move every
        // agent onto managed search and bill the platform for it.
        Some(raw) => serde_json::from_str(&raw).map_err(|err| {
            crate::error::OpenCompanyError::InvalidRequest(format!(
                "stored search provider index is not readable: {err}"
            ))
        })?,
        None => Vec::new(),
    };

    let mut providers = Vec::with_capacity(index.len() + 1);
    let entry_zero = entry_zero_slug(company, secrets).await?;

    if let Some(slug) = entry_zero.as_deref()
        && !index.iter().any(|entry| entry.slug == slug)
    {
        providers.push(SearchProvider {
            endpoint: read(company, secrets, ENDPOINT_SECRET).await?,
            slug: slug.to_string(),
            enabled: true,
        });
    }

    for entry in index {
        // 1. The index row itself — where every write since #2306 puts it.
        let mut endpoint = non_blank(entry.endpoint);
        // 2. The per-slug key — where it lived from 2026-09-11 until #2306, and
        //    where a rolled-back binary still writes it.
        if endpoint.is_none() {
            endpoint = read(company, secrets, &provider_endpoint_key(&entry.slug)).await?;
        }
        // 3. The flat entry-zero address, for the slug the flat keys describe.
        //
        // **The flat address survives the slug entering the index.**
        //
        // An upgraded company keeps its SearXNG URL in `search/endpoint` alone,
        // and the synthesized row above is where it is read from. But the row is
        // synthesized only while the slug is NOT indexed — and any index
        // mutation at all puts it there: toggling this row, or connecting a
        // second provider. From the next read on, the branch changes to this one
        // and looks at `search/provider/searxng/endpoint`, which nothing ever
        // wrote.
        //
        // The address was then gone. SearXNG went incomplete, every agent
        // silently fell back to managed search, and nothing on the page said
        // why — the exact class of failure convergence exists to avoid, from an
        // operator action as ordinary as flipping a switch.
        //
        // Read-side fallback rather than a copy at write time: the store has no
        // delete, so a copy would leave the same value at two addresses with
        // nothing to say which is current, and convergence is deliberately a
        // thing that happens on a real save rather than behind the operator's
        // back. Since #2306 the next index mutation also writes this address
        // into the row, which is safe because nothing writes `search/endpoint`
        // any more — it can only be cleared.
        if endpoint.is_none() && entry_zero.as_deref() == Some(entry.slug.as_str()) {
            endpoint = read(company, secrets, ENDPOINT_SECRET).await?;
        }
        providers.push(SearchProvider {
            slug: entry.slug,
            enabled: entry.enabled,
            endpoint,
        });
    }

    Ok(providers)
}

/// Writes the index, preserving the order it is given.
async fn save_index(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    providers: &[SearchProvider],
) -> Result<()> {
    let index: Vec<IndexEntry> = providers
        .iter()
        .map(|provider| IndexEntry {
            slug: provider.slug.clone(),
            enabled: provider.enabled,
            endpoint: non_blank(provider.endpoint.clone()),
        })
        .collect();
    let raw = serde_json::to_string(&index).map_err(|err| {
        crate::error::OpenCompanyError::InvalidRequest(format!(
            "search provider index could not be encoded: {err}"
        ))
    })?;
    write(company, secrets, PROVIDER_INDEX_KEY, &raw).await
}

/// Adds or replaces one provider record, converging entry zero if that is what
/// it is.
///
/// Does **not** touch credentials — [`store_provider_key`] owns those, so that a
/// record flush and a credential write are separately orderable. The connect
/// flow writes the credential first, because the probe resolves it by slug.
pub async fn put_provider(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    provider: SearchProvider,
) -> Result<()> {
    let _guard = index_guard(company).await;
    put_provider_locked(company, secrets, provider).await
}

/// Adds a provider **only if its slug is not already connected**, and says
/// which happened.
///
/// # Why the check cannot live in the caller
///
/// The connect flow read the index, decided the slug was free, and wrote it in
/// three separate awaits. Two admins connecting the same provider at once both
/// got past the read — and then the loser did real damage rather than merely
/// duplicating work: the connect flow rolls back on an `Auth` probe failure by
/// deleting the row **and** the credential, so a request whose key was rejected
/// deleted the row and the working key the other request had just stored, while
/// that request still answered `saved: true` from its own request-local copy.
///
/// Test-and-set under the one lock is the only version of this check that is
/// worth having, so the check moved in here rather than the lock moving out.
///
/// `false` means somebody else got there first and **nothing was written** —
/// which is what makes it safe to call before the credential is stored, so a
/// loser cannot overwrite the winner's key on its way to being refused.
pub async fn claim_provider(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    provider: SearchProvider,
) -> Result<bool> {
    let _guard = index_guard(company).await;
    if list_providers(company, secrets)
        .await?
        .iter()
        .any(|existing| existing.slug == provider.slug)
    {
        return Ok(false);
    }
    put_provider_locked(company, secrets, provider).await?;
    Ok(true)
}

/// Re-addresses a connected provider, or says it is not connected.
///
/// The read and the write are one critical section. Split, as they were in the
/// caller, a removal landing between them made the write **recreate** the row:
/// a provider the operator had just disconnected came back enabled, with its
/// old `enabled` flag and a fresh address, and started receiving agent searches
/// again after the removal had answered 200.
///
/// `false` means it was not connected — either it never was, or it stopped being
/// while this call was waiting for the lock. Those are the same answer from the
/// caller's side and it does not need to tell them apart.
pub async fn update_endpoint_if_present(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
    endpoint: Option<String>,
) -> Result<bool> {
    let _guard = index_guard(company).await;
    let Some(existing) = list_providers(company, secrets)
        .await?
        .into_iter()
        .find(|provider| provider.slug == slug)
    else {
        return Ok(false);
    };
    put_provider_locked(
        company,
        secrets,
        SearchProvider {
            slug: slug.to_string(),
            enabled: existing.enabled,
            endpoint,
        },
    )
    .await?;
    Ok(true)
}

/// Stores a credential **only for a provider that is connected**, atomically.
///
/// Same race as [`update_endpoint_if_present`], with a worse residue: a removal
/// landing between the caller's existence check and the write left a credential
/// at an address absent from the index, which the status route never reports
/// and `DELETE …/search/key` never clears, because both walk the index.
///
/// `false` means it is not connected, and nothing was written.
pub async fn store_key_if_connected(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
    key: &str,
) -> Result<bool> {
    let _guard = index_guard(company).await;
    if !list_providers(company, secrets)
        .await?
        .iter()
        .any(|provider| provider.slug == slug)
    {
        return Ok(false);
    }
    // Takes no lock of its own — it writes credential addresses, not the index
    // — so calling it while the guard is held is safe rather than re-entrant.
    store_provider_key(company, secrets, slug, key).await?;
    Ok(true)
}

/// What a guarded write against a possibly-default provider found, under one
/// hold of the index lock end to end.
///
/// KR review comment 4012261309: the HTTP layer used to read
/// [`load_default_slug`] and decide whether to refuse *before* calling into
/// one of this module's own locked mutations, which reopened exactly the
/// check-then-act window the index lock exists to close — a concurrent
/// `set_default_if_connected` naming this slug could land in the gap between
/// the read and the write, so an unconfirmed disable/removal/key-clear could
/// still strand the newly selected default. [`set_enabled_guarded`],
/// [`delete_provider_guarded`] and [`store_key_guarded`] read the marker,
/// apply the in-use rule, and write, all under one `index_guard` hold, so
/// nothing else touching this company's index can interleave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardedWrite {
    /// `slug` has no row — nothing was checked or changed.
    NotConnected,
    /// `slug` is the marked default and the caller did not confirm — nothing
    /// changed. Search's only `usedBy` shape is the default marker itself
    /// (`docs/key-reworks/in-use-guards.md` §1), so the caller needs nothing
    /// more than this to build the guard's `usedBy: { default: true }`.
    Blocked,
    /// The mutation ran.
    Applied,
}

/// [`set_enabled`], but the marked-default check and the write are one
/// critical section (see [`GuardedWrite`]). Enabling a row is never guarded —
/// turning a provider on cannot strand anything — so `confirm` is only
/// consulted when `enabled` is false.
pub async fn set_enabled_guarded(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
    enabled: bool,
    confirm: bool,
) -> Result<GuardedWrite> {
    let _guard = index_guard(company).await;
    if !enabled && load_default_slug(company, secrets).await?.as_deref() == Some(slug) && !confirm {
        return Ok(GuardedWrite::Blocked);
    }
    let mut providers = list_providers(company, secrets).await?;
    let Some(target) = providers.iter_mut().find(|p| p.slug == slug) else {
        return Ok(GuardedWrite::NotConnected);
    };
    target.enabled = enabled;
    save_index(company, secrets, &providers).await?;
    Ok(GuardedWrite::Applied)
}

/// [`delete_provider`], but the marked-default check and the removal are one
/// critical section (see [`GuardedWrite`]).
pub async fn delete_provider_guarded(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
    confirm: bool,
) -> Result<GuardedWrite> {
    let _guard = index_guard(company).await;
    if load_default_slug(company, secrets).await?.as_deref() == Some(slug) && !confirm {
        return Ok(GuardedWrite::Blocked);
    }
    if !list_providers(company, secrets)
        .await?
        .iter()
        .any(|provider| provider.slug == slug)
    {
        return Ok(GuardedWrite::NotConnected);
    }
    delete_provider_locked(company, secrets, slug).await?;
    Ok(GuardedWrite::Applied)
}

/// [`store_key_if_connected`], but the marked-default check and the write are
/// one critical section (see [`GuardedWrite`]). Only a **clear**
/// (`key.is_empty()`) is guarded — a rotate keeps serving whatever already
/// depended on it, the same rule the HTTP route's own guard already applied
/// before this fix, just not atomically with the write.
pub async fn store_key_guarded(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
    key: &str,
    confirm: bool,
) -> Result<GuardedWrite> {
    let _guard = index_guard(company).await;
    if key.is_empty()
        && load_default_slug(company, secrets).await?.as_deref() == Some(slug)
        && !confirm
    {
        return Ok(GuardedWrite::Blocked);
    }
    if !list_providers(company, secrets)
        .await?
        .iter()
        .any(|provider| provider.slug == slug)
    {
        return Ok(GuardedWrite::NotConnected);
    }
    // Takes no lock of its own — it writes credential addresses, not the index
    // — so calling it while the guard is held is safe rather than re-entrant.
    store_provider_key(company, secrets, slug, key).await?;
    Ok(GuardedWrite::Applied)
}

/// [`put_provider`]'s body, for a caller that already holds the index lock.
///
/// Separate because the lock is not re-entrant: [`claim_provider`] holds it
/// across a read and a write, and calling the public wrapper from inside that
/// would deadlock rather than recurse.
async fn put_provider_locked(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    provider: SearchProvider,
) -> Result<()> {
    let mut providers: Vec<SearchProvider> = list_providers(company, secrets).await?;
    let mut provider = provider;
    provider.endpoint = non_blank(provider.endpoint.take());

    if let Some(endpoint) = provider.endpoint.as_deref() {
        // **Also** the per-slug key, for one release (#2306). The index entry is
        // what this binary reads first; this copy is what a binary rolled back to
        // before #2306 reads, since it ignores `IndexEntry.endpoint`. Remove this
        // write only in a release after the one that ships the index field.
        //
        // DEPRECATED(keys-rework #2306): this write is the compatibility
        // mirror to the pre-index per-slug address ([`provider_endpoint_key`]);
        // replaced by the index row's own `endpoint` field, written via
        // `save_index` below; removable when a later release stops the
        // one-release mirror. Not `#[deprecated]`: this write is live and
        // required on every save until then, and `clippy -D warnings` would
        // fail on that usage.
        write(
            company,
            secrets,
            &provider_endpoint_key(&provider.slug),
            endpoint,
        )
        .await?;
    }

    // **In place, not moved to the end.** Order is not cosmetic here: with no
    // default marked, `resolve::active` picks the first usable row. Removing
    // the existing row and appending the new one meant that changing
    // SearXNG's address — an edit that should change nothing else — could
    // silently move every agent, and the bill, onto whichever provider had been
    // second. A genuinely new slug still goes at the end.
    match providers
        .iter_mut()
        .find(|existing| existing.slug == provider.slug)
    {
        Some(existing) => {
            existing.enabled = provider.enabled;
            // **Merge, never replace.** An omitted address keeps the stored one:
            // a toggle, a legacy select with no address, or a key replace sends
            // `None`, and replacing the row would wipe a SearXNG URL — agents then
            // fall back to managed search and the bill moves to the platform.
            // Nothing clears an address through here; removal does that.
            if provider.endpoint.is_some() {
                existing.endpoint = provider.endpoint;
            }
        }
        None => providers.push(provider),
    }
    save_index(company, secrets, &providers).await
}

/// Turns one provider on or off.
///
/// Keys rework (#2306), decision D-never-clear-default (X14, 2026-09-15,
/// `docs/key-reworks/README.md`, extended to search by
/// `docs/key-reworks/in-use-guards.md` §4): disabling the marked provider used
/// to **clear the marker** here, on the theory that moving to the first
/// enabled provider was "the same answer" with less claim to intent than
/// leaving a marker pointing at something switched off. Silently retargeting
/// the default is itself an undocumented second decision the operator did not
/// make when they only asked to disable a row — so the marker is now left
/// exactly as it was. [`crate::company::search::resolve::active`] already
/// falls through a marked-but-disabled slug to the first usable candidate
/// (and has since before this change — see its own doc comment), so nothing
/// about *resolution* changes here; only the **stored** marker stops being
/// rewritten out from under the operator. The HTTP layer
/// (`server/ops/search.rs`) is what now asks for confirmation before a
/// disable that would leave the default pointing at something switched off.
pub async fn set_enabled(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
    enabled: bool,
) -> Result<()> {
    let _guard = index_guard(company).await;
    let mut providers = list_providers(company, secrets).await?;
    let Some(target) = providers.iter_mut().find(|p| p.slug == slug) else {
        return Ok(());
    };
    target.enabled = enabled;
    save_index(company, secrets, &providers).await
}

/// Removes a provider, clearing its credential in the same operation.
///
/// The borrowed design leaves the credential behind and silently reuses it when
/// the provider is re-added. Here the clear is issued, and a failed clear is
/// logged loudly: an orphaned secret on disk is the shape of an incident rather
/// than a tidiness problem.
pub async fn delete_provider(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
) -> Result<()> {
    let _guard = index_guard(company).await;
    delete_provider_locked(company, secrets, slug).await
}

/// Removes every connected provider under **one** hold of the index lock.
///
/// Disconnect all used to take a snapshot of the list and then remove each
/// entry with its own lock. A connect landing after the snapshot was never
/// visited: its row and credential survived a bulk removal that reported
/// success, and the console announced "Disconnected" over a provider that was
/// still answering. Holding the lock from the snapshot to the last removal
/// makes the snapshot the truth — and a connect that claimed before it and is
/// still writing its key is refused by [`store_key_if_connected`] when it gets
/// there.
pub async fn delete_all_providers(company: &CompanyId, secrets: &dyn SecretStore) -> Result<()> {
    let _guard = index_guard(company).await;
    for provider in list_providers(company, secrets).await? {
        delete_provider_locked(company, secrets, &provider.slug).await?;
    }
    Ok(())
}

/// [`delete_provider`]'s body, for a caller that already holds the index lock.
///
/// # The order is clear-then-unlist, and it is the whole point
///
/// It used to unlist the row first and clear the credential second. A clear
/// that failed after the index write — a transient store error is enough —
/// returned an error with the row already gone and the secret still stored: an
/// invisible credential the status route never reports and Disconnect all never
/// visits, which is the orphaned-secret state this module exists to prevent.
///
/// Clearing first inverts which half can be left behind. If a clear fails, the
/// row is still listed, visibly incomplete, and removing it again finishes the
/// job. If the index write fails after the clears, the same is true. Nothing
/// can end up stored and unlisted.
///
/// Both answers that depend on the flat keys — the list and whether this slug
/// is entry zero — are read before anything is cleared, because clearing
/// `search/provider` changes them.
async fn delete_provider_locked(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
) -> Result<()> {
    let is_entry_zero = entry_zero_slug(company, secrets).await?.as_deref() == Some(slug);
    let remaining: Vec<SearchProvider> = list_providers(company, secrets)
        .await?
        .into_iter()
        .filter(|provider| provider.slug != slug)
        .collect();

    for key in [provider_key_key(slug), provider_endpoint_key(slug)] {
        if let Err(err) = write(company, secrets, &key, "").await {
            tracing::error!(
                company = %company,
                key = %key,
                "[search] removing a provider could not clear its credential; the row is kept \
                 so the removal can be retried: {err}"
            );
            return Err(err);
        }
    }

    // Entry zero lives at the flat keys, so removing it has to clear those too —
    // which is exactly what `DELETE …/search/key` has always done.
    if is_entry_zero {
        // `PROVIDER_SECRET` LAST. It is what makes this slug entry zero, and
        // the retry depends on still recognising it: clear it before a flat
        // value whose clear then fails, and the retry sees an ordinary row,
        // skips the flat keys, and leaves that value stored for good.
        for key in [API_KEY_SECRET, ENDPOINT_SECRET, PROVIDER_SECRET] {
            if let Err(err) = write(company, secrets, key, "").await {
                tracing::error!(
                    company = %company,
                    key = %key,
                    "[search] removing the legacy provider could not clear its flat credential; \
                     the row is kept so the removal can be retried: {err}"
                );
                return Err(err);
            }
        }
    }

    // Only once nothing of it is left stored.
    save_index(company, secrets, &remaining).await?;

    // Keys rework (#2306), decision D-never-clear-default (X14, 2026-09-15,
    // `docs/key-reworks/README.md`, extended to search by
    // `docs/key-reworks/in-use-guards.md` §4): `search/default` is left
    // exactly as it was marked, even though it may now name a slug with no
    // row at all. There is no carve-out for delete versus disable — a marker
    // naming a deleted slug is exactly the state X14 asks for, on the same
    // footing as one naming a disabled slug (see the identical note in
    // `set_enabled` above). An operator who confirmed this removal made one
    // decision — delete the provider — and this function silently also
    // retargeting the default used to be a second, undocumented one.
    // `resolve::active` already falls through a marker naming nothing back to
    // the first usable candidate, so nothing about resolution depends on the
    // marker being cleared; the HTTP layer's in-use guard is what now asks
    // for confirmation before a removal that would do this at all.
    Ok(())
}

/// Stores one provider's credential at its own address, converging entry zero.
pub async fn store_provider_key(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
    key: &str,
) -> Result<()> {
    write(company, secrets, &provider_key_key(slug), key).await?;

    // Only for the slug the flat keys describe. Clearing `search/api_key` while
    // writing some *other* provider's key would destroy the entry-zero
    // provider's credential.
    if entry_zero_slug(company, secrets).await?.as_deref() == Some(slug)
        && let Err(err) = write(company, secrets, API_KEY_SECRET, "").await
    {
        tracing::error!(
            company = %company,
            "[search] the credential moved to its per-provider address but the legacy \
             `search/api_key` could not be cleared; a secret is now orphaned there: {err}"
        );
        return Err(err);
    }
    Ok(())
}

/// One provider's credential, trying its own address and temporarily falling
/// back to the deprecated flat one for entry zero.
///
/// The fallback cannot be removed until stored instances have been migrated:
/// a company that connected Search before the provider list and never revisited
/// the page still has its only credential there. `store_provider_key` converges
/// it on the next write in the meantime.
pub async fn load_provider_key(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
) -> Result<Option<String>> {
    if let Some(key) = read(company, secrets, &provider_key_key(slug)).await? {
        return Ok(Some(key));
    }
    if entry_zero_slug(company, secrets).await?.as_deref() == Some(slug) {
        return read(company, secrets, API_KEY_SECRET).await;
    }
    Ok(None)
}

/// Whether a credential is stored for `slug`. Never the credential.
///
/// Derived by asking the store, never by storing a flag, which can go stale
/// against a cleared secret.
pub async fn provider_key_configured(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
) -> Result<bool> {
    Ok(load_provider_key(company, secrets, slug).await?.is_some())
}

/// The slug the operator marked, if any.
pub async fn load_default_slug(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<Option<String>> {
    read(company, secrets, DEFAULT_PROVIDER_KEY).await
}

/// The legacy "select this provider" save, as one critical section.
///
/// `PUT …/search` naming a provider used to do this in four unlocked steps: read
/// the row, write it back with the address it had read, store the key, mark it
/// default. Two things slipped through the gaps:
///
/// - A Change address landing after the read had its new address overwritten
///   with the one the read captured, so both requests answered 200 and agents
///   kept searching the old instance.
/// - A removal landing before the marker write left a deleted slug marked as
///   the default — the dangling-marker state `set_default_if_connected` exists
///   to prevent on the modern route.
///
/// Held under the index lock end to end, neither can interleave. An omitted
/// `endpoint` is left exactly as stored rather than rewritten from a snapshot:
/// [`put_provider_locked`] merges: a `None` address keeps the stored one.
///
/// Creating the row when it does not exist is deliberate — naming a provider
/// on this route has always connected it — and the credential is written after
/// the row, inside the same lock, so it can never land without one.
pub async fn select_provider(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
    endpoint: Option<String>,
    key: Option<&str>,
) -> Result<()> {
    let _guard = index_guard(company).await;
    put_provider_locked(
        company,
        secrets,
        SearchProvider {
            slug: slug.to_string(),
            // Selecting a provider means using it; a selected row that is
            // switched off resolves to something else.
            enabled: true,
            endpoint,
        },
    )
    .await?;
    if let Some(key) = key {
        store_provider_key(company, secrets, slug, key).await?;
    }
    set_default_slug(company, secrets, slug).await
}

/// Marks a provider as the default **only if it is connected**, atomically.
///
/// The route checked the row existed and then wrote the marker, two separate
/// steps. A removal landing between them saw no marker to clear, deleted the
/// row, and the write then left the deleted slug in `search/default`: the
/// request answered 200 without selecting anything, and reconnecting that slug
/// later made it active without anyone choosing it. Removal clears the marker
/// under this same lock, so check-and-write here and check-and-clear there
/// cannot interleave.
///
/// `false` means it is not connected, and the marker was not touched.
pub async fn set_default_if_connected(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
) -> Result<bool> {
    let _guard = index_guard(company).await;
    if !list_providers(company, secrets)
        .await?
        .iter()
        .any(|provider| provider.slug == slug)
    {
        return Ok(false);
    }
    set_default_slug(company, secrets, slug).await?;
    Ok(true)
}

/// Marks one provider as the one agents search through.
pub async fn set_default_slug(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    slug: &str,
) -> Result<()> {
    write(company, secrets, DEFAULT_PROVIDER_KEY, slug).await
}

/// Unmarks whatever was marked. Resolution falls back to the first enabled.
pub async fn clear_default_slug(company: &CompanyId, secrets: &dyn SecretStore) -> Result<()> {
    write(company, secrets, DEFAULT_PROVIDER_KEY, "").await
}

#[cfg(test)]
#[path = "store_address_tests.rs"]
mod tests_address;
#[cfg(test)]
#[path = "store_concurrency_tests.rs"]
mod tests_concurrency;
#[cfg(test)]
#[path = "store_config_tests.rs"]
mod tests_config;
#[cfg(test)]
#[path = "store_failure_tests.rs"]
mod tests_failure;
