use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Slug-only catalog entries — what a manifest list, a fallback list, or a
/// backend predating the dynamic catalog yields.
fn slugs(list: &[&str]) -> Vec<CatalogEntry> {
    list.iter().map(|s| CatalogEntry::from_slug(*s)).collect()
}

/// A fetched catalog is served as the backend's answer, with nothing
/// apologising for it.
#[test]
fn a_fetched_catalog_is_reported_as_the_backend_answer() {
    let resolved = OpenModeToolkits::from_outcome(Ok(slugs(&["gmail", "hubspot", "zendesk"])));
    assert_eq!(resolved.source, CatalogSource::Backend);
    assert_eq!(resolved.toolkits, slugs(&["gmail", "hubspot", "zendesk"]));
    assert_eq!(resolved.notice, None, "a real catalog needs no caveat");
}

/// The honesty requirement: a failed fetch still yields a usable list, but
/// it is marked as a fallback AND says why, so the console can tell the
/// operator the list may be incomplete. A fallback that reported
/// `CatalogSource::Backend`, or carried no notice, would look exactly like
/// the real catalog — which is the failure mode this test exists to stop.
#[test]
fn a_failed_fetch_degrades_visibly_and_says_why() {
    let resolved = OpenModeToolkits::from_outcome(Err("backend unreachable".to_string()));
    assert_eq!(resolved.source, CatalogSource::Fallback);
    assert_eq!(resolved.toolkits, slugs(FALLBACK_TOOLKITS));
    let notice = resolved.notice.expect("a fallback must explain itself");
    assert!(
        notice.contains("backend unreachable"),
        "the operator is told the actual reason: {notice}"
    );
    assert!(
        notice.contains("may be incomplete"),
        "the operator is told the list is not authoritative: {notice}"
    );
}

/// An upstream error that arrives as a wall of text is bounded before it
/// reaches a status response.
#[test]
fn an_enormous_upstream_reason_is_bounded() {
    let resolved = OpenModeToolkits::from_outcome(Err("x".repeat(5_000)));
    let notice = resolved.notice.expect("fallback notice");
    assert!(
        notice.len() < 500,
        "unbounded reason: {} bytes",
        notice.len()
    );
    assert!(notice.contains('…'), "the cut is visible: {notice}");
}

/// Bounding is UTF-8 safe — a multi-byte reason must not panic on a byte
/// slice through a codepoint.
#[test]
fn bounding_a_multibyte_reason_does_not_panic() {
    let bounded = bound_reason(&"é".repeat(500));
    assert!(bounded.ends_with('…'));
}

/// A newline-laden reason collapses to one clause, and an empty one still
/// produces a sentence rather than an awkward gap.
#[test]
fn a_reason_is_normalised() {
    assert_eq!(bound_reason("  a\nb  "), "a b");
    assert_eq!(bound_reason("   "), "no reason given");
}

/// A stored success is served for `CATALOG_TTL` and not a moment longer.
#[test]
fn a_cached_catalog_expires_at_the_ttl() {
    let cache = CatalogCache::default();
    let at = Instant::now();
    cache.store("k", Ok(slugs(&["gmail"])), at);

    assert_eq!(
        cache.lookup("k", at + CATALOG_TTL - Duration::from_secs(1)),
        Some(Ok(slugs(&["gmail"]))),
        "inside the TTL the cache answers without a fetch"
    );
    assert_eq!(
        cache.lookup("k", at + CATALOG_TTL + Duration::from_secs(1)),
        None,
        "past the TTL the caller must re-fetch"
    );
}

/// A failure is remembered — so an outage does not cost a timeout per poll
/// — but for a fraction of the success TTL, so recovery is visible quickly.
#[test]
fn a_cached_failure_expires_far_sooner_than_a_success() {
    let cache = CatalogCache::default();
    let at = Instant::now();
    cache.store("k", Err("boom".to_string()), at);

    assert!(
        FAILURE_TTL < CATALOG_TTL,
        "a remembered failure must not outlive a remembered success"
    );
    assert_eq!(
        cache.lookup("k", at + FAILURE_TTL - Duration::from_secs(1)),
        Some(Err("boom".to_string())),
        "a repeat poll during an outage is served from cache, not the network"
    );
    assert_eq!(
        cache.lookup("k", at + FAILURE_TTL + Duration::from_secs(1)),
        None,
        "a recovered backend is re-probed within the minute"
    );
}

/// An eviction forces the next read to re-fetch — the credential-rotation
/// path.
#[test]
fn eviction_forces_a_refetch() {
    let cache = CatalogCache::default();
    let at = Instant::now();
    cache.store("k", Ok(slugs(&["gmail"])), at);
    cache.evict("k");
    assert_eq!(cache.lookup("k", at), None);
}

/// Two callers arriving on a cold key perform exactly ONE fetch, and both
/// get its answer.
///
/// This is the page-load case: the console asks `GET …/composio` more than
/// once per paint, and before coalescing each ask dialled the backend for a
/// list they were certain to agree on. `join!` polls the first future to its
/// first await point before starting the second, so the second is guaranteed
/// to arrive while the first is still fetching.
#[tokio::test]
async fn two_cold_callers_share_one_fetch() {
    let cache = CatalogCache::default();
    let fetches = AtomicUsize::new(0);
    let fetch = || async {
        fetches.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(slugs(&["gmail"]))
    };

    let (first, second) = tokio::join!(
        cache.get_or_fetch("k", fetch),
        cache.get_or_fetch("k", fetch)
    );

    assert_eq!(
        fetches.load(Ordering::SeqCst),
        1,
        "a second caller on a cold key must wait on the fetch in flight, not start another"
    );
    assert_eq!(first, Ok(slugs(&["gmail"])));
    assert_eq!(second, first, "both callers get the same answer");
    assert_eq!(
        cache.lookup("k", Instant::now()),
        Some(Ok(slugs(&["gmail"]))),
        "the shared fetch still fills the cache"
    );
}

/// A shared FAILURE is shared too — and cached under [`FAILURE_TTL`], so an
/// outage costs one fetch for every caller in the window rather than one
/// each.
#[tokio::test]
async fn two_cold_callers_share_one_failure() {
    let cache = CatalogCache::default();
    let fetches = AtomicUsize::new(0);
    let fetch = || async {
        fetches.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Err("the Composio backend did not answer".to_string())
    };

    let (first, second) = tokio::join!(
        cache.get_or_fetch("k", fetch),
        cache.get_or_fetch("k", fetch)
    );

    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    assert_eq!(
        first,
        Err("the Composio backend did not answer".to_string())
    );
    assert_eq!(second, first);
}

/// A cached key never reaches the fetch at all — coalescing is added in
/// front of the cache, not in place of it.
#[tokio::test]
async fn a_cached_key_is_served_without_a_fetch() {
    let cache = CatalogCache::default();
    cache.store("k", Ok(slugs(&["slack"])), Instant::now());
    let fetches = AtomicUsize::new(0);

    let served = cache
        .get_or_fetch("k", || async {
            fetches.fetch_add(1, Ordering::SeqCst);
            Ok(slugs(&["gmail"]))
        })
        .await;

    assert_eq!(served, Ok(slugs(&["slack"])));
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

/// Two companies fetching at once do not coalesce onto each other: the
/// in-flight map is keyed exactly like the cache, so one tenant can never be
/// served another's catalog.
#[tokio::test]
async fn different_keys_do_not_share_a_fetch() {
    let cache = CatalogCache::default();
    let fetches = AtomicUsize::new(0);
    let acme = || async {
        fetches.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(slugs(&["gmail"]))
    };
    let globex = || async {
        fetches.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(slugs(&["slack"]))
    };

    let (a, g) = tokio::join!(
        cache.get_or_fetch("acme", acme),
        cache.get_or_fetch("globex", globex)
    );

    assert_eq!(fetches.load(Ordering::SeqCst), 2);
    assert_eq!(a, Ok(slugs(&["gmail"])));
    assert_eq!(g, Ok(slugs(&["slack"])));
}

/// A key is released once its fetch is done, so the NEXT cold caller after
/// an eviction fetches rather than waiting on a flight that already ended.
#[tokio::test]
async fn a_finished_flight_is_released() {
    let cache = CatalogCache::default();
    let fetches = AtomicUsize::new(0);
    let fetch = || async {
        fetches.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        Ok(slugs(&["gmail"]))
    };

    assert_eq!(cache.get_or_fetch("k", fetch).await, Ok(slugs(&["gmail"])));
    cache.evict("k");
    assert_eq!(cache.get_or_fetch("k", fetch).await, Ok(slugs(&["gmail"])));

    assert_eq!(
        fetches.load(Ordering::SeqCst),
        2,
        "an evicted key must re-fetch, not join a flight that has finished"
    );
}

/// A rotation retires the fetch that was already running for the key.
///
/// `evict` fires the moment a company's credential changes, and the same
/// request then re-reads the status. Once callers share a flight, dropping
/// only the cached entry leaves two ways for the replaced account's catalog
/// to survive the rotation: that re-read joins the fetch dialled with the
/// old credential, and that fetch afterwards stores its answer on top of the
/// eviction, serving it for a full TTL. Both halves are asserted — what the
/// post-rotation read receives, and what is left in the cache behind it.
#[tokio::test(start_paused = true)]
async fn an_eviction_retires_the_fetch_already_in_flight() {
    let cache = CatalogCache::default();
    let fetches = AtomicUsize::new(0);

    // Dialled with the credential that is about to be replaced.
    let stale = || async {
        fetches.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(slugs(&["stale"]))
    };

    let rotation = async {
        // Let the fetch above register itself and reach its await point.
        tokio::task::yield_now().await;
        cache.evict("k");
        // The status re-read the rotation performs, under the new credential.
        cache
            .get_or_fetch("k", || async {
                fetches.fetch_add(1, Ordering::SeqCst);
                Ok(slugs(&["fresh"]))
            })
            .await
    };

    let (_retired, after) = tokio::join!(cache.get_or_fetch("k", stale), rotation);

    assert_eq!(
        after,
        Ok(slugs(&["fresh"])),
        "a read after a rotation must not be answered by a fetch dialled with the replaced credential"
    );
    assert_eq!(
        cache.lookup("k", Instant::now()),
        Some(Ok(slugs(&["fresh"]))),
        "a fetch that began before the eviction must not reinstate what it removed"
    );
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

/// A fetch finishing after an eviction does not release the fetch that
/// replaced it.
///
/// The slot is keyed by company alone, so releasing it blindly lets a
/// straggler remove its own successor — and the next caller, finding no
/// flight, dials a third time for a list somebody is already fetching. The
/// answer the latecomer receives pins the other half: it must be the
/// successor's, never the retired fetch's.
#[tokio::test(start_paused = true)]
async fn a_straggler_does_not_release_its_successor() {
    let cache = CatalogCache::default();
    let fetches = AtomicUsize::new(0);

    let straggler = || async {
        fetches.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(slugs(&["stale"]))
    };
    let successor = || async {
        fetches.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(slugs(&["fresh"]))
    };
    let third = || async {
        fetches.fetch_add(1, Ordering::SeqCst);
        Ok(slugs(&["third"]))
    };

    let rotate = async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cache.evict("k");
        cache.get_or_fetch("k", successor).await
    };
    // Arrives after the straggler has finished, while the successor still runs.
    let latecomer = async {
        tokio::time::sleep(Duration::from_millis(60)).await;
        cache.get_or_fetch("k", third).await
    };

    let (_retired, replaced, joined) =
        tokio::join!(cache.get_or_fetch("k", straggler), rotate, latecomer);

    assert_eq!(replaced, Ok(slugs(&["fresh"])));
    assert_eq!(
        joined,
        Ok(slugs(&["fresh"])),
        "a caller arriving after the rotation must get the successor's answer, not the retired fetch's"
    );
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        2,
        "the latecomer must join the fetch in flight rather than start a third"
    );
}

/// Companies do not share an entry: one tenant's outage cannot mark another
/// tenant's panel degraded, and a BYO token pointing at a different Composio
/// account cannot leak its catalog sideways.
#[test]
fn companies_do_not_share_a_cache_entry() {
    use crate::ports::types::CompanyId;
    let url = "https://api.example.test";
    assert_ne!(
        cache_key(&CompanyId::new("acme"), url),
        cache_key(&CompanyId::new("globex"), url)
    );
    assert_ne!(
        cache_key(&CompanyId::new("acme"), url),
        cache_key(&CompanyId::new("acme"), "https://other.example.test")
    );
}
