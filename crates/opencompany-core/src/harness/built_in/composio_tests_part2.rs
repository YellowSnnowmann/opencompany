use super::*;

/// The rotation contract: a projected platform token whose bytes change every
/// few minutes must NOT move the roster fingerprint, or every agent's tool
/// roster is rebuilt on the cluster's rotation schedule. A tier change or a
/// changed *stored* token must still move it.
#[tokio::test]
async fn fingerprint_is_stable_across_a_projected_rotation() {
    let dir = tempfile::Builder::new()
        .prefix("oc-composio-fp-")
        .tempdir()
        .expect("tempdir");
    let path = dir.path().join("token");
    std::fs::write(&path, "token-before").unwrap();

    let projected = config_with(Credential::from_source(Arc::new(
        TinyhumansTokenSource::projected_file(&path),
    )));
    let before = TenantComposio::fingerprint(&projected);
    assert_eq!(
        token_of(projected.as_ref().unwrap()).await.as_deref(),
        Some("token-before")
    );

    // The kubelet rewrites the file in place.
    std::fs::write(&path, "token-after").unwrap();
    assert_eq!(
        token_of(projected.as_ref().unwrap()).await.as_deref(),
        Some("token-after"),
        "the call must pick up the rotated token"
    );
    assert_eq!(
        TenantComposio::fingerprint(&projected),
        before,
        "a rotation must NOT rebuild the roster"
    );

    // A different projected path is a different identity.
    let other = config_with(Credential::from_source(Arc::new(
        TinyhumansTokenSource::projected_file(dir.path().join("other")),
    )));
    assert_ne!(TenantComposio::fingerprint(&other), before);
}

#[test]
fn fingerprint_moves_on_tier_and_stored_value_changes() {
    let a = config_with(Credential::from_value("token-a"));
    let b = config_with(Credential::from_value("token-b"));
    assert_ne!(
        TenantComposio::fingerprint(&a),
        TenantComposio::fingerprint(&b),
        "a rotated stored token must move the fingerprint"
    );
    assert_ne!(
        TenantComposio::fingerprint(&a),
        TenantComposio::fingerprint(&None),
        "None (fail-closed) must differ from a configured tenant"
    );
    assert_eq!(
        TenantComposio::fingerprint(&a),
        TenantComposio::fingerprint(&a.clone()),
        "the same config fingerprints stably"
    );

    // Swapping a stored token for the platform identity is a tier change.
    let attested = config_with(Credential::from_source(Arc::new(
        TinyhumansTokenSource::projected_file("/var/run/secrets/tinyhumans.ai/token"),
    )));
    assert_ne!(
        TenantComposio::fingerprint(&attested),
        TenantComposio::fingerprint(&a),
        "a tier change must move the fingerprint"
    );
}

/// Switching routes is an identity change even when the credential's bytes
/// do not move: the same string means a different Composio account
/// depending on which host it is presented to, so the roster has to rebuild
/// or the agents keep calling the account the company just left.
#[test]
fn fingerprint_moves_when_the_route_changes() {
    let managed = config_with(Credential::from_value("same-bytes"));
    let byok = Some(TenantComposio::from_access(
        "https://api.tinyhumans.ai",
        crate::company::composio::ComposioAccess {
            mode: ComposioMode::Byok,
            credential: Credential::from_value("same-bytes"),
        },
        vec!["gmail".to_string()],
    ));
    assert_ne!(
        TenantComposio::fingerprint(&managed),
        TenantComposio::fingerprint(&byok),
        "managed and BYOK must not fingerprint alike"
    );
}

/// Under BYOK the config carries a *second* live credential — the
/// managed-chain bearer `list_toolkits` fetches OpenHuman's curated catalog
/// with. Rotating a company's TinyHumans key while BYOK is active changes
/// neither `mode` nor the Composio `credential`, so this bearer is the only
/// thing that moves; if the fingerprint did not cover it, the roster would
/// keep the stale one until some unrelated change happened to rebuild it,
/// and the curated fetch would keep failing on a bearer the operator
/// already rotated away from.
#[test]
fn fingerprint_moves_when_the_catalog_credential_rotates() {
    let byok_with = |catalog: Credential| {
        Some(
            TenantComposio::from_access(
                "https://api.tinyhumans.ai",
                crate::company::composio::ComposioAccess {
                    mode: ComposioMode::Byok,
                    credential: Credential::from_value("ak_live"),
                },
                vec!["gmail".to_string()],
            )
            .with_catalog_credential(catalog),
        )
    };
    let before = byok_with(Credential::from_value("th-company-a"));
    let after = byok_with(Credential::from_value("th-company-b"));
    assert_ne!(
        TenantComposio::fingerprint(&before),
        TenantComposio::fingerprint(&after),
        "rotating the catalog bearer alone must still rebuild the roster"
    );

    // A managed config's `catalog` is always `Credential::None` (see
    // `TenantComposio::new`), so this must be a genuine no-op there rather
    // than a source of spurious rebuilds on every managed roster build.
    let managed_a = config_with(Credential::from_value("same-bytes"));
    let managed_b = config_with(Credential::from_value("same-bytes"));
    assert_eq!(
        TenantComposio::fingerprint(&managed_a),
        TenantComposio::fingerprint(&managed_b),
        "an always-None catalog credential must not itself vary the managed fingerprint"
    );
}

/// The endpoint a config reports is the host it dials — under BYOK that is
/// Composio itself, whatever managed backend URL was resolved alongside it.
#[test]
fn a_byok_config_reports_composios_own_host() {
    let managed = TenantComposio::new(
        "https://api.tinyhumans.ai",
        Credential::from_value("k"),
        vec![],
    );
    assert_eq!(managed.endpoint(), "https://api.tinyhumans.ai");
    assert_eq!(managed.mode(), ComposioMode::Managed);

    let byok = TenantComposio::from_access(
        "https://api.tinyhumans.ai",
        crate::company::composio::ComposioAccess {
            mode: ComposioMode::Byok,
            credential: Credential::from_value("k"),
        },
        vec![],
    );
    assert_eq!(byok.endpoint(), DIRECT_BASE_URL);
    assert_eq!(byok.mode(), ComposioMode::Byok);
}

/// The hazard `from_access` exists to make unrepresentable: a BYOK
/// credential is a **Composio** key, and pairing it with the managed route
/// would send it to `api.tinyhumans.ai` as a bearer. Building from the
/// resolved pair carries the route with the credential, so the two cannot
/// be separated by a caller that forgets.
#[test]
fn a_byok_credential_cannot_be_built_onto_the_managed_route() {
    let config = TenantComposio::from_access(
        "https://api.tinyhumans.ai",
        crate::company::composio::ComposioAccess {
            mode: ComposioMode::Byok,
            credential: Credential::from_value("ak_live"),
        },
        vec![],
    );
    assert_eq!(config.mode(), ComposioMode::Byok);
    assert_eq!(
        config.endpoint(),
        DIRECT_BASE_URL,
        "the key must be presented to Composio, never to the managed backend"
    );
}

/// A BYOK company still resolves the managed chain — not to act through, but
/// to ask OpenHuman which providers to offer. The two credentials are kept
/// apart: the Composio key is what calls present, the managed bearer is only
/// ever the curated list's.
#[tokio::test]
async fn byok_keeps_the_managed_credential_for_the_curated_catalog_only() {
    use crate::company::company_key;
    use crate::company::composio::store_api_key;
    use crate::store::FsSecretStore;

    let dir = tempfile::Builder::new()
        .prefix("oc-composio-catalog-")
        .tempdir()
        .expect("tempdir");
    let secrets = FsSecretStore::new(dir.path());
    let company = CompanyId::new("acme");

    company_key::store_key(&company, &secrets, "th_company")
        .await
        .unwrap();
    store_api_key(&company, &secrets, "ak_live").await.unwrap();

    let config = TenantComposio::resolve(&company, &secrets, vec![], None, None)
        .await
        .expect("a BYOK company has a config");

    assert_eq!(config.mode(), ComposioMode::Byok);
    assert_eq!(
        config.current_token().await.unwrap().as_deref(),
        Some("ak_live"),
        "calls present the company's own Composio key"
    );
    assert_eq!(
        config.catalog_token().await.unwrap().as_deref(),
        Some("th_company"),
        "the curated list is fetched with the managed credential, not the Composio key"
    );
}

/// With no managed tier at all — a standalone host carrying no TinyHumans
/// identity — there is no curated list to fetch, and the config says so
/// rather than presenting the Composio key to the OpenHuman backend.
#[tokio::test]
async fn byok_without_a_managed_tier_has_no_curated_catalog_credential() {
    use crate::company::composio::store_api_key;
    use crate::store::FsSecretStore;

    let dir = tempfile::Builder::new()
        .prefix("oc-composio-standalone-")
        .tempdir()
        .expect("tempdir");
    let secrets = FsSecretStore::new(dir.path());
    let company = CompanyId::new("acme");
    store_api_key(&company, &secrets, "ak_live").await.unwrap();

    let config = TenantComposio::resolve(&company, &secrets, vec![], None, None)
        .await
        .expect("a BYOK company has a config");
    assert_eq!(
        config.current_token().await.unwrap().as_deref(),
        Some("ak_live")
    );
    assert!(
        config.catalog_token().await.unwrap().is_none(),
        "no managed tier means no curated list — never the Composio key standing in for one"
    );
}

/// The roster path honours the stored route: a company that brought its own
/// Composio account resolves to a BYOK config carrying that key, and one
/// that selected BYOK without storing a key resolves to **no tools** rather
/// than to the platform identity standing in for it.
#[tokio::test]
async fn resolve_follows_the_stored_route() {
    use crate::company::composio::{BYOK_MODE, MODE_KEY, store_api_key};
    use crate::store::FsSecretStore;

    let dir = tempfile::Builder::new()
        .prefix("oc-composio-byok-")
        .tempdir()
        .expect("tempdir");
    let secrets = FsSecretStore::new(dir.path());
    let company = CompanyId::new("acme");
    store_api_key(&company, &secrets, "ak_live").await.unwrap();

    let config = TenantComposio::resolve(&company, &secrets, vec![], None, None)
        .await
        .expect("a BYOK company has a config");
    assert_eq!(config.mode(), ComposioMode::Byok);
    assert_eq!(
        config.current_token().await.unwrap().as_deref(),
        Some("ak_live")
    );

    // BYOK selected with nothing stored: fail closed.
    let bare = CompanyId::new("bare");
    secrets
        .set(&bare, MODE_KEY, SecretValue(BYOK_MODE.into()))
        .await
        .unwrap();
    assert!(
        TenantComposio::resolve(&bare, &secrets, vec![], None, None)
            .await
            .is_none(),
        "an operator who asked for their own account must never silently get the platform's"
    );
}
