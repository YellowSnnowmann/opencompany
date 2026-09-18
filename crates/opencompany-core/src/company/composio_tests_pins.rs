//! Composio account-pin and mode-selection tests: the MemSecrets fixture,
//! pin storage/round-trip, and BYOK-vs-managed mode selection (split out
//! of `composio_tests.rs`).

use super::*;

#[derive(Default)]
pub(super) struct MemSecrets {
    pub(super) map: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[async_trait::async_trait]
impl SecretStore for MemSecrets {
    async fn get(&self, _c: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(key)
            .map(|v| SecretValue(v.clone())))
    }
    async fn set(&self, _c: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.0);
        Ok(())
    }
}

/// Raw slot contents, blank-or-absent collapsed to `""`, so an assertion holds
/// on a backend that stores `""` and on one that treats it as absent.
pub(super) async fn raw(secrets: &dyn SecretStore, company: &CompanyId, key: &str) -> String {
    secrets
        .get(company, key)
        .await
        .unwrap()
        .map(|SecretValue(v)| v)
        .unwrap_or_default()
}

#[tokio::test]
async fn a_company_with_no_stored_preference_pins_nothing() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    assert!(load_defaults(&company, &secrets).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_pin_round_trips_and_is_replaced_rather_than_appended() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    let after = set_default(&company, &secrets, "gmail", "ca_ops")
        .await
        .unwrap();
    assert_eq!(after.get("gmail").map(String::as_str), Some("ca_ops"));
    assert_eq!(
        load_defaults(&company, &secrets).await.unwrap(),
        after,
        "the stored blob is what the setter reported"
    );

    // A second toolkit is additive; naming gmail again replaces it.
    set_default(&company, &secrets, "slack", "ca_workspace")
        .await
        .unwrap();
    let after = set_default(&company, &secrets, "gmail", "ca_billing")
        .await
        .unwrap();
    assert_eq!(after.get("gmail").map(String::as_str), Some("ca_billing"));
    assert_eq!(
        after.get("slack").map(String::as_str),
        Some("ca_workspace"),
        "pinning one toolkit must not disturb another"
    );
}

#[tokio::test]
async fn toolkits_are_normalized_so_a_pin_is_found_by_the_slug_prefix() {
    // `slug_toolkit` lowercases (`GMAIL_SEND_EMAIL` → `gmail`), so a pin
    // stored under `GMail` would be invisible to the execute path.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    set_default(&company, &secrets, " GMail ", " ca_ops ")
        .await
        .unwrap();
    assert_eq!(
        load_defaults(&company, &secrets)
            .await
            .unwrap()
            .get("gmail")
            .map(String::as_str),
        Some("ca_ops")
    );
}

#[tokio::test]
async fn clearing_a_pin_returns_the_toolkit_to_composios_own_resolution() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    set_default(&company, &secrets, "gmail", "ca_ops")
        .await
        .unwrap();
    let after = clear_default(&company, &secrets, "gmail").await.unwrap();
    assert!(after.is_empty());
    assert!(load_defaults(&company, &secrets).await.unwrap().is_empty());
    // Clearing what was never pinned is not an error.
    assert!(
        clear_default(&company, &secrets, "gmail")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn revoking_the_pinned_account_drops_the_pin() {
    // Otherwise the next execute sends an id Composio no longer knows, and
    // disconnecting the *other* account breaks the toolkit.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    set_default(&company, &secrets, "gmail", "ca_ops")
        .await
        .unwrap();
    set_default(&company, &secrets, "slack", "ca_workspace")
        .await
        .unwrap();

    assert!(
        !forget_connection(&company, &secrets, "ca_unrelated")
            .await
            .unwrap(),
        "revoking an unpinned account changes nothing"
    );
    assert!(
        forget_connection(&company, &secrets, "ca_ops")
            .await
            .unwrap()
    );

    let left = load_defaults(&company, &secrets).await.unwrap();
    assert_eq!(left.get("slack").map(String::as_str), Some("ca_workspace"));
    assert!(!left.contains_key("gmail"));
}

#[tokio::test]
async fn an_unparseable_blob_reads_as_no_preference() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(&company, DEFAULTS_KEY, SecretValue("not json".into()))
        .await
        .unwrap();
    assert!(
        load_defaults(&company, &secrets).await.unwrap().is_empty(),
        "a hand-edited blob must fall back to Composio's resolution, not withhold the tools"
    );
}
// ── BYOK routing ────────────────────────────────────────────────

#[tokio::test]
async fn a_company_that_configured_nothing_is_openhuman_managed() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    assert_eq!(
        load_mode(&company, &secrets).await.unwrap(),
        ComposioMode::Managed
    );
    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(access.mode, ComposioMode::Managed);
}

#[tokio::test]
async fn storing_a_key_selects_byok_and_clearing_it_gives_the_managed_route_back() {
    // The two writes travel together deliberately — a mode without a key is
    // a company with no Composio tools, and a key without a mode is inert.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    let mode = store_api_key(&company, &secrets, " ak_live ")
        .await
        .unwrap();
    assert_eq!(mode, ComposioMode::Byok);
    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(access.mode, ComposioMode::Byok);
    assert_eq!(
        access.credential.current().await.unwrap().as_deref(),
        Some("ak_live"),
        "the key is trimmed on the way in — Composio rejects a padded x-api-key"
    );

    let mode = store_api_key(&company, &secrets, "").await.unwrap();
    assert_eq!(mode, ComposioMode::Managed);
    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(access.mode, ComposioMode::Managed);
}

/// Wraps [`MemSecrets`] and fails every `set` for one chosen key, so a test
/// can land a `store_api_key` call exactly at its second write and inspect
/// what the first one left behind.
pub(super) struct SecretsFailingToWrite {
    pub(super) inner: MemSecrets,
    pub(super) blocked_key: &'static str,
}

#[async_trait::async_trait]
impl SecretStore for SecretsFailingToWrite {
    async fn get(&self, c: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        self.inner.get(c, key).await
    }
    async fn set(&self, c: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        if key == self.blocked_key {
            return Err(crate::error::OpenCompanyError::Store(
                "write refused by test".into(),
            ));
        }
        self.inner.set(c, key, value).await
    }
}

/// A clear that dies on its **first** write (the mode) must leave the
/// company exactly as it was: still `Byok`, still holding the key that was
/// working a moment ago. This is the direction issue #… — writing the mode
/// first for a clear — exists to protect: a fixed key-then-mode order would
/// have written the key EMPTY here, before ever reaching the (blocked) mode
/// write, stranding a `Byok` company with no key.
#[tokio::test]
async fn a_clear_that_fails_on_the_mode_write_leaves_byok_intact() {
    let company = CompanyId::new("acme");
    let secrets = SecretsFailingToWrite {
        inner: MemSecrets::default(),
        blocked_key: MODE_KEY,
    };
    // Seeded directly on `inner`, bypassing the wrapper's own blocking
    // `set()` — the state under test is "already BYOK", not "how it got
    // there", and going through `store_api_key` here would hit the very
    // block this test exists to trigger before the test has even started.
    secrets
        .inner
        .set(&company, BYOK_KEY_KEY, SecretValue("ak_live".into()))
        .await
        .unwrap();
    secrets
        .inner
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
        .await
        .unwrap();

    let err = store_api_key(&company, &secrets, "").await;
    assert!(
        err.is_err(),
        "the blocked write must propagate, not swallow"
    );

    assert_eq!(
        load_mode(&company, &secrets).await.unwrap(),
        ComposioMode::Byok
    );
    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(
        access.credential.current().await.unwrap().as_deref(),
        Some("ak_live"),
        "the key a moment ago worked and must still work — nothing broke"
    );
}

/// A clear that dies on its **second** write (the key) must still have
/// landed the mode: the company reads back as `Managed`, with a stale
/// unused key sitting inert in `BYOK_KEY_KEY` — never consulted once the
/// mode says managed.
#[tokio::test]
async fn a_clear_that_fails_on_the_key_write_still_lands_managed() {
    let company = CompanyId::new("acme");
    let secrets = SecretsFailingToWrite {
        inner: MemSecrets::default(),
        blocked_key: BYOK_KEY_KEY,
    };
    // Seeded directly on `inner` for the same reason as the sibling test
    // above: this test's block is `BYOK_KEY_KEY`, and `store_api_key`'s set
    // direction writes that key first — routing the initial BYOK selection
    // through the wrapper would block before there was anything to clear.
    secrets
        .inner
        .set(&company, BYOK_KEY_KEY, SecretValue("ak_live".into()))
        .await
        .unwrap();
    secrets
        .inner
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
        .await
        .unwrap();

    let err = store_api_key(&company, &secrets, "").await;
    assert!(err.is_err());

    assert_eq!(
        load_mode(&company, &secrets).await.unwrap(),
        ComposioMode::Managed,
        "the mode write is first for a clear, and it landed before the blocked one"
    );
    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(
        access.mode,
        ComposioMode::Managed,
        "a stale key under a managed mode is inert — resolve_access never reads it"
    );
}

#[tokio::test]
async fn byok_never_falls_back_to_a_managed_credential() {
    // The load-bearing one. A company that asked to act through its own
    // Composio account must not silently act through the platform's: that
    // would connect providers into the wrong tenant and bill the wrong
    // party. No key means no tools.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    // A managed-tier credential that *would* answer, so the test proves the
    // BYOK arm ignores it rather than that there was nothing to fall back to.
    // Seeded at both the new and legacy managed addresses, so the test also
    // proves neither managed address leaks into BYOK (#2306).
    secrets
        .set(
            &company,
            TINYHUMANS_KEY_KEY,
            SecretValue("backend-bearer".into()),
        )
        .await
        .unwrap();
    secrets
        .set(
            &company,
            LEGACY_TOKEN_KEY,
            SecretValue("th-not-a-real-key".into()),
        )
        .await
        .unwrap();
    secrets
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
        .await
        .unwrap();

    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(access.mode, ComposioMode::Byok);
    assert!(
        !access.credential.configured(),
        "BYOK with no API key resolves to nothing, not to the backend token"
    );
}

#[tokio::test]
async fn the_byok_key_is_never_confused_with_the_backend_token() {
    // Two different secrets authenticating two different hosts. Selecting
    // BYOK must present the Composio key, not the bearer stored next to it.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company,
            TINYHUMANS_KEY_KEY,
            SecretValue("backend-bearer".into()),
        )
        .await
        .unwrap();
    store_api_key(&company, &secrets, "ak_live").await.unwrap();

    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(
        access.credential.current().await.unwrap().as_deref(),
        Some("ak_live")
    );

    // And clearing the key hands the backend token back untouched — a
    // switch to BYOK and back must not cost the company its override.
    store_api_key(&company, &secrets, "").await.unwrap();
    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(access.mode, ComposioMode::Managed);
    assert_eq!(
        access.credential.current().await.unwrap().as_deref(),
        Some("backend-bearer")
    );
}

#[tokio::test]
async fn a_hand_edited_mode_slot_cannot_take_a_company_s_tools_away() {
    // Read on the roster path: an unrecognised mode must degrade to the
    // route that works without anything stored, not to no route at all.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(&company, MODE_KEY, SecretValue("nonsense".into()))
        .await
        .unwrap();
    assert_eq!(
        load_mode(&company, &secrets).await.unwrap(),
        ComposioMode::Managed,
        "an unrecognised mode falls back to the route that works without config"
    );

    // OpenHuman's and TinyMemory's spelling of the same route. Never
    // written here, but a slot that carried it must not silently mean the
    // opposite — a company asking for its own account would get the
    // platform's.
    secrets
        .set(&company, MODE_KEY, SecretValue("dirEct".into()))
        .await
        .unwrap();
    assert_eq!(
        load_mode(&company, &secrets).await.unwrap(),
        ComposioMode::Byok
    );

    // …and the managed spellings the other repos use need no alias: they
    // already mean managed through the fallback.
    for spelling in ["backend", "proxied"] {
        secrets
            .set(&company, MODE_KEY, SecretValue(spelling.into()))
            .await
            .unwrap();
        assert_eq!(
            load_mode(&company, &secrets).await.unwrap(),
            ComposioMode::Managed,
            "`{spelling}` means managed everywhere it is used"
        );
    }

    secrets
        .set(&company, MODE_KEY, SecretValue("  BYOK  ".into()))
        .await
        .unwrap();
    assert_eq!(
        load_mode(&company, &secrets).await.unwrap(),
        ComposioMode::Byok,
        "the one spelling this repo does use is matched case-insensitively"
    );
}
