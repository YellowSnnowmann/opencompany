//! Product analytics for the desktop shell: on by default, with a user opt-out.
//!
//! Reported from the Rust host through the core's `Tracker`, never from the
//! webview, so the CSP stays as it is. Three pieces:
//!
//! - [`DesktopAnalyticsEnv`] answers the core resolver's questions. A launched
//!   `.app` has no environment, so the shell supplies the deployment, the
//!   switch, the client id and the endpoint itself — below the process env
//!   (an operator's own `OPENCOMPANY_ANALYTICS_*` still wins), and above the
//!   user's saved preference only for *off*. This mirrors
//!   [`crate::crash::DesktopEnv`] for the Sentry DSN.
//! - [`ConsentGate`] / [`GatedTracker`] make the opt-out **immediate**: turning
//!   it off drops new events and discards queued ones in the same call.
//! - [`AnalyticsSetup`] is the shell-wide bundle each embedded host starts with.
//!
//! **Opt-in applies at the next launch.** The core installs its tracker into a
//! `OnceLock` at boot; a launch that resolved to silence installed a no-op that
//! cannot be swapped. Opting out needs no restart; opting back in from a launch
//! that started silent does.
//!
//! **Identity is the instance, not the person.** The tracker identity is
//! `i_` + the data root's random `instance-id`; the tenant variables answer
//! `None` here so nothing keyed or hashed can replace it.
//!
//! See `docs/spec/runtime/analytics-desktop.md`.

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use opencompany::analytics::config::{
    CLIENT_ID_ENV, DEFAULT_ENDPOINT, ENABLE_ENV, ENDPOINT_ENV, ID_KEY_ENV,
};
use opencompany::analytics::{AnalyticsStatus, DeferredTracker, Event, Tracker};
use opencompany::app::config::{EnvSource, MapEnv, ProcessEnv};
use opencompany::app::deployment::DEPLOYMENT_ENV;

/// The OpenPanel client id the desktop reports as when
/// `OPENCOMPANY_ANALYTICS_CLIENT_ID` is unset or blank.
///
/// This is the same write client the browser console and the hosted tenants
/// ship, reused for now. A key inside a downloadable bundle is readable by
/// anyone who unzips it, so a dedicated, revocable desktop write client
/// (`ignoreCorsAndSecret = true`) should replace it; changing this one constant
/// is the whole migration.
pub const DESKTOP_CLIENT_ID: &str = "afe8ec4e-0a6a-427a-aa22-49cbbf137d0a";

/// The tenant-namespace variable. Answered `None` so a desktop is never
/// mistaken for a hosted tenant and never derives a tenant digest id.
const TENANT_ID_ENV: &str = "OPENCOMPANY_TENANT_ID";

/// Whether a raw environment value is "nobody set this": absent, empty or only
/// whitespace. Bytes that are not UTF-8 are a *value* (malformed), not blank.
fn is_blank(raw: &Option<OsString>) -> bool {
    raw.as_ref()
        .is_none_or(|raw| raw.is_empty() || raw.to_str().is_some_and(|s| s.trim().is_empty()))
}

/// An [`EnvSource`] that gives the core resolver a desktop's answers.
pub struct DesktopAnalyticsEnv<E> {
    inner: E,
    preference_enabled: bool,
}

impl<E: EnvSource> DesktopAnalyticsEnv<E> {
    /// Wraps `inner`; `preference_enabled` is the user's saved choice
    /// (`true` when they never made one).
    pub fn new(inner: E, preference_enabled: bool) -> Self {
        Self {
            inner,
            preference_enabled,
        }
    }
}

impl<E: EnvSource> EnvSource for DesktopAnalyticsEnv<E> {
    fn get_os(&self, key: &str) -> Option<OsString> {
        let value = self.inner.get_os(key);
        match key {
            // Always a desktop, even if the launching shell says otherwise: a
            // hosted-tenant value inherited from somewhere must not change
            // what this binary is.
            DEPLOYMENT_ENV => Some("desktop".into()),
            // The user's "off" outranks everything; then any value the
            // operator set (including their own `off`, or one the resolver
            // cannot read, which it reports); else the desktop default, on.
            ENABLE_ENV => {
                if !self.preference_enabled {
                    Some("off".into())
                } else if is_blank(&value) {
                    Some("on".into())
                } else {
                    value
                }
            }
            CLIENT_ID_ENV if is_blank(&value) => Some(DESKTOP_CLIENT_ID.into()),
            ENDPOINT_ENV if is_blank(&value) => Some(DEFAULT_ENDPOINT.into()),
            // Identity is always the instance id.
            ID_KEY_ENV | TENANT_ID_ENV => None,
            _ => value,
        }
    }
}

/// The user's choice and the operator's env, compressed to where the effective
/// answer came from: `default`, `setting` or `env`.
pub fn preference_source(saved: Option<bool>, env: &dyn EnvSource) -> &'static str {
    if saved == Some(false) {
        "setting"
    } else if !is_blank(&env.get_os(ENABLE_ENV)) {
        "env"
    } else if saved.is_some() {
        "setting"
    } else {
        "default"
    }
}

/// The shell-wide consent switch every embedded host's tracker consults.
#[derive(Debug)]
pub struct ConsentGate {
    enabled: AtomicBool,
    trackers: Mutex<Vec<Weak<DeferredTracker>>>,
}

impl ConsentGate {
    /// A gate that starts as `enabled`.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            trackers: Mutex::new(Vec::new()),
        }
    }

    /// Whether events may flow right now.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Flips the gate. Turning it **off** also discards whatever every
    /// registered tracker has queued, so events recorded before the user said
    /// no are not delivered after it.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        if enabled {
            return;
        }
        let mut trackers = self.trackers.lock().expect("consent gate");
        trackers.retain(|weak| match weak.upgrade() {
            Some(tracker) => {
                tracker.discard_pending();
                true
            }
            None => false,
        });
    }

    fn register(&self, tracker: &Arc<DeferredTracker>) {
        self.trackers
            .lock()
            .expect("consent gate")
            .push(Arc::downgrade(tracker));
    }
}

/// A [`Tracker`] that forwards to a [`DeferredTracker`] only while consent
/// holds, and reports that consent in its status.
pub struct GatedTracker {
    gate: Arc<ConsentGate>,
    inner: Arc<DeferredTracker>,
}

impl GatedTracker {
    /// Gates `inner` behind `gate`, registering it for discard on opt-out.
    pub fn new(gate: Arc<ConsentGate>, inner: Arc<DeferredTracker>) -> Self {
        gate.register(&inner);
        Self { gate, inner }
    }
}

#[async_trait]
impl Tracker for GatedTracker {
    fn track(&self, event: Event) {
        if self.gate.is_enabled() {
            self.inner.track(event);
        }
    }

    async fn flush(&self) {
        self.inner.flush().await;
    }

    fn observe_cognition(&self, cognition: opencompany::ports::brain::Cognition) {
        self.inner.observe_cognition(cognition);
    }

    fn status(&self) -> Option<AnalyticsStatus> {
        let mut status = self.inner.status()?;
        status.consent = Some(self.gate.is_enabled());
        Some(status)
    }

    fn discard_pending(&self) {
        self.inner.discard_pending();
    }
}

/// What every embedded host a shell starts is told about analytics.
#[derive(Clone)]
pub struct AnalyticsSetup {
    /// The shell-wide consent switch.
    pub gate: Arc<ConsentGate>,
    /// The environment the core resolver reads, normally a
    /// [`DesktopAnalyticsEnv`] over the process env.
    pub env: Arc<dyn EnvSource + Send + Sync>,
}

impl AnalyticsSetup {
    /// The setup a launched app uses: the user's saved choice over the process
    /// environment.
    pub fn for_preference(enabled: bool, gate: Arc<ConsentGate>) -> Self {
        Self {
            gate,
            env: Arc::new(DesktopAnalyticsEnv::new(ProcessEnv, enabled)),
        }
    }

    /// Reports nothing and touches no network: consent off, resolver told
    /// "off". The default for `embedded::start_with`, so the test suites and
    /// any embedder that never asked for analytics stay silent.
    pub fn disabled() -> Self {
        Self {
            gate: Arc::new(ConsentGate::new(false)),
            env: Arc::new(DesktopAnalyticsEnv::new(
                MapEnv::new(Vec::<(&str, &str)>::new()),
                false,
            )),
        }
    }
}

/// `opencompany-desktop analytics-test`: the core self-test through the
/// desktop's own environment, headless, before Tauri starts. Exit codes are the
/// core's: `0` accepted, `2` silent, `1` anything else.
pub fn run_analytics_test() -> std::process::ExitCode {
    let saved = crate::preferences::Preferences::load(&crate::default_data_dir()).analytics;
    let env = DesktopAnalyticsEnv::new(ProcessEnv, saved.unwrap_or(true));
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("could not start a runtime for the analytics self-test: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(opencompany::analytics::selftest::run_cli(&env));
    std::process::ExitCode::from(u8::try_from(code).unwrap_or(1))
}

/// Recognises the hidden `analytics-test` argument. Exact match only, for the
/// reason [`crate::crash::sentry_test_args`] gives.
pub fn analytics_test_args<I: IntoIterator<Item = String>>(args: I) -> bool {
    args.into_iter().nth(1).as_deref() == Some("analytics-test")
}

#[cfg(test)]
#[path = "analytics_tests.rs"]
mod tests;
