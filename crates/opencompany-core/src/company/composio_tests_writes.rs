//! Token-write and dual-address-mirroring tests: clears, legacy-mirror
//! failures, and read/write isolation between the two addresses (split
//! out of `composio_tests.rs`).

use super::tests_addresses::SecretsFailingToRead;
use super::tests_pins::{MemSecrets, SecretsFailingToWrite, raw};
use super::*;

#[tokio::test]
async fn clearing_the_token_clears_both_addresses() {
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

    store_token(&company, &secrets, "").await.unwrap();
    assert_eq!(raw(&secrets, &company, TINYHUMANS_KEY_KEY).await, "");
    assert_eq!(raw(&secrets, &company, LEGACY_TOKEN_KEY).await, "");
    assert!(!token_configured(&company, &secrets).await.unwrap());
    let credential = resolve_credential(&company, &secrets, None).await.unwrap();
    assert!(!credential.configured());
}

/// A blank new-address token AND a blank (whitespace-only) legacy address
/// must fall all the way through to the company's own TinyHumans key
/// (`company_key::resolve`) — never read as "not configured" and never
/// cross the two address pairs (P2-3). The legacy value is written as
/// whitespace rather than left absent, so this also proves trimming: an
/// unwritten slot and a whitespace-only one must resolve identically.
#[tokio::test]
async fn blank_new_and_blank_legacy_addresses_fall_through_to_the_company_key() {
    use crate::company::credentials::CredentialSource;

    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(&company, TINYHUMANS_KEY_KEY, SecretValue("".into()))
        .await
        .unwrap();
    secrets
        .set(&company, LEGACY_TOKEN_KEY, SecretValue("  ".into()))
        .await
        .unwrap();
    company_key::store_key(&company, &secrets, "th-not-a-real-company-key")
        .await
        .unwrap();

    let credential = resolve_credential(
        &company,
        &secrets,
        Some(Arc::new(TinyhumansTokenSource::static_key(
            "th-not-a-real-platform-identity",
        ))),
    )
    .await
    .unwrap();
    assert!(credential.configured());
    assert_eq!(credential.source(), CredentialSource::Company);
    assert_eq!(
        credential.current().await.unwrap().as_deref(),
        Some("th-not-a-real-company-key"),
        "the company key must win over the platform identity, exactly as \
         company_key::resolve's own precedence says"
    );
}

/// Lands `store_token` exactly at its second write (the legacy mirror)
/// and leaves the first write's result inspectable. `store_api_key`'s
/// equivalent failure is already covered by
/// `a_byok_set_whose_legacy_mirror_fails_leaves_a_managed_company_managed`
/// below, so it is not duplicated here.
#[tokio::test]
async fn a_failed_legacy_mirror_write_propagates() {
    let secrets = SecretsFailingToWrite {
        inner: MemSecrets::default(),
        blocked_key: LEGACY_TOKEN_KEY,
    };
    let company = CompanyId::new("acme");
    secrets
        .inner
        .set(
            &company,
            LEGACY_TOKEN_KEY,
            SecretValue("th-not-a-real-key".into()),
        )
        .await
        .unwrap();

    let err = store_token(&company, &secrets, "th-not-a-real-key-2").await;
    assert!(
        matches!(err, Err(crate::error::OpenCompanyError::Store(_))),
        "{err:?}"
    );
    assert_eq!(
        raw(&secrets.inner, &company, TINYHUMANS_KEY_KEY).await,
        "th-not-a-real-key-2",
        "the new address is written first and keeps the new value"
    );
    assert_eq!(
        raw(&secrets.inner, &company, LEGACY_TOKEN_KEY).await,
        "th-not-a-real-key"
    );
    assert_eq!(
        load_tinyhumans_key(&company, &secrets).await.unwrap(),
        Some("th-not-a-real-key-2".to_string())
    );
}

#[tokio::test]
async fn a_failed_legacy_clear_keeps_the_old_value_readable() {
    let secrets = SecretsFailingToWrite {
        inner: MemSecrets::default(),
        blocked_key: LEGACY_TOKEN_KEY,
    };
    let company = CompanyId::new("acme");
    secrets
        .inner
        .set(
            &company,
            TINYHUMANS_KEY_KEY,
            SecretValue("th-not-a-real-key-2".into()),
        )
        .await
        .unwrap();
    secrets
        .inner
        .set(
            &company,
            LEGACY_TOKEN_KEY,
            SecretValue("th-not-a-real-key".into()),
        )
        .await
        .unwrap();

    let err = store_token(&company, &secrets, "").await;
    assert!(err.is_err());
    assert_eq!(
        load_tinyhumans_key(&company, &secrets).await.unwrap(),
        Some("th-not-a-real-key".to_string()),
        "the legacy address still holds the pre-clear value until a retry"
    );
}

#[tokio::test]
async fn clearing_the_byok_key_clears_both_addresses() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
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

    let mode = store_api_key(&company, &secrets, "").await.unwrap();
    assert_eq!(mode, ComposioMode::Managed);
    assert_eq!(raw(&secrets, &company, BYOK_KEY_KEY).await, "");
    assert_eq!(raw(&secrets, &company, LEGACY_API_KEY_KEY).await, "");
    assert_eq!(raw(&secrets, &company, MODE_KEY).await, MANAGED_MODE);
}

#[tokio::test]
async fn byok_with_blank_new_and_blank_legacy_keys_withholds_tools() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    secrets
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
        .await
        .unwrap();
    secrets
        .set(&company, BYOK_KEY_KEY, SecretValue("".into()))
        .await
        .unwrap();
    secrets
        .set(&company, LEGACY_API_KEY_KEY, SecretValue("  ".into()))
        .await
        .unwrap();
    secrets
        .set(
            &company,
            TINYHUMANS_KEY_KEY,
            SecretValue("th-not-a-real-key".into()),
        )
        .await
        .unwrap();

    let access = resolve_access(
        &company,
        &secrets,
        Some(Arc::new(TinyhumansTokenSource::static_key(
            "platform-identity",
        ))),
    )
    .await
    .unwrap();
    assert_eq!(access.mode, ComposioMode::Byok);
    assert!(!access.credential.configured());
}

#[tokio::test]
async fn a_byok_set_whose_legacy_mirror_fails_leaves_a_managed_company_managed() {
    let secrets = SecretsFailingToWrite {
        inner: MemSecrets::default(),
        blocked_key: LEGACY_API_KEY_KEY,
    };
    let company = CompanyId::new("acme");
    secrets
        .inner
        .set(
            &company,
            LEGACY_API_KEY_KEY,
            SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();

    let err = store_api_key(&company, &secrets, "ak-not-a-real-key-2").await;
    assert!(err.is_err());
    assert_eq!(
        load_mode(&company, &secrets).await.unwrap(),
        ComposioMode::Managed
    );
    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(access.mode, ComposioMode::Managed);
    assert_eq!(
        raw(&secrets.inner, &company, MODE_KEY).await,
        "",
        "the mode write never ran"
    );
    assert_eq!(
        raw(&secrets.inner, &company, BYOK_KEY_KEY).await,
        "ak-not-a-real-key-2",
        "inert: written but never selected"
    );
}

#[tokio::test]
async fn a_byok_clear_whose_legacy_clear_fails_still_lands_managed() {
    let secrets = SecretsFailingToWrite {
        inner: MemSecrets::default(),
        blocked_key: LEGACY_API_KEY_KEY,
    };
    let company = CompanyId::new("acme");
    secrets
        .inner
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
        .await
        .unwrap();
    secrets
        .inner
        .set(
            &company,
            BYOK_KEY_KEY,
            SecretValue("ak-not-a-real-key-2".into()),
        )
        .await
        .unwrap();
    secrets
        .inner
        .set(
            &company,
            LEGACY_API_KEY_KEY,
            SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();

    let err = store_api_key(&company, &secrets, "").await;
    assert!(err.is_err());
    assert_eq!(
        load_mode(&company, &secrets).await.unwrap(),
        ComposioMode::Managed
    );
    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(access.mode, ComposioMode::Managed);
    assert_eq!(raw(&secrets.inner, &company, BYOK_KEY_KEY).await, "");
    assert_eq!(
        raw(&secrets.inner, &company, LEGACY_API_KEY_KEY).await,
        "ak-not-a-real-key",
        "inert: the mode already says managed"
    );
}

/// A read error on the legacy address must fail resolution outright — never
/// fall through to [`company_key::resolve`], which would let an unreadable
/// store silently change which account a call is attributed to.
#[tokio::test]
async fn a_store_read_error_on_the_legacy_token_propagates() {
    let secrets = SecretsFailingToRead {
        inner: MemSecrets::default(),
        blocked_key: LEGACY_TOKEN_KEY,
    };
    let company = CompanyId::new("acme");

    let err = resolve_credential(&company, &secrets, None)
        .await
        .expect_err("an unreadable legacy address must not resolve to any credential");
    assert!(matches!(err, crate::error::OpenCompanyError::Store(_)));
}

#[tokio::test]
async fn a_non_empty_new_address_does_not_read_the_legacy_one() {
    let secrets = SecretsFailingToRead {
        inner: MemSecrets::default(),
        blocked_key: LEGACY_API_KEY_KEY,
    };
    let company = CompanyId::new("acme");
    secrets
        .inner
        .set(
            &company,
            BYOK_KEY_KEY,
            SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();
    secrets
        .inner
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
        .await
        .unwrap();

    let access = resolve_access(&company, &secrets, None).await.unwrap();
    assert_eq!(
        access.credential.current().await.unwrap().as_deref(),
        Some("ak-not-a-real-key")
    );
}

#[tokio::test]
async fn reading_never_writes_either_address() {
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

    let _ = resolve_access(&company, &secrets, None).await.unwrap();
    let _ = resolve_credential(&company, &secrets, None).await.unwrap();
    let _ = token_configured(&company, &secrets).await.unwrap();
    let _ = load_tinyhumans_key(&company, &secrets).await.unwrap();
    let _ = load_byok_key(&company, &secrets).await.unwrap();

    assert!(
        secrets
            .get(&company, TINYHUMANS_KEY_KEY)
            .await
            .unwrap()
            .is_none(),
        "a read must never write the new address, not even as an empty value"
    );
    assert!(secrets.get(&company, BYOK_KEY_KEY).await.unwrap().is_none());
    assert_eq!(
        raw(&secrets, &company, LEGACY_TOKEN_KEY).await,
        "th-not-a-real-key"
    );
    assert_eq!(
        raw(&secrets, &company, LEGACY_API_KEY_KEY).await,
        "ak-not-a-real-key"
    );
}
