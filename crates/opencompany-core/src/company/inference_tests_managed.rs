//! The managed-credential-chain and provider-list tests: the managed
//! row's honest state, and how the provider list reaches the resolver
//! (split out of `inference_tests.rs`).

use super::inference_tests_support::*;
use super::tests_validation::managed_env;
use super::*;

// ---- the managed credential chain (issue #2266) -------------------------
//
// ```text
//   1. provider/tinyhumans/key   a key pasted specifically for inference
//   2. inference/key             the legacy address, read-only
//   3. tinyhumans/key            the company's account identity
//   4. instance identity         TINYHUMANS_TOKEN_FILE, else TINYHUMANS_API_KEY
//   5. nothing                   fail closed
// ```
//
// Steps 3 and 4 apply **only** when the vendor at the other end is the
// identity's own vendor. The OpenRouter test below is the important one.

async fn write(secrets: &MemSecrets, key: &str, value: &str) {
    secrets
        .set(&CompanyId::new("acme"), key, SecretValue(value.into()))
        .await
        .unwrap();
}

async fn resolve_managed(secrets: &MemSecrets) -> InferenceDecl {
    let company = CompanyId::new("acme");
    let config = RuntimeInference {
        provider: "managed".into(),
        base_url: None,
        models: BTreeMap::new(),
    };
    save_runtime_config(&company, secrets, &config)
        .await
        .unwrap();
    resolve_effective(
        &company,
        &Inference::default(),
        Some(&managed_env()),
        secrets,
    )
    .await
    .unwrap()
    .expect("a managed config resolves")
}

#[tokio::test]
async fn managed_with_a_pasted_inference_key_uses_it() {
    let secrets = MemSecrets::default();
    write(
        &secrets,
        &store::provider_key_key(MANAGED_SLUG),
        "sk-not-a-real-key",
    )
    .await;
    // Present but outranked, so the ordering is actually exercised.
    write(&secrets, crate::company::company_key::KEY_KEY, "th-account").await;

    let decl = resolve_managed(&secrets).await;
    assert_eq!(bearer(&decl).await.as_deref(), Some("sk-not-a-real-key"));
    assert_eq!(decl.base_url, managed_env().base_url);
}

#[tokio::test]
async fn managed_falls_back_to_the_company_account_key_and_keeps_the_platform_endpoint() {
    // The substance of #2266: a company key set in the console reached
    // Composio and never reached inference, so setting it moved the app
    // connections onto the company's account and left every agent turn —
    // the expensive half — on whoever runs the server.
    let secrets = MemSecrets::default();
    write(&secrets, crate::company::company_key::KEY_KEY, "th-account").await;

    let decl = resolve_managed(&secrets).await;
    assert_eq!(bearer(&decl).await.as_deref(), Some("th-account"));
    // **Assert the endpoint, not only the bearer.** Sending a `th_…` key to
    // openrouter.ai is the shipped bug this chain must not reproduce, and a
    // test that checked the bearer alone is exactly how it shipped.
    assert_eq!(decl.base_url, managed_env().base_url);
    assert!(
        !decl.base_url.contains("openrouter.ai"),
        "{}",
        decl.base_url
    );
}

#[tokio::test]
async fn managed_with_neither_uses_the_instance_identity() {
    let secrets = MemSecrets::default();
    let decl = resolve_managed(&secrets).await;
    assert_eq!(bearer(&decl).await.as_deref(), Some("platform-key"));
    assert_eq!(decl.base_url, managed_env().base_url);
}

#[tokio::test]
async fn openrouter_never_receives_the_company_identity_or_the_instance_one() {
    // THE test. An identity flows to a surface only when the vendor at the
    // other end is the identity's own vendor: a `th_…` key means nothing to
    // OpenRouter, and presenting it there is both a failed request and a
    // credential disclosed to a third party.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    write(&secrets, crate::company::company_key::KEY_KEY, "th-account").await;

    let config = RuntimeInference {
        provider: "openrouter".into(),
        // An explicit endpoint is what makes this unambiguously the tenant's
        // own OpenRouter rather than the platform proxy in front of it.
        base_url: Some(OPENROUTER_BASE_URL.into()),
        models: BTreeMap::new(),
    };
    save_runtime_config(&company, &secrets, &config)
        .await
        .unwrap();
    let decl = resolve_effective(
        &company,
        &Inference::default(),
        Some(&managed_env()),
        &secrets,
    )
    .await
    .unwrap()
    .expect("an openrouter config resolves");

    assert_eq!(decl.base_url, OPENROUTER_BASE_URL);
    assert!(!decl.is_proxied());
    let presented = bearer(&decl).await;
    assert_ne!(
        presented.as_deref(),
        Some("th-account"),
        "the company identity leaked to a vendor"
    );
    assert_ne!(
        presented.as_deref(),
        Some("platform-key"),
        "the instance identity leaked to a vendor"
    );
    assert_eq!(
        presented, None,
        "no credential at all is the correct answer here"
    );
}

#[tokio::test]
async fn managed_runtime_override_never_sends_the_managed_key_to_its_endpoint() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    write(
        &secrets,
        &store::provider_key_key(MANAGED_SLUG),
        "th-write-only-account-key",
    )
    .await;
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: "managed".into(),
            base_url: Some("https://gateway.example/v1".into()),
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();

    let decl = resolve_effective(
        &company,
        &Inference::default(),
        Some(&managed_env()),
        &secrets,
    )
    .await
    .unwrap()
    .expect("an explicit endpoint resolves");

    assert_eq!(decl.base_url, "https://gateway.example/v1");
    assert!(!decl.is_proxied());
    assert_eq!(bearer(&decl).await, None);
}

#[tokio::test]
async fn a_legacy_company_reads_the_flat_slot_and_one_save_moves_it() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    write(&secrets, KEY_KEY, "sk-not-a-real-key").await;

    let decl = resolve_managed(&secrets).await;
    assert_eq!(
        bearer(&decl).await.as_deref(),
        Some("sk-not-a-real-key"),
        "the legacy address is still read, so an untouched company keeps working"
    );

    // One save through the provider store converges the address.
    let zero = store::list_providers(&company, &secrets).await.unwrap()[0].clone();
    store::store_provider_key(&company, &secrets, &zero, "sk-not-a-real-key")
        .await
        .unwrap();
    assert_eq!(
        secrets.get(&company, KEY_KEY).await.unwrap(),
        Some(SecretValue(String::new())),
        "and clears the old one, so no secret is orphaned"
    );
    assert_eq!(
        secrets
            .get(&company, &store::provider_key_key(MANAGED_SLUG))
            .await
            .unwrap(),
        Some(SecretValue("sk-not-a-real-key".into()))
    );
}

// ---- the managed row's honest state -------------------------------------

#[test]
fn managed_reports_which_step_of_the_chain_answers() {
    // Not a boolean, and not "always on". The row that renders this used to
    // claim permanent availability, inherited from a design where the same
    // company runs the managed backend — here it needs a credential and can
    // resolve to nothing.
    let env = managed_env();
    let company = Credential::from_company_key("th-account");

    assert_eq!(
        managed_source(true, &company, Some(&env)),
        ManagedSource::ProviderKey,
        "a key pasted for inference outranks everything below it"
    );
    assert_eq!(
        managed_source(false, &company, Some(&env)),
        ManagedSource::CompanyAccount,
    );
    assert_eq!(
        managed_source(false, &Credential::None, Some(&env)),
        ManagedSource::Instance,
        "the server's account pays, and the row has to say so"
    );
    assert_eq!(
        managed_source(false, &Credential::None, None),
        ManagedSource::None,
        "nothing resolves — not set up, and not a green badge"
    );
}

#[test]
fn the_two_paying_states_are_not_collapsed() {
    // An operator deciding whether to connect their account needs to know
    // which one they are on. "On" for both hides the decision.
    let env = managed_env();
    assert_ne!(
        managed_source(
            false,
            &Credential::from_company_key("th-account"),
            Some(&env)
        ),
        managed_source(false, &Credential::None, Some(&env)),
    );
}

#[test]
fn an_env_default_that_would_yield_nothing_is_not_availability() {
    // `configured()` rather than presence: a projected-token source reports
    // itself present while its file can still yield nothing, and what
    // decides availability is whether a value would reach the wire.
    let empty = EnvDefault {
        base_url: "https://env.example/openai/v1".into(),
        credential: Credential::None,
    };
    assert_eq!(
        managed_source(false, &Credential::None, Some(&empty)),
        ManagedSource::None
    );
}

// ---- the provider list actually reaches the resolver ---------------------
//
// Every other test in this module seeds `inference/config` or exercises the
// store in isolation, which is exactly how a feature comes to be fully built
// on both sides and connected in neither: the write routes populated
// `inference/providers`, the status route rendered it, and nothing on the
// turn path ever read it. A company that added a provider through the
// console had configured its *display*, not its company — the chat pane said
// "no model configured" and was telling the truth.
//
// These write a provider through the store with NO legacy blob anywhere and
// assert a turn resolves to it.

pub(super) async fn add_indexed(secrets: &MemSecrets, slug: &str, key: &str) {
    let company = CompanyId::new("acme");
    store::put_provider(
        &company,
        secrets,
        store::ProviderDraft {
            slug: slug.to_string(),
            label: slug.to_string(),
            kind: "openai_compatible".to_string(),
            base_url: format!("https://{slug}.example/v1"),
            models: BTreeMap::new(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    secrets
        .set(
            &company,
            &store::provider_key_key(slug),
            SecretValue(key.to_string()),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_provider_added_through_the_console_resolves_for_a_turn() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "acme", "sk-not-a-real-key").await;
    // Deliberately no `inference/config`: this is what a company that only
    // ever used the provider list looks like on disk.
    assert!(
        load_runtime_config(&company, &secrets)
            .await
            .unwrap()
            .is_none(),
        "the legacy blob must be absent for this test to mean anything"
    );

    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .expect("a company with a provider resolves");
    assert_eq!(decl.base_url, "https://acme.example/v1");
    assert_eq!(bearer(&decl).await.as_deref(), Some("sk-not-a-real-key"));
    assert!(decl.key_configured());
}

#[tokio::test]
async fn the_marked_default_is_the_one_a_turn_reaches() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    add_indexed(&secrets, "second", "sk-not-a-real-key-2").await;

    // No marker: list order, which is the behaviour that predates the marker.
    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.base_url, "https://first.example/v1");

    store::set_default_slug(&company, &secrets, "second")
        .await
        .unwrap();
    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        decl.base_url, "https://second.example/v1",
        "marking a default has to move where a turn actually goes, not just a badge"
    );
    assert_eq!(bearer(&decl).await.as_deref(), Some("sk-not-a-real-key-2"));
}

#[tokio::test]
async fn a_disabled_provider_is_not_where_a_turn_goes() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "off", "sk-not-a-real-key-1").await;
    add_indexed(&secrets, "on", "sk-not-a-real-key-2").await;
    store::set_enabled(&company, &secrets, "off", false)
        .await
        .unwrap();

    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.base_url, "https://on.example/v1");
}

#[tokio::test]
async fn the_legacy_blob_still_wins_when_it_is_the_only_thing_there() {
    // Entry zero sorts first in the list, so a company that had one provider
    // before any of this existed keeps resolving exactly where it did. The
    // whole entry-zero design exists to make that true without a migration.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: "openai_compatible".into(),
            base_url: Some("https://legacy.example/v1".into()),
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();
    store_key(&company, &secrets, "sk-not-a-real-key")
        .await
        .unwrap();

    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decl.base_url, "https://legacy.example/v1");
    assert_eq!(bearer(&decl).await.as_deref(), Some("sk-not-a-real-key"));
    assert_eq!(decl.source, InferenceSource::Runtime);
}

#[tokio::test]
async fn nothing_configured_still_resolves_to_nothing() {
    // The list being empty must not become a way to resolve *something*.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    assert!(
        resolve_effective(&company, &Inference::default(), None, &secrets)
            .await
            .unwrap()
            .is_none()
    );
}
