//! `proxy_base_url` tests: a fan-out that derives the TinyHumans OpenRouter
//! proxy base from this instance's `api_url` rather than the catalogue's
//! static production endpoint, and migrates an existing row when that base
//! changes (split out of `fan_out_tests.rs`).

use super::fan_out_tests_support::*;
use super::*;

/// `proxy_base_url` (a staging or local platform's `api_url`) is not just
/// consulted for the health probe — the stored `tinyhumans` row's `base_url`
/// itself must carry the derived proxy URL, or a later resolver reading the
/// row back would probe the wrong platform (tinysweeper review finding on
/// this PR: every other `fan_out` unit test passes `proxy_base_url: None`,
/// which only exercises the production fallback in
/// [`catalogue::tinyhumans_proxy_url`]).
#[tokio::test]
async fn proxy_base_url_override_lands_on_the_stored_row() {
    let cid = company("proxy-base-url");
    let secrets = MemSecrets::default();
    let prober = FakeProber::ok(&[MODEL]);
    let api_url = "https://staging-api.tinyhumans.ai";

    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: Some(MODEL),
            confirm_in_use: true,
            proxy_base_url: Some(api_url),
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(outcome(&report, Slot::Provider), SlotOutcome::Filled);
    let providers = inference_store::list_providers(&cid, &secrets)
        .await
        .unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].slug, "tinyhumans");
    let expected = catalogue::tinyhumans_proxy_url(api_url);
    assert_eq!(providers[0].base_url, expected);
    assert_ne!(
        providers[0].base_url,
        catalogue::cloud_provider(inference::MANAGED_SLUG)
            .unwrap()
            .endpoint
    );
    // The health probe reads the same override, not the catalogue's
    // production endpoint (tinysweeper review: the row and the probe base
    // must not be able to drift apart).
    assert_eq!(prober.last_base_url(), Some(expected));
}

/// A `tinyhumans` row minted before this instance followed
/// `TINYHUMANS_API_URL` (or minted under a since-changed one) carries a stale
/// `base_url` forever unless a later save corrects it — `Kept(RowExists)`
/// used to leave an existing row's endpoint untouched no matter what
/// `proxy_base_url` the request carried, so a staging/local deployment with a
/// pre-existing row kept probing and pointing at whatever the row was first
/// created against, typically production (Codex review: "migrate existing
/// TinyHumans rows to the configured platform"). A save now refreshes the
/// row's `base_url` in place — and only that field, so the row's model and
/// enabled state survive untouched — and probes the *new* endpoint rather
/// than the stale stored one.
#[tokio::test]
async fn an_existing_row_migrates_to_a_changed_proxy_base_url() {
    let cid = company("migrate-row");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    // Seeded at the catalogue's static production endpoint — the same shape a
    // row minted before `TINYHUMANS_API_URL` existed, or under a different
    // one, would carry.
    seed_row(&secrets, &cid, MODEL).await;
    let stale_base_url = catalogue::cloud_provider(inference::MANAGED_SLUG)
        .unwrap()
        .endpoint
        .to_string();
    assert_eq!(
        inference_store::list_providers(&cid, &secrets)
            .await
            .unwrap()[0]
            .base_url,
        stale_base_url,
        "sanity: seeded on the stale endpoint"
    );

    let new_api_url = "https://staging-api.tinyhumans.ai";
    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: Some(new_api_url),
        },
        &prober,
    )
    .await
    .unwrap();

    let migrated = catalogue::tinyhumans_proxy_url(new_api_url);
    assert_ne!(migrated, stale_base_url);
    let providers = inference_store::list_providers(&cid, &secrets)
        .await
        .unwrap();
    assert_eq!(providers.len(), 1, "migrating in place, not adding a row");
    assert_eq!(providers[0].base_url, migrated);
    // Everything else about the row survived the migration untouched.
    assert_eq!(
        providers[0].model(),
        inference_store::ModelOnRow::One(MODEL.to_string())
    );
    assert!(providers[0].enabled);
    // The health probe checked the row's *new* endpoint, not the one it was
    // about to migrate away from.
    assert_eq!(prober.last_base_url(), Some(migrated));
    assert_eq!(outcome(&report, Slot::Provider), SlotOutcome::Rotated);
}

/// A failed row migration must not read as a clean `Kept(RowExists)` — the
/// account/inference keys are already rotated and the health probe already
/// checked the *new* endpoint by this point, so reporting success here would
/// tell the operator the migration landed while the stored row keeps routing
/// every turn to the old one, with nothing on the response or in the journal
/// saying otherwise (Codex P1 / CodeRabbit review).
#[tokio::test]
async fn a_failed_row_migration_reports_failed_not_kept() {
    let cid = company("migrate-row-fails");
    let inner = MemSecrets::default();
    raw_set(&inner, &cid, ACCOUNT_KEY_KEY, OLD).await;
    seed_row(&inner, &cid, MODEL).await;
    let secrets = FailsWriting {
        inner,
        failing_key: inference_store::PROVIDER_INDEX_KEY.to_string(),
    };

    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: Some("https://staging-api.tinyhumans.ai"),
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(outcome(&report, Slot::Provider), SlotOutcome::Failed);
    assert_eq!(outcome(&report, Slot::Health), SlotOutcome::Failed);
    // The row itself really is unchanged — the write failed, not merely the
    // report of it.
    let providers = inference_store::list_providers(&cid, &secrets.inner)
        .await
        .unwrap();
    assert_eq!(
        providers[0].base_url,
        catalogue::cloud_provider(inference::MANAGED_SLUG)
            .unwrap()
            .endpoint
    );
    // The probe recorded health `ok` for the endpoint the row was *supposed*
    // to migrate to; since the migration didn't land, that record must not
    // survive — a reader trusting it would think turns reach a platform they
    // do not (CodeRabbit review).
    let health = inference_store::load_health(&cid, &secrets.inner)
        .await
        .unwrap();
    assert!(
        !health.contains_key(inference::MANAGED_SLUG),
        "a failed migration must not leave a stale healthy record: {health:?}"
    );
}
