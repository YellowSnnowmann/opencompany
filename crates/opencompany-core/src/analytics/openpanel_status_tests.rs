//! The transport's status bookkeeping and first-send self-check: every arm of
//! `Inner::drain` must record what happened, and the first outcome must be said
//! out loud exactly once, at a level the `serve` default filter lets through
//! when it is bad news.
//!
//! Log levels are asserted through a capturing `tracing` layer that reads
//! `Metadata::level()` itself (precedent: `runtime/builder_tests_part3.rs`),
//! because a rendered line cannot tell a `warn!` from an `error!`.

use super::*;
use crate::analytics::config::{CLIENT_ID_ENV, ENDPOINT_ENV, resolve};
use crate::analytics::types::OpaqueId;
use crate::analytics::{Event, LastSend};
use crate::app::config::MapEnv;
use crate::app::deployment::{DEPLOYMENT_ENV, Deployment};
use crate::ports::brain::Cognition;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// An obviously-fake client id. Never a real one, in a file or anywhere else.
const FAKE_CLIENT_ID: &str = "fake-status-client-id-7f3a";

/// Every event the capture layer saw: level, then message and fields rendered.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<(tracing::Level, String, String)>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Logs {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct Render(String);
        impl tracing::field::Visit for Render {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push_str(&format!("{}={value:?} ", field.name()));
            }
        }
        let mut rendered = Render(String::new());
        event.record(&mut rendered);
        self.0.lock().unwrap().push((
            *event.metadata().level(),
            event.metadata().target().to_string(),
            rendered.0,
        ));
    }
}

impl Logs {
    /// Installs this capture for the current thread until the guard drops. The
    /// tests run on tokio's current-thread runtime, so a drain awaited from the
    /// test body logs on this thread.
    fn install(&self) -> tracing::subscriber::DefaultGuard {
        use tracing_subscriber::layer::SubscriberExt;
        tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone()))
    }

    /// This module's own lines at `level`. Filtered by target because the
    /// capture sees every crate on the thread, and `hyper`/`reqwest` emit their
    /// own `debug!`s while a request is in flight.
    fn at(&self, level: tracing::Level) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(l, target, _)| *l == level && target.starts_with("opencompany::analytics"))
            .map(|(_, _, text)| text.clone())
            .collect()
    }

    fn everything(&self) -> String {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, text)| text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A collector that answers every POST with `status` and counts the requests.
async fn collector(
    status: axum::http::StatusCode,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let app = axum::Router::new().route(
        "/track",
        axum::routing::post(move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                if status.is_redirection() {
                    return (
                        status,
                        [(axum::http::header::LOCATION, "http://127.0.0.1:1/elsewhere")],
                    )
                        .into_response();
                }
                status.into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/track", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (url, hits, handle)
}

use axum::response::IntoResponse;

fn tracker_for(endpoint: &str) -> Arc<dyn Tracker> {
    let env = MapEnv::new(vec![
        (CLIENT_ID_ENV, FAKE_CLIENT_ID),
        (ENDPOINT_ENV, endpoint),
        (DEPLOYMENT_ENV, "hosted-tenant"),
    ]);
    build(
        &resolve(Deployment::from_env(&env), &env),
        Envelope::new(
            OpaqueId::instance("0123456789abcdef0123456789abcdef"),
            Deployment::HostedTenant,
            Cognition::default(),
        ),
    )
}

fn track(tracker: &Arc<dyn Tracker>, n: usize) {
    for _ in 0..n {
        tracker.track(Event::InstanceStarted {
            companies: 1,
            storage: "fs",
            setup_complete: true,
        });
    }
}

#[tokio::test]
async fn analytics_status_before_any_send_is_reporting_with_nothing_sent() {
    let (url, _hits, server) = collector(axum::http::StatusCode::OK).await;
    let status = tracker_for(&url)
        .status()
        .expect("the transport has a status");

    assert_eq!(status.decision, "reporting");
    assert_eq!(status.deployment, "hosted-tenant");
    assert!(status.in_build);
    assert_eq!(status.last_send, LastSend::Never);
    assert_eq!((status.accepted, status.dropped), (0, 0));
    assert!(status.last_at.is_none() && status.last_status.is_none());
    server.abort();
}

#[tokio::test]
async fn analytics_accepted_sends_are_counted_and_the_first_is_said_once_at_info() {
    let logs = Logs::default();
    let _guard = logs.install();
    let (url, hits, server) = collector(axum::http::StatusCode::OK).await;
    let tracker = tracker_for(&url);

    track(&tracker, 3);
    tracker.flush().await;
    track(&tracker, 2);
    tracker.flush().await;

    let status = tracker.status().unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 5);
    assert_eq!(status.last_send, LastSend::Accepted);
    assert_eq!(status.last_status, Some(200));
    assert_eq!((status.accepted, status.dropped), (5, 0));
    assert!(status.last_at.is_some());

    let info = logs.at(tracing::Level::INFO);
    assert_eq!(info.len(), 1, "exactly one first-send line: {info:?}");
    assert!(info[0].contains("accepted the first event (HTTP 200)"));
    assert!(logs.at(tracing::Level::WARN).is_empty());
    assert!(!logs.everything().contains(FAKE_CLIENT_ID));
    server.abort();
}

#[tokio::test]
async fn analytics_a_refused_credential_warns_exactly_once_and_names_no_client_id() {
    let logs = Logs::default();
    let _guard = logs.install();
    let (url, hits, server) = collector(axum::http::StatusCode::UNAUTHORIZED).await;
    let tracker = tracker_for(&url);

    track(&tracker, 3);
    tracker.flush().await;
    track(&tracker, 3);
    tracker.flush().await;

    let status = tracker.status().unwrap();
    assert_eq!(status.last_send, LastSend::RefusedCredential);
    assert_eq!(status.last_status, Some(401));
    assert_eq!(status.accepted, 0);
    assert_eq!(status.dropped, 6, "the rest of each drain is dropped");
    assert_eq!(hits.load(Ordering::SeqCst), 2, "one request per drain");

    let warns = logs.at(tracing::Level::WARN);
    assert_eq!(
        warns.len(),
        1,
        "one WARN for the life of the process: {warns:?}"
    );
    assert!(warns[0].contains("refused this instance's credential (401)"));
    assert!(logs.at(tracing::Level::INFO).is_empty());
    assert!(
        !logs.everything().contains(FAKE_CLIENT_ID),
        "the client id must never reach a log line"
    );
    server.abort();
}

#[tokio::test]
async fn analytics_a_persistent_per_event_rejection_warns_first_then_stays_at_debug() {
    let logs = Logs::default();
    let _guard = logs.install();
    let (url, hits, server) = collector(axum::http::StatusCode::BAD_REQUEST).await;
    let tracker = tracker_for(&url);

    track(&tracker, 4);
    tracker.flush().await;

    let status = tracker.status().unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        4,
        "a refused event keeps the drain going"
    );
    assert_eq!(status.last_send, LastSend::RejectedEvent);
    assert_eq!(status.last_status, Some(400));
    assert_eq!((status.accepted, status.dropped), (0, 4));

    assert_eq!(logs.at(tracing::Level::WARN).len(), 1);
    assert_eq!(logs.at(tracing::Level::DEBUG).len(), 3);
    assert!(!logs.everything().contains(FAKE_CLIENT_ID));
    server.abort();
}

#[tokio::test]
async fn analytics_a_redirect_is_recorded_and_warned_once() {
    let logs = Logs::default();
    let _guard = logs.install();
    let (url, _hits, server) = collector(axum::http::StatusCode::TEMPORARY_REDIRECT).await;
    let tracker = tracker_for(&url);

    track(&tracker, 3);
    tracker.flush().await;

    let status = tracker.status().unwrap();
    assert_eq!(status.last_send, LastSend::Redirect);
    assert_eq!(status.last_status, Some(307));
    assert_eq!(status.dropped, 3);
    assert_eq!(logs.at(tracing::Level::WARN).len(), 1);
    assert!(!logs.everything().contains(FAKE_CLIENT_ID));
    server.abort();
}

#[tokio::test]
async fn analytics_a_busy_collector_warns_on_the_first_outcome_only() {
    let logs = Logs::default();
    let _guard = logs.install();
    let (url, _hits, server) = collector(axum::http::StatusCode::SERVICE_UNAVAILABLE).await;
    let tracker = tracker_for(&url);

    track(&tracker, 3);
    tracker.flush().await;
    track(&tracker, 3);
    tracker.flush().await;

    let status = tracker.status().unwrap();
    assert_eq!(status.last_send, LastSend::CollectorBusy);
    assert_eq!(status.last_status, Some(503));
    assert_eq!(status.dropped, 6);

    let warns = logs.at(tracing::Level::WARN);
    assert_eq!(warns.len(), 1, "{warns:?}");
    assert!(warns[0].contains("collector-busy"));
    assert_eq!(logs.at(tracing::Level::DEBUG).len(), 1, "the second drain");
    assert!(!logs.everything().contains(FAKE_CLIENT_ID));
    server.abort();
}

#[tokio::test]
async fn analytics_an_unreachable_collector_is_recorded_without_a_status() {
    let logs = Logs::default();
    let _guard = logs.install();
    // Bind, note the port, and close: nothing listens there any more.
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };
    let tracker = tracker_for(&format!("http://{closed}/track"));

    track(&tracker, 3);
    tracker.flush().await;

    let status = tracker.status().unwrap();
    assert_eq!(status.last_send, LastSend::Unreachable);
    assert_eq!(status.last_status, None);
    assert_eq!(status.dropped, 3);
    let warns = logs.at(tracing::Level::WARN);
    assert_eq!(warns.len(), 1, "{warns:?}");
    assert!(warns[0].contains("unreachable"));
    assert!(!logs.everything().contains(FAKE_CLIENT_ID));
}

#[tokio::test]
async fn analytics_discard_pending_drops_what_was_queued_without_sending_it() {
    let (url, hits, server) = collector(axum::http::StatusCode::OK).await;
    let tracker = tracker_for(&url);

    track(&tracker, 3);
    tracker.discard_pending();
    tracker.flush().await;

    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(tracker.status().unwrap().last_send, LastSend::Never);
    server.abort();
}

/// The one-shot first drain: events tracked right after boot reach the
/// collector without waiting out the 30-second interval, and no flush is
/// involved. Real time, so it costs about five seconds.
#[tokio::test]
async fn analytics_the_first_drain_runs_on_its_own_shortly_after_construction() {
    let (url, hits, server) = collector(axum::http::StatusCode::OK).await;
    let tracker = tracker_for(&url);
    track(&tracker, 2);

    let waited = tokio::time::timeout(http::FIRST_DRAIN_DELAY + Duration::from_secs(10), async {
        while hits.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    assert!(waited.is_ok(), "the first drain never ran");
    assert_eq!(tracker.status().unwrap().last_send, LastSend::Accepted);
    server.abort();
}
