//! Routes and health tests (split out of `store_tests.rs`).

use super::store_tests_support::*;
use super::*;

// ---- routes -------------------------------------------------------------

#[tokio::test]
async fn a_company_with_no_routes_reads_an_empty_table() {
    let secrets = MemSecrets::default();
    assert!(load_routes(&company(), &secrets).await.unwrap().is_empty());
}

#[tokio::test]
async fn routes_round_trip_through_the_grammar_an_operator_types() {
    let secrets = MemSecrets::default();
    let mut routes = Routes::new();
    routes.insert("reasoning-v1".to_string(), ProviderRef::parse("acme:gpt-5"));
    routes.insert("chat-v1".to_string(), ProviderRef::Managed);
    routes.insert("vision-v1".to_string(), ProviderRef::parse("local:llava"));
    save_routes(&company(), &secrets, &routes).await.unwrap();

    let read = load_routes(&company(), &secrets).await.unwrap();
    assert_eq!(read, routes);
    // And the stored form really is the text, so a person reading raw keys
    // sees what they would have typed.
    let raw = secrets
        .get(&company(), ROUTES_KEY)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(raw.contains("acme:gpt-5"), "{raw}");
}

#[tokio::test]
async fn an_unset_route_is_dropped_rather_than_stored_as_empty() {
    let secrets = MemSecrets::default();
    let mut routes = Routes::new();
    routes.insert("chat-v1".to_string(), ProviderRef::Default);
    routes.insert("agentic-v1".to_string(), ProviderRef::parse("acme"));
    save_routes(&company(), &secrets, &routes).await.unwrap();

    let read = load_routes(&company(), &secrets).await.unwrap();
    assert!(
        !read.contains_key("chat-v1"),
        "unset must not persist: {read:?}"
    );
    assert_eq!(read.get("agentic-v1"), Some(&ProviderRef::parse("acme")));
}

#[tokio::test]
async fn an_unreadable_routes_blob_is_an_error_rather_than_silently_empty() {
    // Routes decide where a company's spend goes. Reading a corrupt table as
    // "no routes" would move every workload onto the primary without saying
    // so, which is the silent-demotion failure the resolver refuses.
    let secrets = MemSecrets::default();
    secrets
        .set(&company(), ROUTES_KEY, SecretValue("{oops".into()))
        .await
        .unwrap();
    let err = load_routes(&company(), &secrets).await.unwrap_err();
    assert!(err.to_string().contains("not valid JSON"), "{err}");
}

// ---- health -------------------------------------------------------------

#[tokio::test]
async fn health_is_latched_once_per_failure_episode_not_once_per_retry() {
    let secrets = MemSecrets::default();
    assert!(
        record_health(&company(), &secrets, "acme", "auth", "2026-09-11T09:14:00Z")
            .await
            .unwrap(),
        "the first observation moves the record"
    );
    assert!(
        !record_health(&company(), &secrets, "acme", "auth", "2026-09-11T09:15:00Z")
            .await
            .unwrap(),
        "the same failure again must not move the record"
    );
    let health = load_health(&company(), &secrets).await.unwrap();
    assert_eq!(
        health.get("acme").unwrap().at,
        "2026-09-11T09:14:00Z",
        "the timestamp names when the episode began, not the latest retry"
    );
}

#[tokio::test]
async fn a_state_change_moves_the_record() {
    let secrets = MemSecrets::default();
    record_health(&company(), &secrets, "acme", "auth", "2026-09-11T09:14:00Z")
        .await
        .unwrap();
    assert!(
        record_health(&company(), &secrets, "acme", "ok", "2026-09-11T10:00:00Z")
            .await
            .unwrap()
    );
    let health = load_health(&company(), &secrets).await.unwrap();
    assert_eq!(health.get("acme").unwrap().state, "ok");
    assert_eq!(health.get("acme").unwrap().at, "2026-09-11T10:00:00Z");
}

#[tokio::test]
async fn health_is_per_provider_so_one_rejection_does_not_condemn_a_sibling() {
    // Two providers, one endpoint, two keys: a 401 is an answer about the
    // credential presented, never about the address.
    let secrets = MemSecrets::default();
    record_health(&company(), &secrets, "acme", "auth", "2026-09-11T09:14:00Z")
        .await
        .unwrap();
    record_health(
        &company(),
        &secrets,
        "acme-team",
        "ok",
        "2026-09-11T09:14:00Z",
    )
    .await
    .unwrap();
    let health = load_health(&company(), &secrets).await.unwrap();
    assert_eq!(health.get("acme").unwrap().state, "auth");
    assert_eq!(health.get("acme-team").unwrap().state, "ok");
}

#[tokio::test]
async fn forgetting_health_stops_a_reused_slug_inheriting_a_state() {
    let secrets = MemSecrets::default();
    record_health(&company(), &secrets, "acme", "auth", "2026-09-11T09:14:00Z")
        .await
        .unwrap();
    forget_health(&company(), &secrets, "acme").await.unwrap();
    assert!(
        !load_health(&company(), &secrets)
            .await
            .unwrap()
            .contains_key("acme")
    );
    // Forgetting something that was never there is not an error.
    forget_health(&company(), &secrets, "ghost").await.unwrap();
}

#[tokio::test]
async fn an_unreadable_health_blob_reads_as_nothing_learnt() {
    // The opposite call from routes, and deliberately: health holds no
    // configuration, so failing a status read over it would take the whole
    // page down to preserve a decoration.
    let secrets = MemSecrets::default();
    secrets
        .set(&company(), HEALTH_KEY, SecretValue("{oops".into()))
        .await
        .unwrap();
    assert!(load_health(&company(), &secrets).await.unwrap().is_empty());
}
/// What a routing write actually leaves behind, versus what was asked for.
///
/// The `PUT` route used to answer with the table it built from the **request
/// body**, which made the response a picture of the ask rather than of the
/// state — so any divergence between the two was invisible by construction,
/// and a save that landed nowhere still came back carrying the operator's own
/// intent. This is the smallest concrete divergence, and it is not
/// hypothetical: `save_routes` drops `Default` entries, because an absence is
/// how "nothing set here" is stored. Echoing the request claimed a row had
/// been written that the store deliberately holds nothing for.
#[tokio::test]
async fn a_routing_write_does_not_store_what_it_was_handed() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    let mut asked = Routes::new();
    asked.insert("chat-v1".into(), ProviderRef::parse("acme:gpt-5"));
    // The operator put this row back to "follow the default".
    asked.insert("reasoning-v1".into(), ProviderRef::parse(""));
    save_routes(&company, &secrets, &asked).await.unwrap();

    let stored = load_routes(&company, &secrets).await.unwrap();
    assert_eq!(
        stored.get("chat-v1"),
        Some(&ProviderRef::parse("acme:gpt-5"))
    );
    assert!(
        !stored.contains_key("reasoning-v1"),
        "an unset row is stored as an absence, so a response echoing the request \
         would claim a row that is not there"
    );
    assert_ne!(
        asked, stored,
        "the ask and the stored table differ, which is why the route reads back"
    );
}

/// A routing write that cannot land must not read back as if it had.
#[tokio::test]
async fn a_dropped_routing_write_is_visible_on_the_read_back() {
    let company = CompanyId::new("acme");
    let secrets = FailsWriting {
        inner: MemSecrets::default(),
        failing_key: ROUTES_KEY.to_string(),
    };

    let mut asked = Routes::new();
    asked.insert("chat-v1".into(), ProviderRef::parse("acme:gpt-5"));
    assert!(
        save_routes(&company, &secrets, &asked).await.is_err(),
        "the write itself reports the failure"
    );
    // And the read-back agrees with the store rather than with the ask —
    // which is the property the route now answers from.
    let stored = load_routes(&company, &secrets).await.unwrap();
    assert!(stored.is_empty());
}
