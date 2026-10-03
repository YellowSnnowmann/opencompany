//! What the transport has done, kept so the host can say so.
//!
//! The counters behind [`AnalyticsStatus`](crate::analytics::AnalyticsStatus).
//! Every arm of `Inner::drain` records here, because an arm that only logs is an
//! arm whose outcome disappears the moment nobody has the right log filter on —
//! the failure this bookkeeping exists to end.
//!
//! Atomics for the counters (read on `/spec`, written from the drain, never
//! across an await) and one small mutex for the last outcome, which is three
//! fields that must be read together.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::analytics::status::{AnalyticsStatus, LastSend};

/// The outcome of the most recent send.
#[derive(Clone, Copy)]
struct Last {
    kind: LastSend,
    status: Option<u16>,
    at_millis: u64,
}

/// Send counters and the last outcome. Owned by the transport's `Inner`.
#[derive(Default)]
pub(super) struct SendStats {
    accepted: AtomicU64,
    dropped: AtomicU64,
    last: Mutex<Option<Last>>,
    /// Whether the first drain outcome has been said out loud. The first-send
    /// self-check: one line, once, whatever that outcome was.
    first_outcome_logged: AtomicBool,
    /// Whether the first per-event rejection has been warned about. Later ones
    /// stay `debug!`, because a persistent `400` would otherwise log per event.
    first_rejection_logged: AtomicBool,
}

impl SendStats {
    /// Records how a send ended. `status` is `None` for a transport error.
    pub(super) fn record(&self, kind: LastSend, status: Option<u16>) {
        if kind == LastSend::Accepted {
            self.accepted.fetch_add(1, Ordering::Relaxed);
        }
        *self.last.lock().expect("analytics send stats") = Some(Last {
            kind,
            status,
            at_millis: crate::ports::now_millis(),
        });
    }

    /// Adds `n` events to the lost count.
    pub(super) fn drop_events(&self, n: usize) {
        self.dropped.fetch_add(n as u64, Ordering::Relaxed);
    }

    /// `true` exactly once: the first caller is the one that should say
    /// something about the first outcome.
    pub(super) fn claim_first_outcome(&self) -> bool {
        !self.first_outcome_logged.swap(true, Ordering::Relaxed)
    }

    /// `true` exactly once: the first per-event rejection.
    pub(super) fn claim_first_rejection(&self) -> bool {
        !self.first_rejection_logged.swap(true, Ordering::Relaxed)
    }

    /// The counters as an [`AnalyticsStatus`], with the decision fields left at
    /// their reporting defaults for the caller to overlay.
    pub(super) fn to_status(
        &self,
        endpoint: String,
        deployment: &'static str,
        lost_to_cancellation: u64,
    ) -> AnalyticsStatus {
        let last = *self.last.lock().expect("analytics send stats");
        AnalyticsStatus {
            decision: "reporting",
            reason: "reporting to the configured collector",
            deployment,
            endpoint: Some(endpoint),
            in_build: true,
            consent: None,
            last_send: last.map_or(LastSend::Never, |l| l.kind),
            last_status: last.and_then(|l| l.status),
            last_at: last.map(|l| crate::ports::iso8601(l.at_millis)),
            accepted: self.accepted.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed) + lost_to_cancellation,
        }
    }
}
