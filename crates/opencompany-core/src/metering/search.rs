//! Emitting [`SampleKind::SearchCall`] usage samples — the write half of the
//! Usage view's `searchCalls` counter and the cost the managed search backend
//! charged (issue #238).
//!
//! ## Why this is not an `OauthCall`
//!
//! [`oauth_call_sample`](super::oauth::oauth_call_sample) hardcodes
//! `cost_usd: 0.0` **by design** — a connected-tool call is counted, not billed
//! through our meter, because the money moves at the provider. A managed web
//! search is the opposite shape: the tinyhumans backend charges the platform
//! per request and reports the amount back on the response, exactly the way
//! managed inference does. Recording it as an `OauthCall` would therefore throw
//! away the only cost figure we have *and* mint a phantom row in the
//! calls-by-provider chart (whose row count **is** the connections KPI), so a
//! company that has connected no account would read as having one. Hence a
//! distinct [`SampleKind`] with its own counter, and a `by_provider` chart that
//! stays OAuth-only.
//!
//! ## Why it lives here (always compiled)
//!
//! Same reason as [`oauth`](super::oauth): the emit site is inside the
//! feature-gated harness, so keeping the sample shape here — beside the
//! aggregation that reads it — means the contract is unit-tested on the default
//! CI build and the gated call site stays one line.
//!
//! ## Metering never fails the work it meters
//!
//! [`record_search_call`] logs and swallows a meter error. The search already
//! happened and the backend already charged for it; a full disk must not turn a
//! completed search into a tool-call failure the agent then retries (and pays
//! for twice).

use crate::ports::types::CompanyId;
use crate::ports::usage::{SampleKind, UsageMeter, UsageSample};

use super::oauth::normalize_provider;

/// The provider slug recorded when the backend does not name the upstream
/// engine it resolved a search to.
///
/// `managed` matches the inference samples' provider slug, so the two priced
/// managed surfaces read as one platform rather than as an unattributed
/// `unknown`.
pub const MANAGED_SEARCH_PROVIDER: &str = "managed";

/// The cost attributed to a completed search when the backend reports none.
///
/// The managed search path is priced per request (OpenHuman's Parallel
/// integration documents ~$0.01/request), and every response carries a
/// `costUsd`. An older or degraded backend can still answer with `0`, and a
/// completed *paid* call recorded at zero cost is worse than a slightly wrong
/// number: it makes the Usage view claim searches are free, which is the exact
/// failure mode the issue set out to end ("a paid call is never free"). So a
/// non-positive reported cost floors to this documented list price rather than
/// to zero.
pub const FALLBACK_SEARCH_COST_USD: f64 = 0.01;

/// The USD cost to attribute to one completed search, given what the backend
/// reported. See [`FALLBACK_SEARCH_COST_USD`] for why zero is never recorded.
pub fn attributed_cost_usd(reported: f64) -> f64 {
    if reported.is_finite() && reported > 0.0 {
        reported
    } else {
        FALLBACK_SEARCH_COST_USD
    }
}

/// Builds the [`UsageSample`] for one **completed** web search.
///
/// Carries no tokens (a search consumes none) but a real `cost_usd`, so it
/// rolls into the window's cost total the way an inference sample does while
/// staying out of the token series and the tokens-by-teammate chart.
pub fn search_call_sample(
    agent: &str,
    provider: &str,
    reported_cost_usd: f64,
    at_millis: u64,
) -> UsageSample {
    let provider = normalize_provider(provider);
    let provider = if provider == super::oauth::UNKNOWN_PROVIDER {
        MANAGED_SEARCH_PROVIDER.to_string()
    } else {
        provider
    };
    UsageSample {
        at_millis,
        agent: agent.to_string(),
        provider,
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        cost_usd: attributed_cost_usd(reported_cost_usd),
        kind: SampleKind::SearchCall,
        run_id: None,
        model: None,
    }
}

/// Records one completed web search against the company's usage meter.
///
/// Call this **only after** the backend has answered — a search that failed
/// (transport error, non-2xx, budget refusal) records nothing, because nothing
/// was charged. A meter failure is logged and swallowed; see the module docs.
pub async fn record_search_call(
    meter: &dyn UsageMeter,
    company: &CompanyId,
    agent: &str,
    provider: &str,
    reported_cost_usd: f64,
    at_millis: u64,
) {
    let sample = search_call_sample(agent, provider, reported_cost_usd, at_millis);
    if let Err(err) = meter.record(company, &sample).await {
        tracing::warn!(
            company = %company,
            agent = %agent,
            provider = %sample.provider,
            error = %err,
            "[usage] failed to record a search-call sample; the search itself succeeded"
        );
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
