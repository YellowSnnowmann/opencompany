//! The store, against an in-memory port: concurrency.
//!
//! Every index mutation is list-modify-save over the whole list, so two
//! concurrent mutations on one company can race. These tests exercise that
//! race with a `SecretStore` that yields on every call, so the interleaving
//! is real rather than theoretical.

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

/// A store that yields on every call, so concurrent callers genuinely interleave.
///
/// [`MemSecrets`] never awaits anything real, so two tasks driven by the same
/// runtime run one after the other and a read-modify-write race cannot be
/// observed through it. A port that yields is the honest stand-in for one that
/// talks to a database, and it is what makes the test below mean anything.
#[derive(Default)]
pub(super) struct SlowSecrets {
    inner: MemSecrets,
}

#[async_trait::async_trait]
impl SecretStore for SlowSecrets {
    async fn get(&self, company: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        tokio::task::yield_now().await;
        self.inner.get(company, key).await
    }
    async fn set(&self, company: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        tokio::task::yield_now().await;
        self.inner.set(company, key, value).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_connects_do_not_overwrite_each_other_in_the_index() {
    // Every index mutation is list-modify-save, and the list is the whole list.
    // Interleave two of them and one edit is lost: both credentials are stored,
    // and only one of the two rows survives in the index — an orphaned secret at
    // an address nothing reads. `SecretStore` offers no compare-and-swap to fix
    // this with, so the mutations are serialised per company instead.
    //
    // Four slugs at once, against a port that yields on every call, so the
    // interleaving is real rather than theoretical.
    let secrets = std::sync::Arc::new(SlowSecrets::default());
    let slugs = ["brave", "exa", "querit", "searxng"];

    let mut tasks = Vec::new();
    for slug in slugs {
        let secrets = secrets.clone();
        tasks.push(tokio::spawn(async move {
            put_provider(
                &company(),
                secrets.as_ref(),
                SearchProvider {
                    slug: slug.to_string(),
                    enabled: true,
                    endpoint: None,
                },
            )
            .await
        }));
    }
    for task in tasks {
        task.await.expect("task").expect("put_provider");
    }

    let mut stored: Vec<String> = list_providers(&company(), secrets.as_ref())
        .await
        .expect("list")
        .into_iter()
        .map(|provider| provider.slug)
        .collect();
    stored.sort();
    let mut expected: Vec<String> = slugs.iter().map(|slug| slug.to_string()).collect();
    expected.sort();
    assert_eq!(
        stored, expected,
        "every connect must survive the others, not just the last one to write"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_remove_does_not_resurrect_the_row_it_removed() {
    // The other direction of the same race: a toggle that read the index before
    // the remove wrote it will save its own copy back, complete with the row the
    // remove had just taken out.
    let secrets = std::sync::Arc::new(SlowSecrets::default());
    for slug in ["brave", "exa"] {
        put_provider(
            &company(),
            secrets.as_ref(),
            SearchProvider {
                slug: slug.to_string(),
                enabled: true,
                endpoint: None,
            },
        )
        .await
        .expect("seed");
    }

    let removing = {
        let secrets = secrets.clone();
        tokio::spawn(async move { delete_provider(&company(), secrets.as_ref(), "brave").await })
    };
    let toggling = {
        let secrets = secrets.clone();
        tokio::spawn(async move { set_enabled(&company(), secrets.as_ref(), "exa", false).await })
    };
    removing.await.expect("task").expect("delete");
    toggling.await.expect("task").expect("toggle");

    let stored = list_providers(&company(), secrets.as_ref())
        .await
        .expect("list");
    assert_eq!(stored.len(), 1, "brave must stay removed: {stored:?}");
    assert_eq!(stored[0].slug, "exa", "{stored:?}");
    assert!(
        !stored[0].enabled,
        "the toggle must survive too: {stored:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exactly_one_of_two_concurrent_claims_on_one_slug_wins() {
    // The connect flow used to read the index, decide the slug was free, and
    // write it three awaits later. Both racers got past the read — and the
    // loser then did real damage, because an `Auth` probe failure rolls back by
    // deleting the row and the credential, taking the winner's working key with
    // it while the winner answered `saved: true`.
    let secrets = std::sync::Arc::new(SlowSecrets::default());
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let secrets = secrets.clone();
        tasks.push(tokio::spawn(async move {
            claim_provider(
                &company(),
                secrets.as_ref(),
                SearchProvider {
                    slug: "brave".to_string(),
                    enabled: true,
                    endpoint: None,
                },
            )
            .await
        }));
    }
    let mut won = 0;
    for task in tasks {
        if task.await.expect("task").expect("claim") {
            won += 1;
        }
    }
    assert_eq!(won, 1, "exactly one claim may succeed");
    assert_eq!(
        list_providers(&company(), secrets.as_ref())
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_refused_claim_writes_nothing() {
    // What makes it safe to claim BEFORE storing the credential: a loser must
    // not have touched the store on its way to being refused, or it would
    // overwrite the winner's key at the shared address.
    let secrets = MemSecrets::default();
    assert!(
        claim_provider(
            &company(),
            &secrets,
            SearchProvider {
                slug: "searxng".to_string(),
                enabled: true,
                endpoint: Some("http://search.acme.internal".to_string()),
            },
        )
        .await
        .unwrap()
    );

    assert!(
        !claim_provider(
            &company(),
            &secrets,
            SearchProvider {
                slug: "searxng".to_string(),
                enabled: true,
                endpoint: Some("http://somewhere.else.internal".to_string()),
            },
        )
        .await
        .unwrap(),
        "the second claim must lose"
    );

    let providers = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(
        providers[0].endpoint.as_deref(),
        Some("http://search.acme.internal"),
        "the loser must not have overwritten the winner's address"
    );
}

#[tokio::test]
async fn a_re_address_refuses_rather_than_recreating_a_removed_row() {
    // The handler read the row, then wrote it back three awaits later with the
    // `enabled` flag it had read. A removal landing between them made the write
    // RECREATE the provider: disconnected, then back, enabled, with a fresh
    // address and receiving agent searches again.
    let secrets = MemSecrets::default();
    assert!(
        !update_endpoint_if_present(
            &company(),
            &secrets,
            "searxng",
            Some("http://search.acme.internal".to_string()),
        )
        .await
        .unwrap(),
        "nothing to re-address"
    );
    assert!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .is_empty(),
        "and nothing created on the way to saying so"
    );

    // Connected, disabled, then re-addressed: the address changes and the
    // enabled flag is preserved rather than reset.
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
    set_enabled(&company(), &secrets, "searxng", false)
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
    let providers = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(
        providers[0].endpoint.as_deref(),
        Some("http://new.acme.internal")
    );
    assert!(!providers[0].enabled, "the switch stays where it was");
}

#[tokio::test]
async fn a_key_is_not_stored_for_a_provider_the_index_does_not_hold() {
    // Otherwise the credential lands at an address the status route never
    // reports and `DELETE …/search/key` never clears, because both walk the
    // index. Checked and written in one critical section so a removal cannot
    // land between them.
    let secrets = MemSecrets::default();
    assert!(
        !store_key_if_connected(&company(), &secrets, "brave", "brave-not-a-real-key")
            .await
            .unwrap()
    );
    assert!(
        !provider_key_configured(&company(), &secrets, "brave")
            .await
            .unwrap(),
        "nothing written on the way to the refusal"
    );

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
    assert!(
        store_key_if_connected(&company(), &secrets, "brave", "brave-not-a-real-key")
            .await
            .unwrap()
    );
    assert!(
        provider_key_configured(&company(), &secrets, "brave")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_legacy_searxng_address_survives_the_slug_entering_the_index() {
    // An upgraded company keeps its URL in `search/endpoint` alone. The
    // synthesized entry-zero row reads it from there — but only while the slug
    // is NOT in the index, and any index mutation at all puts it there.
    // Toggling the row, or connecting a second provider, used to move the read
    // to `search/provider/searxng/endpoint`, which nothing had written: the
    // address vanished, SearXNG went incomplete, and every agent silently fell
    // back to managed search.
    let secrets = MemSecrets::default();
    seed(
        &secrets,
        &[
            (PROVIDER_SECRET, "searxng"),
            (ENDPOINT_SECRET, "http://search.acme.internal"),
        ],
    )
    .await;

    let before = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(
        before[0].endpoint.as_deref(),
        Some("http://search.acme.internal")
    );

    // The ordinary operator action that used to lose it: flip the switch.
    set_enabled(&company(), &secrets, "searxng", false)
        .await
        .unwrap();

    let after = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(
        after[0].endpoint.as_deref(),
        Some("http://search.acme.internal"),
        "the address has to survive the slug being indexed: {after:?}"
    );

    // And connecting a second provider, which indexes the first as a side
    // effect.
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
    let with_two = list_providers(&company(), &secrets).await.unwrap();
    let searxng = with_two
        .iter()
        .find(|provider| provider.slug == "searxng")
        .expect("searxng row");
    assert_eq!(
        searxng.endpoint.as_deref(),
        Some("http://search.acme.internal"),
        "{with_two:?}"
    );

    // A per-provider address, once written, wins over the flat one — which is
    // what convergence means.
    put_provider(
        &company(),
        &secrets,
        SearchProvider {
            slug: "searxng".to_string(),
            enabled: true,
            endpoint: Some("http://moved.acme.internal".to_string()),
        },
    )
    .await
    .unwrap();
    let moved = list_providers(&company(), &secrets).await.unwrap();
    let searxng = moved
        .iter()
        .find(|provider| provider.slug == "searxng")
        .expect("searxng row");
    assert_eq!(
        searxng.endpoint.as_deref(),
        Some("http://moved.acme.internal"),
        "{moved:?}"
    );
}
