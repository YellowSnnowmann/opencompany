use super::*;

#[test]
fn default_signer_is_deterministic_and_prefixed() {
    let signer = DefaultHashSigner;
    let a = signer.sign("secret", b"body");
    let b = signer.sign("secret", b"body");
    assert_eq!(a, b);
    assert!(a.starts_with("kh1="));
    // A different secret or body changes the signature.
    assert_ne!(a, signer.sign("other", b"body"));
    assert_ne!(a, signer.sign("secret", b"other"));
}

#[tokio::test]
async fn recording_sink_captures_deliveries() {
    let (config, sink) = WebhookConfig::recording("s3cret");
    let event = WebhookEvent::now(
        WebhookKind::ApprovalRequested,
        CompanyId::new("acme"),
        serde_json::json!({ "approval_id": "a1" }),
    );
    config.emit(&event).await;

    let delivered = sink.delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0.kind, WebhookKind::ApprovalRequested);
    assert!(delivered[0].1.starts_with("kh1="));
}

/// A sink whose first `fail_first` calls return an error, so `emit`'s
/// bounded retry has something real to exercise. Every attempt — failing
/// and succeeding alike — is recorded, so a test can tell "retried and
/// then delivered" from "delivered on the first try".
#[derive(Clone, Default)]
struct FlakySink {
    attempts: Arc<Mutex<u32>>,
    fail_first: u32,
}

impl FlakySink {
    fn new(fail_first: u32) -> Self {
        Self {
            attempts: Arc::default(),
            fail_first,
        }
    }

    fn attempts(&self) -> u32 {
        *self.attempts.lock().expect("attempts poisoned")
    }
}

#[async_trait::async_trait]
impl WebhookSink for FlakySink {
    async fn deliver(&self, _event: &WebhookEvent, _signature: &str) -> Result<()> {
        let mut attempts = self.attempts.lock().expect("attempts poisoned");
        *attempts += 1;
        if *attempts <= self.fail_first {
            Err(crate::error::OpenCompanyError::Store(
                "flaky sink: simulated failure".to_string(),
            ))
        } else {
            Ok(())
        }
    }
}

fn event() -> WebhookEvent {
    WebhookEvent::now(
        WebhookKind::WorkCompleted,
        CompanyId::new("acme"),
        serde_json::Value::Null,
    )
}

/// A sink that fails on the first attempt but succeeds on the retry must
/// end up delivered — `emit`'s "a few bounded attempts" is dead code
/// without a sink that ever returns `Err` at all, and `RecordingWebhookSink`
/// never does.
#[tokio::test]
async fn emit_retries_a_transient_failure_and_still_delivers() {
    let sink = FlakySink::new(2);
    let config = WebhookConfig {
        sink: Arc::new(sink.clone()),
        signer: Arc::new(DefaultHashSigner),
        secret: "s3cret".to_string(),
    };
    config.emit(&event()).await;
    assert_eq!(
        sink.attempts(),
        3,
        "two failures then a success is three attempts"
    );
}

/// A sink that fails every attempt must not panic or block the caller —
/// `emit` logs and swallows after the bound, exactly as a delivery that
/// eventually succeeds does. Bounded, not unbounded: exactly the three
/// documented attempts, not one more.
#[tokio::test]
async fn emit_gives_up_after_the_bound_without_panicking() {
    let sink = FlakySink::new(u32::MAX);
    let config = WebhookConfig {
        sink: Arc::new(sink.clone()),
        signer: Arc::new(DefaultHashSigner),
        secret: "s3cret".to_string(),
    };
    config.emit(&event()).await;
    assert_eq!(sink.attempts(), 3, "must stop at the documented bound");
}

/// `RecordingWebhookSink` is `Mutex`-guarded so concurrent cycles across
/// different companies can emit webhooks at the same time without losing
/// or corrupting a delivery. Prove it under genuine concurrent access
/// rather than only the single-threaded call the existing test makes.
#[tokio::test]
async fn recording_sink_loses_nothing_under_concurrent_emits() {
    const N: usize = 50;
    let (config, sink) = WebhookConfig::recording("s3cret");
    let config = Arc::new(config);

    let mut tasks = Vec::with_capacity(N);
    for i in 0..N {
        let config = config.clone();
        tasks.push(tokio::spawn(async move {
            let event = WebhookEvent::now(
                WebhookKind::FeedbackCreated,
                CompanyId::new(format!("company-{i}")),
                serde_json::json!({ "i": i }),
            );
            config.emit(&event).await;
        }));
    }
    for task in tasks {
        task.await.expect("a concurrent emit must not panic");
    }

    assert_eq!(sink.count(), N);
    let delivered = sink.delivered();
    let mut seen: Vec<usize> = delivered
        .iter()
        .map(|(event, _)| event.company_id.as_ref().to_string())
        .map(|id| id.strip_prefix("company-").unwrap().parse().unwrap())
        .collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..N).collect::<Vec<_>>(),
        "every emit must land exactly once"
    );
}

#[test]
fn webhook_event_serializes_type_and_snake_case_kind() {
    let event = WebhookEvent::now(
        WebhookKind::WorkCompleted,
        CompanyId::new("acme"),
        serde_json::Value::Null,
    );
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "work_completed");
    assert_eq!(json["company_id"], "acme");
}

// -----------------------------------------------------------------
// PLAT-050: `HmacSha256Signer` and `HttpWebhookSink` — the only two
// pieces a real deployment uses — behind `webhooks`. Every test above
// this point exercises `DefaultHashSigner` / `RecordingWebhookSink`
// only.
// -----------------------------------------------------------------

/// A local, in-process HTTP server the tests below post to. Not the
/// network `HttpWebhookSink` is forbidden from reaching — a loopback
/// listener this test process owns and tears down, the same pattern
/// `chargebee::client::test` uses for its own outbound-HTTP-client
/// coverage.
#[cfg(feature = "webhooks")]
struct CapturedRequest {
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
}

#[cfg(feature = "webhooks")]
async fn capturing_mock_server() -> (
    String,
    Arc<Mutex<Vec<CapturedRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    let captured: Arc<Mutex<Vec<CapturedRequest>>> = Arc::default();
    let captured_for_handler = captured.clone();
    let app = axum::Router::new().fallback(axum::routing::any(
        move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
            let captured = captured_for_handler.clone();
            async move {
                captured
                    .lock()
                    .expect("captured poisoned")
                    .push(CapturedRequest { headers, body });
                axum::http::StatusCode::OK
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), captured, server)
}

/// PLAT-050 (AUTH / STATE): a real `HttpWebhookSink::deliver` actually
/// reaches an HTTP receiver and carries a signature that receiver can
/// independently authenticate — a keyed HMAC over the exact body it
/// received, computed with the correct secret and rejected under any
/// other. Nothing before this test ever drove `HttpWebhookSink` over a
/// real connection at all.
#[cfg(feature = "webhooks")]
#[tokio::test]
async fn http_webhook_sink_delivers_a_signature_a_receiver_can_authenticate() {
    let (url, captured, server) = capturing_mock_server().await;
    let config = WebhookConfig {
        sink: Arc::new(HttpWebhookSink::new(url)),
        signer: Arc::new(HmacSha256Signer),
        secret: "s3cret".to_string(),
    };
    let event = WebhookEvent::now(
        WebhookKind::ApprovalRequested,
        CompanyId::new("acme"),
        serde_json::json!({ "approval_id": "a1" }),
    );
    config.emit(&event).await;

    let requests = captured.lock().expect("captured poisoned");
    assert_eq!(
        requests.len(),
        1,
        "exactly one delivery must reach the receiver"
    );
    let request = &requests[0];
    let signature = request
        .headers
        .get(HttpWebhookSink::SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .expect("signature header present");

    // The receiver's own independent check: recompute the HMAC over the
    // exact bytes it received, with the correct secret.
    let expected = HmacSha256Signer.sign("s3cret", &request.body);
    assert_eq!(
        signature, expected,
        "the receiver must be able to authenticate the delivery itself"
    );
    // And under the wrong secret, authentication must fail — the
    // signature is not just present, it is actually keyed.
    let wrong = HmacSha256Signer.sign("not-the-secret", &request.body);
    assert_ne!(signature, wrong);
    server.abort();
}

/// PLAT-050 (FAIL): a receiver that answers non-2xx must be reported as
/// a delivery failure, not swallowed as success — the distinction
/// `emit`'s bounded retry depends on to know whether to try again.
#[cfg(feature = "webhooks")]
#[tokio::test]
async fn http_webhook_sink_reports_a_non_success_status_as_an_error() {
    let app = axum::Router::new().fallback(axum::routing::any(|| async {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let sink = HttpWebhookSink::new(format!("http://{addr}"));
    let event = WebhookEvent::now(
        WebhookKind::WorkCompleted,
        CompanyId::new("acme"),
        serde_json::Value::Null,
    );
    let result = sink.deliver(&event, "sha256=deadbeef").await;
    assert!(
        result.is_err(),
        "a 500 response must be reported as a delivery failure"
    );
    server.abort();
}

/// PLAT-050 (CONC): production concurrency is many companies' cycles
/// calling `emit` on one tenant's `Arc<WebhookConfig>` — one shared
/// `HttpWebhookSink`, one shared `reqwest::Client`, many callers. This
/// drives that same single sink with real concurrent deliveries and
/// proves none are lost or corrupted in transit, matching
/// `recording_sink_loses_nothing_under_concurrent_emits` for the sink a
/// real deployment actually uses.
#[cfg(feature = "webhooks")]
#[tokio::test]
async fn http_webhook_sink_loses_nothing_under_concurrent_delivery() {
    const N: usize = 20;
    let (url, captured, server) = capturing_mock_server().await;
    let sink = Arc::new(HttpWebhookSink::new(url));

    let mut tasks = Vec::with_capacity(N);
    for i in 0..N {
        let sink = sink.clone();
        tasks.push(tokio::spawn(async move {
            let event = WebhookEvent::now(
                WebhookKind::ApprovalRequested,
                CompanyId::new(format!("company-{i}")),
                serde_json::json!({ "i": i }),
            );
            sink.deliver(&event, &format!("sha256={i:064x}")).await
        }));
    }
    for task in tasks {
        assert!(
            task.await
                .expect("a concurrent delivery must not panic")
                .is_ok(),
            "every concurrent delivery to one shared sink must succeed"
        );
    }

    let requests = captured.lock().expect("captured poisoned");
    assert_eq!(
        requests.len(),
        N,
        "every delivery must reach the receiver exactly once"
    );
    let mut seen: Vec<usize> = requests
        .iter()
        .map(|r| {
            let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            body["company_id"]
                .as_str()
                .unwrap()
                .strip_prefix("company-")
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..N).collect::<Vec<_>>(),
        "none dropped, none duplicated"
    );
    server.abort();
}

/// PLAT-050 (LIMIT / BOUND): the sink the runtime builds carries the
/// deadline, not just one a test can construct.
///
/// Exercising a timeout through a sink the case builds itself proves the
/// mechanism and nothing about the wiring: `new` could stop setting one
/// and that case would still pass. This reads the deadline off the
/// constructor the runtime actually calls.
#[cfg(feature = "webhooks")]
#[test]
fn the_sink_the_runtime_builds_carries_a_delivery_deadline() {
    let sink = HttpWebhookSink::new("http://example.invalid/hook");
    assert_eq!(
        sink.timeout,
        HttpWebhookSink::DELIVERY_TIMEOUT,
        "a delivery with no deadline hangs every bounded retry behind it"
    );
    assert!(
        sink.timeout <= Duration::from_secs(30),
        "the deadline must be short enough to keep a cycle moving: {:?}",
        sink.timeout
    );
}

/// PLAT-050 (LIMIT / BOUND): a receiver that accepts the connection and
/// never answers must not hang `deliver` forever — `reqwest::Client::new()`
/// alone carries no timeout, so this is the one failure mode `emit`'s
/// bounded-attempt retry cannot protect against on its own: an attempt that
/// never *returns* at all rather than one that returns quickly with an
/// error.
#[cfg(feature = "webhooks")]
#[tokio::test]
async fn http_webhook_sink_times_out_rather_than_hanging_forever() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    // Accepts the TCP connection and then never reads or writes anything
    // — a receiver that hung mid-request, not one that refused the
    // connection outright (which `reqwest` would fail on immediately
    // regardless of any timeout).
    let server = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            // Hold the connection open and do nothing with it.
            std::mem::forget(socket);
        }
    });

    let sink = HttpWebhookSink::with_timeout(
        format!("http://{addr}"),
        std::time::Duration::from_millis(200),
    );
    let event = WebhookEvent::now(
        WebhookKind::WorkCompleted,
        CompanyId::new("acme"),
        serde_json::Value::Null,
    );
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sink.deliver(&event, "sha256=deadbeef"),
    )
    .await;
    server.abort();

    let delivered = outcome.expect(
        "deliver must return on its own within the client timeout, not hang until this \
         test's outer 5s bound",
    );
    assert!(
        delivered.is_err(),
        "a receiver that never responds must be reported as a failed delivery"
    );
}
