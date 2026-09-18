use super::*;

use super::test_support::*;

#[test]
fn one_url_read_in_two_shapes_is_two_cache_slots() {
    const ENDPOINT: &str = "https://shape-split.example/v1";
    assert_eq!(
        shaped_endpoint(ENDPOINT, CatalogShape::OpenAi),
        cache_key(ENDPOINT)
    );
    assert_ne!(
        shaped_endpoint(ENDPOINT, CatalogShape::OpenAi),
        shaped_endpoint(ENDPOINT, CatalogShape::PagedEnvelope)
    );
}

#[test]
fn catalog_parser_returns_every_non_empty_unique_model_in_provider_order() {
    let parsed = parse_models(RegistryResponse {
        data: vec![
            serde_json::json!({"id": " vendor/zeta ", "name": " Zeta ", "context_length": 128_000}),
            serde_json::json!({"id": "vendor/alpha"}),
            serde_json::json!({"id": "vendor/zeta", "name": "duplicate"}),
            serde_json::json!({"id": "   "}),
        ],
    });

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id, "vendor/zeta");
    assert_eq!(parsed[0].name.as_deref(), Some("Zeta"));
    assert_eq!(parsed[0].context_length, Some(128_000));
    assert_eq!(parsed[1], model("vendor/alpha"));
}

/// Regression for a Minor review finding on #1838: the previous
/// `BTreeMap`-keyed parser returned ids in lexicographic order, so
/// `src/server/setup.rs` taking `.next()` off the result silently swapped
/// from "the provider's first-listed model" (what the deleted
/// `discover_local_model` returned) to "the alphabetically first model" —
/// an arbitrary pick on any multi-model host whose ids don't already
/// sort first-to-preferred.
#[test]
fn catalog_parser_does_not_alphabetize_a_local_hosts_leading_model() {
    let parsed = parse_models(RegistryResponse {
        data: vec![
            serde_json::json!({"id": "zephyr-preferred"}),
            serde_json::json!({"id": "alpaca-not-preferred"}),
        ],
    });

    assert_eq!(
        parsed.first().map(|m| m.id.as_str()),
        Some("zephyr-preferred"),
        "the provider's leading model must survive `.next()` in setup.rs, not lose to sort order"
    );
}

/// Regression for a P2 review finding on #1838: a single malformed
/// record (here, a numeric `id`) used to fail `RegistryResponse`
/// deserialization outright — `discover_models` never reached
/// `parse_models` at all, so a valid model earlier or later in the same
/// `data` array was lost with it. `data` is now decoded as raw JSON
/// first, so only the bad entry drops out.
#[test]
fn catalog_parser_skips_malformed_entries_instead_of_rejecting_the_response() {
    let payload: RegistryResponse = serde_json::from_str(
        r#"{"data": [
            {"id": "vendor/good-one"},
            {"id": 12345},
            {"id": "vendor/good-three"}
        ]}"#,
    )
    .expect("RegistryResponse itself must still deserialize leniently");

    let parsed = parse_models(payload);

    let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["vendor/good-one", "vendor/good-three"]);
}

/// Regression for a P2 review finding on #1838's follow-up round: an
/// entry whose `id` is fine but whose *optional* `context_length` is not
/// used to lose the id right along with it. The old parser decoded each
/// `data` entry into `RegistryModel` in one shot, and serde fails that
/// whole decode on a type-mismatched field even when it is
/// `Option<u64>` — a string-valued `context_length` isn't "field
/// absent", it's "field present with the wrong type", and `Option`
/// deserialization does not paper over that. So this id used to vanish
/// from the catalog entirely instead of just losing its context length.
#[test]
fn catalog_parser_preserves_a_valid_id_with_malformed_optional_metadata() {
    let payload: RegistryResponse = serde_json::from_str(
        r#"{"data": [
            {"id": "vendor/good-one"},
            {"id": "vendor/good-two", "context_length": "not-a-number"},
            {"id": "vendor/good-three", "name": 42}
        ]}"#,
    )
    .expect("RegistryResponse itself must still deserialize leniently");

    let parsed = parse_models(payload);

    let ids: Vec<&str> = parsed.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["vendor/good-one", "vendor/good-two", "vendor/good-three"],
        "a bad optional field must not discard the id sitting next to it"
    );
    let good_two = parsed
        .iter()
        .find(|m| m.id == "vendor/good-two")
        .expect("vendor/good-two must survive");
    assert_eq!(good_two.context_length, None);
    let good_three = parsed
        .iter()
        .find(|m| m.id == "vendor/good-three")
        .expect("vendor/good-three must survive");
    assert_eq!(good_three.name, None);
}

#[test]
fn cache_serves_only_fresh_catalogs() {
    let cache = ModelCatalogCache::default();
    let stored_at = Instant::now();
    cache.store(vec![model("vendor/model")], stored_at);

    assert_eq!(
        cache.lookup(stored_at + MODEL_CATALOG_TTL - Duration::from_secs(1)),
        Some(vec![model("vendor/model")])
    );
    assert_eq!(cache.lookup(stored_at + MODEL_CATALOG_TTL), None);
}

/// A failure used to store nothing, so an unreachable provider cost a fresh
/// `MODEL_CATALOG_TIMEOUT` on every request that consulted it — once per
/// status read and, now that the turn path consults the vocabulary, once
/// per turn. Remembering it briefly turns that into one attempt a minute.
#[test]
fn a_failure_is_remembered_briefly_and_cleared_by_the_next_success() {
    let cache = ModelCatalogCache::default();
    let failed_at = Instant::now();
    cache.store_failure("provider.example did not answer".to_string(), failed_at);

    assert_eq!(
        cache.lookup_failure(failed_at + MODEL_CATALOG_FAILURE_TTL - Duration::from_secs(1)),
        Some("provider.example did not answer".to_string()),
    );
    assert_eq!(
        cache.lookup_failure(failed_at + MODEL_CATALOG_FAILURE_TTL),
        None,
        "the memo expires far sooner than a success, so a provider coming back up is \
         picked up promptly"
    );

    cache.store_failure("still down".to_string(), Instant::now());
    let recovered_at = Instant::now();
    cache.store(vec![model("vendor/model")], recovered_at);
    assert_eq!(
        cache.lookup_failure(recovered_at),
        None,
        "a success clears the memo — leaving it would keep reporting an outage that ended"
    );
}

/// An authenticated catalog is not shared across companies.
///
/// The positive cache is keyed on the endpoint, which is right for a public
/// catalog and wrong for one read with a company's own credential: an
/// endpoint may publish an entitlement-scoped list, and a base-URL-only key
/// would serve one company's answer to the next for the rest of the hour
/// (CodeRabbit security review on #2045). A keyless read stays shared,
/// because there is nothing company-specific in it to leak.
#[test]
fn an_authenticated_catalog_is_partitioned_per_company_and_a_keyless_one_is_not() {
    const ENDPOINT: &str = "https://shared-gateway.example/v1";
    // Company ids nothing else uses. The registry is process-global and
    // `evict_company_catalogs` clears a whole company, so a scope named
    // `acme` would be wiped by any route test in another module that saves a
    // key for the company of that name, mid-assertion and at random.
    const ONE: &str = "partition-one";
    const TWO: &str = "partition-two";
    let now = Instant::now();

    let acme = catalog_cache_scoped(ENDPOINT, Some(ONE));
    let other = catalog_cache_scoped(ENDPOINT, Some(TWO));
    acme.store(vec![model("acme/entitled-only")], now);

    assert_eq!(acme.lookup(now), Some(vec![model("acme/entitled-only")]));
    assert_eq!(
        other.lookup(now),
        None,
        "one company's authenticated catalog must not answer for another on the same endpoint"
    );
    assert_eq!(
        catalog_cache_scoped(ENDPOINT, None).lookup(now),
        None,
        "nor must it answer a keyless read of the same endpoint"
    );

    // The same company reaching the same endpoint does reuse its own entry,
    // so the partition costs one fetch per company rather than one per call.
    assert_eq!(
        catalog_cache_scoped(ENDPOINT, Some(ONE)).lookup(now),
        Some(vec![model("acme/entitled-only")])
    );

    // A keyless catalog is a public property of the endpoint, and stays
    // shared by everyone reading it that way.
    const PUBLIC: &str = "https://public-registry.example/v1";
    catalog_cache_scoped(PUBLIC, None).store(vec![model("vendor/public")], now);
    assert_eq!(
        catalog_cache_scoped(PUBLIC, None).lookup(now),
        Some(vec![model("vendor/public")])
    );
}

/// The partition goes one level finer than the company: per **harness**.
///
/// `resolve_effective_scoped` resolves config and credentials per
/// `HarnessScope`, which is what lets one `built_in` harness ride the
/// subscription while another runs on a key of its own. Two harnesses in one
/// company can therefore present different credentials to the same endpoint,
/// and a company-only key reused the first one's entitlement-scoped catalog
/// for the second without its credential ever being presented (Codex review
/// on #2045).
///
/// This asserts the property at the cache level, on the exact scope-string
/// shape a per-harness catalog read builds — company and harness joined by
/// the same control character `catalog_cache_scoped` uses, so a
/// three-field key cannot be spelled two ways.
#[test]
fn two_harnesses_in_one_company_do_not_share_an_authenticated_catalog() {
    const ENDPOINT: &str = "https://gateway.example/v1";
    // A company id nothing else uses — see the note in the test above.
    const COMPANY: &str = "harness-partition-co";
    let now = Instant::now();
    let subscription = format!("{COMPANY}\u{1}{}", "default");
    let own_key = format!("{COMPANY}\u{1}{}", "research");

    catalog_cache_scoped(ENDPOINT, Some(&subscription))
        .store(vec![model("gateway/subscription-tier")], now);

    assert_eq!(
        catalog_cache_scoped(ENDPOINT, Some(&own_key)).lookup(now),
        None,
        "a second harness's key may reach a different entitlement, so it must read for itself"
    );
    assert_eq!(
        catalog_cache_scoped(ENDPOINT, Some(&subscription)).lookup(now),
        Some(vec![model("gateway/subscription-tier")]),
        "the harness that did the read still reuses its own entry"
    );
    // The company-only key is a third, distinct slot — proof the harness
    // half genuinely participates rather than being absorbed into the id.
    assert_eq!(
        catalog_cache_scoped(ENDPOINT, Some(COMPANY)).lookup(now),
        None
    );
}

/// Rotating a credential drops that company's authenticated catalogs, and
/// nobody else's.
///
/// The cache key holds non-secret ids only, so a rotation is invisible to it
/// — the previous credential's catalog would otherwise answer for the rest
/// of [`MODEL_CATALOG_TTL`] and the new bearer would never reach `/models`
/// (Codex review on #2045). Eviction on the write is what keeps that
/// invariant affordable.
#[test]
fn rotating_a_credential_evicts_only_that_companys_authenticated_catalogs() {
    const ENDPOINT: &str = "https://rotating-gateway.example/v1";
    let now = Instant::now();
    let acme_console = "rot-acme".to_string();
    let acme_harness = format!("rot-acme\u{1}{}", "research");

    catalog_cache_scoped(ENDPOINT, Some(&acme_console)).store(vec![model("old/entitlement")], now);
    catalog_cache_scoped(ENDPOINT, Some(&acme_harness)).store(vec![model("old/entitlement")], now);
    catalog_cache_scoped(ENDPOINT, Some("rot-other")).store(vec![model("other/entitlement")], now);
    catalog_cache_scoped(ENDPOINT, None).store(vec![model("public/model")], now);

    evict_company_catalogs("rot-acme");

    assert_eq!(
        catalog_cache_scoped(ENDPOINT, Some(&acme_console)).lookup(now),
        None,
        "the console's own scoped read must be re-fetched with the new credential"
    );
    assert_eq!(
        catalog_cache_scoped(ENDPOINT, Some(&acme_harness)).lookup(now),
        None,
        "and so must every harness scope beneath that company"
    );
    assert_eq!(
        catalog_cache_scoped(ENDPOINT, Some("rot-other")).lookup(now),
        Some(vec![model("other/entitlement")]),
        "another company's credential did not change, so its catalog stands"
    );
    assert_eq!(
        catalog_cache_scoped(ENDPOINT, None).lookup(now),
        Some(vec![model("public/model")]),
        "a keyless catalog is a public property of the endpoint and no \
         credential change can alter it"
    );
}

/// A credential-specific rejection is reported and **not** remembered.
///
/// The negative memo is keyed on the endpoint, which is right for "this
/// endpoint did not answer" and wrong for "this key was rejected". On a
/// multi-company host the second would let one company's bad key answer for
/// the next company reaching the same endpoint with a valid one, and would
/// make a company that has just rotated a bad key wait the memo out before
/// its good key is ever presented (Codex review on #2045).
///
/// Asserted at the seam rather than over the network: the classification
/// lives in [`DiscoveryError`], and [`catalog_models`] is what must not
/// write a `Credential` failure into the endpoint's memo.
#[test]
fn a_credential_rejection_is_not_written_to_the_endpoints_failure_memo() {
    const ENDPOINT: &str = "https://rejects-one-key.example/v1";
    let cache = catalog_cache(ENDPOINT);
    let now = Instant::now();
    assert_eq!(cache.lookup_failure(now), None, "nothing remembered yet");

    // What an endpoint-level failure does: it is remembered, so an outage
    // costs one attempt a minute rather than one per request.
    cache.store_failure(format!("{ENDPOINT} did not answer within 10 seconds"), now);
    assert!(cache.lookup_failure(now).is_some());

    // And the classification that keeps a 401 out of that path.
    let rejection = DiscoveryError::credential(
        reqwest::StatusCode::UNAUTHORIZED,
        "401 Unauthorized".to_string(),
    );
    assert!(
        rejection.credential_status.is_some(),
        "a 401 is an answer about the key, not about the endpoint"
    );
    assert_eq!(rejection.credential_status, Some(401));
    assert!(
        DiscoveryError::endpoint("connection refused".to_string())
            .credential_status
            .is_none(),
        "a transport failure is an answer about the endpoint, and is memoized"
    );
}

/// Two endpoints are two caches. A single process-wide slot is what let one
/// company's catalog answer for another's endpoint in the first place.
#[test]
fn each_endpoint_gets_its_own_cache_and_trailing_slashes_do_not_split_one() {
    let a = catalog_cache("https://a.example/v1");
    let b = catalog_cache("https://b.example/v1");
    let now = Instant::now();
    a.store(vec![model("a-only")], now);

    assert_eq!(a.lookup(now), Some(vec![model("a-only")]));
    assert_eq!(b.lookup(now), None, "b must not inherit a's catalog");
    assert_eq!(
        catalog_cache("https://a.example/v1/").lookup(now),
        Some(vec![model("a-only")]),
        "a trailing slash is the same endpoint"
    );
}

/// Regression for a P2 review finding on #1838's follow-up round: without
/// `fetch_lock`, every concurrent caller that observed the same
/// empty/stale entry would independently "fetch" — a multi-tenant host
/// bursting several identical upstream calls at once. This exercises the
/// exact lock-then-recheck sequence [`catalog_models`] runs (acquire
/// `fetch_lock`, re-`lookup`, only then do the (here, simulated) fetch),
/// against the real `ModelCatalogCache`, so a regression that drops the
/// lock or the re-check fails this test rather than only showing up as
/// upstream rate-limit noise in production.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_misses_coalesce_into_a_single_fetch() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let cache = Arc::new(ModelCatalogCache::default());
    let fetch_count = Arc::new(AtomicUsize::new(0));

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let fetch_count = Arc::clone(&fetch_count);
            tokio::spawn(async move {
                let now = Instant::now();
                if let Some(models) = cache.lookup(now) {
                    return models;
                }
                let _fetch_guard = cache.fetch_lock.lock().await;
                let now = Instant::now();
                if let Some(models) = cache.lookup(now) {
                    return models;
                }
                fetch_count.fetch_add(1, Ordering::SeqCst);
                // Hold the lock across a slow "upstream" call so every
                // other task is still queued on `fetch_lock` — the same
                // shape a real registry round-trip has.
                tokio::time::sleep(Duration::from_millis(20)).await;
                let models = vec![model("vendor/single-flight")];
                cache.store(models.clone(), Instant::now());
                models
            })
        })
        .collect();

    for handle in handles {
        let models = handle.await.expect("fetch task must not panic");
        assert_eq!(models, vec![model("vendor/single-flight")]);
    }

    assert_eq!(
        fetch_count.load(Ordering::SeqCst),
        1,
        "only the first caller through fetch_lock should fetch; the rest must reuse its result"
    );
}

/// Regression for a P2 review finding on #1838's follow-up round, filed
/// against the single-flight fix directly above: `fetch_lock` serializes
/// misses, but a *failed* fetch stores nothing, so during a registry
/// outage every queued caller would previously run its own fresh
/// `bound`-length attempt after acquiring the lock — the Nth caller
/// through the queue waiting roughly `N * bound` before ever finding out,
/// which is exactly the docs/spec/runtime/providers.md "at most
/// `MODEL_CATALOG_TIMEOUT` seconds" promise this test defends. Mirrors
/// `catalog_models`'s real composition (`tokio::time::timeout` wrapped
/// around lock-acquire + recheck + fetch) against the real
/// `ModelCatalogCache`, with a simulated fetch standing in for
/// `discover_models` so the assertion is deterministic instead of racing
/// a real HTTP timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_caller_never_waits_longer_than_the_catalog_timeout() {
    use std::sync::Arc;

    let cache = Arc::new(ModelCatalogCache::default());
    // Short enough to keep the suite fast, long enough that three tasks
    // queuing on one real `tokio::sync::Mutex` stay well-ordered.
    let bound = Duration::from_millis(120);
    let fetch_delay = Duration::from_millis(80);

    async fn bounded_miss(cache: Arc<ModelCatalogCache>, bound: Duration, fetch_delay: Duration) {
        let _ = tokio::time::timeout(bound, async {
            let _fetch_guard = cache.fetch_lock.lock().await;
            let now = Instant::now();
            if cache.lookup(now).is_some() {
                return;
            }
            // Simulates a registry that is down: takes real time, then
            // fails without storing anything — so the next caller through
            // the lock faces the same empty cache this one did.
            tokio::time::sleep(fetch_delay).await;
        })
        .await;
    }

    let handles: Vec<_> = (0..3)
        .map(|_| {
            let cache = Arc::clone(&cache);
            tokio::spawn(async move {
                let started = Instant::now();
                bounded_miss(cache, bound, fetch_delay).await;
                started.elapsed()
            })
        })
        .collect();

    for handle in handles {
        let elapsed = handle.await.expect("task must not panic");
        assert!(
            elapsed <= bound + Duration::from_millis(40),
            "a caller waited {elapsed:?}, which exceeds its own {bound:?} budget by more \
             than scheduling slack — queue position must not multiply the wait"
        );
    }
}
