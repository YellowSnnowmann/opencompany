//! The store, against an in-memory port: failure and guarded-write behavior.
//!
//! What happens when a write to the backing `SecretStore` itself fails
//! partway through a multi-step operation (remove, re-address, disconnect
//! all), and the "guarded write" pattern that checks a precondition and
//! writes only if it still holds.

use super::tests_concurrency::SlowSecrets;
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

/// A store whose writes to one address fail, as a transient backend error would.
#[derive(Default)]
struct FailingSecrets {
    inner: MemSecrets,
    fail_on: std::sync::Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl SecretStore for FailingSecrets {
    async fn get(&self, company: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        self.inner.get(company, key).await
    }
    async fn set(&self, company: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        if self.fail_on.lock().unwrap().as_deref() == Some(key) {
            return Err(crate::error::OpenCompanyError::InvalidRequest(
                "the store is unavailable".to_string(),
            ));
        }
        self.inner.set(company, key, value).await
    }
}

#[tokio::test]
async fn a_removal_whose_credential_clear_fails_keeps_the_row() {
    // It used to unlist the row first. A clear that then failed returned an
    // error with the row gone and the key still stored — invisible in status and
    // skipped by Disconnect all. Clearing first means a failure leaves a visible,
    // retryable row instead.
    let secrets = FailingSecrets::default();
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
    store_provider_key(&company(), &secrets, "brave", "brave-not-a-real-key")
        .await
        .unwrap();

    *secrets.fail_on.lock().unwrap() = Some("search/provider/brave/key".to_string());
    assert!(
        delete_provider(&company(), &secrets, "brave")
            .await
            .is_err()
    );

    let still = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(
        still.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(),
        vec!["brave"],
        "the row must stay listed while its credential is still stored"
    );
    assert!(
        provider_key_configured(&company(), &secrets, "brave")
            .await
            .unwrap()
    );

    // And the retry, once the store recovers, finishes the job.
    *secrets.fail_on.lock().unwrap() = None;
    delete_provider(&company(), &secrets, "brave")
        .await
        .unwrap();
    assert!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !provider_key_configured(&company(), &secrets, "brave")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn removing_a_legacy_entry_zero_provider_still_clears_the_flat_keys() {
    // The reorder reads "is this entry zero" before clearing anything, because
    // clearing `search/provider` changes the answer. If it read it after, the
    // flat credential would be left behind.
    let secrets = MemSecrets::default();
    seed(
        &secrets,
        &[(PROVIDER_SECRET, "exa"), (API_KEY_SECRET, "exa-key")],
    )
    .await;

    delete_provider(&company(), &secrets, "exa").await.unwrap();

    assert!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .is_empty()
    );
    for key in [PROVIDER_SECRET, API_KEY_SECRET, ENDPOINT_SECRET] {
        assert_eq!(
            secrets.map.lock().unwrap().get(key).cloned(),
            Some(String::new()),
            "{key}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_all_removes_what_is_connected_when_it_runs() {
    let secrets = std::sync::Arc::new(SlowSecrets::default());
    for slug in ["brave", "exa", "querit"] {
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
        .unwrap();
        store_provider_key(&company(), secrets.as_ref(), slug, "not-a-real-key")
            .await
            .unwrap();
    }

    delete_all_providers(&company(), secrets.as_ref())
        .await
        .unwrap();

    assert!(
        list_providers(&company(), secrets.as_ref())
            .await
            .unwrap()
            .is_empty()
    );
    for slug in ["brave", "exa", "querit"] {
        assert!(
            !provider_key_configured(&company(), secrets.as_ref(), slug)
                .await
                .unwrap(),
            "{slug}"
        );
    }
}

#[tokio::test]
async fn re_addressing_a_provider_keeps_its_place_in_the_list() {
    // With no default marked, the first usable row answers. Moving an edited
    // row to the end turned "change SearXNG's address" into "switch every agent
    // to the other provider".
    let secrets = MemSecrets::default();
    for (slug, endpoint) in [
        ("searxng", Some("http://old.acme.internal".to_string())),
        ("brave", None),
    ] {
        put_provider(
            &company(),
            &secrets,
            SearchProvider {
                slug: slug.to_string(),
                enabled: true,
                endpoint,
            },
        )
        .await
        .unwrap();
    }

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

    let order: Vec<String> = list_providers(&company(), &secrets)
        .await
        .unwrap()
        .into_iter()
        .map(|provider| provider.slug)
        .collect();
    assert_eq!(order, vec!["searxng", "brave"]);
}

#[tokio::test]
async fn a_default_is_not_marked_on_a_provider_that_is_not_connected() {
    let secrets = MemSecrets::default();
    assert!(
        !set_default_if_connected(&company(), &secrets, "brave")
            .await
            .unwrap()
    );
    assert_eq!(load_default_slug(&company(), &secrets).await.unwrap(), None);

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
        set_default_if_connected(&company(), &secrets, "brave")
            .await
            .unwrap()
    );
    assert_eq!(
        load_default_slug(&company(), &secrets)
            .await
            .unwrap()
            .as_deref(),
        Some("brave")
    );
}

/// Keys rework (#2306), decision D-never-clear-default (X14, 2026-09-15):
/// this test used to be named
/// `marking_a_default_while_it_is_removed_never_leaves_a_dangling_marker` and
/// asserted that no ordering may leave `brave` marked with no `brave` row —
/// which was true only because `delete_provider` used to clear the marker
/// itself. It no longer does (see the two tests above), so "marked with no
/// row" is now a legitimate outcome of the mark-then-remove ordering, not a
/// bug to guard against. What must still hold under either ordering is that
/// the store itself stays consistent: no resurrected row, no panic, and a
/// marker that is *either* unset (remove-then-mark: `set_default_if_connected`
/// finds nothing connected and refuses) *or* still naming `brave` (mark-then-
/// remove: the mark lands, then the remove leaves it alone) — never anything
/// else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn marking_and_removing_the_same_provider_concurrently_leaves_a_consistent_store() {
    for _ in 0..20 {
        let secrets = std::sync::Arc::new(SlowSecrets::default());
        put_provider(
            &company(),
            secrets.as_ref(),
            SearchProvider {
                slug: "brave".to_string(),
                enabled: true,
                endpoint: None,
            },
        )
        .await
        .unwrap();

        let marking = {
            let secrets = secrets.clone();
            tokio::spawn(async move {
                set_default_if_connected(&company(), secrets.as_ref(), "brave").await
            })
        };
        let removing = {
            let secrets = secrets.clone();
            tokio::spawn(
                async move { delete_provider(&company(), secrets.as_ref(), "brave").await },
            )
        };
        marking.await.expect("task").expect("mark");
        removing.await.expect("task").expect("remove");

        assert!(
            list_providers(&company(), secrets.as_ref())
                .await
                .unwrap()
                .is_empty(),
            "the row is gone either way"
        );
        let marker = load_default_slug(&company(), secrets.as_ref())
            .await
            .unwrap()
            .filter(|slug| !slug.is_empty());
        assert!(
            marker.is_none() || marker.as_deref() == Some("brave"),
            "the marker must be exactly unset or still naming brave, got {marker:?}"
        );
    }
}

#[tokio::test]
async fn selecting_a_provider_without_an_address_keeps_the_one_stored_now() {
    // The legacy save read the row and wrote back the address it had read. A
    // Change address landing in between was overwritten with the old value.
    // Nothing is carried from a snapshot any more: an omitted address is simply
    // not written.
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
    // The concurrent re-address, landed.
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

    select_provider(&company(), &secrets, "searxng", None, None)
        .await
        .unwrap();

    let providers = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(
        providers[0].endpoint.as_deref(),
        Some("http://new.acme.internal"),
        "the newer address must survive: {providers:?}"
    );
    assert_eq!(
        load_default_slug(&company(), &secrets)
            .await
            .unwrap()
            .as_deref(),
        Some("searxng")
    );
}

/// Keys rework (#2306), decision D-never-clear-default (X14, 2026-09-15):
/// this test used to be named
/// `a_legacy_selection_racing_a_removal_never_leaves_a_dangling_marker` and
/// asserted that `brave` must never end up marked with no `brave` row — true
/// only because `delete_provider` used to clear the marker whenever it
/// matched the slug being removed. It no longer does, so a select landing
/// first (creating the row and marking it) followed by a delete (removing the
/// row, leaving the marker alone per X14) legitimately produces exactly that
/// state now — it is the same outcome `deleting_the_marked_provider_never_
/// clears_the_marker` above pins deliberately for the non-concurrent case.
/// What must still hold is that the credential is never left stored with no
/// row to list it (a `delete_provider_locked` invariant this change does not
/// touch) and that the store itself never corrupts under the race.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_legacy_selection_racing_a_removal_leaves_a_consistent_store() {
    for _ in 0..20 {
        let secrets = std::sync::Arc::new(SlowSecrets::default());
        put_provider(
            &company(),
            secrets.as_ref(),
            SearchProvider {
                slug: "brave".to_string(),
                enabled: true,
                endpoint: None,
            },
        )
        .await
        .unwrap();

        let selecting = {
            let secrets = secrets.clone();
            tokio::spawn(async move {
                select_provider(
                    &company(),
                    secrets.as_ref(),
                    "brave",
                    None,
                    Some("brave-not-a-real-key"),
                )
                .await
            })
        };
        let removing = {
            let secrets = secrets.clone();
            tokio::spawn(
                async move { delete_provider(&company(), secrets.as_ref(), "brave").await },
            )
        };
        selecting.await.expect("task").expect("select");
        removing.await.expect("task").expect("remove");

        let connected = list_providers(&company(), secrets.as_ref())
            .await
            .unwrap()
            .iter()
            .any(|provider| provider.slug == "brave");
        // Under X14 a marker naming a slug with no row is a legitimate
        // outcome of the select-then-delete ordering — see the doc comment
        // above. The property worth holding is only that the credential is
        // never orphaned, checked below.
        let key = provider_key_configured(&company(), secrets.as_ref(), "brave")
            .await
            .unwrap();
        assert!(!key || connected, "brave's key is stored with no brave row");
    }
}

#[tokio::test]
async fn a_failed_legacy_address_clear_is_still_retried_as_entry_zero() {
    // Clearing `search/provider` before `search/endpoint` meant a failed
    // address clear left the flat value stored while the retry no longer knew
    // the row was entry zero — so it never cleared it.
    let secrets = FailingSecrets::default();
    seed_failing(
        &secrets,
        &[
            (PROVIDER_SECRET, "searxng"),
            (ENDPOINT_SECRET, "http://search.acme.internal"),
        ],
    )
    .await;

    *secrets.fail_on.lock().unwrap() = Some(ENDPOINT_SECRET.to_string());
    assert!(
        delete_provider(&company(), &secrets, "searxng")
            .await
            .is_err()
    );
    assert_eq!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .iter()
            .map(|p| p.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["searxng"],
        "the legacy row must still be recognisable"
    );

    *secrets.fail_on.lock().unwrap() = None;
    delete_provider(&company(), &secrets, "searxng")
        .await
        .unwrap();
    assert!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .is_empty()
    );
    for key in [PROVIDER_SECRET, ENDPOINT_SECRET] {
        assert_eq!(
            secrets.inner.map.lock().unwrap().get(key).cloned(),
            Some(String::new()),
            "{key} must be cleared on the retry"
        );
    }
}

async fn seed_failing(secrets: &FailingSecrets, pairs: &[(&str, &str)]) {
    for (key, value) in pairs {
        secrets
            .set(&company(), key, SecretValue((*value).to_string()))
            .await
            .expect("seed");
    }
}

// ---------------------------------------------------------------------------
// GuardedWrite — KR review comment 4012261309: the disable/remove/key-clear
// in-use check and the mutation it guards must be one critical section.
// ---------------------------------------------------------------------------

/// `server/ops/search.rs`'s guards used to read [`load_default_slug`] and
/// decide *before* ever calling into one of this module's locked mutations —
/// two separate operations with a gap between them wide enough for an entire
/// concurrent [`set_default_if_connected`] to land and complete, unconfirmed,
/// stranding whatever it just made the default. Reproduced here by hand
/// (deterministically — the vulnerable pattern is unsafe no matter how the
/// interleaving is scheduled, so nothing about this needs a real race) to
/// show [`set_enabled_guarded`] closes the gap: it re-reads the marker inside
/// the same lock hold as the write, so it can never act on an answer staler
/// than the moment it actually applies.
#[tokio::test]
async fn set_enabled_guarded_sees_a_default_set_after_its_caller_first_checked() {
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
    .expect("seed");

    // The vulnerable pattern's own first step: read the marker before
    // deciding anything. Nothing is marked yet.
    let marked_before = load_default_slug(&company(), &secrets).await.unwrap();
    assert_eq!(marked_before, None, "nothing marked yet");

    // The concurrent request that used to be able to land in the gap between
    // that read and the disable's own write: something else marks brave as
    // the default in the meantime.
    assert!(
        set_default_if_connected(&company(), &secrets, "brave")
            .await
            .unwrap(),
        "brave is connected"
    );
    assert_ne!(
        marked_before.as_deref(),
        Some("brave"),
        "the stale read is exactly what made the old pattern unsafe"
    );

    // The vulnerable pattern: decide from that stale read, then call the
    // plain mutation directly with no fresh check of its own — exactly what
    // the HTTP layer used to do.
    set_enabled(&company(), &secrets, "brave", false)
        .await
        .unwrap();
    let stranded = list_providers(&company(), &secrets)
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.slug == "brave")
        .unwrap();
    assert!(
        !stranded.enabled,
        "documents the bug this fix closes: an unconfirmed disable went through against a \
         stale read, stranding what is now the marked default"
    );

    // Reset, and prove the guarded function does not repeat this.
    set_enabled(&company(), &secrets, "brave", true)
        .await
        .unwrap();
    let outcome = set_enabled_guarded(&company(), &secrets, "brave", false, false)
        .await
        .unwrap();
    assert_eq!(
        outcome,
        GuardedWrite::Blocked,
        "the guarded path re-reads the marker under its own lock, sees brave IS the default, \
         and refuses rather than stranding it"
    );
    let still_enabled = list_providers(&company(), &secrets)
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.slug == "brave")
        .unwrap();
    assert!(
        still_enabled.enabled,
        "the guarded call must not have disabled it"
    );
}

/// The same closed gap for [`delete_provider_guarded`] and
/// [`store_key_guarded`], plus their ordinary `NotConnected`/`Applied` paths —
/// exercised together since all three share [`GuardedWrite`]'s shape and the
/// same lock discipline.
#[tokio::test]
async fn guarded_writes_report_not_connected_blocked_and_applied() {
    let secrets = MemSecrets::default();

    // No row at all: every guarded write reports `NotConnected` rather than
    // silently doing nothing, so a caller building an error message can tell
    // "refused, in use" apart from "there was nothing to change".
    assert_eq!(
        set_enabled_guarded(&company(), &secrets, "brave", false, false)
            .await
            .unwrap(),
        GuardedWrite::NotConnected
    );
    assert_eq!(
        delete_provider_guarded(&company(), &secrets, "brave", false)
            .await
            .unwrap(),
        GuardedWrite::NotConnected
    );
    assert_eq!(
        store_key_guarded(&company(), &secrets, "brave", "", false)
            .await
            .unwrap(),
        GuardedWrite::NotConnected
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
    .expect("seed");
    store_provider_key(&company(), &secrets, "brave", "brave-not-a-real-key")
        .await
        .expect("seed key");
    assert!(
        set_default_if_connected(&company(), &secrets, "brave")
            .await
            .unwrap()
    );

    // Marked default, unconfirmed: every one of the three guards refuses.
    assert_eq!(
        delete_provider_guarded(&company(), &secrets, "brave", false)
            .await
            .unwrap(),
        GuardedWrite::Blocked
    );
    assert_eq!(
        store_key_guarded(&company(), &secrets, "brave", "", false)
            .await
            .unwrap(),
        GuardedWrite::Blocked,
        "clearing the key is guarded the same as a disable"
    );
    // A rotate (non-empty key) is never guarded, even while brave is default.
    assert_eq!(
        store_key_guarded(&company(), &secrets, "brave", "brave-new-key", false)
            .await
            .unwrap(),
        GuardedWrite::Applied
    );

    // Confirmed: the removal actually runs.
    assert_eq!(
        delete_provider_guarded(&company(), &secrets, "brave", true)
            .await
            .unwrap(),
        GuardedWrite::Applied
    );
    assert!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .is_empty()
    );
}
