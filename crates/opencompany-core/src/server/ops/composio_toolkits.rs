//! The toolkit catalog behind Composio **open mode** (issue #397).
//!
//! An empty `[tools.composio].toolkits` means "defer to the backend's
//! server-enforced allowlist" — every toolkit the backend permits, which today
//! is roughly a hundred slugs. Until this module existed the console was handed
//! a hardcoded eight instead and the backend's own catalog
//! (`GET /agent-integrations/composio/toolkits`) was never consulted, so the
//! list drifted the moment the backend added an integration.
//!
//! ## What this module owns
//!
//! * the answer for open mode — the fetched catalog, or an honestly-marked
//!   fallback;
//! * the process-level cache that keeps a page-load path off the network; and
//! * [`FALLBACK_TOOLKITS`], the last-resort list, which is **never** presented
//!   as if it were the real catalog.
//!
//! ## Why a fallback still exists
//!
//! The fetch needs a credential and a reachable backend, and a build that
//! compiled the `composio` feature in. A company missing any of those still has
//! to see something it can click — a console that renders nothing is the exact
//! failure #397 was filed about. So the fallback stays, but
//! [`CatalogSource::Fallback`] and a plain-language notice ride along with it so
//! the console can tell the operator the list may be incomplete. A fallback
//! served as though it were the catalog would be worse than no fallback: it
//! looks authoritative and is silently eight items long.
//!
//! ## This is a console affordance, not a capability change
//!
//! Nothing here widens or narrows what an agent may reach. Agent-side admission
//! is [`toolkit_allowed`](crate::harness::composio), which is untouched: an
//! empty manifest allowlist still admits every toolkit, a non-empty one still
//! admits only its members. Equally, a **non-empty** manifest allowlist never
//! consults this module at all — a company that deliberately narrowed its belt
//! is offered exactly what it chose, and the catalog cannot widen it.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;

use serde::Serialize;

use crate::company::composio::CatalogEntry;

/// The last-resort provider list, used only when the real catalog cannot be
/// fetched — and always accompanied by [`CatalogSource::Fallback`] plus a
/// notice, so it is never mistaken for the backend's answer.
///
/// These are the toolkits a company reaches for first and are all in the
/// backend's default allowlist, which makes them a defensible thing to show
/// when the alternative is an empty page. They are **not** a ceiling: the
/// console's free-text slug field still authorizes anything the backend
/// permits, and agent-side admission never consulted this list.
pub(crate) const FALLBACK_TOOLKITS: &[&str] = &[
    "gmail",
    "googlecalendar",
    "googledrive",
    "github",
    "slack",
    "notion",
    "linear",
    "discord",
];

/// How long a successfully fetched catalog is served before it is re-fetched.
///
/// Fifteen minutes. The catalog changes when Composio adds an integration or
/// the backend widens its allowlist — churn measured in days, not seconds. The
/// cost of staleness is that a brand-new provider is missing from a picker for
/// at most a quarter of an hour; the cost of no cache is a backend round-trip
/// on *every* console status poll, on a page-load path, for a list that will be
/// byte-identical. Fifteen minutes puts the ceiling at four fetches an hour per
/// company however hard the console polls.
pub(crate) const CATALOG_TTL: Duration = Duration::from_secs(15 * 60);

/// How long a *failed* fetch is remembered before another is attempted.
///
/// One minute — deliberately much shorter than [`CATALOG_TTL`]. A failure is
/// cached at all because a backend that is down would otherwise be dialled once
/// per status poll, adding a whole `FETCH_TIMEOUT` to every console page-load for as
/// long as the outage lasts. It is cached only briefly because the operator
/// staring at a degraded notice wants it to clear as soon as the backend
/// recovers, and a minute is short enough that a refresh feels like it worked.
pub(crate) const FAILURE_TTL: Duration = Duration::from_secs(60);

/// How long the status route waits on the catalog before degrading.
///
/// The shared integration client's own timeout is 60s, which is a sane budget
/// for an agent tool and a catastrophe here: this call sits on `GET …/composio`,
/// which the console fetches on page load and after every mutation. Five
/// seconds is long enough for a healthy backend and short enough that a sick one
/// costs the operator a visible notice rather than a hung panel.
///
/// Gated with its only caller: a build without the `composio` feature has no
/// client to time out.
#[cfg(feature = "composio")]
pub(crate) const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the toolkit list the console renders actually came from.
///
/// The console needs this told to it rather than inferred — the whole shape of
/// #397 is that an inferred answer was wrong in the one case that mattered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CatalogSource {
    /// The company's own `[tools.composio].toolkits`, offered verbatim. The
    /// catalog is not consulted and cannot widen it.
    Manifest,
    /// The backend's live toolkit catalog. This is the answer open mode is
    /// supposed to give.
    Backend,
    /// [`FALLBACK_TOOLKITS`] — the catalog could not be fetched. Always paired
    /// with a notice saying so.
    Fallback,
}

/// The toolkits open mode offers, and how honest the answer is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpenModeToolkits {
    /// The providers to offer, with whatever display metadata the backend
    /// published for each (issue #600). A manifest or fallback list carries
    /// slugs only; a fetched catalog carries names, logos, descriptions and
    /// categories too.
    pub toolkits: Vec<CatalogEntry>,
    /// Where they came from.
    pub source: CatalogSource,
    /// Plain-language reason the list is a fallback, for the console to show the
    /// operator. `None` exactly when [`Self::source`] is not
    /// [`CatalogSource::Fallback`].
    pub notice: Option<String>,
}

impl OpenModeToolkits {
    /// The good case: the backend answered.
    pub(crate) fn from_backend(toolkits: Vec<CatalogEntry>) -> Self {
        Self {
            toolkits,
            source: CatalogSource::Backend,
            notice: None,
        }
    }

    /// The honest degradation: the built-in list, marked as such, with the
    /// reason attached.
    ///
    /// Slug-only entries, and unavoidably so: a fallback exists precisely
    /// because the metadata could not be fetched. The console renders these
    /// with its own typography, which is why that fallback survives #600 rather
    /// than being deleted in favour of the backend's names.
    pub(crate) fn degraded(reason: &str) -> Self {
        Self {
            toolkits: FALLBACK_TOOLKITS
                .iter()
                .map(|s| CatalogEntry::from_slug(*s))
                .collect(),
            source: CatalogSource::Fallback,
            notice: Some(format!(
                "Composio's provider catalog could not be fetched ({}), so this is a built-in \
                 starter list and may be incomplete. Any other provider the backend permits can \
                 still be connected by its slug.",
                bound_reason(reason)
            )),
        }
    }

    /// Turn a cached-or-fresh fetch outcome into the rendered answer.
    pub(crate) fn from_outcome(outcome: Result<Vec<CatalogEntry>, String>) -> Self {
        match outcome {
            Ok(toolkits) => Self::from_backend(toolkits),
            Err(reason) => Self::degraded(&reason),
        }
    }

    /// Just the slugs, in render order — the wire field the console has always
    /// had and every existing consumer still reads.
    ///
    /// Kept as a derived view rather than a second stored list so the two can
    /// never disagree about which providers are on offer.
    pub(crate) fn slugs(&self) -> Vec<String> {
        self.toolkits.iter().map(|t| t.slug.clone()).collect()
    }
}

/// Keep an upstream reason to one readable clause.
///
/// The upstream text has already had the tenant bearer stripped out of it by
/// the harness before it reaches here; this is a legibility bound, not a
/// security one. A backend that answers with a wall of HTML should not push a
/// wall of HTML into a status response.
fn bound_reason(reason: &str) -> String {
    const MAX: usize = 160;
    let reason = reason.trim().replace(['\n', '\r'], " ");
    if reason.is_empty() {
        return "no reason given".to_string();
    }
    match reason.char_indices().nth(MAX) {
        None => reason,
        Some((cut, _)) => format!("{}…", &reason[..cut]),
    }
}

/// One cached fetch outcome and when it was recorded.
struct CacheEntry {
    at: Instant,
    outcome: Result<Vec<CatalogEntry>, String>,
}

impl CacheEntry {
    /// Successes live for [`CATALOG_TTL`], failures for [`FAILURE_TTL`].
    fn ttl(&self) -> Duration {
        if self.outcome.is_ok() {
            CATALOG_TTL
        } else {
            FAILURE_TTL
        }
    }
}

/// What a catalog fetch produced: the entries, or a plain-language reason.
type FetchOutcome = Result<Vec<CatalogEntry>, String>;

/// A fetch running right now, and the key generation it began under.
struct InFlight {
    /// The key's generation when this fetch started. A fetch whose generation
    /// is no longer current was dialled with a credential the company has since
    /// replaced: it may still answer the callers already waiting on it, but it
    /// may not be cached and no new caller may join it.
    generation: u64,
    tx: broadcast::Sender<FetchOutcome>,
}

/// The fetches in flight, and how many times each key has been evicted.
///
/// Behind one lock because every decision here reads them together: whether a
/// caller may join a running fetch, and whether a finished one is still
/// entitled to speak for its key. The generation counter outlives the cache
/// entry it guards — it must, since its whole job is to describe fetches that
/// started before an eviction — and costs one integer per company.
#[derive(Default)]
struct Flights {
    running: HashMap<String, InFlight>,
    generations: HashMap<String, u64>,
}

/// A process-level, per-company cache of the fetched catalog.
///
/// Keyed per company rather than per backend URL even though the catalog is
/// mostly a property of the backend: a company's own BYO token can resolve to a
/// different Composio account with a different answer, and sharing one entry
/// across tenants would let one company's outage mark another company's panel
/// degraded. Entries are small (a hundred short strings) and bounded by the
/// number of companies an instance hosts.
#[derive(Default)]
pub(crate) struct CatalogCache {
    entries: Mutex<HashMap<String, CacheEntry>>,
    flights: Mutex<Flights>,
}

/// Releases a leader's slot however its fetch ends — a panic or a dropped
/// request future included, so neither strands later callers on a fetch that
/// will never answer.
///
/// Releases it only while it still holds THIS flight. Removing blindly by key
/// lets a slow fetch finishing after an eviction delete the successor that
/// replaced it, and the next caller then starts a third fetch instead of
/// joining the second.
struct FlightGuard<'a> {
    cache: &'a CatalogCache,
    key: &'a str,
    generation: u64,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut flights) = self.cache.flights.lock()
            && flights
                .running
                .get(self.key)
                .is_some_and(|running| running.generation == self.generation)
        {
            flights.running.remove(self.key);
        }
    }
}

impl CatalogCache {
    /// The catalog for `key`: the cached outcome, the answer of a fetch already
    /// running for the same key, or — for exactly one caller — `fetch`.
    ///
    /// The middle arm is what this adds. `GET …/composio` sits on a page-load
    /// path the console asks more than once per paint, and on a cold key every
    /// concurrent caller used to dial the backend for a list they were certain
    /// to agree on, each paying the full [`FETCH_TIMEOUT`] before the page could
    /// paint.
    ///
    /// Coalescing is an optimisation and never a correctness dependency: a
    /// poisoned map, a leader that was cancelled, or a caller that arrives just
    /// as one finishes all fall through to a fetch of their own.
    pub(crate) async fn get_or_fetch<F, Fut>(
        &self,
        key: &str,
        fetch: F,
    ) -> Result<Vec<CatalogEntry>, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<CatalogEntry>, String>>,
    {
        if let Some(cached) = self.lookup(key, Instant::now()) {
            return cached;
        }
        let joined = {
            let Ok(mut flights) = self.flights.lock() else {
                return fetch().await;
            };
            let generation = flights.generations.get(key).copied().unwrap_or(0);
            match flights.running.get(key) {
                Some(running) => Err(running.tx.subscribe()),
                None => {
                    let (tx, _) = broadcast::channel(1);
                    let running = InFlight {
                        generation,
                        tx: tx.clone(),
                    };
                    flights.running.insert(key.to_string(), running);
                    Ok((tx, generation))
                }
            }
        };
        let (tx, generation) = match joined {
            Ok(led) => led,
            Err(mut rx) => {
                return match rx.recv().await {
                    Ok(outcome) => outcome,
                    // The leader went away without answering, or answered just
                    // before this caller subscribed. Its result is in the cache
                    // in the second case; in the first there is nothing to wait
                    // for any more.
                    Err(_) => match self.lookup(key, Instant::now()) {
                        Some(outcome) => outcome,
                        None => fetch().await,
                    },
                };
            }
        };
        let guard = FlightGuard {
            cache: self,
            key,
            generation,
        };
        let outcome = fetch().await;
        // Cached only while the credential it was dialled with is still the
        // company's. An eviction during the fetch means this describes an
        // account the company no longer reaches, and storing it would reinstate
        // for a full TTL exactly what the eviction removed.
        if self.is_current(key, generation) {
            self.store(key, outcome.clone(), Instant::now());
        }
        drop(guard);
        let _ = tx.send(outcome.clone());
        outcome
    }

    /// Whether a fetch begun at `generation` still speaks for `key`.
    ///
    /// A poisoned map answers `false`: it cannot prove the answer is current,
    /// and serving a superseded catalog is the failure this guards.
    fn is_current(&self, key: &str, generation: u64) -> bool {
        self.flights
            .lock()
            .map(|flights| flights.generations.get(key).copied().unwrap_or(0) == generation)
            .unwrap_or(false)
    }

    /// The cached outcome for `key`, if one was recorded within its TTL of
    /// `now`. An expired entry reads as a miss and is left for the next
    /// [`Self::store`] to overwrite.
    pub(crate) fn lookup(
        &self,
        key: &str,
        now: Instant,
    ) -> Option<Result<Vec<CatalogEntry>, String>> {
        let entries = self.entries.lock().ok()?;
        let entry = entries.get(key)?;
        (now.saturating_duration_since(entry.at) < entry.ttl()).then(|| entry.outcome.clone())
    }

    /// Record an outcome as observed at `at`.
    pub(crate) fn store(&self, key: &str, outcome: Result<Vec<CatalogEntry>, String>, at: Instant) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(key.to_string(), CacheEntry { at, outcome });
        }
    }

    /// Drop `key`'s cached entry so the next read re-fetches, and retire every
    /// fetch already in flight for it.
    ///
    /// Called when the company's credential changes: a rotated BYO token can
    /// resolve to a different Composio account, and continuing to serve the old
    /// account's catalog for up to [`CATALOG_TTL`] would be exactly the kind of
    /// stale-by-construction answer this issue is about.
    ///
    /// Retiring the in-flight fetches is what keeps that true once callers
    /// share one. Dropping the cached entry alone leaves a fetch dialled with
    /// the replaced credential free to be joined by the very next caller — the
    /// status re-read the rotation itself performs is one — and free to store
    /// its answer afterwards, reinstating the evicted catalog for a full TTL.
    ///
    /// The callers already waiting on such a fetch still receive its answer.
    /// Their request predates the rotation, and without a shared flight each
    /// would have had exactly this fetch of its own running.
    pub(crate) fn evict(&self, key: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(key);
        }
        if let Ok(mut flights) = self.flights.lock() {
            *flights.generations.entry(key.to_string()).or_default() += 1;
            flights.running.remove(key);
        }
    }
}

/// The process-wide catalog cache.
pub(crate) fn cache() -> &'static CatalogCache {
    static CACHE: OnceLock<CatalogCache> = OnceLock::new();
    CACHE.get_or_init(CatalogCache::default)
}

/// The cache key for a company on a backend. Both halves matter: the same
/// company reconfigured onto a different backend must not read the old
/// backend's catalog.
pub(crate) fn cache_key(company: &crate::ports::types::CompanyId, backend_url: &str) -> String {
    format!("{company}|{backend_url}")
}

#[cfg(test)]
#[path = "composio_toolkits_tests.rs"]
mod tests;
