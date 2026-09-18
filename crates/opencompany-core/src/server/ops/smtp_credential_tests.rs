use super::*;

/// Obviously fake, and distinctive enough that a substring hit is a real
/// hit. Same sentinel as the other planted-secret tests in this crate.
const FAKE_SECRET: &str = "NOT-A-REAL-KEY-planted-for-tests";

/// Case-**insensitive**: a leak that arrives lowercased, uppercased or
/// otherwise case-mangled is still a leak, and an exact-case search reads
/// it as clean.
fn leaks(rendering: &str) -> bool {
    rendering
        .to_ascii_lowercase()
        .contains(&FAKE_SECRET.to_ascii_lowercase())
}

/// Sanity: the detector detects. Without this, every assertion built on
/// [`leaks`] could be vacuous and still read as green.
#[test]
fn the_leak_detector_can_see_a_lowercased_sentinel() {
    assert!(
        leaks(&format!("password={}", FAKE_SECRET.to_ascii_lowercase())),
        "the leak detector cannot see a lowercased sentinel; every \
         assertion in this module would be vacuous"
    );
    assert!(!leaks("password=nothing-planted-here"));
}

/// Issue #1770. `SmtpCredentials` derived both `Debug` and `Serialize`
/// over a plain `String` password, so either surface emitted the
/// plaintext — the `Debug` half was documented as a known leak rather than
/// fixed, and the `Serialize` half was not considered at all.
///
/// The assertions go through containers this struct knows nothing about,
/// because the guard now lives on the field's *type*: [`MailCredentials`]
/// (an externally tagged enum), a struct with a plain
/// `#[derive(Serialize)]`, plus `Option`, `Vec`, a map value and
/// `#[serde(flatten)]` — a genuinely different serde code path — across
/// both `to_string` and `to_value`, and both `{:?}` and `{:#?}`.
#[test]
fn planted_password_never_reaches_debug_or_serialize() {
    use std::collections::BTreeMap;

    /// The next struct somebody writes: derives `Serialize` and `Debug`
    /// with no idea a credential is in there.
    #[derive(Debug, Serialize)]
    struct UnsuspectingConfig {
        label: String,
        primary: SmtpCredentials,
        optional: Option<SmtpCredentials>,
        many: Vec<SmtpCredentials>,
        by_name: BTreeMap<String, SmtpCredentials>,
        tagged: MailCredentials,
        #[serde(flatten)]
        nested: Nested,
    }

    /// Flattened, so serde uses `FlatMapSerializer` rather than the
    /// ordinary struct serializer.
    #[derive(Debug, Serialize)]
    struct Nested {
        inner: SmtpCredentials,
    }

    let creds = SmtpCredentials {
        host: "smtp.example.com".into(),
        port: 587,
        security: SmtpSecurity::Starttls,
        username: "mailer".into(),
        password: SecretValue(FAKE_SECRET.to_string()),
        from_name: "Acme".into(),
        from_email: "ceo@acme.test".into(),
    };
    let config = UnsuspectingConfig {
        label: "company mail".to_string(),
        primary: creds.clone(),
        optional: Some(creds.clone()),
        many: vec![creds.clone(), creds.clone()],
        by_name: BTreeMap::from([("acme".to_string(), creds.clone())]),
        tagged: MailCredentials::Smtp(creds.clone()),
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

    // Still diagnosable, and the credential still reachable by the one
    // named door so the transport can still authenticate.
    let rendered = format!("{creds:?}");
    assert!(rendered.contains("smtp.example.com"), "{rendered}");
    assert!(rendered.contains("ceo@acme.test"), "{rendered}");
    assert_eq!(creds.password.expose(), FAKE_SECRET);
}

/// Issue #1770, the half that is about *persistence* rather than logging.
///
/// [`StoredConfig`] is written back to [`SMTP_KEY`] by [`store_config`] on
/// every save. Its legacy `password` was safe only because
/// `skip_serializing_if = "Option::is_none"` met one construction site that
/// hardcoded `None` — an invariant a second construction site would break
/// silently. Both guards that replaced it are asserted here against a
/// `StoredConfig` that *is* carrying a legacy password, which is exactly
/// the state [`load_config`] produces when it reads a pre-split blob.
#[test]
fn a_legacy_password_is_never_written_back_into_the_stored_blob() {
    let stored: StoredConfig = serde_json::from_value(serde_json::json!({
        "host": "smtp.acme.test",
        "port": 587,
        "security": "starttls",
        "username": "mailer",
        "password": FAKE_SECRET,
        "from_name": "Acme",
        "from_email": "ceo@acme.test",
    }))
    .expect("a pre-split blob still parses");

    // It really did load — otherwise the assertions below pass on a `None`
    // and prove nothing.
    assert_eq!(
        stored.password.as_ref().map(SecretValue::expose),
        Some(FAKE_SECRET),
        "the legacy read path stopped working, so this test is vacuous"
    );

    let as_string = serde_json::to_string(&stored).expect("serializes");
    let as_value = serde_json::to_value(&stored).expect("serializes");

    for rendering in [
        as_string.clone(),
        as_value.to_string(),
        format!("{stored:?}"),
        format!("{stored:#?}"),
    ] {
        assert!(
            !leaks(&rendering),
            "the legacy password escaped: {rendering}"
        );
    }

    // Absent, not redacted. A `"password": "[redacted]"` in the blob would
    // be written back over the pre-split credential and then handed to
    // `load_credentials` as the fallback password.
    assert!(
        as_value.get("password").is_none(),
        "the legacy password key was written back: {as_string}"
    );
    // The rest of the blob still round-trips.
    assert_eq!(as_value["host"], "smtp.acme.test");
    assert_eq!(as_value["username"], "mailer");
    assert_eq!(as_value["from_email"], "ceo@acme.test");
}
