//! §6 carry-over matrix tests, part 2: matrix rows M8 through C3
//! (split out of `fan_out_tests.rs`).

use std::sync::atomic::Ordering;

use super::fan_out_tests_support::*;
use super::*;

#[tokio::test]
async fn matrix_m8() {
    let cid = company("m8");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    inference::save_runtime_config(
        &cid,
        &secrets,
        &inference::RuntimeInference {
            provider: "managed".to_string(),
            base_url: None,
            models: Default::default(),
        },
    )
    .await
    .unwrap();
    raw_set(&secrets, &cid, LEGACY_INFERENCE_KEY_KEY, OLD).await;

    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, NEW);
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, NEW);
    assert_eq!(
        raw_get(&secrets, &cid, LEGACY_INFERENCE_KEY_KEY).await,
        "",
        "the set_managed_key convergence rule clears the legacy slot on the copy too"
    );
    assert!(
        inference_store::list_providers(&cid, &secrets)
            .await
            .unwrap()
            .iter()
            .all(|p| p.origin != inference_store::ProviderOrigin::Indexed),
        "the legacy Managed row and an indexed tinyhumans row never both exist"
    );
    assert_eq!(
        inference_store::load_default(&cid, &secrets).await.unwrap(),
        inference_store::DefaultChoice::Unset
    );

    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Rotated);
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Skipped(SkipReason::LegacyManagedConfig)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Skipped(SkipReason::LegacyManagedConfig)
    );
    assert_eq!(
        outcome(&report, Slot::Health),
        SlotOutcome::Skipped(SkipReason::LegacyManagedConfig)
    );
    assert!(!report.needs_model);
    assert_eq!(
        prober.calls.load(Ordering::SeqCst),
        0,
        "entry zero being managed skips the probe entirely"
    );
}

#[tokio::test]
async fn matrix_m9() {
    let cid = company("m9");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    inference::save_runtime_config(
        &cid,
        &secrets,
        &inference::RuntimeInference {
            provider: "openrouter".to_string(),
            base_url: None,
            models: Default::default(),
        },
    )
    .await
    .unwrap();
    raw_set(
        &secrets,
        &cid,
        LEGACY_INFERENCE_KEY_KEY,
        "sk-not-a-real-key",
    )
    .await;

    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: Some(MODEL),
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, NEW);
    assert_eq!(
        raw_get(&secrets, &cid, LEGACY_INFERENCE_KEY_KEY).await,
        "sk-not-a-real-key",
        "entry zero's own vendor credential must never be touched"
    );
    let providers = inference_store::list_providers(&cid, &secrets)
        .await
        .unwrap();
    assert!(
        providers
            .iter()
            .any(|p| p.slug == "tinyhumans" && p.origin == inference_store::ProviderOrigin::Indexed)
    );
    assert_eq!(
        inference_store::load_default(&cid, &secrets).await.unwrap(),
        full("tinyhumans", MODEL)
    );

    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Provider), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Default), SlotOutcome::Filled);
}

#[tokio::test]
async fn matrix_m10() {
    let cid = company("m10");
    let secrets = MemSecrets::default();
    let prober = FakeProber::failing(probe::ProbeClass::Auth);

    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(
        raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await,
        NEW,
        "the account key is kept"
    );
    assert_eq!(
        raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await,
        NEW,
        "the Composio copy is kept"
    );
    assert_eq!(
        raw_get(&secrets, &cid, &llm_key_key()).await,
        "",
        "the LLM copy is rolled back"
    );
    assert!(
        inference_store::list_providers(&cid, &secrets)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        inference_store::load_default(&cid, &secrets).await.unwrap(),
        inference_store::DefaultChoice::Unset
    );

    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::RolledBack);
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Skipped(SkipReason::InferenceRejected)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Skipped(SkipReason::InferenceRejected)
    );
    assert_eq!(
        outcome(&report, Slot::Health),
        SlotOutcome::HealthFailed(probe::ProbeClass::Auth)
    );
    assert!(!report.needs_model);
}

#[tokio::test]
async fn matrix_m11() {
    let cid = company("m11");
    let secrets = MemSecrets::default();
    let prober = FakeProber::ok(&[MODEL]);
    fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: Some(MODEL),
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, NEW);
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, NEW);
    assert_eq!(
        inference_store::list_providers(&cid, &secrets)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        inference_store::load_default(&cid, &secrets).await.unwrap(),
        full("tinyhumans", MODEL)
    );

    assert_eq!(
        outcome(&report, Slot::Composio),
        SlotOutcome::Kept(SkipReason::AlreadyCurrent)
    );
    assert_eq!(
        outcome(&report, Slot::Inference),
        SlotOutcome::Kept(SkipReason::AlreadyCurrent)
    );
    assert_eq!(outcome(&report, Slot::Provider), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Default), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Health), SlotOutcome::HealthOk);
    assert!(!report.needs_model);
}

#[tokio::test]
async fn matrix_m12() {
    let cid = company("m12");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    raw_set(&secrets, &cid, COMPOSIO_KEY_KEY, OLD).await;
    raw_set(&secrets, &cid, &llm_key_key(), OLD).await;
    seed_row(&secrets, &cid, MODEL).await;

    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, NEW);
    assert_eq!(
        inference_store::load_default(&cid, &secrets).await.unwrap(),
        full("tinyhumans", MODEL),
        "the default fills from the row's own model, none having been sent"
    );

    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Kept(SkipReason::RowExists)
    );
    assert_eq!(outcome(&report, Slot::Default), SlotOutcome::Filled);
}

#[tokio::test]
async fn matrix_m13() {
    let cid = company("m13");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, "composio/mode", "byok").await;
    raw_set(&secrets, &cid, "composio/byok/key", "ak-not-a-real-key").await;

    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Filled);
    assert_eq!(
        raw_get(&secrets, &cid, "composio/mode").await,
        "byok",
        "the fan-out never reads or writes composio/mode"
    );
    assert_eq!(
        raw_get(&secrets, &cid, "composio/byok/key").await,
        "ak-not-a-real-key",
        "the fan-out never reads or writes composio/byok/key"
    );
}

#[tokio::test]
async fn matrix_m14() {
    let cid = company("m14");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    // Only the legacy address holds the old bearer — the fallback-read case.
    raw_set(&secrets, &cid, COMPOSIO_LEGACY_KEY, OLD).await;
    raw_set(&secrets, &cid, &llm_key_key(), OLD).await;
    seed_row(&secrets, &cid, MODEL).await;
    inference_store::set_default_choice(
        &cid,
        &secrets,
        &inference_store::ModelChoice {
            provider: "tinyhumans".to_string(),
            model: MODEL.to_string(),
        },
    )
    .await
    .unwrap();

    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(
        raw_get(&secrets, &cid, COMPOSIO_LEGACY_KEY).await,
        "",
        "the fan-out's own copy retires the legacy mirror rather than carrying it forward"
    );
    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Rotated);
}

#[tokio::test]
async fn matrix_c1() {
    let cid = company("c1");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, NEW).await;
    raw_set(&secrets, &cid, COMPOSIO_KEY_KEY, NEW).await;
    raw_set(&secrets, &cid, &llm_key_key(), NEW).await;
    seed_row(&secrets, &cid, MODEL).await;
    inference_store::set_default_choice(
        &cid,
        &secrets,
        &inference_store::ModelChoice {
            provider: "tinyhumans".to_string(),
            model: MODEL.to_string(),
        },
    )
    .await
    .unwrap();
    inference_store::record_health(&cid, &secrets, "tinyhumans", "ok", "2026-01-01T00:00:00Z")
        .await
        .unwrap();

    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(""),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, "");
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, "");
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, "");
    assert_eq!(
        inference_store::list_providers(&cid, &secrets)
            .await
            .unwrap()
            .len(),
        1,
        "the row stays"
    );
    assert_eq!(
        inference_store::load_default(&cid, &secrets).await.unwrap(),
        full("tinyhumans", MODEL),
        "the default stays"
    );
    assert!(
        !inference_store::load_health(&cid, &secrets)
            .await
            .unwrap()
            .contains_key("tinyhumans"),
        "health is forgotten"
    );

    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Cleared);
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Cleared);
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Skipped(SkipReason::KeyCleared)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Skipped(SkipReason::KeyCleared)
    );
    assert_eq!(outcome(&report, Slot::Health), SlotOutcome::Cleared);
}

#[tokio::test]
async fn matrix_c2() {
    let cid = company("c2");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, NEW).await;
    raw_set(&secrets, &cid, COMPOSIO_KEY_KEY, CUSTOM).await;
    raw_set(&secrets, &cid, &llm_key_key(), CUSTOM).await;
    seed_row(&secrets, &cid, MODEL).await;

    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(""),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, "");
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, CUSTOM);
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, CUSTOM);

    assert_eq!(
        outcome(&report, Slot::Composio),
        SlotOutcome::Kept(SkipReason::CustomKey)
    );
    assert_eq!(
        outcome(&report, Slot::Inference),
        SlotOutcome::Kept(SkipReason::CustomKey)
    );
}

#[tokio::test]
async fn matrix_c3() {
    let cid = company("c3");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, NEW).await;

    let prober = FakeProber::ok(&[MODEL]);
    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(""),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, "");
    assert_eq!(
        outcome(&report, Slot::Composio),
        SlotOutcome::Skipped(SkipReason::AlreadyEmpty)
    );
    assert_eq!(
        outcome(&report, Slot::Inference),
        SlotOutcome::Skipped(SkipReason::AlreadyEmpty)
    );
}
