use super::*;

// The three helper tests below follow their subjects behind the `composio`
// feature: `toolkit_allowed` / `slug_toolkit` do not exist in an
// `openhuman`-without-`composio` build.
#[cfg(feature = "composio")]
#[test]
fn toolkit_allowed_empty_defers_to_backend() {
    // Empty allowlist = open mode: every toolkit admitted.
    assert!(toolkit_allowed(&[], "gmail"));
    assert!(toolkit_allowed(&[], "anything"));
}

#[cfg(feature = "composio")]
#[test]
fn toolkit_allowed_non_empty_narrows_case_insensitively() {
    let allow = vec!["gmail".to_string(), "github".to_string()];
    assert!(toolkit_allowed(&allow, "gmail"));
    assert!(toolkit_allowed(&allow, "GMAIL"));
    assert!(toolkit_allowed(&allow, "GitHub"));
    assert!(!toolkit_allowed(&allow, "slack"));
}

#[cfg(feature = "composio")]
#[test]
fn slug_toolkit_extracts_lowercased_prefix() {
    assert_eq!(slug_toolkit("GMAIL_SEND_EMAIL"), "gmail");
    assert_eq!(slug_toolkit("SLACK_POST_MESSAGE"), "slack");
    assert_eq!(slug_toolkit("GITHUB_CREATE_ISSUE"), "github");
    assert_eq!(slug_toolkit(""), "");
}

#[test]
fn debug_redacts_the_token() {
    let config = TenantComposio::new(
        "https://api.tinyhumans.ai",
        Credential::from_value("super-secret-tenant-token"),
        vec!["gmail".to_string()],
    );
    let shown = format!("{config:?}");
    assert!(
        !shown.contains("super-secret-tenant-token"),
        "token leaked: {shown}"
    );
    assert!(shown.contains("<redacted>"), "{shown}");
    assert!(shown.contains("api.tinyhumans.ai"), "{shown}");
    assert!(
        shown.contains("gmail"),
        "toolkits should be visible: {shown}"
    );
}

#[test]
fn debug_marks_unset_token() {
    let config = TenantComposio::new("https://api.tinyhumans.ai", Credential::None, Vec::new());
    let shown = format!("{config:?}");
    assert!(shown.contains("<unset>"), "{shown}");
}

/// The resolver's precedence and its fail-closed floor: the company's own
/// stored token always wins; with none stored the instance's platform token
/// source is used; with neither there is no config at all (no tools) — never a
/// borrowed identity. A raw `TINYHUMANS_API_KEY` in the environment is not a
/// source: the platform identity is passed in explicitly by the caller.
#[tokio::test]
async fn resolve_prefers_the_stored_token_then_the_token_source_then_fails_closed() {
    use crate::ports::SecretStore;
    use crate::ports::types::CompanyId;
    use crate::store::FsSecretStore;

    let dir = tempfile::Builder::new()
        .prefix("oc-composio-res-")
        .tempdir()
        .expect("tempdir");
    let secrets = FsSecretStore::new(dir.path());
    let company = CompanyId::new("acme");
    let source = || Arc::new(TinyhumansTokenSource::static_key("platform-identity"));

    // Nothing stored and no platform identity → fail closed. That an ambient
    // `TINYHUMANS_API_KEY` cannot be consulted is guaranteed by the signature
    // — `resolve` takes the source explicitly and has no `EnvSource` — so it
    // needs no proof by process-env mutation. Setting one here used to leak
    // into every other test in this binary (`std::env` is process-wide and
    // nothing restored it), which made an ops-route assertion on
    // `credentialSource == "none"` flake depending on test order.
    assert!(
        TenantComposio::resolve(&company, &secrets, Vec::new(), None, None)
            .await
            .is_none(),
        "no credential at all must fail closed"
    );

    // Nothing stored, but this instance has an identity → it is used.
    let attested = TenantComposio::resolve(&company, &secrets, Vec::new(), None, Some(source()))
        .await
        .expect("the platform identity resolves");
    assert_eq!(
        token_of(&attested).await.as_deref(),
        Some("platform-identity")
    );

    // An explicitly-empty stored token is not a token: still the source.
    secrets
        .set(&company, TINYHUMANS_KEY_KEY, SecretValue("   ".to_string()))
        .await
        .unwrap();
    let attested = TenantComposio::resolve(&company, &secrets, Vec::new(), None, Some(source()))
        .await
        .expect("the platform identity resolves");
    assert_eq!(
        token_of(&attested).await.as_deref(),
        Some("platform-identity")
    );
    // …and with no source either, an empty stored token fails closed.
    assert!(
        TenantComposio::resolve(&company, &secrets, Vec::new(), None, None)
            .await
            .is_none()
    );

    // The company's OWN token wins over the platform identity.
    secrets
        .set(
            &company,
            TINYHUMANS_KEY_KEY,
            SecretValue("tenant-token-xyz".to_string()),
        )
        .await
        .unwrap();
    let resolved = TenantComposio::resolve(
        &company,
        &secrets,
        vec!["gmail".into()],
        None,
        Some(source()),
    )
    .await
    .expect("a stored token resolves");
    assert_eq!(
        token_of(&resolved).await.as_deref(),
        Some("tenant-token-xyz"),
        "a company that brought its own token keeps it"
    );
    assert_eq!(resolved.backend_url, "https://api.tinyhumans.ai");
    assert_eq!(resolved.toolkits, vec!["gmail".to_string()]);

    // The tenant API base is threaded into the backend URL so a staging
    // tenant's Composio follows staging.
    let staged = TenantComposio::resolve(
        &company,
        &secrets,
        Vec::new(),
        Some("https://staging-api.tinyhumans.ai".into()),
        None,
    )
    .await
    .expect("a stored token resolves");
    assert_eq!(staged.backend_url, "https://staging-api.tinyhumans.ai");
}

/// Issue #586: the company's own TinyHumans key sits between its pasted
/// Composio token and the instance's identity, and it is enough on its own —
/// a company with a key set connects providers with no Composio token and no
/// per-tenant provider app.
#[tokio::test]
async fn the_company_key_credentials_composio_between_a_byo_token_and_the_instance() {
    use crate::company::credentials::CredentialSource;
    use crate::ports::SecretStore;
    use crate::ports::types::CompanyId;
    use crate::store::FsSecretStore;

    let dir = tempfile::Builder::new()
        .prefix("oc-composio-companykey-")
        .tempdir()
        .expect("tempdir");
    let secrets = FsSecretStore::new(dir.path());
    let company = CompanyId::new("acme");
    let source = || Arc::new(TinyhumansTokenSource::static_key("platform-identity"));

    company_key::store_key(&company, &secrets, "th_company_key")
        .await
        .unwrap();

    // With no instance identity at all, the company key alone credentials
    // Composio — the case this issue exists to fix, since a pod with no
    // projected token previously had to fall back to a pasted token.
    let resolved = TenantComposio::resolve(&company, &secrets, Vec::new(), None, None)
        .await
        .expect("the company key resolves without any instance identity");
    assert_eq!(token_of(&resolved).await.as_deref(), Some("th_company_key"));
    assert_eq!(resolved.credential().source(), CredentialSource::Company);

    // And it outranks the instance's identity: the company acts as itself,
    // not as the pod it happens to run in.
    let resolved = TenantComposio::resolve(&company, &secrets, Vec::new(), None, Some(source()))
        .await
        .expect("resolves");
    assert_eq!(token_of(&resolved).await.as_deref(), Some("th_company_key"));

    // A pasted Composio token still outranks it — the BYO hatch survives.
    secrets
        .set(
            &company,
            TINYHUMANS_KEY_KEY,
            SecretValue("byo-composio".to_string()),
        )
        .await
        .unwrap();
    let resolved = TenantComposio::resolve(&company, &secrets, Vec::new(), None, Some(source()))
        .await
        .expect("resolves");
    assert_eq!(token_of(&resolved).await.as_deref(), Some("byo-composio"));
    assert_eq!(resolved.credential().source(), CredentialSource::Static);

    // Clearing the BYO token falls back to the company key, not to the
    // instance — clearing one tier must not silently re-borrow another.
    secrets
        .set(&company, TINYHUMANS_KEY_KEY, SecretValue(String::new()))
        .await
        .unwrap();
    // Also blank the legacy address explicitly (P2-3): a whitespace-only
    // `composio/token` must not itself be read as "the legacy address
    // holds a value" and shadow the company key — it must fall through
    // exactly as an unwritten legacy address does.
    secrets
        .set(
            &company,
            crate::company::composio::LEGACY_TOKEN_KEY,
            SecretValue("   ".to_string()),
        )
        .await
        .unwrap();
    let resolved = TenantComposio::resolve(&company, &secrets, Vec::new(), None, Some(source()))
        .await
        .expect("resolves");
    assert_eq!(token_of(&resolved).await.as_deref(), Some("th_company_key"));
    assert_eq!(resolved.credential().source(), CredentialSource::Company);

    // Clearing the company key too falls all the way back to the instance.
    company_key::store_key(&company, &secrets, "")
        .await
        .unwrap();
    let resolved = TenantComposio::resolve(&company, &secrets, Vec::new(), None, Some(source()))
        .await
        .expect("resolves");
    assert_eq!(
        token_of(&resolved).await.as_deref(),
        Some("platform-identity")
    );
}

/// Storage addresses and the legacy fallback (#2306), exercised against a
/// real backend so the nested address (`composio/tinyhumans/key`) is proven
/// on disk, not just in `MemSecrets`.
#[tokio::test]
async fn a_legacy_only_token_still_wires_managed_composio() {
    use crate::company::credentials::CredentialSource;
    use crate::ports::SecretStore;
    use crate::ports::types::CompanyId;
    use crate::store::FsSecretStore;

    let dir = tempfile::Builder::new()
        .prefix("oc-composio-legacy-token-")
        .tempdir()
        .expect("tempdir");
    let secrets = FsSecretStore::new(dir.path());
    let company = CompanyId::new("acme");
    secrets
        .set(
            &company,
            crate::company::composio::LEGACY_TOKEN_KEY,
            SecretValue("th-not-a-real-key".into()),
        )
        .await
        .unwrap();

    let resolved = TenantComposio::resolve(&company, &secrets, Vec::new(), None, None)
        .await
        .expect("a legacy-only token still resolves");
    assert_eq!(
        token_of(&resolved).await.as_deref(),
        Some("th-not-a-real-key")
    );
    assert_eq!(resolved.credential().source(), CredentialSource::Static);
}

#[tokio::test]
async fn a_legacy_only_byok_key_still_resolves_the_byok_route() {
    use crate::company::composio::{BYOK_MODE, MODE_KEY};
    use crate::ports::SecretStore;
    use crate::ports::types::CompanyId;
    use crate::store::FsSecretStore;

    let dir = tempfile::Builder::new()
        .prefix("oc-composio-legacy-byok-")
        .tempdir()
        .expect("tempdir");
    let secrets = FsSecretStore::new(dir.path());
    let company = CompanyId::new("acme");
    secrets
        .set(&company, MODE_KEY, SecretValue(BYOK_MODE.into()))
        .await
        .unwrap();
    secrets
        .set(
            &company,
            crate::company::composio::LEGACY_API_KEY_KEY,
            SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();

    let resolved = TenantComposio::resolve(&company, &secrets, Vec::new(), None, None)
        .await
        .expect("a legacy-only BYOK key still resolves");
    assert_eq!(resolved.mode(), ComposioMode::Byok);
    assert_eq!(
        token_of(&resolved).await.as_deref(),
        Some("ak-not-a-real-key")
    );

    crate::company::composio::store_api_key(&company, &secrets, "ak-not-a-real-key-2")
        .await
        .unwrap();
    assert_eq!(
        secrets
            .get(&company, BYOK_KEY_KEY)
            .await
            .unwrap()
            .map(|SecretValue(v)| v)
            .as_deref(),
        Some("ak-not-a-real-key-2")
    );
    assert_eq!(
        secrets
            .get(&company, crate::company::composio::LEGACY_API_KEY_KEY)
            .await
            .unwrap()
            .map(|SecretValue(v)| v)
            .as_deref(),
        Some("ak-not-a-real-key-2")
    );
}

/// The roster path's half of the store-error contract: it must **fail
/// closed** — no tools this cycle — rather than fall through to the
/// instance identity or bubble and brick the build.
///
/// Fewer tools for a cycle is recoverable and visible. Silently presenting
/// a different identity is neither: it would attribute whatever the agents
/// did in that window to the wrong account.
#[tokio::test]
async fn an_unreadable_store_withholds_tools_rather_than_borrowing_an_identity() {
    use crate::ports::types::CompanyId;

    struct BrokenSecrets;

    #[async_trait::async_trait]
    impl SecretStore for BrokenSecrets {
        async fn get(
            &self,
            _c: &CompanyId,
            _key: &str,
        ) -> crate::Result<Option<crate::ports::types::SecretValue>> {
            Err(crate::error::OpenCompanyError::Store("boom".into()))
        }
        async fn set(
            &self,
            _c: &CompanyId,
            _key: &str,
            _v: crate::ports::types::SecretValue,
        ) -> crate::Result<()> {
            Err(crate::error::OpenCompanyError::Store("boom".into()))
        }
    }

    let company = CompanyId::new("acme");
    let resolved = TenantComposio::resolve(
        &company,
        &BrokenSecrets,
        Vec::new(),
        None,
        // An instance identity IS available — and must still not be used,
        // because we cannot tell whether this company has a key of its own.
        Some(Arc::new(TinyhumansTokenSource::static_key(
            "platform-identity",
        ))),
    )
    .await;
    assert!(
        resolved.is_none(),
        "an unreadable store must withhold the tools, not present the instance's identity"
    );
}

/// The rotation guarantee (issue #586 acceptance): rotating the company key
/// moves the roster fingerprint, so agents cannot be left on the previous
/// credential after a console rotation.
#[tokio::test]
async fn rotating_the_company_key_moves_the_composio_fingerprint() {
    use crate::ports::types::CompanyId;
    use crate::store::FsSecretStore;

    let dir = tempfile::Builder::new()
        .prefix("oc-composio-rotate-")
        .tempdir()
        .expect("tempdir");
    let secrets = FsSecretStore::new(dir.path());
    let company = CompanyId::new("acme");

    let resolve =
        async || TenantComposio::resolve(&company, &secrets, Vec::new(), None, None).await;

    company_key::store_key(&company, &secrets, "key-a")
        .await
        .unwrap();
    let before = TenantComposio::fingerprint(&resolve().await);

    company_key::store_key(&company, &secrets, "key-b")
        .await
        .unwrap();
    let after = TenantComposio::fingerprint(&resolve().await);
    assert_ne!(
        before, after,
        "a rotated company key must rebuild the roster, or agents keep the old credential"
    );

    // And clearing it is a change too — the roster must drop the tools.
    company_key::store_key(&company, &secrets, "")
        .await
        .unwrap();
    assert_eq!(
        TenantComposio::fingerprint(&resolve().await),
        TenantComposio::fingerprint(&None),
        "a cleared credential resolves to nothing, so no tools are wired"
    );
}
