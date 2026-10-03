//! The Tauri surface over the user's analytics choice.
//!
//! Kept out of `commands.rs`, which is at the file-size cap. Two commands: read
//! the preference with where it came from and what the tracker is actually
//! doing, and set it. The logic lives in plain functions ([`read_preference`],
//! [`apply_preference`]) so it is testable without a Tauri runtime; the
//! `#[tauri::command]` wrappers only fetch managed state.
//!
//! Opting out is immediate: the gate flips and queued events are discarded.
//! Opting in is saved at once but applies at the next launch (see
//! `analytics.rs`), which is what `restart_required` tells the Privacy page.

use std::path::Path;
use std::sync::Arc;

use opencompany::analytics::AnalyticsStatus;
use opencompany::analytics::config::{Decision, Silence, resolve};
use opencompany::app::config::{EnvSource, ProcessEnv};
use opencompany::app::deployment::Deployment;
use serde::Serialize;
use tauri::State;

use crate::AppHandleState;
use crate::analytics::{ConsentGate, DesktopAnalyticsEnv, preference_source};
use crate::preferences::Preferences;

/// The answer both commands give the console.
#[derive(Debug, Clone, Serialize)]
pub struct AnalyticsPreference {
    /// Whether analytics is on as far as the user and operator have said: off
    /// when the user turned it off or `OPENCOMPANY_ANALYTICS` is off.
    pub enabled: bool,
    /// Where that answer came from: `default`, `setting` or `env`.
    pub source: &'static str,
    /// What the default instance's tracker is doing right now, if one is
    /// running. The truth, where `enabled` is the intent.
    pub status: Option<AnalyticsStatus>,
    /// `true` when the user wants analytics on but this launch is not reporting
    /// (it started opted out), so it begins at the next launch.
    pub restart_required: bool,
}

/// Builds the answer from what is on disk and in the environment.
pub fn read_preference(
    data_dir: &Path,
    env: &dyn EnvSource,
    status: Option<AnalyticsStatus>,
) -> AnalyticsPreference {
    let saved = Preferences::load(data_dir).analytics;
    let desktop_env = DesktopAnalyticsEnv::new(BorrowedEnv(env), saved.unwrap_or(true));
    let decision = resolve(Deployment::Desktop, &desktop_env);
    let enabled = !matches!(
        decision,
        Decision::Silent(Silence::OptedOut | Silence::Unreadable)
    );
    let reporting = status.as_ref().is_some_and(|s| s.decision == "reporting");
    let would_report = decision.reports();
    AnalyticsPreference {
        enabled,
        source: preference_source(saved, env),
        restart_required: would_report && status.is_some() && !reporting,
        status,
    }
}

/// Saves the user's choice and flips the gate.
///
/// An opt-out takes effect **before** the disk write, so a full disk cannot keep
/// events flowing; the write's failure is still returned.
pub fn apply_preference(data_dir: &Path, gate: &ConsentGate, enabled: bool) -> Result<(), String> {
    if !enabled {
        gate.set_enabled(false);
    }
    let mut preferences = Preferences::load(data_dir);
    preferences.analytics = Some(enabled);
    preferences.save(data_dir)?;
    if enabled {
        gate.set_enabled(true);
    }
    Ok(())
}

/// Lends a `&dyn EnvSource` to a wrapper that wants to own its inner source.
struct BorrowedEnv<'a>(&'a dyn EnvSource);

impl EnvSource for BorrowedEnv<'_> {
    fn get_os(&self, key: &str) -> Option<std::ffi::OsString> {
        self.0.get_os(key)
    }
}

/// The user's analytics choice, where it came from, and what the tracker is
/// doing.
#[tauri::command]
pub async fn oc_analytics_preference(
    state: State<'_, AppHandleState>,
) -> Result<AnalyticsPreference, String> {
    let status = state.local.lock().await.analytics_status();
    Ok(read_preference(&state.data_dir, &ProcessEnv, status))
}

/// Records the user's choice: persists it and flips the consent gate, discarding
/// queued events on an opt-out.
#[tauri::command]
pub async fn oc_set_analytics_preference(
    enabled: bool,
    state: State<'_, AppHandleState>,
    gate: State<'_, Arc<ConsentGate>>,
) -> Result<AnalyticsPreference, String> {
    apply_preference(&state.data_dir, &gate, enabled)?;
    let status = state.local.lock().await.analytics_status();
    Ok(read_preference(&state.data_dir, &ProcessEnv, status))
}

#[cfg(test)]
#[path = "commands_analytics_tests.rs"]
mod tests;
