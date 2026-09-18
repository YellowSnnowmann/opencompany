//! [`TrackingUsageMeter`]: the seam that turns every metered sample into a
//! `turn_metered` event.
//!
//! # Why here and not at the cost hook
//!
//! The obvious place to read token counts is the harness cost hook
//! (`harness::built_in::cost::record_turn_cost`), which has the agent, the
//! provider, the run id and the totals in scope at once. It is also
//! `openhuman`-gated, and the *cycle*-level metering path deliberately reports
//! **zero** tokens on that build so the harness's per-turn accounting is not
//! double-counted (`ports::brain::UsageMetering`). So an event written at either
//! one of those two places is right for one build and blind on the other.
//!
//! [`UsageMeter::record`](crate::ports::UsageMeter::record) is where both paths
//! meet. Every `metering::record_*` function ends here, on every build, whether
//! the sample came from a per-turn harness hook, a per-cycle hosted brain, an
//! OAuth tool call or a search. Wrapping the port therefore instruments all of
//! them and changes not one call site — and the decorator shape is one this
//! tree already uses for exactly this reason (`EventingRunStore`,
//! `WorkspaceAnnouncer`, `QuotaEnforcedWorkspace`).
//!
//! # It never changes what is stored
//!
//! The inner meter's result is returned untouched, and the event is emitted
//! **only after** the inner write succeeds — a sample that failed to persist is
//! not usage that happened. A tracking wrapper that could fail a write, or
//! report one that did not land, would be worse than no instrumentation.

use std::sync::Arc;

use async_trait::async_trait;

use crate::Result;
use crate::analytics::{Event, Tracker};
use crate::ports::types::CompanyId;
use crate::ports::usage::{UsageMeter, UsageSample};

/// A [`UsageMeter`] that reports each recorded sample to a [`Tracker`].
pub struct TrackingUsageMeter {
    inner: Arc<dyn UsageMeter>,
    tracker: Arc<dyn Tracker>,
}

impl TrackingUsageMeter {
    /// Wraps `inner`.
    pub fn new(inner: Arc<dyn UsageMeter>, tracker: Arc<dyn Tracker>) -> Self {
        Self { inner, tracker }
    }
}

impl std::fmt::Debug for TrackingUsageMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TrackingUsageMeter")
    }
}

#[async_trait]
impl UsageMeter for TrackingUsageMeter {
    async fn record(&self, company: &CompanyId, sample: &UsageSample) -> Result<()> {
        let outcome = self.inner.record(company, sample).await;
        if outcome.is_ok() {
            // `Event::metered` is the only constructor for this event, and it
            // folds the sample's two free-form fields away: the agent name is
            // dropped entirely and the provider goes through the closed
            // vocabulary. The company id never appears — attribution is the
            // envelope's opaque instance id and nothing finer.
            self.tracker.track(Event::metered(sample));
        }
        outcome
    }

    async fn query(&self, company: &CompanyId, since_millis: u64) -> Result<Vec<UsageSample>> {
        self.inner.query(company, since_millis).await
    }
}

#[cfg(test)]
#[path = "meter_tests.rs"]
mod tests;
