use super::CORTEXDB_DRIVER_ID;
use super::tests::{ACTOR, TOKEN, client, spawn_mock};
use tinymemory_api::traits::Memory;
use tinymemory_api::types::{MemoryCategory, RecallOpts};

#[tokio::test]
async fn health_probe_reports_ready() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);
    assert!(memory.health_check().await);
}

#[tokio::test]
async fn health_probe_reports_down_on_actor_mismatch() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, "wrong-actor");
    assert!(!memory.health_check().await);
    match memory.health_probe().await {
        Some(tinymemory_api::health::MemoryHealth::Down { .. }) => {}
        other => panic!("expected Down, got {other:?}"),
    }
}

/// Regression for the `namespace_summaries` finding: it used to always
/// return an empty set, which made the portability/export path (`opencompany
/// memory migrate`) believe a populated CortexDB store held nothing.
#[tokio::test]
async fn namespace_summaries_enumerates_every_namespace_this_driver_wrote() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    memory
        .store("company-a", "one", "a1", MemoryCategory::Core, None)
        .await
        .expect("store succeeds");
    memory
        .store("company-a", "two", "a2", MemoryCategory::Core, None)
        .await
        .expect("store succeeds");
    memory
        .store("company-b", "one", "b1", MemoryCategory::Core, None)
        .await
        .expect("store succeeds");

    let mut summaries = memory
        .namespace_summaries()
        .await
        .expect("namespace_summaries succeeds");
    summaries.sort_by(|a, b| a.namespace.cmp(&b.namespace));

    assert_eq!(
        summaries.len(),
        2,
        "expected exactly the two populated namespaces: {summaries:?}"
    );
    assert_eq!(summaries[0].namespace, "company-a");
    assert_eq!(summaries[0].count, 2);
    assert_eq!(summaries[1].namespace, "company-b");
    assert_eq!(summaries[1].count, 1);
}

/// Regression: `count()` used to always return `0`, so every populated
/// CortexDB instance reported an empty engine to any `Memory` consumer that
/// checks it. It should report the same per-namespace key counts
/// `namespace_summaries` does, summed across every namespace this driver has
/// written to.
#[tokio::test]
async fn count_reports_live_keys_across_every_namespace() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    memory
        .store("company-a", "one", "a1", MemoryCategory::Core, None)
        .await
        .expect("store succeeds");
    memory
        .store("company-a", "two", "a2", MemoryCategory::Core, None)
        .await
        .expect("store succeeds");
    memory
        .store("company-b", "one", "b1", MemoryCategory::Core, None)
        .await
        .expect("store succeeds");
    // A second write under the same key is a replay, not a second live key.
    memory
        .store("company-a", "one", "a1", MemoryCategory::Core, None)
        .await
        .expect("store succeeds");

    assert_eq!(
        memory.count().await.expect("count succeeds"),
        3,
        "three live keys across the two namespaces, not the raw event count and not zero"
    );
}

/// Regression for the `forget` finding: retracting only the newest event for
/// a key left an older event behind, and `get` immediately started returning
/// it again as if `forget` had never run.
#[tokio::test]
async fn forget_removes_every_version_of_a_key_not_only_the_latest() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    memory
        .store("company-a", "note", "first", MemoryCategory::Core, None)
        .await
        .expect("first store succeeds");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    memory
        .store("company-a", "note", "second", MemoryCategory::Core, None)
        .await
        .expect("second store succeeds");

    let removed = memory
        .forget("company-a", "note")
        .await
        .expect("forget succeeds");
    assert!(removed, "forget should report the key was removed");

    assert!(
        memory
            .get("company-a", "note")
            .await
            .expect("get succeeds")
            .is_none(),
        "an older version of the key must not resurface after forget"
    );
    assert!(
        memory
            .list(Some("company-a"), None, None)
            .await
            .expect("list succeeds")
            .is_empty(),
        "list must not resurrect an older version of the forgotten key either"
    );
}

/// Regression for the "recall now discards `/v1/recall`'s ranking" finding:
/// canonicalizing each hit to its key's current content re-derives the list
/// from a `HashMap` fold with no defined order, and the surrounding code only
/// sorted by `observed_at` (to pick which duplicate of *one* key to keep) —
/// nothing re-sorted the *distinct-key* results by relevance before
/// `truncate(limit)`. A newer, weaker match could therefore displace an
/// older, more relevant one.
#[tokio::test]
async fn recall_keeps_the_highest_scored_hit_under_a_tight_limit_not_the_newest() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    // Stored first (older `observed_at`), and marked so the mock scores it
    // as the most relevant hit.
    memory
        .store(
            "company-a",
            "high-relevance",
            "widget alpha HIGH_RELEVANCE_MARKER",
            MemoryCategory::Core,
            None,
        )
        .await
        .expect("store succeeds");
    // `now_rfc3339` (`crate::ports::iso8601`) has one-second
    // resolution, so the two writes need to straddle a real second boundary
    // for their `observed_at` to differ — otherwise the initial recency sort
    // is a no-op tie and this test would pass on insertion order alone,
    // proving nothing about the score-sort fix.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    // Stored second (newer `observed_at`), with no marker — the mock scores
    // it lower, but a recency-only sort would rank it first.
    memory
        .store(
            "company-a",
            "low-relevance",
            "widget beta",
            MemoryCategory::Core,
            None,
        )
        .await
        .expect("store succeeds");

    let hits = memory
        .recall(
            "widget",
            1,
            RecallOpts {
                namespace: Some("company-a"),
                ..Default::default()
            },
        )
        .await
        .expect("recall succeeds");

    assert_eq!(
        hits.len(),
        1,
        "limit=1 must return exactly one hit: {hits:?}"
    );
    assert_eq!(
        hits[0].key, "high-relevance",
        "truncating by score must keep the more relevant (older) hit, not the more recent \
         (less relevant) one: {hits:?}"
    );
}

/// Regression for the `recall` finding: deduplicating only within one
/// query's own hits does not stop a superseded event from surfacing when its
/// (now-stale) content still matches the query but the current content does
/// not.
///
/// A stricter follow-up ("drop the hit instead of swapping in current
/// content, since a query for 'cat' returning 'dog' is a bait-and-switch")
/// was tried and reverted: `ProviderContextStore` (`src/store/memory/
/// facades.rs`) reuses one key — a content-address of a chunk's *body* —
/// across writes that only add a label to an otherwise-unchanged body, which
/// is a legitimate, common rewrite, not a superseding one. But the
/// serialized envelope's `labels` field changing is enough to make the raw
/// `content` string differ, so "drop on any content difference" also drops
/// every later-labeled read of an otherwise-unchanged fact — verified via
/// `tests/hivemind_e2e.rs::a_desk_reasons_with_memory_held_in_a_remote_engine`,
/// which regressed under the drop behavior: a small-case table stored by one
/// teammate and re-labeled by two more became unrecallable. Swapping in
/// current content is the safer failure mode of the two: it can occasionally
/// surface an unrelated current value for a query that only matched stale
/// content, but it never makes a real, current fact unrecallable.
#[tokio::test]
async fn recall_resolves_hits_to_the_key_s_current_content_not_a_superseded_match() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    memory
        .store("company-a", "pet", "cat", MemoryCategory::Core, None)
        .await
        .expect("first store succeeds");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    memory
        .store("company-a", "pet", "dog", MemoryCategory::Core, None)
        .await
        .expect("second store succeeds");

    // The mock's `/v1/recall` only matches events whose stored content
    // contains the query text, exactly like the reviewer-described failure
    // mode: "cat" only matches the superseded first event, since the current
    // event's content is "dog".
    let hits = memory
        .recall(
            "cat",
            10,
            RecallOpts {
                namespace: Some("company-a"),
                ..Default::default()
            },
        )
        .await
        .expect("recall succeeds");

    assert_eq!(
        hits.len(),
        1,
        "expected the key's one current hit: {hits:?}"
    );
    assert_eq!(
        hits[0].content, "dog",
        "recall must resolve a stale hit to the key's current content, not the superseded \
         version the query happened to match"
    );

    // get() must agree with recall(): the key is currently "dog".
    let fetched = memory
        .get("company-a", "pet")
        .await
        .expect("get succeeds")
        .expect("entry exists");
    assert_eq!(fetched.content, "dog");
}

/// Regression for the e2e failure the "drop" behavior above caused: a key
/// whose stored content only gained a label (the value/body is byte-for-byte
/// unchanged) must remain recallable by whatever originally matched it.
/// `ProviderContextStore` produces exactly this shape — the same
/// content-address key, a grown `labels` array, everything else identical —
/// whenever a second caller references an existing chunk under a new label.
#[tokio::test]
async fn recall_still_finds_a_fact_after_a_label_only_rewrite_under_the_same_key() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    let original = r#"{"v":1,"record":{"label":"agent-memory/theorist/small-case-table","body":"small-case table\n\nThe lab's small-case table for this recurrence is n=1 -> 1, n=2 -> 3, n=3 -> 7.","stored_at_millis":1,"labels":["agent-memory/theorist/small-case-table"]}}"#;
    let relabeled = r#"{"v":1,"record":{"label":"agent-memory/theorist/small-case-table","body":"small-case table\n\nThe lab's small-case table for this recurrence is n=1 -> 1, n=2 -> 3, n=3 -> 7.","stored_at_millis":1,"labels":["agent-memory/theorist/small-case-table","agent-memory/programmer/small-case-table"]}}"#;

    memory
        .store(
            "company-a",
            "small-case-table",
            original,
            MemoryCategory::Core,
            None,
        )
        .await
        .expect("first store succeeds");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    // Same key, same underlying body, one more label — exactly the rewrite
    // `ProviderContextStore::put` performs, not a genuine value change.
    memory
        .store(
            "company-a",
            "small-case-table",
            relabeled,
            MemoryCategory::Core,
            None,
        )
        .await
        .expect("second store succeeds");

    let hits = memory
        .recall(
            "n=3 -> 7",
            10,
            RecallOpts {
                namespace: Some("company-a"),
                ..Default::default()
            },
        )
        .await
        .expect("recall succeeds");

    assert_eq!(
        hits.len(),
        1,
        "a label-only rewrite of an otherwise-unchanged fact must not make it unrecallable: \
         {hits:?}"
    );
    assert!(
        hits[0].content.contains("n=3 -> 7"),
        "the fact itself must still be present: {:?}",
        hits[0].content
    );
}

/// Regression for the `list` finding: a single-page `/v1/recall`-backed
/// listing silently truncated at [`super::EVENTS_LAYER_LIMIT`] (500) and
/// reported that page as the whole namespace. `list` now walks the paginated
/// `GET /v1/events` instead, which must not lose anything past one page.
#[tokio::test]
async fn list_does_not_truncate_a_namespace_larger_than_one_recall_page() {
    let (base_url, _state) = spawn_mock(ACTOR).await;
    let memory = client(&base_url, ACTOR);

    // Comfortably past both the old 500-event `/v1/recall` cap and the
    // 200-event `/v1/events` page size this driver now pages through.
    const KEY_COUNT: usize = 520;
    for i in 0..KEY_COUNT {
        memory
            .store(
                "company-a",
                &format!("key-{i}"),
                "v",
                MemoryCategory::Core,
                None,
            )
            .await
            .expect("store succeeds");
    }

    let entries = memory
        .list(Some("company-a"), None, None)
        .await
        .expect("list succeeds");
    assert_eq!(
        entries.len(),
        KEY_COUNT,
        "list must report every key in a namespace larger than one page"
    );
}

/// The bind-time capability audit — the same one `open_driver` runs in
/// production — passes for this driver: `MemoryTraitProvider` derives its
/// advertised capabilities from its accessors, and this driver implements the
/// mandatory `Memory` trait in full, so the two can never disagree.
#[tokio::test]
async fn the_bind_time_capability_audit_passes() {
    use crate::store::memory::driver::{
        MemoryDriverConfig, MemoryMode, RemoteDeployment, open_driver,
    };

    let (base_url, _state) = spawn_mock("opencompany").await;
    // SAFETY (test-only): OPENCOMPANY_MEMORY_ACTOR is read once inside
    // `open_driver`, and this test does not run concurrently with another
    // that reads the same variable within this crate's cortexdb driver path.
    // Not set here: the default actor is "opencompany", matching the mock.
    let config = MemoryDriverConfig {
        mode: MemoryMode::Remote,
        driver_id: Some(CORTEXDB_DRIVER_ID.to_string()),
        url: Some(base_url),
        api_key: Some(TOKEN.to_string()),
        data_dir: None,
        deployment: RemoteDeployment::SelfHosted,
    };
    let (provider, class) = open_driver(&config)
        .expect("cortexdb must bind")
        .expect("a driver_id was named, so this is not the store-mode None");
    assert_eq!(provider.driver_id(), CORTEXDB_DRIVER_ID);
    assert_eq!(class, tinymemory::registry::DriverClass::External);
    // `open_driver` itself runs `audit_provider` before returning; getting a
    // provider back at all is the audit already having passed. Assert it a
    // second time explicitly, since that is exactly what this test is for.
    tinymemory_api::provider::audit_provider(provider.as_ref())
        .expect("advertised capabilities must match the implemented surface");
}
