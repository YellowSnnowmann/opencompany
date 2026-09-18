//! Updating the desktop shell itself.
//!
//! Three pieces, none of which needs a webview to exercise:
//!
//! - [`is_configured`] — whether this build carries a real signing key, so a
//!   build that shipped the placeholder stays *silent* instead of offering an
//!   update every client would then refuse.
//! - [`PendingUpdate`] — the slot the downloaded bytes sit in between the
//!   background download and the restart the operator asked for.
//! - [`classify`] / [`backoff_for`] — the bounded retry the download is wrapped
//!   in, factored out here because a `tauri_plugin_updater::Error` cannot be
//!   constructed in a test and a `reqwest::Error` has no public constructor.
//!
//! The commands over all three are in [`crate::commands`], where every other
//! `#[tauri::command]` in this crate lives.
//!
//! ## Why the bytes are staged rather than installed
//!
//! `Update::download_and_install` is one call, and it restarts the application
//! at a moment nobody chose. The console is a place where somebody is halfway
//! through a sentence to an agent, so the download runs in the background and
//! the *install* waits behind a button. That split is the whole reason this
//! module holds state at all: [`PendingUpdate`] is what the download hands to
//! the install, so pressing "Restart now" does not re-fetch ~100 MB.

use std::time::Duration;

/// A downloaded update, verified and waiting for the operator to say when.
pub struct StagedUpdate {
    /// The handle `install` is called on. Carries the target and the
    /// signature the bytes were already checked against.
    pub update: tauri_plugin_updater::Update,
    /// The bundle itself, in memory. ~100 MB for the macOS `.app`, which is
    /// cheap next to asking the operator to download it twice.
    pub bytes: Vec<u8>,
    /// What version those bytes are, for the prompt.
    pub version: String,
}

/// The one staging slot, managed by Tauri.
///
/// `None` means nothing has been downloaded since launch. A second download
/// overwrites it, which is correct: the newest staged bytes are the ones a
/// restart should apply.
#[derive(Default)]
pub struct PendingUpdate(pub tokio::sync::Mutex<Option<StagedUpdate>>);

/// The prefix every minisign public key carries, in Tauri's encoding.
///
/// Tauri stores `plugins.updater.pubkey` as base64 of the whole minisign
/// public-key *file*, whose first line is always `untrusted comment: …`, and
/// `dW50cnVzdGVkIGNvbW1lbnQ6` is the base64 of `untrusted comment:`. So a key
/// that starts with it is at least shaped like the real thing, and the
/// placeholder committed to `tauri.conf.json` cannot be.
const MINISIGN_PUBKEY_PREFIX: &str = "dW50cnVzdGVkIGNvbW1lbnQ6";

/// Whether this build was compiled against a real updater signing key.
///
/// `tauri.conf.json` ships an obvious placeholder, because the private half is
/// an operator secret that is not in this repository and must never be. The
/// plugin does not notice: it never parses `pubkey` until it verifies a
/// downloaded bundle, so a placeholder build checks the endpoint happily,
/// downloads 100 MB, and *then* fails the signature — an error banner in front
/// of every user, for a release that was misconfigured at build time.
///
/// So the check command asks this first and reports "no update" when it is
/// false. A placeholder build is inert and quiet; the loud half is in CI, where
/// `scripts/release/assert-updater-configured.sh` refuses to cut a release
/// while the placeholder is still in the config. See
/// `docs/spec/runtime/desktop-updates.md`.
pub fn is_configured(pubkey: &str) -> bool {
    pubkey.starts_with(MINISIGN_PUBKEY_PREFIX)
}

/// Total download attempts before giving up — one initial, two retries.
pub const MAX_DOWNLOAD_ATTEMPTS: u32 = 3;

/// What to do with an attempt that just failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Transient, and there is budget left: sleep [`backoff_for`] and go again.
    Retry,
    /// Fatal, or the budget is spent. Surface the error.
    GiveUp,
}

/// Decide whether a failed download attempt is worth repeating.
///
/// * `attempt` — 1-based index of the attempt that just failed.
/// * `max` — the budget, normally [`MAX_DOWNLOAD_ATTEMPTS`].
/// * `transient` — whether the error was a network error, per [`is_transient`].
///
/// Pure: no I/O and no error construction, which is what makes the policy
/// testable at all.
pub fn classify(attempt: u32, max: u32, transient: bool) -> RetryDecision {
    if transient && attempt < max {
        RetryDecision::Retry
    } else {
        RetryDecision::GiveUp
    }
}

/// How long to wait before the next attempt: 2s, then 4s.
///
/// Deliberately short. Nothing is blocked on this — the download is background
/// work the operator cannot see — but a long backoff on a laptop that is about
/// to sleep is a retry that never happens.
pub fn backoff_for(attempt: u32) -> Duration {
    Duration::from_secs(2 * attempt as u64)
}

/// Whether an updater error is a network failure worth repeating.
///
/// The line is *fetching* versus *what was fetched*. A transport failure and an
/// unsuccessful HTTP status are both the download not arriving, and asking
/// again is the whole remedy. A signature failure, a missing target or a
/// filesystem error cannot be fixed by fetching the same bytes again, and
/// looping on a failed *signature* check is the one retry that would turn a
/// tampered bundle into a denial-of-service against the user's battery.
///
/// Two variants sit on the transport side, and both matter. `Reqwest` is a
/// connection that dropped or never opened. `Network` is the one the plugin
/// raises for **every** non-2xx response to the download request — it carries
/// only a formatted string, so a 503 from GitHub's asset CDN cannot be told
/// apart from a 404 for an asset that was never uploaded. Treating the whole
/// variant as transient costs three attempts and six seconds against a 404, and
/// buys the retry for the failure it was written for: matching `Reqwest` alone
/// meant a release-day 503 gave up after one attempt.
pub fn is_transient(err: &tauri_plugin_updater::Error) -> bool {
    matches!(
        err,
        tauri_plugin_updater::Error::Reqwest(_) | tauri_plugin_updater::Error::Network(_)
    )
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
