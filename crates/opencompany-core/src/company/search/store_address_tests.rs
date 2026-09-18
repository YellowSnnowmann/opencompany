//! The store, against an in-memory port: address handling and index encoding
//! compatibility.
//!
//! Endpoint round-tripping through the index row and per-slug key, address
//! precedence, and reading index blobs written by earlier code shapes.

use super::*;

#[derive(Default)]
struct MemSecrets {
    map: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[async_trait::async_trait]
impl SecretStore for MemSecrets {
    async fn get(&self, _company: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(key)
            .map(|value| SecretValue(value.clone())))
    }
    async fn set(&self, _company: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.0);
        Ok(())
    }
}

fn company() -> CompanyId {
    CompanyId::new("acme")
}

async fn seed(secrets: &MemSecrets, pairs: &[(&str, &str)]) {
    for (key, value) in pairs {
        secrets
            .set(&company(), key, SecretValue((*value).to_string()))
            .await
            .expect("seed");
    }
}

fn stored_index(secrets: &MemSecrets) -> serde_json::Value {
    let raw = secrets
        .map
        .lock()
        .unwrap()
        .get(PROVIDER_INDEX_KEY)
        .cloned()
        .expect("index written");
    serde_json::from_str(&raw).expect("index is JSON")
}

#[tokio::test]
async fn a_blank_address_in_the_index_row_reads_as_absent() {
    let secrets = MemSecrets::default();
    seed(
        &secrets,
        &[
            (
                PROVIDER_INDEX_KEY,
                r#"[{"slug":"searxng","enabled":true,"endpoint":"  "}]"#,
            ),
            (
                &provider_endpoint_key("searxng"),
                "http://slug.acme.internal",
            ),
        ],
    )
    .await;

    let providers = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(
        providers[0].endpoint.as_deref(),
        Some("http://slug.acme.internal")
    );
}

#[tokio::test]
async fn re_addressing_writes_the_index_row_and_the_per_slug_key() {
    let secrets = MemSecrets::default();
    put_provider(
        &company(),
        &secrets,
        SearchProvider {
            slug: "searxng".to_string(),
            enabled: true,
            endpoint: Some("http://old.acme.internal".to_string()),
        },
    )
    .await
    .unwrap();

    assert!(
        update_endpoint_if_present(
            &company(),
            &secrets,
            "searxng",
            Some("http://new.acme.internal".to_string()),
        )
        .await
        .unwrap()
    );

    assert_eq!(
        stored_index(&secrets)[0]["endpoint"],
        "http://new.acme.internal"
    );
    assert_eq!(
        secrets
            .map
            .lock()
            .unwrap()
            .get(&provider_endpoint_key("searxng"))
            .cloned(),
        Some("http://new.acme.internal".to_string())
    );
}

#[tokio::test]
async fn an_omitted_address_survives_toggle_select_reconnect_and_a_second_provider() {
    let secrets = MemSecrets::default();
    put_provider(
        &company(),
        &secrets,
        SearchProvider {
            slug: "searxng".to_string(),
            enabled: true,
            endpoint: Some("http://kept.acme.internal".to_string()),
        },
    )
    .await
    .unwrap();
    // Blank the per-slug key so only the index row can be the source of the
    // address below — otherwise the per-slug fallback would hide a broken merge
    // and every assertion here would pass for the wrong reason.
    seed(&secrets, &[(&provider_endpoint_key("searxng"), "")]).await;

    async fn assert_kept(secrets: &MemSecrets) {
        let providers = list_providers(&company(), secrets).await.unwrap();
        let searxng = providers
            .iter()
            .find(|provider| provider.slug == "searxng")
            .expect("searxng row");
        assert_eq!(
            searxng.endpoint.as_deref(),
            Some("http://kept.acme.internal"),
            "{providers:?}"
        );
        let index = stored_index(secrets);
        let row = index
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["slug"] == "searxng")
            .expect("searxng row in index");
        assert_eq!(row["endpoint"], "http://kept.acme.internal");
    }

    set_enabled(&company(), &secrets, "searxng", false)
        .await
        .unwrap();
    assert_kept(&secrets).await;

    select_provider(&company(), &secrets, "searxng", None, None)
        .await
        .unwrap();
    assert_kept(&secrets).await;
    assert!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .iter()
            .find(|provider| provider.slug == "searxng")
            .unwrap()
            .enabled
    );
    assert_eq!(
        load_default_slug(&company(), &secrets)
            .await
            .unwrap()
            .as_deref(),
        Some("searxng")
    );

    assert!(
        update_endpoint_if_present(&company(), &secrets, "searxng", None)
            .await
            .unwrap()
    );
    assert_kept(&secrets).await;

    put_provider(
        &company(),
        &secrets,
        SearchProvider {
            slug: "brave".to_string(),
            enabled: true,
            endpoint: None,
        },
    )
    .await
    .unwrap();
    assert_kept(&secrets).await;

    assert!(
        claim_provider(
            &company(),
            &secrets,
            SearchProvider {
                slug: "exa".to_string(),
                enabled: true,
                endpoint: None,
            },
        )
        .await
        .unwrap()
    );
    assert_kept(&secrets).await;
}

#[tokio::test]
async fn removing_a_provider_takes_its_address_with_it() {
    let secrets = MemSecrets::default();
    put_provider(
        &company(),
        &secrets,
        SearchProvider {
            slug: "searxng".to_string(),
            enabled: true,
            endpoint: Some("http://gone.acme.internal".to_string()),
        },
    )
    .await
    .unwrap();

    delete_provider(&company(), &secrets, "searxng")
        .await
        .unwrap();
    assert_eq!(stored_index(&secrets), serde_json::json!([]));
    assert_eq!(
        secrets
            .map
            .lock()
            .unwrap()
            .get(&provider_endpoint_key("searxng"))
            .cloned(),
        Some(String::new())
    );

    put_provider(
        &company(),
        &secrets,
        SearchProvider {
            slug: "searxng".to_string(),
            enabled: true,
            endpoint: None,
        },
    )
    .await
    .unwrap();
    let providers = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(
        providers[0].endpoint, None,
        "a re-add must not resurrect the removed address: {providers:?}"
    );
}

#[tokio::test]
async fn an_index_blob_written_before_endpoint_existed_still_parses() {
    let secrets = MemSecrets::default();
    seed(
        &secrets,
        &[(
            PROVIDER_INDEX_KEY,
            r#"[{"slug":"brave","enabled":true},{"slug":"exa"}]"#,
        )],
    )
    .await;

    let providers = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(providers.len(), 2, "{providers:?}");
    let exa = providers
        .iter()
        .find(|provider| provider.slug == "exa")
        .expect("exa row");
    assert!(exa.enabled);
    assert!(providers.iter().all(|provider| provider.endpoint.is_none()));
}

#[tokio::test]
async fn a_row_with_no_address_is_stored_without_the_field() {
    let secrets = MemSecrets::default();
    put_provider(
        &company(),
        &secrets,
        SearchProvider {
            slug: "brave".to_string(),
            enabled: true,
            endpoint: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        secrets.map.lock().unwrap().get(PROVIDER_INDEX_KEY).cloned(),
        Some(r#"[{"slug":"brave","enabled":true}]"#.to_string())
    );
}

#[tokio::test]
async fn a_blob_with_endpoint_parses_as_the_row_shape_before_it() {
    // Rollback safety: a binary from before this slice must still be able to
    // parse a blob this one wrote — it simply ignores the new field.
    #[derive(Debug, serde::Deserialize)]
    struct PreviousIndexEntry {
        slug: String,
        #[serde(default = "yes")]
        #[allow(dead_code)]
        enabled: bool,
    }

    let parsed = serde_json::from_str::<Vec<PreviousIndexEntry>>(
        r#"[{"slug":"searxng","enabled":true,"endpoint":"http://row.acme.internal"}]"#,
    );
    assert!(parsed.is_ok(), "{parsed:?}");
    assert_eq!(parsed.unwrap()[0].slug, "searxng");
}

#[tokio::test]
async fn switching_off_entry_zero_copies_its_flat_address_into_the_row() {
    let secrets = MemSecrets::default();
    seed(
        &secrets,
        &[
            (PROVIDER_SECRET, "searxng"),
            (ENDPOINT_SECRET, "http://flat.acme.internal"),
        ],
    )
    .await;

    set_enabled(&company(), &secrets, "searxng", false)
        .await
        .unwrap();

    assert_eq!(
        stored_index(&secrets)[0]["endpoint"],
        "http://flat.acme.internal"
    );
    assert_eq!(
        secrets.map.lock().unwrap().get(ENDPOINT_SECRET).cloned(),
        Some("http://flat.acme.internal".to_string()),
        "the flat key is a read fallback and must not be cleared here"
    );
    assert!(
        secrets
            .map
            .lock()
            .unwrap()
            .get(&provider_endpoint_key("searxng"))
            .is_none(),
        "switching off must not write the per-slug key when the address came from the flat one"
    );
}

#[tokio::test]
async fn an_unreadable_index_is_reported_rather_than_read_as_empty() {
    // Resolving a corrupt index to "no providers" would quietly move every agent
    // onto managed search and bill the platform for it.
    let secrets = MemSecrets::default();
    seed(&secrets, &[(PROVIDER_INDEX_KEY, "{not json")]).await;
    assert!(list_providers(&company(), &secrets).await.is_err());
}
