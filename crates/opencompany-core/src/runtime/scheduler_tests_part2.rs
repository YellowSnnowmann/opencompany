use super::tests_core::*;

/// A pass that hit a transient store error does NOT latch, so a later pass
/// retries and fires — 0 then 1 across a flaky-once store.
#[tokio::test]
async fn a_failed_pass_does_not_latch() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let manifest = scheduled_manifest();
    let schedules = manifest.schedules.clone();
    let flaky = Arc::new(FlakyOnceFires::new(1));
    let rt = Arc::new(
        RuntimeBuilder::new(home, manifest)
            .with_brain(Arc::new(ScheduleBrain))
            .with_schedule_fires(flaky.clone())
            .build()
            .await
            .unwrap(),
    );
    let sid = manifest_schedule_id("0 9 * * MON", "weekly standup");
    // Seed the anchor directly (bypassing the fail budget) so the miss is real.
    flaky.seed(rt.id(), &sid, millis_at(2026, 7, 6, 9, 0) / MINUTE_MS);
    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 20, 9, 5)));
    let mut scheduler = CompanyScheduler::new(rt.clone(), &schedules, clock).unwrap();

    // First pass: the anchor read errors, so the pass does not complete and
    // must NOT latch.
    assert_eq!(
        scheduler.catch_up().await.unwrap(),
        0,
        "a store error fires nothing and does not latch"
    );
    assert_eq!(fired_count(&rt).await, 0);

    // Second pass: the store now works, so the deferred catch-up fires.
    assert_eq!(
        scheduler.catch_up().await.unwrap(),
        1,
        "the retry makes up the missed fire"
    );
    assert_eq!(fired_count(&rt).await, 1);
}
