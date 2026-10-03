//! The status summary: what `/spec` serves under `analytics`, and how the boot
//! decision and a transport's counters merge into it. Un-gated, so the default
//! build proves the "off, and why" half.

use super::*;
use crate::analytics::config::{CLIENT_ID_ENV, ClientCredentials, ENDPOINT_ENV, Silence};
use crate::analytics::{
    BuildFlags, DeferredTracker, Event, NullTracker, RecordingTracker, Tracker, boot,
};
use crate::app::config::MapEnv;
use crate::app::deployment::DEPLOYMENT_ENV;
use async_trait::async_trait;
use std::sync::Arc;

/// An obviously-fake client id, searched for in every serialized status.
const FAKE_CLIENT_ID: &str = "fake-status-summary-id-55d1";

fn reporting(endpoint: &str) -> Decision {
    Decision::Report {
        endpoint: endpoint.to_string(),
        credentials: ClientCredentials::new(FAKE_CLIENT_ID),
    }
}

#[test]
fn analytics_status_for_a_silent_decision_says_off_and_why() {
    let status = AnalyticsStatus::from_decision(
        &Decision::Silent(Silence::NotHosted),
        Deployment::SelfHosted,
    );
    assert_eq!(status.decision, "off");
    assert_eq!(status.reason, Silence::NotHosted.as_str());
    assert_eq!(status.deployment, "self-hosted");
    assert_eq!(status.endpoint, None);
    assert_eq!(status.last_send, LastSend::Never);
    assert_eq!((status.accepted, status.dropped), (0, 0));
}

#[test]
fn analytics_status_for_a_reporting_decision_redacts_the_endpoint() {
    let status = AnalyticsStatus::from_decision(
        &reporting("https://user:hunter2@collector.example/track?key=abc123"),
        Deployment::HostedTenant,
    );
    let endpoint = status
        .endpoint
        .as_deref()
        .expect("a reporting decision names its collector");
    assert!(
        endpoint.starts_with("https://collector.example/track"),
        "{endpoint}"
    );
    assert!(!endpoint.contains("hunter2") && !endpoint.contains("abc123"));
    // What the process will actually do depends on whether the transport was
    // compiled: a configured-but-impossible report is `off`, as `describe` says.
    let compiled = BuildFlags::of_this_build().analytics;
    assert_eq!(status.in_build, compiled);
    assert_eq!(status.decision, if compiled { "reporting" } else { "off" });
}

#[test]
fn analytics_status_serializes_without_the_client_id_and_with_kebab_case_enums() {
    let status = AnalyticsStatus::from_decision(
        &reporting("https://collector.example/track"),
        Deployment::HostedTenant,
    );
    let json = serde_json::to_value(&status).unwrap();
    assert!(!json.to_string().contains(FAKE_CLIENT_ID));
    for key in [
        "decision",
        "reason",
        "deployment",
        "endpoint",
        "in_build",
        "consent",
        "last_send",
        "last_status",
        "last_at",
        "accepted",
        "dropped",
    ] {
        assert!(json.get(key).is_some(), "missing {key}: {json}");
    }
    assert_eq!(json["last_send"], "never");
    assert_eq!(
        serde_json::to_value(LastSend::RefusedCredential).unwrap(),
        "refused-credential"
    );
    // The slug a log line uses is the slug the JSON carries.
    for kind in [
        LastSend::Never,
        LastSend::Accepted,
        LastSend::RefusedCredential,
        LastSend::Redirect,
        LastSend::CollectorBusy,
        LastSend::Unreachable,
        LastSend::RejectedEvent,
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
    }
}

/// A tracker that reports fixed send statistics, standing in for the transport.
struct Counting;

#[async_trait]
impl Tracker for Counting {
    fn track(&self, _event: Event) {}
    async fn flush(&self) {}
    fn status(&self) -> Option<AnalyticsStatus> {
        Some(AnalyticsStatus {
            last_send: LastSend::Accepted,
            last_status: Some(202),
            last_at: Some("2026-10-01T00:00:00Z".into()),
            accepted: 9,
            dropped: 2,
            ..AnalyticsStatus::not_wired()
        })
    }
}

#[test]
fn analytics_a_deferred_tracker_merges_the_decision_with_the_transports_counters() {
    let deferred = DeferredTracker::new();
    assert!(deferred.status().is_none(), "nothing installed yet");

    let decision = reporting("https://collector.example/track");
    assert!(deferred.install_with_decision(
        Arc::new(Counting),
        &decision,
        Deployment::HostedTenant
    ));
    let status = deferred.status().expect("installed");

    assert_eq!(
        status.deployment, "hosted-tenant",
        "the decision's fields win"
    );
    assert_eq!(
        status.endpoint.as_deref(),
        Some("https://collector.example/track")
    );
    assert_eq!(
        status.last_send,
        LastSend::Accepted,
        "the transport's counters win"
    );
    assert_eq!(
        (status.accepted, status.dropped, status.last_status),
        (9, 2, Some(202))
    );
}

#[test]
fn analytics_a_silent_install_still_reports_the_reason() {
    let deferred = DeferredTracker::new();
    deferred.install_with_decision(
        Arc::new(NullTracker),
        &Decision::Silent(Silence::OptedOut),
        Deployment::SelfHosted,
    );
    let status = deferred.status().expect("installed");
    assert_eq!(status.decision, "off");
    assert_eq!(status.reason, Silence::OptedOut.as_str());
}

#[test]
fn analytics_discarding_clears_the_pre_install_buffer() {
    let deferred = DeferredTracker::new();
    let event = Event::InstanceStarted {
        companies: 1,
        storage: "fs",
        setup_complete: true,
    };
    deferred.track(event);
    deferred.discard_pending();

    let recorder = Arc::new(RecordingTracker::new());
    deferred.install(recorder.clone());
    assert!(
        recorder.events().is_empty(),
        "a discarded event must not replay"
    );
}

#[test]
fn analytics_the_default_tracker_methods_are_inert() {
    assert!(NullTracker.status().is_none());
    NullTracker.discard_pending();
    assert_eq!(AnalyticsStatus::not_wired().decision, "off");
    assert_eq!(AnalyticsStatus::not_wired().reason, "not wired");
}

#[test]
fn analytics_boot_install_stores_the_decision_for_status() {
    let home = tempfile::tempdir().expect("tempdir");
    let state = crate::AppState::new(crate::AppConfig::default()).with_home(home.path());
    let handle = DeferredTracker::new();
    let env = MapEnv::new([
        (CLIENT_ID_ENV, FAKE_CLIENT_ID),
        (ENDPOINT_ENV, "https://collector.invalid/track"),
        (DEPLOYMENT_ENV, "hosted-tenant"),
    ]);
    boot::install(&state, &handle, &env);

    let status = handle
        .status()
        .expect("boot installs through the decision-carrying path");
    assert_eq!(status.deployment, "hosted-tenant");
    assert_eq!(
        status.endpoint.as_deref(),
        Some("https://collector.invalid/track")
    );
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains(FAKE_CLIENT_ID)
    );
}
