//! Per-tenant, per-period, fail-closed capability gating (issue #108).
//!
//! Cell A wired the exec-grade tool families (`shell`, `code`, `web`, and the
//! reserved `subagent`) behind grant namespaces, but its
//! [`CapabilityFilter`](crate::harness::toolbelt::CapabilityFilter) is a
//! process-wide static — a granted tier stays cost-unbounded forever because
//! nothing consults the [`UsageMeter`] before a turn. This module closes that
//! gap: it reads a tenant's token spend for the current budget period and
//! resolves the filter that gates the tenant's next turn.
//!
//! The **pure budget math** (plan resolution, period boundaries, spend summing,
//! exhaustion) lives in always-compiled
//! [`crate::metering::capability`] so the console read surface can share it; this
//! module re-exports those types and adds the `CapabilityFilter` wiring —
//! [`resolve_filter`] and [`filter_fingerprint`] — on top.
//!
//! ## Fail-closed
//!
//! Metering is a spend guard, so it defaults **closed**: a plan with no reachable
//! meter, or a meter whose query errors, denies **every** gateable namespace
//! (the turn still runs — intrinsic memory / file / MCP tools survive — it just
//! runs without exec tools). A gateable namespace absent from the plan's budget
//! map is likewise always denied: the map's key set *is* the capability set.
//!
//! ## Turn-boundary
//!
//! The gate resolves at [`HarnessPool::ensure`](crate::harness::HarnessPool),
//! which runs before every turn. A turn that starts under budget may finish over
//! it; the next `ensure` observes the new spend and flips the tier off. The
//! window is one turn — acceptable for a token budget, and documented so.

use crate::harness::toolbelt::{CapabilityFilter, GATEABLE_NAMESPACES};
use crate::ports::UsageMeter;
use crate::ports::types::CompanyId;

// Re-export the pure math so harness call sites keep addressing it as
// `capability_budget::{CapabilityPlan, BudgetPeriod, tokens_in, ...}`.
pub use crate::metering::capability::{
    BudgetPeriod, CapabilityPlan, TierBudgetStatus, plan_named, tokens_in,
};

/// Resolves the per-tenant [`CapabilityFilter`] for the turn about to run.
///
/// **Fail-closed**: with no meter, or a meter whose query errors, every gateable
/// namespace is denied (a `tracing::warn` records the error — it is *not*
/// propagated as a turn failure, so the turn still runs on its intrinsic tools).
/// Otherwise the tenant's period spend is summed and the plan's exhausted (and
/// unmapped) namespaces are denied; an empty deny set collapses to
/// [`CapabilityFilter::AllowAll`] (the identity fast path).
pub async fn resolve_filter(
    plan: &CapabilityPlan,
    meter: Option<&dyn UsageMeter>,
    company: &CompanyId,
    now: u64,
) -> CapabilityFilter {
    let Some(meter) = meter else {
        tracing::warn!(
            company = %company,
            "[capability-budget] no usage meter available; denying all gateable tools (fail-closed)"
        );
        return deny_all_gateable();
    };

    let since = plan.period.period_start_millis(now);
    let spent = match meter.query(company, since).await {
        Ok(samples) => tokens_in(&samples),
        Err(error) => {
            tracing::warn!(
                company = %company,
                %error,
                "[capability-budget] usage query failed; denying all gateable tools (fail-closed)"
            );
            return deny_all_gateable();
        }
    };

    let denied = plan.denied_namespaces(spent);
    if denied.is_empty() {
        CapabilityFilter::AllowAll
    } else {
        CapabilityFilter::DenyNamespaces(denied)
    }
}

/// The fail-closed filter: deny every gateable namespace.
fn deny_all_gateable() -> CapabilityFilter {
    CapabilityFilter::DenyNamespaces(GATEABLE_NAMESPACES.iter().copied().collect())
}

/// A stable fingerprint of a resolved filter's denied set, so
/// [`HarnessPool::ensure`](crate::harness::HarnessPool) can detect a capability
/// change between turns the same way it fingerprints the MCP and overlay-agent
/// sets. [`CapabilityFilter::AllowAll`] fingerprints as the empty set; two
/// `DenyNamespaces` filters with the same members fingerprint equal regardless
/// of insertion order.
pub fn filter_fingerprint(filter: &CapabilityFilter) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut denied: Vec<&str> = match filter {
        CapabilityFilter::AllowAll => Vec::new(),
        CapabilityFilter::DenyNamespaces(set) => set.iter().copied().collect(),
    };
    denied.sort_unstable();

    let mut hasher = DefaultHasher::new();
    denied.len().hash(&mut hasher);
    for namespace in denied {
        namespace.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
#[path = "capability_budget_tests.rs"]
mod tests;
