//! The embedded host and analytics, end to end against a loopback collector.

use std::sync::Arc;

use opencompany::analytics::selftest::{SelfTestOutcome, run};
use opencompany::app::config::MapEnv;

use super::*;
use crate::analytics::{ConsentGate, DESKTOP_CLIENT_ID, DesktopAnalyticsEnv};
use crate::test_collector::TestCollector;

/// A setup that reports to `collector` as a desktop would, over `preference`.
fn setup(collector: &TestCollector, preference: bool) -> AnalyticsSetup {
    AnalyticsSetup {
        gate: Arc::new(ConsentGate::new(preference)),
        env: Arc::new(DesktopAnalyticsEnv::new(
            MapEnv::new([("OPENCOMPANY_ANALYTICS_ENDPOINT", collector.url.as_str())]),
            preference,
        )),
    }
}

fn is_instance_profile(id: &str) -> bool {
    id.strip_prefix("i_").is_some_and(|hex| {
        hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// **I2: an embedded host reports `instance_started` as a desktop.** One request,
/// under the instance's own id, with the desktop's client id in the header and
/// the shell's version on the event.
#[tokio::test(flavor = "multi_thread")]
async fn an_embedded_host_reports_instance_started_as_a_desktop() {
    let collector = TestCollector::start(200).await;
    let dir = tempfile::tempdir().unwrap();
    let host = start_with_analytics(
        dir.path().to_path_buf(),
        FirstRun::RunSetupWizard,
        setup(&collector, true),
    )
    .await
    .expect("host starts");
    host.flush_analytics().await;

    let requests = collector.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    let request = &requests[0];
    assert_eq!(request.path, "/track");
    assert_eq!(
        request.header("openpanel-client-id"),
        Some(DESKTOP_CLIENT_ID)
    );

    let body = request.json();
    assert_eq!(body["payload"]["name"], "instance_started");
    assert_eq!(
        body["payload"]["profileId"],
        format!("i_{}", host.instance_id()).as_str()
    );
    let properties = &body["payload"]["properties"];
    assert_eq!(properties["deployment"], "desktop");
    assert_eq!(properties["analytics_in_build"], true);
    assert_eq!(properties["shell_version"], env!("CARGO_PKG_VERSION"));

    let status = host.analytics_status().expect("a tracker is installed");
    assert_eq!(status.decision, "reporting");
    assert_eq!(status.consent, Some(true));
    assert_eq!(status.accepted, 1);
}

/// **The default entry points send nothing.** `start_with` is what every other
/// suite uses; it must stay silent and say why.
#[tokio::test(flavor = "multi_thread")]
async fn start_with_reports_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let host = start_with(dir.path().to_path_buf(), FirstRun::RunSetupWizard)
        .await
        .expect("host starts");
    host.flush_analytics().await;
    let status = host.analytics_status().expect("a tracker is installed");
    assert_eq!(status.decision, "off");
    assert_eq!(status.reason, "operator opted out");
    assert_eq!(status.consent, Some(false));
}

/// A user who opted out gets a host that builds no transport at all.
#[tokio::test(flavor = "multi_thread")]
async fn an_opted_out_launch_sends_nothing() {
    let collector = TestCollector::start(200).await;
    let dir = tempfile::tempdir().unwrap();
    let host = start_with_analytics(
        dir.path().to_path_buf(),
        FirstRun::RunSetupWizard,
        setup(&collector, false),
    )
    .await
    .expect("host starts");
    host.flush_analytics().await;
    assert!(collector.requests().is_empty());
    assert_eq!(host.analytics_status().unwrap().decision, "off");
}

/// **U5: identity is the instance, stable across launches.** Two boots over one
/// data root report the same `i_` id; a second root gets a different one.
#[tokio::test(flavor = "multi_thread")]
async fn identity_is_the_instance_id_and_is_stable() {
    let collector = TestCollector::start(200).await;
    let first_root = tempfile::tempdir().unwrap();
    let second_root = tempfile::tempdir().unwrap();

    let mut ids = Vec::new();
    for root in [&first_root, &first_root, &second_root] {
        let host = start_with_analytics(
            root.path().to_path_buf(),
            FirstRun::RunSetupWizard,
            setup(&collector, true),
        )
        .await
        .expect("host starts");
        host.flush_analytics().await;
        drop(host);
        let seen = collector.requests();
        let id = seen.last().expect("a request").json()["payload"]["profileId"]
            .as_str()
            .unwrap()
            .to_string();
        ids.push(id);
    }
    assert!(ids.iter().all(|id| is_instance_profile(id)), "{ids:?}");
    assert_eq!(ids[0], ids[1], "the same root is the same instance");
    assert_ne!(ids[0], ids[2], "another root is another instance");
}

/// **I6, desktop side: the self-test through the desktop's own environment.**
/// Accepted is `0` under an `s_` id that is never an instance id; a refusing
/// collector is non-zero; an opt-out is `2`.
#[tokio::test(flavor = "multi_thread")]
async fn the_self_test_exit_codes_hold_through_the_desktop_environment() {
    let accepting = TestCollector::start(200).await;
    let env = |c: &TestCollector, preference| {
        DesktopAnalyticsEnv::new(
            MapEnv::new([("OPENCOMPANY_ANALYTICS_ENDPOINT", c.url.as_str())]),
            preference,
        )
    };

    let outcome = run(&env(&accepting, true)).await;
    assert_eq!(outcome.exit_code(), 0, "{outcome:?}");
    let SelfTestOutcome::Sent { profile_id, .. } = outcome else {
        panic!("expected a send");
    };
    assert!(profile_id.starts_with("s_") && !is_instance_profile(&profile_id));
    assert_eq!(accepting.requests().len(), 1);

    let refusing = TestCollector::start(401).await;
    assert_ne!(run(&env(&refusing, true)).await.exit_code(), 0);

    assert_eq!(run(&env(&accepting, false)).await.exit_code(), 2);
}
