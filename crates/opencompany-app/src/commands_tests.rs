use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// The guarantee the console cannot make about itself.
///
/// `pairDevice` in the console returns whatever the core sends, so a mock
/// there proves nothing — this is where "the token never reaches the
/// webview" is actually enforced, by a type that has nowhere to put one.
/// If a `token` field is ever added to `PairedDevice`, this fails.
#[test]
fn a_paired_device_carries_no_token() {
    let wire = serde_json::to_value(PairedDevice {
        company: "acme".into(),
        device_id: "dev-1".into(),
        expires_at_millis: 1,
    })
    .expect("serialise");

    // Sorted, for the same reason as the instance row below: the closed set
    // is the claim. `PairedDevice`'s field order happens to be alphabetical
    // today, so an ordered comparison passes by coincidence rather than by
    // design — and would go red the moment a field is inserted out of that
    // position, for a reason that has nothing to do with what this test is
    // about.
    let mut keys: Vec<&str> = wire
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["company", "deviceId", "expiresAtMillis"],
        "pairing must answer with these three fields and nothing else"
    );
    assert!(!wire.to_string().to_lowercase().contains("token"));
}

/// The keys the console reads off an instance row, by name.
///
/// Same argument as `the_embedded_record_answers_in_the_keys_the_console_reads`:
/// nothing type-checks a Rust struct against the TypeScript that reads it,
/// and every optional field degrades silently. A renamed key lands as "the
/// instance list is full of blank rows", not as an error.
#[test]
fn an_instance_row_answers_in_the_keys_the_console_reads() {
    let wire = serde_json::to_value(LocalInstanceInfo {
        id: "acme".into(),
        label: "Acme".into(),
        data_dir: "/data/instances/acme".into(),
        running: true,
        base_url: Some("http://127.0.0.1:1234".into()),
        instance_id: Some("inst-1".into()),
        companies: vec!["acme".into()],
        error: None,
    })
    .expect("serialise");

    // Sorted before comparing, because the set is what this asserts and the
    // order is not. This crate inherits `serde_json`'s `preserve_order`
    // through its path dependency on `opencompany` (root `Cargo.toml:86`),
    // so a JSON object is backed by an `IndexMap` and emits **struct field
    // order**, not alphabetical order. Pinning the order here asserted a
    // property nothing needs — JSON object order means nothing to the
    // TypeScript that reads these by name — and it would break again the
    // next time a field is added in the middle of the struct.
    let mut keys: Vec<&str> = wire
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "baseUrl",
            "companies",
            "dataDir",
            "id",
            "instanceId",
            "label",
            "running",
        ],
        "the instance row answers in exactly these keys: {wire}"
    );
}

/// A stopped row carries no address, so the console cannot render one that
/// would fail its probe forever.
#[test]
fn a_stopped_instance_carries_no_address() {
    let wire = serde_json::to_value(LocalInstanceInfo {
        id: "acme".into(),
        label: "Acme".into(),
        data_dir: "/data/instances/acme".into(),
        running: false,
        base_url: None,
        instance_id: None,
        companies: Vec::new(),
        error: Some("the data root is in use".into()),
    })
    .expect("serialise");

    let object = wire.as_object().expect("an object");
    assert!(!object.contains_key("baseUrl"));
    assert_eq!(object["error"], "the data root is in use");
    assert_eq!(object["running"], false);
}

/// A one-shot host that answers every request with `head`, then closes.
///
/// Returns its base url and a handle that says whether anything ever
/// connected — which is the assertion for a refusal that must happen
/// *before* the wire, not on the answer that comes back over it.
async fn host(head: &'static str) -> (String, Arc<AtomicBool>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let reached = Arc::new(AtomicBool::new(false));
    let flag = reached.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            flag.store(true, Ordering::SeqCst);
            use tokio::io::AsyncWriteExt as _;
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://{address}"), reached)
}

/// The claim is refused before a socket is opened, not after an answer.
///
/// A token that travelled once has been read; there is no recovering from
/// it by rejecting the response. So this asserts on the connection, not on
/// the `Err` — the message alone would pass on a version that sent the
/// pairing code first and complained afterwards (#731).
#[tokio::test]
async fn pairing_over_an_unencrypted_remote_host_sends_nothing() {
    // A real listener, addressed by a name that is not loopback. The
    // resolver never runs, because the refusal comes first — which is the
    // point.
    let (_, reached) = host("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
    for base in [
        "http://192.168.1.20:8080",
        "http://acme.example.com",
        "http://10.0.0.4:8080",
    ] {
        // `let else` rather than `expect_err`, which would need
        // `ClaimedDevice: Debug` — and a `token` field behind a `{:?}` is
        // the thing the type is shaped to prevent.
        let Err(error) = claim(base, "code-123", None).await else {
            panic!("{base} must not be paired with");
        };
        assert!(
            error.contains("not encrypted"),
            "{base} must be refused for the reason it is refused: {error}"
        );
    }
    assert!(
        !reached.load(Ordering::SeqCst),
        "nothing may be sent to a host the rule refuses"
    );
}

/// Loopback still pairs — the embedded host is reached no other way.
#[tokio::test]
async fn pairing_with_a_host_on_this_machine_still_works() {
    let body = r#"{"token":"t","company":"acme","deviceId":"dev-1","expiresAtMillis":1}"#;
    let head: &'static str = Box::leak(
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_boxed_str(),
    );
    let (base, reached) = host(head).await;

    let claimed = claim(&base, "code-123", Some("a laptop"))
        .await
        .expect("a loopback host pairs");

    assert!(reached.load(Ordering::SeqCst));
    assert_eq!(claimed.company, "acme");
    assert_eq!(claimed.device_id, "dev-1");
}

/// A redirect is not followed, so an https base cannot be walked to http.
///
/// `reqwest`'s default policy follows up to ten, and a 307 re-sends the
/// body — so a host answering `307 → http://…` would put the pairing code
/// on exactly the wire the check above refuses, having passed it. Checking
/// the first url is worth nothing if the client will walk to a second.
#[tokio::test]
async fn a_redirect_away_from_the_checked_host_is_not_followed() {
    let (base, _) = host(
        "HTTP/1.1 307 Temporary Redirect\r\nlocation: http://192.168.1.20:8080/api/v1/devices/claim\r\ncontent-length: 0\r\n\r\n",
    )
    .await;

    let Err(error) = claim(&base, "code-123", None).await else {
        panic!("a redirect is an answer, not a detour to follow");
    };
    // The host's status, passed through — which is what "not followed"
    // looks like from here.
    assert!(error.contains("307"), "{error}");
}

/// The console reads these keys by name, and a rename here is silent on
/// both sides: TypeScript has nothing to check a Rust struct against, and
/// every field is optional in the console precisely so an older shell
/// degrades instead of failing. A wrong `instanceId` therefore lands as a
/// sidebar accumulating one dead connection per launch (#615), not as an
/// error anybody sees.
///
/// The set is deliberately *shrinking* here: `operatorEmail` left with the
/// desktop's sign-in. A console built before that still reads it as
/// optional and simply finds nothing, which is the same degrade an older
/// shell has always got from the other direction.
#[test]
fn the_embedded_record_answers_in_the_keys_the_console_reads() {
    let wire = serde_json::to_value(EmbeddedInfo {
        base_url: "http://127.0.0.1:1234".into(),
        data_dir: "/data".into(),
        instance_id: "inst-1".into(),
    })
    .expect("serialise");

    // Sorted, as above. `EmbeddedInfo`'s field order is simultaneously
    // struct order and alphabetical, which is why this test passed either
    // way and why it could never have established the precedent the
    // instance-row test cited it for.
    let mut keys: Vec<&str> = wire
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["baseUrl", "dataDir", "instanceId"],
        "the embedded record answers in exactly these keys: {wire}"
    );
}
