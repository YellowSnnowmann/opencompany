//! Endpoint and stored-token-address tests: which host a call reaches,
//! the pinned storage addresses, and which of the legacy/new addresses
//! wins on read (split out of `composio_tests.rs`).

use super::tests_pins::{MemSecrets, raw};
use super::*;

#[test]
fn the_endpoint_reported_is_the_host_the_calls_reach() {
    let byok = ComposioAccess {
        mode: ComposioMode::Byok,
        credential: Credential::from_value("ak_live"),
    };
    assert_eq!(byok.endpoint("https://api.tinyhumans.ai"), DIRECT_BASE_URL);

    let managed = ComposioAccess {
        mode: ComposioMode::Managed,
        credential: Credential::from_value("bearer"),
    };
    assert_eq!(
        managed.endpoint("https://api.tinyhumans.ai"),
        "https://api.tinyhumans.ai"
    );
}

/// Fails every `get` for one chosen key, so a test can prove a read error
/// propagates instead of being swallowed into a fallback tier.
pub(super) struct SecretsFailingToRead {
    pub(super) inner: MemSecrets,
    pub(super) blocked_key: &'static str,
}

#[async_trait::async_trait]
impl SecretStore for SecretsFailingToRead {
    async fn get(&self, c: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        if key == self.blocked_key {
            return Err(crate::error::OpenCompanyError::Store(
                "read refused by test".into(),
            ));
        }
        self.inner.get(c, key).await
    }
    async fn set(&self, c: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        self.inner.set(c, key, value).await
    }
}

/// A store read error on the BYO override key must fail the whole
/// resolution rather than degrade to the shared brokered tier: an
/// unreadable store that silently fell through to
/// [`company_key::resolve`] would let a call be attributed to the wrong
/// account precisely when the store cannot be trusted to say which
/// account was configured.
#[tokio::test]
async fn a_store_read_error_on_the_byo_token_propagates_rather_than_falling_back() {
    let company = CompanyId::new("acme");
    let secrets = SecretsFailingToRead {
        inner: MemSecrets::default(),
        blocked_key: TINYHUMANS_KEY_KEY,
    };
    // A managed-tier credential that *would* answer if resolution fell
    // through to it — proving the error surfaces rather than that there
    // was nothing to fall back to.
    secrets
        .inner
        .set(
            &company,
            TINYHUMANS_KEY_KEY,
            SecretValue("unreachable".into()),
        )
        .await
        .unwrap();

    let err = resolve_credential(&company, &secrets, None)
        .await
        .expect_err("an unreadable secret store must not resolve to any credential");
    assert!(
        matches!(err, crate::error::OpenCompanyError::Store(_)),
        "expected the store error to propagate untouched, got {err:?}"
    );
}

// ── Storage addresses and the legacy fallback (#2306) ──────────────

#[test]
fn the_storage_addresses_are_pinned() {
    assert_eq!(TINYHUMANS_KEY_KEY, "composio/tinyhumans/key");
    assert_eq!(LEGACY_TOKEN_KEY, "composio/token");
    assert_eq!(BYOK_KEY_KEY, "composio/byok/key");
    assert_eq!(LEGACY_API_KEY_KEY, "composio/api_key");
    assert_eq!(MODE_KEY, "composio/mode");
    assert_eq!(DEFAULTS_KEY, "composio/defaults");
}

#[tokio::test]
async fn a_legacy_only_tinyhumans_key_is_still_presented() {
    use crate::company::credentials::CredentialSource;

    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company,
            LEGACY_TOKEN_KEY,
            SecretValue("th-not-a-real-key".into()),
        )
        .await
        .unwrap();

    let credential = resolve_credential(&company, &secrets, None).await.unwrap();
    assert_eq!(
        credential.current().await.unwrap().as_deref(),
        Some("th-not-a-real-key")
    );
    assert_eq!(credential.source(), CredentialSource::Static);
    assert!(token_configured(&company, &secrets).await.unwrap());
    assert_eq!(
        load_tinyhumans_key(&company, &secrets).await.unwrap(),
        Some("th-not-a-real-key".to_string())
    );
}

#[tokio::test]
async fn a_legacy_only_byok_key_is_still_presented() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
        .await
        .unwrap();
    secrets
        .set(
            &company,
            LEGACY_API_KEY_KEY,
            SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();

    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(access.mode, ComposioMode::Byok);
    assert_eq!(
        access.credential.current().await.unwrap().as_deref(),
        Some("ak-not-a-real-key")
    );
    assert_eq!(
        load_byok_key(&company, &secrets).await.unwrap(),
        Some("ak-not-a-real-key".to_string())
    );
}

#[tokio::test]
async fn the_new_address_wins_over_the_legacy_one() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company,
            TINYHUMANS_KEY_KEY,
            SecretValue("th-not-a-real-key-2".into()),
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
        .set(
            &company,
            BYOK_KEY_KEY,
            SecretValue("ak-not-a-real-key-2".into()),
        )
        .await
        .unwrap();
    secrets
        .set(
            &company,
            LEGACY_API_KEY_KEY,
            SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();

    assert_eq!(
        load_tinyhumans_key(&company, &secrets).await.unwrap(),
        Some("th-not-a-real-key-2".to_string())
    );
    assert_eq!(
        load_byok_key(&company, &secrets).await.unwrap(),
        Some("ak-not-a-real-key-2".to_string())
    );
}

#[tokio::test]
async fn a_blank_new_address_falls_back_to_a_non_empty_legacy_one() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(&company, TINYHUMANS_KEY_KEY, SecretValue("   ".into()))
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
        .set(&company, BYOK_KEY_KEY, SecretValue("".into()))
        .await
        .unwrap();
    secrets
        .set(
            &company,
            LEGACY_API_KEY_KEY,
            SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();

    assert_eq!(
        load_tinyhumans_key(&company, &secrets).await.unwrap(),
        Some("th-not-a-real-key".to_string())
    );
    assert_eq!(
        load_byok_key(&company, &secrets).await.unwrap(),
        Some("ak-not-a-real-key".to_string())
    );
}

#[tokio::test]
async fn a_byok_value_is_never_presented_as_the_tinyhumans_bearer() {
    use crate::company::credentials::CredentialSource;

    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company,
            LEGACY_API_KEY_KEY,
            SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();
    secrets
        .set(
            &company,
            BYOK_KEY_KEY,
            SecretValue("ak-not-a-real-key-2".into()),
        )
        .await
        .unwrap();

    let credential = resolve_credential(&company, &secrets, None).await.unwrap();
    assert!(!credential.configured());
    assert_eq!(credential.source(), CredentialSource::None);
    assert!(!token_configured(&company, &secrets).await.unwrap());
    assert_eq!(load_tinyhumans_key(&company, &secrets).await.unwrap(), None);
}

#[tokio::test]
async fn a_tinyhumans_value_is_never_presented_as_the_byok_key() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
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
        .set(
            &company,
            TINYHUMANS_KEY_KEY,
            SecretValue("th-not-a-real-key-2".into()),
        )
        .await
        .unwrap();

    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(access.mode, ComposioMode::Byok);
    assert!(!access.credential.configured());
    assert_eq!(load_byok_key(&company, &secrets).await.unwrap(), None);
}

#[tokio::test]
async fn a_write_mirrors_to_the_legacy_address_for_one_release() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company,
            LEGACY_TOKEN_KEY,
            SecretValue("th-not-a-real-key".into()),
        )
        .await
        .unwrap();
    secrets
        .set(
            &company,
            LEGACY_API_KEY_KEY,
            SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();

    store_token(&company, &secrets, " th-not-a-real-key-2 ")
        .await
        .unwrap();
    assert_eq!(
        raw(&secrets, &company, TINYHUMANS_KEY_KEY).await,
        "th-not-a-real-key-2"
    );
    assert_eq!(
        raw(&secrets, &company, LEGACY_TOKEN_KEY).await,
        "th-not-a-real-key-2"
    );
    assert_eq!(raw(&secrets, &company, BYOK_KEY_KEY).await, "");
    assert_eq!(
        raw(&secrets, &company, LEGACY_API_KEY_KEY).await,
        "ak-not-a-real-key",
        "the BYOK addresses are untouched by a token write"
    );

    let mode = store_api_key(&company, &secrets, "ak-not-a-real-key-2")
        .await
        .unwrap();
    assert_eq!(mode, ComposioMode::Byok);
    assert_eq!(
        raw(&secrets, &company, BYOK_KEY_KEY).await,
        "ak-not-a-real-key-2"
    );
    assert_eq!(
        raw(&secrets, &company, LEGACY_API_KEY_KEY).await,
        "ak-not-a-real-key-2"
    );
    assert_eq!(raw(&secrets, &company, MODE_KEY).await, BYOK_MODE);
    assert_eq!(
        raw(&secrets, &company, TINYHUMANS_KEY_KEY).await,
        "th-not-a-real-key-2",
        "the token addresses are untouched by an API-key write"
    );
}
