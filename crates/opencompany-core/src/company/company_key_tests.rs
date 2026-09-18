use super::*;
use crate::company::credentials::CredentialSource;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

#[derive(Default)]
struct MemSecrets {
    map: Mutex<HashMap<String, String>>,
}

#[async_trait]
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

/// A store whose reads always fail — the transient-hiccup case.
struct BrokenSecrets;

#[async_trait]
impl SecretStore for BrokenSecrets {
    async fn get(&self, _c: &CompanyId, _key: &str) -> Result<Option<SecretValue>> {
        Err(crate::error::OpenCompanyError::Store("boom".into()))
    }
    async fn set(&self, _c: &CompanyId, _key: &str, _value: SecretValue) -> Result<()> {
        Err(crate::error::OpenCompanyError::Store("boom".into()))
    }
}

fn source() -> Arc<TinyhumansTokenSource> {
    Arc::new(TinyhumansTokenSource::static_key("instance-identity"))
}

#[tokio::test]
async fn the_company_key_outranks_the_instance_identity() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    // Nothing set: the tenant borrows the instance's identity, exactly as
    // it did before this seam existed.
    let credential = resolve(&company, &secrets, Some(source())).await.unwrap();
    assert_eq!(credential.source(), CredentialSource::Static);
    assert_eq!(
        credential.current().await.unwrap().as_deref(),
        Some("instance-identity")
    );

    // Admin sets the company's own key: every brokered call now presents it.
    store_key(&company, &secrets, "th_company_key")
        .await
        .unwrap();
    let credential = resolve(&company, &secrets, Some(source())).await.unwrap();
    assert_eq!(credential.source(), CredentialSource::Company);
    assert_eq!(
        credential.current().await.unwrap().as_deref(),
        Some("th_company_key")
    );
}

#[tokio::test]
async fn no_key_and_no_instance_identity_fails_closed() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let credential = resolve(&company, &secrets, None).await.unwrap();
    assert_eq!(credential.source(), CredentialSource::None);
    assert!(!credential.configured());
}

#[tokio::test]
async fn clearing_the_key_falls_back_rather_than_stranding_the_company() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    store_key(&company, &secrets, "th_company_key")
        .await
        .unwrap();
    assert!(key_configured(&company, &secrets).await.unwrap());

    // An empty value is a clear. The store has no delete, so this is how
    // "unset" reads back.
    store_key(&company, &secrets, "").await.unwrap();
    assert!(!key_configured(&company, &secrets).await.unwrap());
    assert_eq!(
        resolve(&company, &secrets, Some(source()))
            .await
            .unwrap()
            .source(),
        CredentialSource::Static
    );
}

#[tokio::test]
async fn a_blank_key_is_not_configuration() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    store_key(&company, &secrets, "   ").await.unwrap();
    assert!(!key_configured(&company, &secrets).await.unwrap());
    assert_eq!(
        resolve(&company, &secrets, None).await.unwrap().source(),
        CredentialSource::None
    );
}

/// A store read that fails must **not** silently resolve to the instance
/// identity.
///
/// The tempting reading is "degrade rather than brick the roster build", and
/// that is the right instinct about availability — but it is the wrong
/// answer about attribution. A connection lives on the backend keyed by the
/// account the bearer resolves to, so a company that *has* a key silently
/// borrowing the instance's identity during a hiccup would attribute its
/// Gmail to the wrong account, and the operator would get no signal in
/// either direction. The error surfaces here; each caller then decides what
/// its own surface can afford.
#[tokio::test]
async fn an_unreadable_store_never_borrows_another_identity() {
    let company = CompanyId::new("acme");

    assert!(
        key_configured(&company, &BrokenSecrets).await.is_err(),
        "an unreadable store is not the same answer as `not configured`"
    );
    assert!(
        resolve(&company, &BrokenSecrets, Some(source()))
            .await
            .is_err(),
        "an unreadable store must never resolve to the instance identity"
    );

    // And an *absent* key still falls through as it always did — the two
    // cases are now distinguishable, which is the whole point.
    let secrets = MemSecrets::default();
    assert_eq!(
        resolve(&company, &secrets, Some(source()))
            .await
            .unwrap()
            .current()
            .await
            .unwrap()
            .as_deref(),
        Some("instance-identity")
    );
}

/// The rotation contract (issue #586 acceptance): a rotated company key is a
/// new identity, so anything fingerprinting the credential rebuilds.
#[tokio::test]
async fn rotating_the_key_moves_the_fingerprint() {
    use std::hash::Hasher;

    let identity_of = |credential: &Credential| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        credential.hash_identity(&mut hasher);
        hasher.finish()
    };

    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    store_key(&company, &secrets, "key-a").await.unwrap();
    let before = identity_of(&resolve(&company, &secrets, Some(source())).await.unwrap());

    store_key(&company, &secrets, "key-b").await.unwrap();
    let after = identity_of(&resolve(&company, &secrets, Some(source())).await.unwrap());
    assert_ne!(
        before, after,
        "a rotated company key must rebuild the roster"
    );

    // And a company key is never confused with an identically-valued
    // per-provider token pasted into some other slot.
    assert_ne!(
        identity_of(&Credential::from_company_key("key-b")),
        identity_of(&Credential::from_value("key-b"))
    );
}

#[tokio::test]
async fn the_key_is_write_only_in_every_rendering() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    store_key(&company, &secrets, "th_super_secret")
        .await
        .unwrap();
    let credential = resolve(&company, &secrets, None).await.unwrap();
    let rendered = format!("{credential:?}");
    assert!(!rendered.contains("th_super_secret"), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");
}
