//! §6 carry-over matrix tests, part 1: the Q7 `decide_copy` table and
//! matrix rows M1 through M7 (split out of `fan_out_tests.rs`).

use std::sync::atomic::Ordering;

use super::fan_out_tests_support::*;
use super::*;

#[test]
fn decide_copy_follows_the_q7_table() {
    // Not clearing.
    assert_eq!(
        decide_copy("B", "A", "B"),
        CopyDecision::Keep(SkipReason::AlreadyCurrent)
    );
    assert_eq!(decide_copy("", "A", "B"), CopyDecision::Write);
    assert_eq!(decide_copy("A", "A", "B"), CopyDecision::Write);
    assert_eq!(
        decide_copy("C", "A", "B"),
        CopyDecision::Keep(SkipReason::CustomKey)
    );
    // No prior account key at all: nothing can be "the old value".
    assert_eq!(decide_copy("", "", "B"), CopyDecision::Write);
    assert_eq!(
        decide_copy("C", "", "B"),
        CopyDecision::Keep(SkipReason::CustomKey)
    );

    // Clearing.
    assert_eq!(
        decide_copy("", "A", ""),
        CopyDecision::Skip(SkipReason::AlreadyEmpty)
    );
    assert_eq!(decide_copy("A", "A", ""), CopyDecision::Clear);
    assert_eq!(
        decide_copy("C", "A", ""),
        CopyDecision::Keep(SkipReason::CustomKey)
    );
}

// ---------------------------------------------------------------------------
// §6 matrix — M1..M14, C1..C3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn matrix_m1() {
    let cid = company("m1");
    let secrets = MemSecrets::default();
    let prober = FakeProber::ok(&[MODEL, "acme/other-model"]);

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
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Filled);
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Skipped(SkipReason::NeedsModel)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Skipped(SkipReason::NeedsModel)
    );
    assert_eq!(outcome(&report, Slot::Health), SlotOutcome::HealthOk);
    assert!(report.needs_model);
    assert!(report.sets_default);
    assert_eq!(
        report.models,
        vec!["acme/other-model".to_string(), MODEL.to_string()]
    );
}

#[tokio::test]
async fn matrix_m2() {
    let cid = company("m2");
    let secrets = MemSecrets::default();
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

    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, NEW);
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, NEW);
    let providers = inference_store::list_providers(&cid, &secrets)
        .await
        .unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].slug, "tinyhumans");
    assert_eq!(
        providers[0].model(),
        inference_store::ModelOnRow::One(MODEL.to_string())
    );
    assert_eq!(
        inference_store::load_default(&cid, &secrets).await.unwrap(),
        full("tinyhumans", MODEL)
    );

    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Provider), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Default), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Health), SlotOutcome::HealthOk);
    assert!(!report.needs_model);
}

#[tokio::test]
async fn search_copy_uses_the_same_custom_key_guard_as_the_other_slots() {
    let cid = company("search-custom");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    raw_set(
        &secrets,
        &cid,
        crate::company::search::MANAGED_KEY_SECRET,
        CUSTOM,
    )
    .await;

    let report = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &FakeProber::ok(&[MODEL]),
    )
    .await
    .unwrap();

    assert_eq!(
        raw_get(&secrets, &cid, crate::company::search::MANAGED_KEY_SECRET).await,
        CUSTOM
    );
    assert_eq!(
        outcome(&report, Slot::Search),
        SlotOutcome::Kept(SkipReason::CustomKey)
    );
}

#[tokio::test]
async fn clearing_refuses_when_managed_search_uses_the_account_key_copy() {
    let cid = company("search-in-use");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    raw_set(
        &secrets,
        &cid,
        crate::company::search::MANAGED_KEY_SECRET,
        OLD,
    )
    .await;

    let err = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(""),
            model: None,
            confirm_in_use: false,
            proxy_base_url: None,
        },
        &FakeProber::ok(&[]),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        err,
        OpenCompanyError::InUse { ref used_by, .. }
            if used_by.surfaces == vec![crate::error::UsedBySurface::Search]
    ));
    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, OLD);
}

#[tokio::test]
async fn matrix_m3() {
    let cid = company("m3");
    let secrets = MemSecrets::default();
    inference_store::put_provider(
        &cid,
        &secrets,
        inference_store::ProviderDraft {
            slug: "openrouter".to_string(),
            label: "OpenRouter".to_string(),
            kind: "openrouter".to_string(),
            base_url: "https://openrouter.example/v1".to_string(),
            models: BTreeMap::new(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    inference_store::set_default_choice(
        &cid,
        &secrets,
        &inference_store::ModelChoice {
            provider: "openrouter".to_string(),
            model: "x".to_string(),
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
            model: Some(MODEL),
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    )
    .await
    .unwrap();

    let providers = inference_store::list_providers(&cid, &secrets)
        .await
        .unwrap();
    assert_eq!(
        providers
            .iter()
            .map(|p| p.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["openrouter", "tinyhumans"]
    );
    assert_eq!(
        inference_store::load_default(&cid, &secrets).await.unwrap(),
        full("openrouter", "x")
    );
    assert_eq!(outcome(&report, Slot::Provider), SlotOutcome::Filled);
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Kept(SkipReason::DefaultAlreadySet)
    );
}

#[tokio::test]
async fn matrix_m4() {
    let cid = company("m4");
    let secrets = MemSecrets::default();
    inference_store::set_default_slug(&cid, &secrets, "openrouter")
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

    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, NEW);
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, NEW);
    assert!(
        inference_store::list_providers(&cid, &secrets)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        inference_store::load_default(&cid, &secrets).await.unwrap(),
        inference_store::DefaultChoice::ProviderOnly("openrouter".to_string())
    );

    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Filled);
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Filled);
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Skipped(SkipReason::NeedsModel)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Kept(SkipReason::DefaultAlreadySet)
    );
    assert!(report.needs_model);
    assert!(
        !report.sets_default,
        "a bare-slug default already counts as set (Q1)"
    );
}

#[tokio::test]
async fn matrix_m5() {
    let cid = company("m5");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    raw_set(&secrets, &cid, COMPOSIO_KEY_KEY, OLD).await;
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

    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Rotated);
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Rotated);
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Kept(SkipReason::RowExists)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Kept(SkipReason::DefaultAlreadySet)
    );
}

/// P2-2 (keys rework #2306 review): M5's "before" state — a genuine
/// **rotation** (`A`·`A`·`A`·row(m)·`{tinyhumans,m}` default), not a
/// first-time fill — but this time the probe answers `auth`. The rollback
/// restores the LLM slot to the raw prior key (`A`), not to empty the way
/// M10's fresh-company rollback does, `rollback_had_prior_key` makes the note
/// say so, and — the documented consequence — a second save of the same new
/// key afterward finds the LLM slot no longer recognisable as "the old
/// account key" (the account key itself already moved to `B` on the first
/// call and is never rolled back), so `decide_copy` reads it as a distinct,
/// custom value and reports `Kept(CustomKey)` rather than rotating it again.
#[tokio::test]
async fn matrix_m5_plus_auth() {
    let cid = company("m5-auth");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    raw_set(&secrets, &cid, COMPOSIO_KEY_KEY, OLD).await;
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

    // The account key and the Composio copy are never rolled back (Q6) — only
    // the LLM slot this specific request wrote is undone.
    assert_eq!(raw_get(&secrets, &cid, ACCOUNT_KEY_KEY).await, NEW);
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(
        raw_get(&secrets, &cid, &llm_key_key()).await,
        OLD,
        "a genuine rotation rolls back to the PRIOR key, not to empty"
    );
    // Untouched: `auth_rejected` skips the provider/default slots outright,
    // so the row and default this test seeded survive exactly as they were.
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

    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Rotated);
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::RolledBack);
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Skipped(SkipReason::InferenceRejected)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Skipped(SkipReason::InferenceRejected)
    );
    assert!(
        report.rollback_had_prior_key,
        "this was a rotation, not a first-time fill: {report:?}"
    );

    let note = fan_out_note(false, &report, None);
    assert!(
        note.contains(
            "TinyHumans on the LLM page still uses your previous key, because the new key \
             was rejected for LLM use."
        ),
        "{note}"
    );

    // The documented consequence: a second save of the SAME new key now
    // treats the LLM slot as a custom key, because the slot holds the raw
    // prior key (`A`) while `tinyhumans/key` itself has already moved on to
    // `B` — `decide_copy` can no longer tell the rolled-back value apart from
    // one an operator pasted by hand on the LLM page.
    let prober2 = FakeProber::ok(&[MODEL]);
    let second = fan_out(
        &cid,
        &secrets,
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: None,
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober2,
    )
    .await
    .unwrap();
    assert_eq!(
        outcome(&second, Slot::Inference),
        SlotOutcome::Kept(SkipReason::CustomKey),
        "{second:?}"
    );
    assert_eq!(
        prober2.calls.load(Ordering::SeqCst),
        0,
        "a slot already read as a custom key is never health-probed"
    );
}

#[tokio::test]
async fn matrix_m6() {
    let cid = company("m6");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    raw_set(&secrets, &cid, COMPOSIO_KEY_KEY, CUSTOM).await;
    raw_set(&secrets, &cid, &llm_key_key(), CUSTOM).await;

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
    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, CUSTOM);
    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, CUSTOM);
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

    assert_eq!(
        outcome(&report, Slot::Composio),
        SlotOutcome::Kept(SkipReason::CustomKey)
    );
    assert_eq!(
        outcome(&report, Slot::Inference),
        SlotOutcome::Kept(SkipReason::CustomKey)
    );
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Skipped(SkipReason::CustomKey)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Skipped(SkipReason::CustomKey)
    );
    assert_eq!(
        outcome(&report, Slot::Health),
        SlotOutcome::Skipped(SkipReason::CustomKey)
    );
    assert_eq!(
        prober.calls.load(Ordering::SeqCst),
        0,
        "a custom key is never probed"
    );
}

#[tokio::test]
async fn matrix_m7() {
    let cid = company("m7");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, OLD).await;
    raw_set(&secrets, &cid, COMPOSIO_KEY_KEY, OLD).await;
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

    assert_eq!(raw_get(&secrets, &cid, &llm_key_key()).await, NEW);
    assert_eq!(outcome(&report, Slot::Inference), SlotOutcome::Filled);
    assert_eq!(
        outcome(&report, Slot::Provider),
        SlotOutcome::Kept(SkipReason::RowExists)
    );
    assert_eq!(
        outcome(&report, Slot::Default),
        SlotOutcome::Kept(SkipReason::DefaultAlreadySet)
    );
}
