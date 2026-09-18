//! Emitting [`SampleKind::OauthCall`] usage samples — the write half of the
//! Usage view's calls-by-provider chart.
//!
//! [`bucket_usage`](super::bucket_usage) has always summed `OauthCall` samples
//! into `oauthCalls`, `byProvider`, and `connections`, but nothing ever emitted
//! one, so those three counters were structurally zero however much
//! connected-tool activity a company had. This module is the emit side.
//!
//! ## Why it lives here (always compiled) and not at the call site
//!
//! The connected-tool execution sites are behind non-default features
//! (`composio`, `mcp`), and CI builds only the default feature set — so code
//! written *at* those sites is neither compiled nor tested by CI. Keeping the
//! sample shape and the record-and-swallow behaviour here, beside the
//! aggregation that reads it, means the contract is unit-tested on every CI run
//! and the gated call site stays a single line.
//!
//! ## Metering never fails the work it meters
//!
//! [`record_oauth_call`] logs and swallows a meter error rather than
//! propagating it. A usage sample is accounting, not the operator's tool call:
//! a full disk or a transient store fault must not turn a Gmail send that
//! already happened into a tool-call failure the agent then retries.

use crate::ports::types::CompanyId;
use crate::ports::usage::{SampleKind, UsageMeter, UsageSample};

/// The provider slug recorded when a call site cannot name its provider.
///
/// Better than dropping the sample: the call genuinely happened, so
/// `oauthCalls` should count it even when the breakdown cannot attribute it.
pub const UNKNOWN_PROVIDER: &str = "unknown";

/// Normalises a provider slug for grouping: trimmed and lowercased, falling
/// back to [`UNKNOWN_PROVIDER`] when empty.
///
/// `byProvider` groups on the raw string, so `GitHub` and `github` would
/// otherwise land as two rows — and, because `connections` is the *count* of
/// those rows, inflate the connection count for a single connected account.
pub fn normalize_provider(provider: &str) -> String {
    let trimmed = provider.trim();
    if trimmed.is_empty() {
        return UNKNOWN_PROVIDER.to_string();
    }
    trimmed.to_ascii_lowercase()
}

/// The namespace prefix for a remote MCP server's provider slug.
///
/// Composio toolkit slugs and MCP server names are two different namespaces
/// that `by_provider` would otherwise merge, because it groups on one flat
/// string. A company with a Composio `gmail` toolkit and an MCP server its
/// operator also called `gmail` would see one row carrying the sum of both —
/// and, since the `connections` KPI is that map's *row count*, would be told it
/// has one connection where it has two (issue #698).
pub const MCP_PROVIDER_PREFIX: &str = "mcp:";

/// The provider slug recorded for a call through a remote MCP server.
///
/// Namespaced rather than bare, so the row cannot collide with a Composio
/// toolkit of the same name. The prefix is part of the grouping key, which
/// means it is also what an operator reads in the calls-by-provider chart —
/// deliberately, because "which of my two `gmail` connections was this?" is a
/// question the chart could not otherwise answer.
///
/// An empty or whitespace-only server name yields a bare [`UNKNOWN_PROVIDER`]
/// rather than a prefixed one. A sample that cannot name its server is not
/// evidence about MCP specifically, and minting `mcp:unknown` would invent a
/// connection row for a server that was never identified.
pub fn mcp_provider(server: &str) -> String {
    let normalized = normalize_provider(server);
    if normalized == UNKNOWN_PROVIDER {
        return normalized;
    }
    format!("{MCP_PROVIDER_PREFIX}{normalized}")
}

/// Builds the [`UsageSample`] for one connected-tool invocation.
///
/// Carries no tokens and no cost: an OAuth call is counted, not billed by token
/// — the money for a connected tool moves at the provider, not through our
/// inference meter. Keeping the token fields at zero is what lets the sample
/// share a stream with inference samples without polluting the token series.
pub fn oauth_call_sample(agent: &str, provider: &str, at_millis: u64) -> UsageSample {
    UsageSample {
        at_millis,
        agent: agent.to_string(),
        provider: normalize_provider(provider),
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        cost_usd: 0.0,
        kind: SampleKind::OauthCall,
        run_id: None,
        model: None,
    }
}

/// Records one connected-tool invocation against the company's usage meter.
///
/// Call this **after** a connected tool has actually reached its provider, so
/// the counters reflect calls that happened rather than calls that were
/// attempted. A meter failure is logged and swallowed — see the module docs.
pub async fn record_oauth_call(
    meter: &dyn UsageMeter,
    company: &CompanyId,
    agent: &str,
    provider: &str,
    at_millis: u64,
) {
    let sample = oauth_call_sample(agent, provider, at_millis);
    if let Err(err) = meter.record(company, &sample).await {
        tracing::warn!(
            company = %company,
            agent = %agent,
            provider = %sample.provider,
            error = %err,
            "[usage] failed to record an OAuth-call sample; the tool call itself succeeded"
        );
    }
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
