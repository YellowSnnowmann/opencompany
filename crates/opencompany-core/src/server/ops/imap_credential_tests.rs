use super::*;

/// Issue #1770. The `Debug` half of this struct was hand-written *and*
/// tested; the derived `Serialize` over a plain `String` password had no
/// guard and no test, so `serde_json::to_value` over these credentials —
/// or over anything embedding them — emitted the plaintext.
///
/// The assertions go through containers `ImapCredentials` knows nothing
/// about, because the guard now lives on the field's *type*: a struct with
/// a plain `#[derive(Serialize)]`, plus `Option`, `Vec`, a map value and
/// `#[serde(flatten)]` (a genuinely different serde code path), across
/// both `to_string` and `to_value` (also different code paths in
/// `serde_json`), and both `{:?}` and `{:#?}`.
#[test]
fn planted_password_never_reaches_debug_or_serialize() {
    use std::collections::BTreeMap;

    // Obviously fake, and distinctive enough that a substring hit is a
    // real hit. Same sentinel as the other planted-secret tests.
    const FAKE_SECRET: &str = "NOT-A-REAL-KEY-planted-for-tests";

    // Case-**insensitive**: a leak that arrives case-mangled is still a
    // leak, and an exact-case search reads it as clean.
    fn leaks(rendering: &str) -> bool {
        rendering
            .to_ascii_lowercase()
            .contains(&FAKE_SECRET.to_ascii_lowercase())
    }

    // Sanity: the detector detects. Without this every assertion below
    // could be vacuous and still read as green.
    assert!(
        leaks(&format!("password={}", FAKE_SECRET.to_ascii_lowercase())),
        "the leak detector cannot see a lowercased sentinel; every \
         assertion below would be vacuous"
    );

    /// The next struct somebody writes: derives `Serialize` and `Debug`
    /// with no idea a credential is in there.
    #[derive(Debug, Serialize)]
    struct UnsuspectingConfig {
        label: String,
        primary: ImapCredentials,
        optional: Option<ImapCredentials>,
        many: Vec<ImapCredentials>,
        by_name: BTreeMap<String, ImapCredentials>,
        #[serde(flatten)]
        nested: Nested,
    }

    /// Flattened, so serde uses `FlatMapSerializer` rather than the
    /// ordinary struct serializer.
    #[derive(Debug, Serialize)]
    struct Nested {
        inner: ImapCredentials,
    }

    let creds = ImapCredentials {
        host: "imap.example.com".into(),
        port: 993,
        username: "acme@opencompany.work".into(),
        password: SecretValue(FAKE_SECRET.to_string()),
    };
    let config = UnsuspectingConfig {
        label: "tenant mailbox".to_string(),
        primary: creds.clone(),
        optional: Some(creds.clone()),
        many: vec![creds.clone(), creds.clone()],
        by_name: BTreeMap::from([("acme".to_string(), creds.clone())]),
        nested: Nested {
            inner: creds.clone(),
        },
    };

    for rendering in [
        serde_json::to_string(&creds).expect("serializes"),
        serde_json::to_value(&creds)
            .expect("serializes")
            .to_string(),
        serde_json::to_string(&config).expect("serializes"),
        serde_json::to_value(&config)
            .expect("serializes")
            .to_string(),
    ] {
        assert!(!leaks(&rendering), "plaintext reached serde: {rendering}");
    }
    for rendering in [
        format!("{creds:?}"),
        format!("{creds:#?}"),
        format!("{config:?}"),
        format!("{config:#?}"),
    ] {
        assert!(!leaks(&rendering), "plaintext reached Debug: {rendering}");
    }

    // Still diagnosable: everything that is not the credential survives.
    let rendered = format!("{creds:?}");
    assert!(rendered.contains("imap.example.com"), "{rendered}");
    assert!(rendered.contains("993"), "{rendered}");
    assert!(rendered.contains("acme@opencompany.work"), "{rendered}");

    // And the credential itself is still reachable by the one named door,
    // so the poller can still log in.
    assert_eq!(creds.password.expose(), FAKE_SECRET);
}
