//! A serializable answer to "is this process tracking, and is it working?".
//!
//! Until this existed the only evidence that analytics was reaching a collector
//! was the collector. A refused credential was a said-once `warn!` that the
//! `serve` default log filter swallowed, and every other failure was a `debug!`:
//! the exact "boot says reporting, nothing arrives" shape this module is built
//! to refuse. An operator (or the desktop shell, which has no collector to look
//! at) now reads the answer from the host itself, on `/spec`.
//!
//! Un-gated, like the decision in [`crate::analytics::config`]: a default build
//! has no transport, but it can still say *why* it is not reporting, and that is
//! the more common question.
//!
//! # What it never carries
//!
//! The collector client id, in any form. The endpoint goes through
//! [`loggable_endpoint`](crate::analytics::boot::loggable_endpoint) — the one
//! redaction helper — because `/spec` is unauthenticated and an authenticated
//! proxy's key lives in exactly the places that helper strips. A test serializes
//! a status and searches the JSON for the id.

use serde::Serialize;

use crate::analytics::config::Decision;
use crate::app::deployment::Deployment;

/// How the most recent send ended. A closed vocabulary, serialized kebab-case.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LastSend {
    /// Nothing has been sent yet (nothing queued, or the first drain has not
    /// run).
    #[default]
    Never,
    /// The collector took the event with a `2xx`.
    Accepted,
    /// `401`: the collector refused this instance's credential. Permanent.
    RefusedCredential,
    /// `3xx`: the endpoint redirects, which this client never follows.
    Redirect,
    /// `429` or `5xx`: the collector cannot take traffic right now.
    CollectorBusy,
    /// No HTTP answer at all: DNS, connect, TLS or timeout.
    Unreachable,
    /// Another `4xx`: the collector refused one event.
    RejectedEvent,
}

impl LastSend {
    /// The same slug the JSON carries, for log lines that name the class.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Accepted => "accepted",
            Self::RefusedCredential => "refused-credential",
            Self::Redirect => "redirect",
            Self::CollectorBusy => "collector-busy",
            Self::Unreachable => "unreachable",
            Self::RejectedEvent => "rejected-event",
        }
    }
}

/// The reporting state of one process, as `/spec` serves it under `analytics`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AnalyticsStatus {
    /// `"reporting"` when this process will send events, `"off"` otherwise.
    /// What the process will actually do, not what was configured: a build
    /// without the `analytics` feature is `off` even when configured to report.
    pub decision: &'static str,
    /// Why — the [`Silence`](crate::analytics::config::Silence) text when off.
    pub reason: &'static str,
    /// The deployment kind slug (`desktop`, `self-hosted`, `hosted-tenant`).
    pub deployment: &'static str,
    /// The collector, redacted by `boot::loggable_endpoint`. Never the client
    /// id. `None` when no endpoint is in play.
    pub endpoint: Option<String>,
    /// Whether this build compiled the network transport at all.
    pub in_build: bool,
    /// The user's consent, where a deployment asks for one. `None` means no
    /// consent question applies to this process.
    pub consent: Option<bool>,
    /// How the most recent send ended.
    pub last_send: LastSend,
    /// The HTTP status of that send, when there was one.
    pub last_status: Option<u16>,
    /// When that send ended, RFC-3339 UTC. `None` until one has.
    pub last_at: Option<String>,
    /// Events the collector accepted since boot.
    pub accepted: u64,
    /// Events lost since boot: refused, abandoned with a drain, shed from a full
    /// queue, or cut off by a cancelled drain.
    pub dropped: u64,
}

impl AnalyticsStatus {
    /// The state a [`Tracker`](crate::analytics::Tracker) that was never wired
    /// reports: off, with nothing to say beyond that.
    pub fn not_wired() -> Self {
        Self {
            decision: "off",
            reason: "not wired",
            deployment: Deployment::default().as_str(),
            endpoint: None,
            in_build: crate::analytics::BuildFlags::of_this_build().analytics,
            consent: None,
            last_send: LastSend::Never,
            last_status: None,
            last_at: None,
            accepted: 0,
            dropped: 0,
        }
    }

    /// Summarizes a boot [`Decision`] with zeroed send statistics.
    pub fn from_decision(decision: &Decision, deployment: Deployment) -> Self {
        let in_build = crate::analytics::BuildFlags::of_this_build().analytics;
        let (decision_word, reason, endpoint) = match decision {
            Decision::Silent(reason) => ("off", reason.as_str(), None),
            Decision::Report { endpoint, .. } if in_build => (
                "reporting",
                "reporting to the configured collector",
                Some(crate::analytics::boot::loggable_endpoint(endpoint)),
            ),
            Decision::Report { endpoint, .. } => (
                "off",
                "reporting was configured, but this build was compiled without the \
                 `analytics` feature",
                Some(crate::analytics::boot::loggable_endpoint(endpoint)),
            ),
        };
        Self {
            decision: decision_word,
            reason,
            deployment: deployment.as_str(),
            endpoint,
            in_build,
            ..Self::not_wired()
        }
    }

    /// Takes the send statistics from `sent` and keeps everything else. How the
    /// boot decision and a transport's counters become one answer.
    pub fn with_send_stats(mut self, sent: &AnalyticsStatus) -> Self {
        self.last_send = sent.last_send;
        self.last_status = sent.last_status;
        self.last_at = sent.last_at.clone();
        self.accepted = sent.accepted;
        self.dropped = sent.dropped;
        self
    }
}

#[cfg(test)]
#[path = "analytics_status_tests.rs"]
mod tests;
