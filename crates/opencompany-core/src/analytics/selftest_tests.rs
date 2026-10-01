//! Tests for `analytics-test`: exit codes, the `s_` id space, and the silent
//! path. The collector-backed ones need the transport, so they are gated.

use super::*;
use crate::analytics::config::ENABLE_ENV;
use crate::app::config::MapEnv;
use crate::app::deployment::DEPLOYMENT_ENV;

/// **I6 (silent): 2 when the process would not report.** No endpoint, no
/// deployment, no opt-in: the decision is `NotHosted`, and the self-test must
/// say so rather than print a plausible id.
#[tokio::test]
async fn a_silent_process_exits_two_with_the_reason() {
    let outcome = run(&MapEnv::new(Vec::<(&str, &str)>::new())).await;
    assert_eq!(
        outcome,
        SelfTestOutcome::Silent("not a hosted tenant and no explicit opt-in")
    );
    assert_eq!(outcome.exit_code(), 2);
}

/// An opt-out is silence too, with its own reason, and the same exit code.
#[tokio::test]
async fn an_opted_out_process_exits_two() {
    let env = MapEnv::new([(DEPLOYMENT_ENV, "desktop"), (ENABLE_ENV, "off")]);
    let outcome = run(&env).await;
    assert_eq!(outcome, SelfTestOutcome::Silent("operator opted out"));
    assert_eq!(outcome.exit_code(), 2);
}

/// The exit-code mapping is the contract automation reads, so pin every arm
/// without a network.
#[test]
fn only_an_accepted_send_exits_zero() {
    let sent = |last_send| SelfTestOutcome::Sent {
        profile_id: "s_00".into(),
        last_send,
    };
    assert_eq!(sent(LastSend::Accepted).exit_code(), 0);
    for refused in [
        LastSend::Never,
        LastSend::RefusedCredential,
        LastSend::Redirect,
        LastSend::CollectorBusy,
        LastSend::Unreachable,
        LastSend::RejectedEvent,
    ] {
        assert_eq!(sent(refused).exit_code(), 1, "{refused:?}");
    }
    assert_eq!(SelfTestOutcome::NoTransport.exit_code(), 1);
}

/// Two runs never share a profile, and neither is a real id space.
#[test]
fn each_run_gets_its_own_throwaway_nonce() {
    let first = random_nonce();
    let second = random_nonce();
    assert_ne!(first, second);
    assert_eq!(first.len(), 32);
    assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[cfg(feature = "analytics")]
mod with_transport {
    use super::*;
    use crate::analytics::config::{CLIENT_ID_ENV, ENDPOINT_ENV};
    use std::sync::Arc;
    use std::sync::Mutex;

    /// An obviously-fake client id. Never a real one, in a file or anywhere else.
    const TEST_CLIENT_ID: &str = "not-a-real-client-id";

    /// A loopback collector answering every `POST /track` with `status`, and
    /// recording the bodies it was sent.
    async fn collector(
        status: axum::http::StatusCode,
    ) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let app = axum::Router::new().route(
            "/track",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let sink = sink.clone();
                async move {
                    sink.lock().unwrap().push(body);
                    status
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/track", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (url, seen)
    }

    fn env(endpoint: &str) -> MapEnv {
        MapEnv::new([
            (DEPLOYMENT_ENV, "desktop"),
            (ENABLE_ENV, "on"),
            (CLIENT_ID_ENV, TEST_CLIENT_ID),
            (ENDPOINT_ENV, endpoint),
        ])
    }

    /// **I6 (accepted): exit 0, an `s_` profile, and exactly one
    /// `analytics_self_test` on the wire under that profile.**
    #[tokio::test]
    async fn an_accepting_collector_exits_zero_under_a_smoke_id() {
        let (url, seen) = collector(axum::http::StatusCode::OK).await;
        let outcome = run(&env(&url)).await;
        let SelfTestOutcome::Sent {
            profile_id,
            last_send,
        } = &outcome
        else {
            panic!("expected a send, got {outcome:?}");
        };
        assert_eq!(*last_send, LastSend::Accepted);
        assert_eq!(outcome.exit_code(), 0);
        assert!(profile_id.starts_with("s_"), "{profile_id}");
        assert_eq!(profile_id.len(), 2 + 32);

        let bodies = seen.lock().unwrap();
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        assert_eq!(bodies[0]["payload"]["name"], "analytics_self_test");
        assert_eq!(bodies[0]["payload"]["profileId"], profile_id.as_str());
        assert_eq!(bodies[0]["payload"]["properties"]["deployment"], "desktop");
    }

    /// **I6 (refused): a `401` is non-zero, never a plausible success.**
    #[tokio::test]
    async fn a_refusing_collector_exits_non_zero() {
        let (url, _seen) = collector(axum::http::StatusCode::UNAUTHORIZED).await;
        let outcome = run(&env(&url)).await;
        assert!(
            matches!(
                outcome,
                SelfTestOutcome::Sent {
                    last_send: LastSend::RefusedCredential,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_ne!(outcome.exit_code(), 0);
        assert_ne!(outcome.exit_code(), 2, "refused is not the silent code");
    }
}
