//! Desktop analytics: the resolver's answers, opt-out precedence, the consent
//! gate, and the self-test arguments.

use std::collections::HashMap;
use std::ffi::OsString;

use opencompany::analytics::config::{ClientCredentials, Decision, Silence, resolve};
use opencompany::app::deployment::Deployment;

use super::*;

/// An env whose values are arbitrary OS strings, for the unreadable cases.
struct OsEnv(HashMap<String, OsString>);

impl EnvSource for OsEnv {
    fn get_os(&self, key: &str) -> Option<OsString> {
        self.0.get(key).cloned()
    }
}

fn env(pairs: &[(&str, &str)], preference: bool) -> DesktopAnalyticsEnv<MapEnv> {
    DesktopAnalyticsEnv::new(MapEnv::new(pairs.iter().copied()), preference)
}

fn resolved(pairs: &[(&str, &str)], preference: bool) -> Decision {
    let env = env(pairs, preference);
    resolve(Deployment::from_env(&env), &env)
}

fn reporting(endpoint: &str, client_id: &str) -> Decision {
    Decision::Report {
        endpoint: endpoint.to_string(),
        credentials: ClientCredentials::new(client_id),
    }
}

/// **U1: an empty environment reports to the desktop's defaults.**
#[test]
fn an_empty_environment_reports_with_the_desktop_defaults() {
    assert_eq!(
        resolved(&[], true),
        reporting("https://panel.tinyhumans.ai/api/track", DESKTOP_CLIENT_ID)
    );
    assert_eq!(Deployment::from_env(&env(&[], true)), Deployment::Desktop);
}

/// The shell is a desktop whatever its launcher says, so a hosted-tenant value
/// (or a tenant namespace) in the environment cannot change what it is.
#[test]
fn the_deployment_is_always_desktop() {
    for pairs in [
        &[("OPENCOMPANY_DEPLOYMENT", "hosted-tenant")][..],
        &[("OPENCOMPANY_DEPLOYMENT", "self-hosted")],
        &[("OPENCOMPANY_TENANT_ID", "acme")],
    ] {
        assert_eq!(
            Deployment::from_env(&env(pairs, true)),
            Deployment::Desktop,
            "{pairs:?}"
        );
    }
}

/// Blank values are "not set" and fall back, like every other reader here.
#[test]
fn blank_values_fall_back_to_the_defaults() {
    assert_eq!(
        resolved(
            &[
                ("OPENCOMPANY_ANALYTICS", "  "),
                ("OPENCOMPANY_ANALYTICS_CLIENT_ID", ""),
                ("OPENCOMPANY_ANALYTICS_ENDPOINT", "\n"),
            ],
            true
        ),
        reporting("https://panel.tinyhumans.ai/api/track", DESKTOP_CLIENT_ID)
    );
}

/// An operator's own client id and endpoint pass through untouched.
#[test]
fn set_values_pass_through() {
    assert_eq!(
        resolved(
            &[
                ("OPENCOMPANY_ANALYTICS_CLIENT_ID", "not-a-real-client-id"),
                (
                    "OPENCOMPANY_ANALYTICS_ENDPOINT",
                    "https://collector.example/track"
                ),
            ],
            true
        ),
        reporting("https://collector.example/track", "not-a-real-client-id")
    );
}

/// A plain-http endpoint to a non-loopback host would put the client id on the
/// wire in the clear, so it is refused; loopback is the documented exception.
#[test]
fn a_non_loopback_http_endpoint_is_refused() {
    assert_eq!(
        resolved(
            &[(
                "OPENCOMPANY_ANALYTICS_ENDPOINT",
                "http://collector.internal/track"
            )],
            true
        ),
        Decision::Silent(Silence::InsecureEndpoint)
    );
    assert!(
        resolved(
            &[("OPENCOMPANY_ANALYTICS_ENDPOINT", "http://127.0.0.1:9/track")],
            true
        )
        .reports()
    );
}

/// **U2: opt-out precedence.** The user's "off" beats an env "on", an env "off"
/// beats the user's "on", and a value nobody can read is reported, not obeyed.
#[test]
fn opt_out_wins_from_either_side() {
    assert_eq!(
        resolved(&[("OPENCOMPANY_ANALYTICS", "on")], false),
        Decision::Silent(Silence::OptedOut)
    );
    assert_eq!(
        resolved(&[("OPENCOMPANY_ANALYTICS", "off")], true),
        Decision::Silent(Silence::OptedOut)
    );
    assert_eq!(
        resolved(&[("OPENCOMPANY_ANALYTICS", "of")], true),
        Decision::Silent(Silence::Unreadable),
        "a typo is silence with a reason, never reporting"
    );
}

/// Bytes that are not UTF-8 are a malformed value, and resolve to `Unreadable`.
#[cfg(unix)]
#[test]
fn an_unreadable_switch_is_unreadable() {
    use std::os::unix::ffi::OsStringExt as _;
    let inner = OsEnv(HashMap::from([(
        "OPENCOMPANY_ANALYTICS".to_string(),
        OsString::from_vec(vec![0xff, 0xfe]),
    )]));
    let env = DesktopAnalyticsEnv::new(inner, true);
    assert_eq!(
        resolve(Deployment::Desktop, &env),
        Decision::Silent(Silence::Unreadable)
    );
}

/// Identity is always the instance id: the keyed-tenant variables answer
/// nothing, however the inner environment is set.
#[test]
fn nothing_can_replace_the_instance_identity() {
    let env = env(
        &[
            ("OPENCOMPANY_ANALYTICS_ID_KEY", "not-a-real-id-key"),
            ("OPENCOMPANY_TENANT_ID", "acme"),
        ],
        true,
    );
    assert!(env.get_os("OPENCOMPANY_ANALYTICS_ID_KEY").is_none());
    assert!(env.get_os("OPENCOMPANY_TENANT_ID").is_none());
    assert!(opencompany::analytics::config::tenant_id_key(&env).is_none());
}

/// A disabled setup resolves to a user opt-out: nothing is built, nothing sent.
#[test]
fn a_disabled_setup_is_silent() {
    let setup = AnalyticsSetup::disabled();
    assert!(!setup.gate.is_enabled());
    assert_eq!(
        resolve(Deployment::Desktop, setup.env.as_ref()),
        Decision::Silent(Silence::OptedOut)
    );
}

/// Where the effective answer came from, in the precedence the docs state.
#[test]
fn the_source_names_who_decided() {
    let none = MapEnv::new(Vec::<(&str, &str)>::new());
    let off = MapEnv::new([("OPENCOMPANY_ANALYTICS", "off")]);
    assert_eq!(preference_source(None, &none), "default");
    assert_eq!(preference_source(Some(true), &none), "setting");
    assert_eq!(preference_source(Some(false), &none), "setting");
    assert_eq!(preference_source(None, &off), "env");
    assert_eq!(preference_source(Some(false), &off), "setting");
}

/// A tracker that records what reaches it and how often it was told to discard.
#[derive(Default)]
struct Counting {
    events: Mutex<Vec<Event>>,
    discards: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl Tracker for Counting {
    fn track(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
    async fn flush(&self) {}
    fn status(&self) -> Option<AnalyticsStatus> {
        Some(AnalyticsStatus::not_wired())
    }
    fn discard_pending(&self) {
        self.discards.fetch_add(1, Ordering::SeqCst);
    }
}

fn gated(enabled: bool) -> (Arc<ConsentGate>, GatedTracker, Arc<Counting>) {
    let gate = Arc::new(ConsentGate::new(enabled));
    let deferred = Arc::new(DeferredTracker::new());
    let inner = Arc::new(Counting::default());
    assert!(deferred.install(inner.clone()));
    (gate.clone(), GatedTracker::new(gate, deferred), inner)
}

/// **U4: while consent holds events flow; once withdrawn they are dropped,
/// whatever was queued is discarded, and the status says consent is false.**
#[test]
fn withdrawing_consent_drops_discards_and_says_so() {
    let (gate, tracker, inner) = gated(true);
    tracker.track(Event::AnalyticsSelfTest {});
    assert_eq!(inner.events.lock().unwrap().len(), 1);
    assert_eq!(tracker.status().unwrap().consent, Some(true));

    gate.set_enabled(false);
    assert_eq!(
        inner.discards.load(Ordering::SeqCst),
        1,
        "queued events are discarded"
    );
    tracker.track(Event::AnalyticsSelfTest {});
    assert_eq!(
        inner.events.lock().unwrap().len(),
        1,
        "new events are dropped"
    );
    assert_eq!(tracker.status().unwrap().consent, Some(false));

    gate.set_enabled(true);
    tracker.track(Event::AnalyticsSelfTest {});
    assert_eq!(
        inner.events.lock().unwrap().len(),
        2,
        "and resume on opt-in"
    );
}

/// Turning consent on does not discard anything.
#[test]
fn granting_consent_discards_nothing() {
    let (gate, _tracker, inner) = gated(false);
    gate.set_enabled(true);
    assert_eq!(inner.discards.load(Ordering::SeqCst), 0);
}

#[test]
fn only_the_exact_argument_runs_the_self_test() {
    fn args(rest: &[&str]) -> Vec<String> {
        std::iter::once("opencompany-desktop")
            .chain(rest.iter().copied())
            .map(str::to_string)
            .collect()
    }
    assert!(analytics_test_args(args(&["analytics-test"])));
    assert!(!analytics_test_args(args(&[])));
    assert!(!analytics_test_args(args(&["-psn_0_12345"])));
    assert!(!analytics_test_args(args(&["sentry-test"])));
}
