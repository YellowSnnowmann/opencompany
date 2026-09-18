use super::*;

/// A route the host allows longer than the core's default gets what it
/// asked for, and a renderer cannot ask for forever.
///
/// The bug: the core applied a flat 30-second `reqwest` timeout that the
/// console could not see, while the host deliberately allows the teammate
/// design pass 90 seconds. Every slow-but-valid design on the desktop app
/// came back as a transport failure and handed the operator the full form
/// — a refusal for a pass that was working.
#[test]
fn a_request_deadline_is_honoured_clamped_and_defaulted() {
    // What `designTeammate` asks for: the host's 90s plus the round trip.
    assert_eq!(
        request_timeout(Some(105_000)),
        Duration::from_millis(105_000),
        "a route that names its own deadline must get it"
    );
    // An older console, or a caller with nothing to say.
    assert_eq!(request_timeout(None), DEFAULT_REQUEST_TIMEOUT);
    // Zero is not "unbounded" — nothing in the console means that, and
    // reading it as unbounded would turn a bug into a hung connection.
    assert_eq!(request_timeout(Some(0)), DEFAULT_REQUEST_TIMEOUT);
    // The value arrives from the webview, so it is capped.
    assert_eq!(request_timeout(Some(u64::MAX)), MAX_REQUEST_TIMEOUT);
    assert!(
        MAX_REQUEST_TIMEOUT > Duration::from_secs(90),
        "the cap has to sit above the host's longest deliberate deadline"
    );
}

/// A one-shot host that answers with the request head it received.
///
/// Deliberately raw TCP rather than a framework: what is under test is
/// which bytes leave this process, and anything that parses the request
/// into a map on the way in could normalise away the very duplicate the
/// test exists to catch.
async fn reflector() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        // Read to the end of the head only; a body would block a reader
        // that does not know the content length.
        while !head.ends_with(b"\r\n\r\n") {
            use tokio::io::AsyncReadExt as _;
            if socket.read_exact(&mut byte).await.is_err() {
                break;
            }
            head.push(byte[0]);
        }
        use tokio::io::AsyncWriteExt as _;
        let body = String::from_utf8_lossy(&head).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.shutdown().await;
    });
    format!("http://{addr}")
}

/// The webview must not be able to choose what a request authenticates as.
///
/// `RequestBuilder::header` appends, and axum reads `HeaderMap::get`, which
/// takes the first value — so a caller-supplied session header that reached
/// the wire would be the one the host honoured, and the connection's own
/// credential would be the one ignored.
#[tokio::test]
async fn a_caller_cannot_supply_the_credential_headers() {
    let base = reflector().await;
    let registry = ProxyRegistry::new();
    registry
        .upsert(
            "primary".into(),
            Connection {
                base_url: base,
                credential: Credential::Device("acme.the-real-token".into()),
            },
        )
        .await
        .expect("an absolute host url");

    let mut headers = HashMap::new();
    headers.insert(
        "x-opencompany-session".to_string(),
        "evil.stolen".to_string(),
    );
    headers.insert("Authorization".to_string(), "Bearer evil".to_string());
    headers.insert("cookie".to_string(), "oc_session_acme=evil".to_string());
    // An ordinary header must still get through; this is a filter, not a
    // seal.
    headers.insert("content-type".to_string(), "application/json".to_string());

    let reflected = registry
        .request(
            "primary",
            ProxyRequest {
                method: "POST".into(),
                path: "/api/v1/anything".into(),
                headers,
                body: None,
                timeout_ms: None,
            },
        )
        .await
        .expect("the reflector answers")
        .text
        .to_ascii_lowercase();

    assert!(
        reflected.contains("x-opencompany-session: acme.the-real-token"),
        "the connection's own credential must be on the wire: {reflected}"
    );
    assert_eq!(
        reflected.matches("x-opencompany-session:").count(),
        1,
        "exactly one session header, or the host reads the wrong one: {reflected}"
    );
    for forbidden in ["evil.stolen", "bearer evil", "oc_session_acme=evil"] {
        assert!(
            !reflected.contains(forbidden),
            "{forbidden:?} must not reach the host: {reflected}"
        );
    }
    assert!(
        reflected.contains("content-type: application/json"),
        "an ordinary header must still pass: {reflected}"
    );
}

#[test]
fn credential_header_matching_ignores_case() {
    // A caller sends whatever casing it likes; HTTP header names are
    // case-insensitive, so a case-sensitive filter would be no filter.
    let reserved = |name: &str| RESERVED_HEADERS.contains(&name.to_ascii_lowercase().as_str());
    for name in ["Authorization", "AUTHORIZATION", "X-OpenCompany-Session"] {
        assert!(reserved(name), "{name} must be filtered");
    }
    for name in ["content-type", "accept", "x-request-id"] {
        assert!(!reserved(name), "{name} must pass");
    }
}

#[tokio::test]
async fn the_public_accessor_hands_back_no_credential() {
    // `get` is private so a credential cannot leave this registry. The one
    // public accessor exists for a caller that needs somewhere to re-point
    // a connection, and it must stay a url — a `Connection` here would put
    // `Credential::Device` one `serde` derive away from the webview.
    let registry = ProxyRegistry::new();
    registry
        .upsert(
            "primary".into(),
            Connection {
                base_url: "https://acme.test".into(),
                credential: Credential::Device("acme.the-secret".into()),
            },
        )
        .await
        .expect("an absolute host url");

    let url = registry
        .base_url("primary")
        .await
        .expect("a registered host");
    assert_eq!(url, "https://acme.test");
    // The type is what enforces this; the assertion is here so a future
    // widening of the return type has to delete a test that says why.
    assert!(!url.contains("the-secret"));
}

#[test]
fn joining_never_doubles_a_slash() {
    assert_eq!(join("http://h.test", "/api/v1"), "http://h.test/api/v1");
    assert_eq!(join("http://h.test/", "/api/v1"), "http://h.test/api/v1");
    assert_eq!(join("http://h.test/", "api/v1"), "http://h.test/api/v1");
}

/// A base url with no authority is not a host, and saying so early is the
/// whole point.
///
/// This test used to assert the opposite — that `join("", "/api/v1")`
/// giving `/api/v1` was fine, on the theory that an empty base meant
/// "same origin, port not known yet". Nothing here has an origin to be the
/// same as: the embedded host reports a real `127.0.0.1:<port>` once it
/// binds, and `reqwest` cannot request a relative url from any of them. The
/// comment made an unreachable connection look intentional, and the desktop
/// duly shipped one (issue #613).
#[tokio::test]
async fn a_base_url_that_names_no_host_is_refused_at_registration() {
    let registry = ProxyRegistry::new();
    for base in [
        "",
        "   ",
        "/api/v1",
        "acme.test",
        // Valid urls, both of them, and neither is a host this client can
        // send a request to. Parsing is not the question; addressability is.
        "mailto:user@example.com",
        "ftp://host",
    ] {
        let error = registry
            .upsert(
                "primary".into(),
                Connection {
                    base_url: base.into(),
                    credential: Credential::None,
                },
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("not an absolute host url"),
            "{base:?} must be refused by name, not silently: {error}"
        );
    }
    // Refused means not registered, rather than registered and broken.
    assert!(registry.ids().await.is_empty());
}

/// A session must not be handed to a host anyone on the path can read.
///
/// Every request for a connection carries `apply_credential`'s header, so a
/// device session registered against a plain-HTTP LAN address is on the
/// wire in the clear on every poll and for the whole life of the event
/// stream — and unlike a leaked request body, it is replayable (#731).
#[tokio::test]
async fn a_credential_is_refused_on_an_unencrypted_remote_host() {
    let registry = ProxyRegistry::new();
    for base in [
        "http://192.168.1.20:8080",
        "http://acme.example.com",
        // Not loopback despite the name. A host may call itself whatever it
        // likes, and only the reserved name resolves here by rule.
        "http://localhost.acme.example.com",
        // The private ranges are the ones this exists for, not exceptions
        // to it: an office LAN is exactly where someone else is on the path.
        "http://10.0.0.4:8080",
    ] {
        for credential in [
            Credential::Device("acme.the-session".into()),
            Credential::Platform("a-bearer".into()),
        ] {
            let error = registry
                .upsert(
                    "primary".into(),
                    Connection {
                        base_url: base.into(),
                        credential,
                    },
                )
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("this host is not encrypted"),
                "{base:?} must be refused for the reason it is refused: {error}"
            );
        }
    }
    assert!(registry.ids().await.is_empty());
}

/// The three ways a connection stays legitimate under that rule.
#[tokio::test]
async fn https_loopback_and_anonymous_http_all_still_register() {
    let registry = ProxyRegistry::new();
    let cases = [
        // HTTPS carries a credential anywhere.
        (
            "https",
            "https://acme.example.com",
            Credential::Device("acme.s".into()),
        ),
        // Loopback is how the embedded host is reached, and it is the one
        // address a certificate cannot be issued for — `embedded.rs` binds
        // `127.0.0.1:0`, so the port is different every launch.
        (
            "v4",
            "http://127.0.0.1:65364",
            Credential::Device("acme.s".into()),
        ),
        // The rest of `127.0.0.0/8`, which a second local host may bind.
        (
            "v4-block",
            "http://127.0.0.2:8080",
            Credential::Device("acme.s".into()),
        ),
        (
            "v6",
            "http://[::1]:8080",
            Credential::Device("acme.s".into()),
        ),
        // What a developer types into `?api=`.
        (
            "name",
            "http://localhost:8080",
            Credential::Device("acme.s".into()),
        ),
        // Anonymous HTTP anywhere: nothing is exposed that a passer-by
        // could not have asked the host for themselves. This is the case
        // the narrow rule exists to keep working — a home-lab or staging
        // box without a certificate stays readable.
        ("anonymous", "http://192.168.1.20:8080", Credential::None),
    ];
    for (id, base, credential) in cases {
        registry
            .upsert(
                id.into(),
                Connection {
                    base_url: base.into(),
                    credential,
                },
            )
            .await
            .unwrap_or_else(|error| panic!("{base:?} must still register: {error}"));
    }
    assert_eq!(registry.ids().await.len(), 6);
}

/// The shorthand spellings of a loopback address, and of everything else.
///
/// `Url::parse` normalises `127.1` and `0177.0.0.1` to `127.0.0.1`, so a
/// textual comparison cannot be walked past — asserted here because that is
/// a property of the parser rather than of this module, and a change to it
/// would otherwise turn into a silently widened rule.
#[test]
fn the_loopback_test_reads_addresses_rather_than_strings() {
    for allowed in [
        "http://127.1:8080",
        "http://0177.0.0.1:8080",
        "http://[::ffff:127.0.0.1]:8080",
        "http://sub.localhost:8080",
        "https://acme.example.com",
    ] {
        assert!(may_carry_a_credential(allowed), "{allowed} must be allowed");
    }
    for refused in [
        "http://192.168.1.20:8080",
        "http://127.0.0.1.acme.example.com",
        "http://acme.example.com",
        // Not a url at all, and not addressable either; `upsert` refuses it
        // first, but this must not answer "yes" on its own.
        "not-a-url",
    ] {
        assert!(
            !may_carry_a_credential(refused),
            "{refused} must be refused"
        );
    }
}

#[tokio::test]
async fn an_unknown_connection_is_named_rather_than_guessed() {
    let registry = ProxyRegistry::new();
    let error = registry
        .request(
            "nope",
            ProxyRequest {
                method: "GET".into(),
                path: "/healthz".into(),
                headers: HashMap::new(),
                body: None,
                timeout_ms: None,
            },
        )
        .await
        .expect_err("an unregistered connection has no host to reach");
    assert!(error.to_string().contains("nope"));
}

#[tokio::test]
async fn connections_are_independent_entries_not_one_active_slot() {
    // The buzz regression, at the Rust boundary: two hosts coexist, and
    // removing one leaves the other addressable.
    let registry = ProxyRegistry::new();
    registry
        .upsert(
            "a".into(),
            Connection {
                base_url: "http://a.test".into(),
                credential: Credential::None,
            },
        )
        .await
        .expect("an absolute host url");
    registry
        .upsert(
            "b".into(),
            Connection {
                base_url: "http://b.test".into(),
                credential: Credential::None,
            },
        )
        .await
        .expect("an absolute host url");

    let mut ids = registry.ids().await;
    ids.sort();
    assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);

    registry.remove("a").await;
    assert_eq!(registry.ids().await, vec!["b".to_string()]);
}
