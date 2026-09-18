//! Rollback and failure-isolation tests: what a probe or a partial write
//! must leave behind, or must not touch (split out of `fan_out_tests.rs`).

use std::sync::atomic::Ordering;

use super::fan_out_tests_support::*;
use super::*;

// ---------------------------------------------------------------------------
// Rollback and failure isolation
// ---------------------------------------------------------------------------

/// Adapted from the plan's "M8 with a prober that answers `auth`": M8 itself
/// gates health on `legacy_managed` (entry zero declared managed), which
/// makes the probe unreachable there no matter what it would answer — see
/// `matrix_m8`'s own `calls == 0` assertion. This is the scenario that
/// actually reaches the probe while still matching M8's *other* defining
/// trait (`legacy_slot_is_managed` reading true through the "no entry zero at
/// all" branch, with a real value sitting in the legacy `inference/key`
/// slot): no entry zero, so nothing is `legacy_managed`, but the flat
/// `inference/key` slot is still what backs the LLM copy — and an `auth`
/// rejection has to restore both of the slots this request touched.
#[tokio::test]
async fn an_auth_probe_restores_the_llm_slots_exactly() {
    let cid = company("auth-restore");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    raw_set(&secrets, &cid, LEGACY_INFERENCE_KEY_KEY, OLD).await;

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
        raw_get(&secrets, &cid, &llm_key_key()).await,
        "",
        "restored to exactly what it held before this request"
    );
    assert_eq!(
        raw_get(&secrets, &cid, LEGACY_INFERENCE_KEY_KEY).await,
        OLD,
        "the legacy slot this request cleared is put back"
    );
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::RolledBack);
    assert_eq!(
        raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await,
        NEW,
        "the account key itself is kept"
    );
}

#[tokio::test]
async fn a_non_auth_probe_failure_keeps_everything() {
    let cid = company("non-auth");
    let secrets = MemSecrets::default();
    let prober = FakeProber::failing(probe::ProbeClass::Endpoint);

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
        outcome(&report, Slot::Health),
        SlotOutcome::HealthFailed(probe::ProbeClass::Endpoint)
    );
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Filled);
}

#[tokio::test]
async fn an_invalid_model_writes_nothing() {
    let cid = company("invalid-model");
    let secrets = MemSecrets::default();
    let prober = FakeProber::ok(&[MODEL]);

    let err = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: Some("chat-v1"),
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("workload name"), "{err}");
    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, "");
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, "");
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, "");
    assert_eq!(prober.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_model_with_a_clear_is_refused() {
    let cid = company("model-clear");
    let secrets = MemSecrets::default();
    let prober = FakeProber::ok(&[MODEL]);

    let err = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(""),
            model: Some(MODEL),
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("cannot be chosen"), "{err}");
    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, "");
}

/// P2-1 (keys rework #2306 review), updated by the `index_lock` fix
/// (653b3444a): a corrupt `inference/default` blob (invalid JSON behind a
/// `{` prefix, per `inference::store::parse_default`) no longer fails
/// `read_slots` at step 4 — that step carries no default-marker read any
/// more. `inference_store::load_default` is now read exactly once, under
/// `index_lock`, in step 9b, **after** the health probe. So a corrupt
/// default degrades only the two slots that depend on that re-read —
/// Provider and Default — to `Failed`; Composio and Inference, which read
/// and write ahead of it, still land as `Filled`, and the probe still runs
/// once. The corrupt blob itself is never rewritten: nothing here treats a
/// read failure as license to overwrite what it could not parse. The
/// account key (`tinyhumans/key`) must hold the minted value regardless —
/// this is still the "no bubbled `Err`" half of the P2-1 contract.
#[tokio::test]
async fn a_read_failure_after_the_account_key_is_stored_still_keeps_the_key() {
    let cid = company("read-fail");
    let secrets = MemSecrets::default();
    // Not valid JSON, but starts with `{` so `parse_default` attempts to
    // parse it rather than reading it as a bare slug — and fails.
    raw_set(&secrets, &cid, inference_store::DEFAULT_PROVIDER_KEY, "{").await;
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
    .expect("a read failure degrades to a Failed report, never a bubbled Err");

    assert_eq!(
        raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await,
        NEW,
        "the account key was already stored before the read that failed"
    );
    assert_eq!(
        outcome(&report, Slot::Composio),
        SlotOutcome::Filled,
        "Composio does not depend on the default re-read: {report:?}"
    );
    assert_eq!(
        outcome(&report, Slot::Inference),
        SlotOutcome::Filled,
        "the LLM key copy does not depend on the default re-read: {report:?}"
    );
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Failed,
        "the provider row read/write is inside the failed index_lock re-read: {report:?}"
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Failed,
        "the default marker is inside the failed index_lock re-read: {report:?}"
    );
    assert_eq!(
        outcome(&report, Slot::Health),
        SlotOutcome::HealthOk,
        "the health probe runs ahead of the failed default re-read: {report:?}"
    );
    assert_eq!(
        raw_get(&secrets, &cid, inference_store::DEFAULT_PROVIDER_KEY).await,
        "{",
        "a default this call could not parse must never be rewritten"
    );
    assert_eq!(
        prober.calls.load(Ordering::SeqCst),
        1,
        "the probe runs before the default re-read fails"
    );
}

#[tokio::test]
async fn failing_account_key_write_writes_nothing_else() {
    let cid = company("acct-fail");
    let secrets = FailsWriting {
        inner: MemSecrets::default(),
        failing_key: ACCOUNT_KEY_KEY.to_string(),
    };
    let prober = FakeProber::ok(&[MODEL]);

    let err = fan_out(
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
    .unwrap_err();

    assert!(matches!(err, OpenCompanyError::Store(_)));
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, "");
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, "");
}

#[tokio::test]
async fn failing_composio_write_still_sets_up_llm() {
    let cid = company("composio-fail");
    let secrets = FailsWriting {
        inner: MemSecrets::default(),
        failing_key: COMPOSIO_KEY_KEY.to_string(),
    };
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

    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Failed);
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
}

#[tokio::test]
async fn failing_inference_write_skips_row_default_and_probe() {
    let cid = company("inference-fail");
    let secrets = FailsWriting {
        inner: MemSecrets::default(),
        failing_key: llm_key_key(),
    };
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

    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Failed);
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Skipped(SkipReason::InferenceNotWritten)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Skipped(SkipReason::InferenceNotWritten)
    );
    assert_eq!(
        outcome(&report, Slot::Health),
        SlotOutcome::Skipped(SkipReason::InferenceNotWritten)
    );
    assert_eq!(prober.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failing_row_write_keeps_the_key_copy() {
    let cid = company("row-fail");
    let secrets = FailsWriting {
        inner: MemSecrets::default(),
        failing_key: inference_store::PROVIDER_INDEX_KEY.to_string(),
    };
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

    assert_eq!(outcome(&report, Slot::Provider), SlotOutcome::Failed);
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, NEW);
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Skipped(SkipReason::InferenceNotWritten)
    );
}

#[tokio::test]
async fn failing_default_write_keeps_the_row() {
    let cid = company("default-fail");
    let secrets = FailsWriting {
        inner: MemSecrets::default(),
        failing_key: inference_store::DEFAULT_PROVIDER_KEY.to_string(),
    };
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

    assert_eq!(outcome(&report, Slot::Default), SlotOutcome::Failed);
    assert_eq!(
        inference_store::list_providers(&cid, &secrets)
            .await
            .unwrap()
            .len(),
        1,
        "the row stays despite the default failing to write"
    );
}

#[tokio::test]
async fn failing_health_record_does_not_change_outcomes() {
    let cid = company("health-fail");
    let secrets = FailsWriting {
        inner: MemSecrets::default(),
        failing_key: inference_store::HEALTH_KEY.to_string(),
    };
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

    assert_eq!(outcome(&report, Slot::Health), SlotOutcome::HealthOk);
}
