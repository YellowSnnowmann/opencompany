use super::adopt_session;
use crate::proxy::{Connection, Credential, ProxyRegistry};

async fn registry_with(id: &str, base_url: &str) -> ProxyRegistry {
    let proxy = ProxyRegistry::new();
    proxy
        .upsert(
            id.to_string(),
            Connection {
                base_url: base_url.to_string(),
                credential: Credential::None,
            },
        )
        .await
        .expect("a bare registration is always accepted");
    proxy
}

/// The bricking sequence issue #1858's review named: a plain-HTTP remote
/// host's sign-in must be refused BEFORE the keychain write, because a
/// stored session the next launch's `oc_connect` presents makes `upsert`
/// refuse the whole registration — a connection unusable until someone
/// finds the hidden keychain entry.
#[tokio::test]
async fn an_insecure_host_is_refused_before_anything_is_stored() {
    let proxy = registry_with("insecure-1", "http://192.168.1.20:8080").await;

    let result = adopt_session(&proxy, "insecure-1".to_string(), "acme.tok".to_string()).await;

    let error = result.expect_err("a credential must not ride plain HTTP off-machine");
    assert!(error.contains("not encrypted"), "{error}");
    assert!(
        crate::keychain::device_session("insecure-1").is_none(),
        "nothing may survive into the keychain for the next launch to trip over"
    );
}

#[tokio::test]
async fn an_unknown_connection_stores_nothing() {
    let proxy = ProxyRegistry::new();

    let result = adopt_session(&proxy, "nobody-1".to_string(), "acme.tok".to_string()).await;

    assert!(result.is_err());
    assert!(crate::keychain::device_session("nobody-1").is_none());
}

#[tokio::test]
async fn an_empty_session_is_refused_outright() {
    let proxy = registry_with("empty-1", "https://acme.example.com").await;

    let result = adopt_session(&proxy, "empty-1".to_string(), "   ".to_string()).await;

    assert!(result.is_err());
    assert!(crate::keychain::device_session("empty-1").is_none());
}

/// The happy path, on the transports a credential may ride: https anywhere,
/// and plain HTTP only to this machine (the embedded host's own case).
#[tokio::test]
async fn a_session_is_kept_where_a_credential_may_travel() {
    for (id, base_url) in [
        ("kept-https", "https://acme.example.com"),
        ("kept-local", "http://127.0.0.1:8080"),
    ] {
        let proxy = registry_with(id, base_url).await;

        adopt_session(&proxy, id.to_string(), "acme.tok".to_string())
            .await
            .expect("a securely-reachable host keeps its sign-in");

        assert_eq!(
            crate::keychain::device_session(id).as_deref(),
            Some("acme.tok"),
            "the next launch's oc_connect reads this back"
        );
    }
}
