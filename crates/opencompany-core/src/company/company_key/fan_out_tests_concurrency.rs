//! Concurrency and account-key-copy tests: interleaved fan-outs, that no
//! report or note ever carries a key, and the account-key copy rules
//! (split out of `fan_out_tests.rs`).

use super::fan_out_tests_support::*;
use super::*;

/// Red-proof in the PR: without `slot_guard` held for the whole call, this
/// fails — one save's read-then-write can interleave with the other's and
/// leave the Composio and LLM copies pointing at different values than
/// `tinyhumans/key` itself.
#[tokio::test]
async fn concurrent_saves_leave_every_copy_equal_to_the_account_key() {
    const OTHER: &str = "th-not-a-real-key-3";
    for i in 0..20 {
        let cid = company(&format!("concurrent-{i}"));
        let secrets = std::sync::Arc::new(SlowSecrets {
            inner: MemSecrets::default(),
        });
        raw_set(secrets.as_ref(), &cid, ACCOUNT_KEY_KEY, OLD).await;
        raw_set(secrets.as_ref(), &cid, COMPOSIO_KEY_KEY, OLD).await;
        raw_set(secrets.as_ref(), &cid, &llm_key_key(), OLD).await;

        let prober_a = FakeProber::ok(&[MODEL]);
        let prober_b = FakeProber::ok(&[MODEL]);
        let (s1, s2) = (secrets.clone(), secrets.clone());
        let (c1, c2) = (cid.clone(), cid.clone());
        let first = fan_out(
            &c1,
            s1.as_ref(),
            FanOutRequest {
                key: FanOutKey::Explicit(NEW),
                model: None,
                confirm_in_use: true,
                proxy_base_url: None,
            },
            &prober_a,
        );
        let second = fan_out(
            &c2,
            s2.as_ref(),
            FanOutRequest {
                key: FanOutKey::Explicit(OTHER),
                model: None,
                confirm_in_use: true,
                proxy_base_url: None,
            },
            &prober_b,
        );
        let (r1, r2) = tokio::join!(first, second);
        r1.unwrap();
        r2.unwrap();

        let account = raw_get(secrets.as_ref(), &cid, ACCOUNT_KEY_KEY).await;
        let composio = raw_get(secrets.as_ref(), &cid, COMPOSIO_KEY_KEY).await;
        let llm = raw_get(secrets.as_ref(), &cid, &llm_key_key()).await;
        assert_eq!(
            composio, account,
            "iteration {i}: composio must equal whichever save landed last"
        );
        assert_eq!(
            llm, account,
            "iteration {i}: the LLM copy must equal whichever save landed last"
        );
    }
}

#[tokio::test]
async fn no_report_or_note_contains_a_key() {
    let cid = company("no-leak");
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
    let note = fan_out_note(false, &report, None);

    let rendered = format!("{report:?}");
    assert!(!rendered.contains(OLD), "{rendered}");
    assert!(!rendered.contains(NEW), "{rendered}");
    assert!(!note.contains(OLD), "{note}");
    assert!(!note.contains(NEW), "{note}");
}

#[test]
fn a_failed_search_copy_points_the_operator_to_search_settings() {
    let report = FanOutReport {
        slots: vec![SlotReport {
            slot: Slot::Search,
            outcome: SlotOutcome::Failed,
        }],
        ..FanOutReport::default()
    };

    let note = fan_out_note(false, &report, None);
    assert!(note.contains("Search pages"), "{note}");
}

// ---------------------------------------------------------------------------
// copy_account_key_to_composio — keys rework #2306, slice 4c (Composio half)
// ---------------------------------------------------------------------------

const MODE_KEY: &str = crate::company::composio::MODE_KEY;
const BYOK_KEY_KEY: &str = crate::company::composio::BYOK_KEY_KEY;

#[tokio::test]
async fn copying_the_account_key_fills_the_composio_tinyhumans_key() {
    let cid = company("copy-fill");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, NEW).await;

    let report = copy_account_key_to_composio(&cid, &secrets).await.unwrap();

    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Filled);
    assert_eq!(
        report.slots.len(),
        1,
        "a single-slot copy reports one slot: {report:?}"
    );
    assert!(!report.needs_model);
    assert!(!report.sets_default);
    assert!(report.models.is_empty());
}

/// `decide_copy` alone answers `Keep(AlreadyCurrent)` when the slot already
/// resolves to the account key — whether that resolution comes from the new
/// address or, as here, purely from 1a's legacy fallback (`composio/token`).
/// This copy is explicit and user-requested, so it goes one step further than
/// the general fan-out's own `Keep` handling (which never writes) and
/// materialises the value on the new address, clearing the legacy address the
/// same way [`super::write_composio_slot`] always does — leaving a company
/// that clicks "Yes" with a clean, new-address-only state rather than one
/// still resolving through the pre-rename mirror.
#[tokio::test]
async fn copying_the_account_key_clears_the_legacy_token_when_it_was_the_old_value() {
    let cid = company("copy-legacy");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, NEW).await;
    // Only the legacy address holds the account key — the new address is
    // untouched, exactly the shape a pre-1a company can still be in.
    raw_set(&secrets, &cid, COMPOSIO_LEGACY_KEY, NEW).await;

    let report = copy_account_key_to_composio(&cid, &secrets).await.unwrap();

    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(
        raw_get(&secrets, &cid, COMPOSIO_LEGACY_KEY).await,
        "",
        "the copy retires the legacy mirror rather than carrying it forward"
    );
    assert_eq!(outcome(&report, Slot::Composio), SlotOutcome::Filled);
}

#[tokio::test]
async fn copying_is_refused_without_an_account_key() {
    let cid = company("copy-no-account-key");
    let secrets = MemSecrets::default();

    let err = copy_account_key_to_composio(&cid, &secrets)
        .await
        .unwrap_err();
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(ref m) if m.contains("no account key to reuse")),
        "{err}"
    );
    assert_eq!(
        raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await,
        "",
        "nothing is written on a refusal"
    );
}

#[tokio::test]
async fn copying_is_refused_over_a_custom_composio_key() {
    let cid = company("copy-custom-key");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, NEW).await;
    raw_set(&secrets, &cid, COMPOSIO_KEY_KEY, CUSTOM).await;

    let err = copy_account_key_to_composio(&cid, &secrets)
        .await
        .unwrap_err();
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(ref m) if m.contains("already has its own key")),
        "{err}"
    );
    assert_eq!(
        raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await,
        CUSTOM,
        "the custom key is left exactly as it was"
    );
}

/// A slot already equal to the account key is not a refusal (§3.1): it
/// answers `Kept`/`AlreadyCurrent` and touches nothing.
#[tokio::test]
async fn copying_the_account_key_is_a_no_op_when_already_current() {
    let cid = company("copy-already-current");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, NEW).await;
    raw_set(&secrets, &cid, COMPOSIO_KEY_KEY, NEW).await;

    let report = copy_account_key_to_composio(&cid, &secrets).await.unwrap();

    assert_eq!(raw_get(&secrets, &cid, COMPOSIO_KEY_KEY).await, NEW);
    assert_eq!(
        outcome(&report, Slot::Composio),
        SlotOutcome::Kept(SkipReason::AlreadyCurrent)
    );
}

#[tokio::test]
async fn copying_the_account_key_never_touches_mode_or_byok() {
    let cid = company("copy-no-mode-byok");
    let secrets = MemSecrets::default();
    raw_set(&secrets, &cid, ACCOUNT_KEY_KEY, NEW).await;
    raw_set(&secrets, &cid, MODE_KEY, "byok").await;
    raw_set(&secrets, &cid, BYOK_KEY_KEY, CUSTOM).await;

    copy_account_key_to_composio(&cid, &secrets).await.unwrap();

    assert_eq!(
        raw_get(&secrets, &cid, MODE_KEY).await,
        "byok",
        "the copy never reads or writes composio/mode"
    );
    assert_eq!(
        raw_get(&secrets, &cid, BYOK_KEY_KEY).await,
        CUSTOM,
        "the copy never reads or writes composio/byok/key"
    );
}

// ---------------------------------------------------------------------------
// index_lock (KR review comment 4012261302): the row and default writes are
// now genuinely serialised against every other `index_lock` holder, not
// merely re-checked immediately before the write.
// ---------------------------------------------------------------------------

/// A fresh company, no row yet: `fan_out` creates the `tinyhumans` row
/// (matrix M1's shape) while a *real* concurrent provider add — the same
/// `index_lock`-then-`put_provider` sequence
/// `server::ops::inference::providers::add_provider` uses, simulated here
/// without the HTTP scaffolding — adds a second, unrelated provider. Neither
/// row may be lost: with both operations taking the same lock around their
/// own read-modify-write of the index, they can only ever run one at a time,
/// never interleaved.
#[tokio::test]
async fn a_fan_out_racing_a_provider_add_loses_neither_row() {
    let cid = company("fanout-vs-add");
    let secrets = std::sync::Arc::new(SlowSecrets {
        inner: MemSecrets::default(),
    });
    let prober = FakeProber::ok(&[MODEL]);

    let (s1, s2) = (secrets.clone(), secrets.clone());
    let (c1, c2) = (cid.clone(), cid.clone());

    let fan_out_fut = fan_out(
        &c1,
        s1.as_ref(),
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: Some(MODEL),
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    );
    let add_fut = async move {
        let _guard = inference_store::index_lock(&c2).await;
        inference_store::put_provider(
            &c2,
            s2.as_ref(),
            inference_store::ProviderDraft {
                slug: "openrouter".to_string(),
                label: "OpenRouter".to_string(),
                kind: "openrouter".to_string(),
                base_url: "https://openrouter.ai/api/v1".to_string(),
                models: tier_overrides("openrouter/test-model"),
                enabled: true,
            },
        )
        .await
    };

    let (fan_out_result, add_result) = tokio::join!(fan_out_fut, add_fut);
    let report = fan_out_result.unwrap();
    add_result.unwrap();

    assert_eq!(outcome(&report, Slot::Provider), SlotOutcome::Filled);

    let mut slugs: Vec<String> = inference_store::list_providers(&cid, secrets.as_ref())
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.slug)
        .collect();
    slugs.sort();
    assert_eq!(
        slugs,
        vec![
            "openrouter".to_string(),
            inference::MANAGED_SLUG.to_string()
        ],
        "neither the fan-out's tinyhumans row nor the concurrent add's openrouter row may be lost"
    );
}

/// The default-slot counterpart: a concurrent, genuinely locked default
/// change to a different, already-connected provider — the same
/// `index_lock`-then-`set_default_slug` sequence a real set-default route
/// uses — must not be clobbered by the fan-out's own default write, and the
/// fan-out must see it rather than blindly overwriting it.
#[tokio::test]
async fn a_fan_out_racing_a_default_change_backs_off_or_wins_but_never_corrupts() {
    let cid = company("fanout-vs-default");
    let secrets = std::sync::Arc::new(SlowSecrets {
        inner: MemSecrets::default(),
    });
    // The provider the concurrent request marks default must already be
    // connected, exactly as the real route requires.
    inference_store::put_provider(
        &cid,
        secrets.as_ref(),
        inference_store::ProviderDraft {
            slug: "openrouter".to_string(),
            label: "OpenRouter".to_string(),
            kind: "openrouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            models: tier_overrides("openrouter/test-model"),
            enabled: true,
        },
    )
    .await
    .unwrap();
    let prober = FakeProber::ok(&[MODEL]);

    let (s1, s2) = (secrets.clone(), secrets.clone());
    let (c1, c2) = (cid.clone(), cid.clone());

    let fan_out_fut = fan_out(
        &c1,
        s1.as_ref(),
        FanOutRequest {
            key: FanOutKey::Explicit(NEW),
            model: Some(MODEL),
            confirm_in_use: true,
            proxy_base_url: None,
        },
        &prober,
    );
    let default_fut = async move {
        let _guard = inference_store::index_lock(&c2).await;
        inference_store::set_default_slug(&c2, s2.as_ref(), "openrouter")
            .await
            .unwrap();
    };

    let (fan_out_result, ()) = tokio::join!(fan_out_fut, default_fut);
    let report = fan_out_result.unwrap();

    // Whichever operation's lock hold went first, the default ends up
    // exactly one of the two — the fan-out's own default write if it ran
    // first and saw `Unset`, or the concurrent marker if that ran first and
    // the fan-out's own re-read (under the same lock) then saw it and backed
    // off. What must never happen is the fan-out reporting `Filled` while a
    // *different* value ends up stored — that would mean it wrote blind to a
    // marker it should have seen.
    let stored = inference_store::load_default(&cid, secrets.as_ref())
        .await
        .unwrap();
    if outcome(&report, Slot::Default) == SlotOutcome::Filled {
        assert_eq!(
            stored,
            inference_store::DefaultChoice::Full(inference_store::ModelChoice {
                provider: inference::MANAGED_SLUG.to_string(),
                model: MODEL.to_string(),
            }),
            "fan_out reported Filled, so the stored default must be its own write: {report:?}"
        );
    } else {
        assert_eq!(
            outcome(&report, Slot::Default),
            SlotOutcome::Kept(SkipReason::DefaultAlreadySet),
            "if fan_out did not fill the default, it must be because it saw the concurrent \
             marker already set: {report:?}"
        );
        assert_eq!(
            stored,
            inference_store::DefaultChoice::ProviderOnly("openrouter".to_string()),
            "the concurrent marker must survive untouched"
        );
    }
}
