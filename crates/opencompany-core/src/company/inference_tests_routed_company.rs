//! Routed-managed-company tests: a routed managed company is a
//! configured company (split out of `inference_tests.rs`).

use super::inference_tests_support::*;
use super::tests_managed::add_indexed;
use super::tests_routing::{route, wire_model};
use super::*;

// ---- a routed-managed company is a configured company ---------------------
//
// The third instance of tonight's shape, and the one that reached furthest:
// `resolve_effective_scoped` is what `RuntimeBuilder::build` asks "is
// anything configured at all", and it had exactly two branches — the
// provider list, then the legacy chain. Managed lives in neither. It has no
// row in `inference/providers` (it resolves through a credential chain, not
// a record), and a company configured through the console's Managed row
// writes neither the runtime blob nor a manifest block: its credential goes
// to `provider/tinyhumans/key` and its choice goes to `inference/routes`.
//
// So a company routing every tier to `managed`, with a managed key stored,
// resolved `None` — and got the offline echo brain. Restarting the host did
// not help, because a fresh boot ran the identical computation. The
// turn-time resolver knew how to resolve those rows the whole time.

/// Stores a managed credential at the address the console's managed key
/// route writes — the new per-provider slot, not the legacy flat one.
async fn managed_key(secrets: &MemSecrets, key: &str) {
    secrets
        .set(
            &CompanyId::new("acme"),
            &store::provider_key_key(MANAGED_SLUG),
            SecretValue(key.to_string()),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_company_routed_to_managed_resolves_rather_than_landing_on_echo() {
    // The reported company, reproduced exactly: one provider, switched off,
    // every tier routed to `managed`, and a managed key stored.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "anthropic", "sk-not-a-real-key-anthropic").await;
    store::set_enabled(&company, &secrets, "anthropic", false)
        .await
        .unwrap();
    for tier in ["chat-v1", "reasoning-v1", "agentic-v1", "vision-v1"] {
        route(&secrets, tier, "managed").await;
    }
    managed_key(&secrets, "sk-not-a-real-key-managed").await;

    // The unrouted resolver — the one `RuntimeBuilder::build` calls, and the
    // one that used to answer `None` here and strand the company on echo.
    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .expect("a company routed to managed with a managed key is configured");
    assert_eq!(
        bearer(&decl).await.as_deref(),
        Some("sk-not-a-real-key-managed"),
        "the credential is the managed key, reached through the managed chain"
    );
    assert!(
        decl.is_proxied(),
        "managed rides the platform endpoint, which is what entitles it to the chain"
    );

    // And it agrees with the turn-time path, which knew all along — the two
    // must not be able to disagree about whether this company can think.
    let routed = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .unwrap()
    .expect("the routed path resolves too");
    assert_eq!(routed.base_url, decl.base_url);
    assert_eq!(bearer(&routed).await, bearer(&decl).await);
}

#[tokio::test]
async fn routing_to_managed_while_the_switch_is_off_resolves_to_nothing() {
    // The boot path and the turn path have to give the same answer, which is
    // the whole reason this branch calls `managed_decl` rather than forming a
    // second opinion. `resolve_effective_for_tier` *refuses* an explicit
    // `managed` route while the switch is off, so a boot that reported this
    // company configured would select the harness brain and then hand every
    // turn to a resolver that errors — inference that looks live on the
    // status card and fails on contact.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    for tier in ["chat-v1", "reasoning-v1", "agentic-v1", "vision-v1"] {
        route(&secrets, tier, "managed").await;
    }
    managed_key(&secrets, "sk-not-a-real-key-managed").await;
    store::set_managed_enabled(&company, &secrets, false)
        .await
        .unwrap();

    assert!(
        resolve_effective(&company, &Inference::default(), None, &secrets)
            .await
            .unwrap()
            .is_none(),
        "a switched-off Managed is not somewhere a workload can be routed, \
         so it is not what makes this company configured either"
    );

    // The turn path's refusal is the other half of the same statement.
    assert!(
        resolve_effective_for_tier(
            &company,
            &Inference::default(),
            None,
            &secrets,
            &HarnessScope::default(),
            "chat-v1",
        )
        .await
        .is_err(),
        "and the routed path refuses, which is the answer boot now matches"
    );

    // Switching it back on restores it, so the gate is the switch and not
    // the credential — which is untouched throughout.
    store::set_managed_enabled(&company, &secrets, true)
        .await
        .unwrap();
    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .expect("switched back on, the same rows resolve");
    assert_eq!(
        bearer(&decl).await.as_deref(),
        Some("sk-not-a-real-key-managed")
    );
}

#[tokio::test]
async fn routing_to_managed_with_nothing_behind_it_still_resolves_to_nothing() {
    // The other half, and the one that keeps the echo brain meaningful: the
    // new branch must widen "configured" only where something can actually
    // answer. A company that picked Managed and put no credential behind it
    // — no pasted key, no company account, no instance identity, because no
    // env default is passed — has configured nothing, and reporting it
    // configured would take it off the echo brain with nothing to think
    // with. That is the mirror-image bug, and it is worse.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    for tier in ["chat-v1", "reasoning-v1", "agentic-v1", "vision-v1"] {
        route(&secrets, tier, "managed").await;
    }

    assert!(
        resolve_effective(&company, &Inference::default(), None, &secrets)
            .await
            .unwrap()
            .is_none(),
        "a managed route with no credential behind it configures nothing"
    );
}

#[tokio::test]
async fn an_unset_route_is_not_a_managed_route() {
    // `ProviderRef::Default` is an absence, not a choice. It maps to
    // `Resolution::Primary` — the provider list, then the legacy chain, both
    // of which the new branch runs after. Counting it as managed would
    // report every company with a managed key configured regardless of what
    // its routing table says, and would quietly disagree with where the turn
    // actually goes.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    managed_key(&secrets, "sk-not-a-real-key-managed").await;

    assert!(
        !resolve::any_route_is_managed(&store::load_routes(&company, &secrets).await.unwrap()),
        "an empty routing table names managed nowhere"
    );
    assert!(
        resolve_effective(&company, &Inference::default(), None, &secrets)
            .await
            .unwrap()
            .is_none(),
        "a stored managed key with no route pointing at it does not configure the company"
    );
}

#[tokio::test]
async fn the_managed_branch_runs_last_and_changes_no_company_that_already_resolved() {
    // Precedence, asserted rather than assumed: the new branch is a tail, so
    // a company whose provider list already answers keeps answering there
    // even with every tier routed to managed and a managed key stored.
    // Widening a resolver is only safe if it can turn `None` into `Some` and
    // nothing else.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    for tier in ["chat-v1", "reasoning-v1", "agentic-v1", "vision-v1"] {
        route(&secrets, tier, "managed").await;
    }
    managed_key(&secrets, "sk-not-a-real-key-managed").await;

    let decl = resolve_effective(&company, &Inference::default(), None, &secrets)
        .await
        .unwrap()
        .expect("the provider list still answers");
    assert_eq!(decl.base_url, "https://first.example/v1");
    assert_eq!(bearer(&decl).await.as_deref(), Some("sk-not-a-real-key-1"));
}

#[tokio::test]
async fn a_route_naming_entry_zero_reaches_the_legacy_config() {
    // Entry zero is the legacy blob wearing a provider's clothes. A route
    // that names it has to reach the blob's own endpoint and credential —
    // not whichever indexed provider happens to be the primary.
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
    store_key(&company, &secrets, "sk-not-a-real-key-legacy")
        .await
        .unwrap();
    add_indexed(&secrets, "second", "sk-not-a-real-key-2").await;
    store::set_default_slug(&company, &secrets, "second")
        .await
        .unwrap();

    let zero = store::list_providers(&company, &secrets).await.unwrap()[0].clone();
    route(&secrets, "chat-v1", &format!("{}:gpt-5", zero.slug)).await;

    let decl = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(decl.base_url, "https://legacy.example/v1");
    assert_eq!(
        bearer(&decl).await.as_deref(),
        Some("sk-not-a-real-key-legacy")
    );
    assert_eq!(wire_model(&decl, "chat-v1"), "gpt-5");
}

#[tokio::test]
async fn a_tier_nobody_routes_resolves_exactly_as_it_always_did() {
    // `embedding-v1` and friends have no row on the Routing tab. They must
    // not acquire one by accident, and they must not fail closed either.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;

    let decl = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "embedding-v1",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(decl.base_url, "https://first.example/v1");
}
