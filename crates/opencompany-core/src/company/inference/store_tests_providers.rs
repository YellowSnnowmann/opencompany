//! Provider CRUD tests: the legacy flat slot, entry-zero, adding and
//! deleting providers, slug derivation, and the index blob (split out
//! of `store_tests.rs`).

use super::store_tests_support::*;
use super::*;

#[tokio::test]
async fn a_company_with_nothing_configured_has_no_providers() {
    let secrets = MemSecrets::default();
    assert!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .is_empty(),
        "no config is an empty list, not an error"
    );
}

#[tokio::test]
async fn the_legacy_flat_slot_reads_back_as_entry_zero() {
    let secrets = MemSecrets::default();
    write_entry_zero(&secrets, "openrouter").await;

    let providers = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(providers.len(), 1);
    let zero = &providers[0];
    assert_eq!(zero.slug, "openrouter");
    assert_eq!(zero.label, "OpenRouter", "the catalogue supplies the label");
    assert_eq!(zero.origin, ProviderOrigin::EntryZero);
    assert_eq!(zero.id.as_str(), ENTRY_ZERO_ID);
    assert!(zero.enabled);
    // Written at the uniform address; still READ from the legacy one until
    // the first save converges it. One address rule, one readable fallback.
    assert_eq!(zero.key_key(), provider_key_key("openrouter"));
    assert_eq!(zero.legacy_key_key(), Some(KEY_KEY));
}

#[tokio::test]
async fn a_legacy_credential_is_read_from_the_flat_slot_and_moved_by_one_save() {
    // Lazy convergence. An existing company keeps working untouched, and the
    // first save of that provider moves the key and clears the old slot —
    // no flag day, and no half-migrated state on a store with no
    // transaction.
    let secrets = MemSecrets::default();
    write_entry_zero(&secrets, "openrouter").await;
    secrets
        .set(&company(), KEY_KEY, SecretValue("sk-not-a-real-key".into()))
        .await
        .unwrap();

    let zero = list_providers(&company(), &secrets).await.unwrap()[0].clone();
    assert_eq!(
        load_provider_key(&company(), &secrets, &zero)
            .await
            .unwrap(),
        "sk-not-a-real-key",
        "the fallback is what keeps an untouched company working"
    );

    store_provider_key(&company(), &secrets, &zero, "sk-not-a-real-key-2")
        .await
        .unwrap();
    assert_eq!(
        secrets.get(&company(), &zero.key_key()).await.unwrap(),
        Some(SecretValue("sk-not-a-real-key-2".into())),
    );
    assert_eq!(
        secrets.get(&company(), KEY_KEY).await.unwrap(),
        Some(SecretValue(String::new())),
        "the legacy slot is cleared in the same operation; a key left there \
         after the new one is written is an orphaned secret"
    );
}

#[tokio::test]
async fn entry_zero_keeps_its_id_across_reads() {
    // A generated id would have to be written back to be stable, and the
    // write path into the legacy slot is the one thing this design will not
    // do on a read.
    let secrets = MemSecrets::default();
    write_entry_zero(&secrets, "openrouter").await;
    let first = list_providers(&company(), &secrets).await.unwrap()[0]
        .id
        .clone();
    let second = list_providers(&company(), &secrets).await.unwrap()[0]
        .id
        .clone();
    assert_eq!(first, second);
}

#[tokio::test]
async fn the_legacy_managed_alias_resolves_rather_than_failing() {
    // A stored runtime blob is data an operator cannot hand-edit, so a value
    // the console itself once wrote must not strand them.
    let secrets = MemSecrets::default();
    write_entry_zero(&secrets, "managed").await;
    let providers = list_providers(&company(), &secrets).await.unwrap();
    // The **kind** normalizes onto OpenRouter — that is the shape of API it
    // speaks. The **slug** does not: it says whose account this is, and a
    // managed config is the TinyHumans account. Keyed on the kind, the
    // managed credential would sit in the slot a real OpenRouter account
    // belongs in, and a company holding both would have one.
    assert_eq!(providers[0].kind, "openrouter");
    assert_eq!(providers[0].slug, super::super::MANAGED_SLUG);
    assert_eq!(providers[0].label, "Managed");
    assert_eq!(
        providers[0].key_key(),
        provider_key_key(super::super::MANAGED_SLUG)
    );
}

#[tokio::test]
async fn adding_a_second_provider_leaves_entry_zero_first() {
    let secrets = MemSecrets::default();
    write_entry_zero(&secrets, "openrouter").await;
    put_provider(&company(), &secrets, draft("acme"))
        .await
        .unwrap();

    let providers = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(
        providers
            .iter()
            .map(|p| p.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["openrouter", "acme"]
    );
    assert_eq!(providers[1].origin, ProviderOrigin::Indexed);
    assert_eq!(providers[1].key_key(), "provider/acme/key");
}

#[tokio::test]
async fn two_providers_hold_two_independent_credentials() {
    // The defect this whole change exists to fix: today one company has one
    // credential slot, so switching provider without re-entering a key
    // presents the previous vendor's credential to the new one.
    let secrets = MemSecrets::default();
    write_entry_zero(&secrets, "openrouter").await;
    let zero = list_providers(&company(), &secrets).await.unwrap()[0].clone();
    let acme = put_provider(&company(), &secrets, draft("acme"))
        .await
        .unwrap();

    store_provider_key(&company(), &secrets, &zero, "sk-not-a-real-key-zero")
        .await
        .unwrap();
    store_provider_key(&company(), &secrets, &acme, "sk-not-a-real-key-acme")
        .await
        .unwrap();

    assert_eq!(
        load_provider_key(&company(), &secrets, &zero)
            .await
            .unwrap(),
        "sk-not-a-real-key-zero"
    );
    assert_eq!(
        load_provider_key(&company(), &secrets, &acme)
            .await
            .unwrap(),
        "sk-not-a-real-key-acme"
    );
    assert!(
        provider_key_configured(&company(), &secrets, &zero)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_blank_credential_reads_as_not_configured() {
    // The store has no delete: clearing is a write of the empty string, so
    // "cleared" and "never set" are deliberately the same state.
    let secrets = MemSecrets::default();
    let acme = put_provider(&company(), &secrets, draft("acme"))
        .await
        .unwrap();
    store_provider_key(&company(), &secrets, &acme, "sk-not-a-real-key")
        .await
        .unwrap();
    assert!(
        provider_key_configured(&company(), &secrets, &acme)
            .await
            .unwrap()
    );
    store_provider_key(&company(), &secrets, &acme, "   ")
        .await
        .unwrap();
    assert!(
        !provider_key_configured(&company(), &secrets, &acme)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn deleting_a_provider_clears_its_credential() {
    let secrets = MemSecrets::default();
    let acme = put_provider(&company(), &secrets, draft("acme"))
        .await
        .unwrap();
    store_provider_key(&company(), &secrets, &acme, "sk-not-a-real-key")
        .await
        .unwrap();

    assert!(delete_provider(&company(), &secrets, "acme").await.unwrap());
    assert!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .is_empty()
    );

    // Re-adding the same slug must NOT inherit the old credential.
    let again = put_provider(&company(), &secrets, draft("acme"))
        .await
        .unwrap();
    assert!(
        !provider_key_configured(&company(), &secrets, &again)
            .await
            .unwrap(),
        "re-adding a deleted slug silently reused its key"
    );
}

#[tokio::test]
async fn a_failed_credential_clear_keeps_the_provider_visible() {
    // Of the two half-states, "still listed, key intact" is the one the
    // operator can see and act on. "Gone from the list, key on disk" is not.
    let secrets = FailsWriting {
        inner: MemSecrets::default(),
        failing_key: provider_key_key("acme"),
    };
    put_provider(&company(), &secrets, draft("acme"))
        .await
        .unwrap();

    let err = delete_provider(&company(), &secrets, "acme")
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("could not clear the stored credential"),
        "a failed clear must be loud, got: {err}"
    );
    assert_eq!(
        list_providers(&company(), &secrets).await.unwrap().len(),
        1,
        "the provider stayed visible"
    );
}

#[tokio::test]
async fn deleting_something_that_is_not_there_is_not_an_error() {
    let secrets = MemSecrets::default();
    assert!(!delete_provider(&company(), &secrets, "nope").await.unwrap());
}

#[tokio::test]
async fn disabling_keeps_the_endpoint_the_label_and_the_credential() {
    let secrets = MemSecrets::default();
    let acme = put_provider(&company(), &secrets, draft("acme"))
        .await
        .unwrap();
    store_provider_key(&company(), &secrets, &acme, "sk-not-a-real-key")
        .await
        .unwrap();

    assert!(
        set_enabled(&company(), &secrets, "acme", false)
            .await
            .unwrap()
    );
    let stored = get_provider(&company(), &secrets, "acme")
        .await
        .unwrap()
        .unwrap();
    assert!(!stored.enabled);
    assert_eq!(stored.base_url, "https://acme.example/v1");
    assert_eq!(stored.label, "acme");
    assert!(
        provider_key_configured(&company(), &secrets, &stored)
            .await
            .unwrap(),
        "disabled is not deleted"
    );
}

#[tokio::test]
async fn replacing_a_provider_keeps_its_id() {
    // Identity survives a rename; that is the reason id and slug are two
    // fields rather than one.
    let secrets = MemSecrets::default();
    let first = put_provider(&company(), &secrets, draft("acme"))
        .await
        .unwrap();
    let mut renamed = draft("acme");
    renamed.label = "Acme gateway".to_string();
    let second = put_provider(&company(), &secrets, renamed).await.unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.label, "Acme gateway");
    assert_eq!(list_providers(&company(), &secrets).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_generated_id_is_not_the_entry_zero_sentinel() {
    let a = ProviderId::new();
    let b = ProviderId::new();
    assert_ne!(a, b, "ids must not repeat");
    assert_ne!(a.as_str(), ENTRY_ZERO_ID);
    assert!(a.as_str().starts_with("prv_"));
}

#[tokio::test]
async fn a_second_record_may_not_shadow_entry_zero() {
    let secrets = MemSecrets::default();
    write_entry_zero(&secrets, "openrouter").await;
    let err = put_provider(&company(), &secrets, draft("openrouter"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("existing provider"), "{err}");
}

#[test]
fn a_slug_is_derived_from_the_name_never_typed() {
    assert_eq!(slugify("Acme Gateway"), "acme-gateway");
    assert_eq!(slugify("  My  OpenRouter!!  "), "my-openrouter");
    assert_eq!(slugify("---"), "");
}

#[test]
fn a_custom_slug_is_refused_for_three_named_reasons() {
    let existing = vec![Provider {
        id: ProviderId::new(),
        slug: "acme".into(),
        label: "Acme".into(),
        kind: "openai_compatible".into(),
        base_url: "https://acme.example/v1".into(),
        models: BTreeMap::new(),
        enabled: true,
        origin: ProviderOrigin::Indexed,
    }];
    assert_eq!(check_slug(&existing, "   "), Err(SlugError::Empty));
    assert_eq!(check_slug(&existing, "acme"), Err(SlugError::Taken));
    assert_eq!(check_slug(&existing, "groq"), Err(SlugError::Reserved));
    assert_eq!(check_slug(&existing, "acme-two"), Ok(()));
}

#[test]
fn a_provider_name_is_bounded_at_the_limit_and_refused_past_it() {
    // The bound exists because the name becomes the address of a secret.
    // At the limit is a legal name; one character past it is not, and the
    // refusal happens here rather than at the store, where it used to
    // arrive as `ENAMETOOLONG` after a write had already landed.
    let at_limit = "a".repeat(MAX_PROVIDER_NAME_CHARS);
    let past_limit = "a".repeat(MAX_PROVIDER_NAME_CHARS + 1);

    assert_eq!(check_provider_name(&at_limit), Ok(()));
    assert_eq!(check_provider_name(&past_limit), Err(SlugError::TooLong));
    assert_eq!(check_provider_name("  "), Err(SlugError::Empty));

    assert_eq!(check_slug(&[], &at_limit), Ok(()));
    assert_eq!(check_slug(&[], &past_limit), Err(SlugError::TooLong));

    // Characters, not bytes: a name of multi-byte characters is judged by
    // what the operator typed rather than by how UTF-8 happens to store it.
    let multibyte = "é".repeat(MAX_PROVIDER_NAME_CHARS);
    assert_eq!(check_provider_name(&multibyte), Ok(()));
}

#[test]
fn a_bounded_name_keeps_its_credential_key_inside_the_filename_budget() {
    // Why 80 and not some larger round number: the derived secret key has
    // to stay short enough that the canonical filename is the readable
    // `%k-` form rather than the truncated-and-digested `%l-` one. The
    // slug alphabet is `[a-z0-9-]`, one byte per character once
    // percent-encoded, and `provider/` + `/key` add 17.
    let key = provider_key_key(&"a".repeat(MAX_PROVIDER_NAME_CHARS));
    // `provider/` + `/key` is 13 characters around the slug.
    assert_eq!(key.len(), MAX_PROVIDER_NAME_CHARS + 13);
    // Percent-encoding is what the budget is measured in. The slug alphabet
    // (`[a-z0-9-]`) survives as one byte per character; the two `/`
    // separators become `%2F`, three bytes each.
    let encoded_len = key.len() + 2 * 2;
    assert!(
        encoded_len < 200,
        "a bounded name must not need a truncated secret filename: {encoded_len} bytes"
    );
}

#[tokio::test]
async fn an_index_written_before_enabled_existed_reads_as_enabled() {
    // A missing field must not read as "every provider is off", which is
    // what `#[serde(default)]` on a bool would have given.
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company(),
            PROVIDER_INDEX_KEY,
            SecretValue(
                r#"[{"id":"prv_old","slug":"acme","label":"Acme","kind":"openai_compatible","base_url":"https://acme.example/v1"}]"#
                    .to_string(),
            ),
        )
        .await
        .unwrap();
    let providers = list_providers(&company(), &secrets).await.unwrap();
    assert_eq!(providers.len(), 1);
    assert!(providers[0].enabled);
}

#[tokio::test]
async fn a_malformed_index_is_surfaced_not_swallowed() {
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company(),
            PROVIDER_INDEX_KEY,
            SecretValue("{not json".into()),
        )
        .await
        .unwrap();
    let err = list_providers(&company(), &secrets).await.unwrap_err();
    assert!(err.to_string().contains("not valid JSON"), "{err}");
}

#[tokio::test]
async fn an_empty_index_blob_is_an_empty_list() {
    let secrets = MemSecrets::default();
    secrets
        .set(&company(), PROVIDER_INDEX_KEY, SecretValue(String::new()))
        .await
        .unwrap();
    assert!(
        list_providers(&company(), &secrets)
            .await
            .unwrap()
            .is_empty()
    );
}
