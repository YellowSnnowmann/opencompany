//! Pure usage aggregation: raw [`UsageSample`]s → the console's [`Usage`] shape.
//!
//! No I/O. WS2's `graphql/usage.rs` resolver queries the
//! [`UsageMeter`](crate::ports::UsageMeter) for the window, resolves the roster
//! display-name map, and calls [`bucket_usage`].

use std::collections::HashMap;

use crate::ports::usage::{SampleKind, UsageSample};

use super::calendar::{epoch_day, iso_day};
use super::types::{AgentTokens, ProviderCalls, Usage, UsagePoint, UsageRange, UsageTotals};

/// Aggregates a company's usage samples into the [`Usage`] read surface.
///
/// - `samples`: the window's samples (the caller queries the meter with
///   `since = now - range.days()`; extra samples outside the day window still
///   feed totals but never widen the series).
/// - `range`: the number of daily buckets (7 / 30 / 90), all ending on the UTC
///   day of `now_millis`.
/// - `roster`: teammate id → display name (prosumer language). Ids missing from
///   the map fall back to the raw id.
pub fn bucket_usage(
    samples: &[UsageSample],
    range: UsageRange,
    now_millis: u64,
    roster: &HashMap<String, String>,
) -> Usage {
    let days = range.days();
    let today = epoch_day(now_millis);
    // The oldest bucket day inclusive: `days` buckets ending today.
    let first_day = today - (days as i64 - 1);

    // Series: zero-filled per-day input/output token sums.
    let mut per_day: HashMap<i64, (u64, u64)> = HashMap::new();
    // Per-agent token and USD sums (keyed by raw agent id).
    let mut per_agent: HashMap<String, (u64, f64)> = HashMap::new();
    // Per-provider OAuth-call counts.
    let mut per_provider: HashMap<String, u64> = HashMap::new();

    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut oauth_calls: u64 = 0;
    let mut search_calls: u64 = 0;

    for s in samples {
        total_input += s.input_tokens;
        total_output += s.output_tokens;
        total_cost += s.cost_usd;

        // "Tokens by teammate" is a token chart, so a token-less sample must not
        // mint a row there. `OauthCall` samples are deliberately zero-token, and
        // before they were ever emitted this branch could not be reached by one;
        // without the guard, an agent that only made connected-tool calls would
        // appear as a 0-token bar.
        let tokens = s.input_tokens + s.output_tokens;
        let agent_attributed = !matches!(s.kind, SampleKind::OauthCall | SampleKind::SearchCall);
        if agent_attributed && (tokens > 0 || s.cost_usd > 0.0) {
            let row = per_agent.entry(s.agent.clone()).or_default();
            row.0 += tokens;
            row.1 += s.cost_usd;
        }

        if s.kind == SampleKind::OauthCall {
            oauth_calls += 1;
            *per_provider.entry(s.provider.clone()).or_default() += 1;
        }

        // Metered web searches (issue #238) get their own counter and stay OUT
        // of `per_provider`. That map's *row count* is the connections KPI, so
        // folding searches in would report a company with no connected account
        // as having one — the concrete reason this is not an `OauthCall`. The
        // search's cost is already in `total_cost` above, so it rolls into the
        // window's spend without a second code path.
        if s.kind == SampleKind::SearchCall {
            search_calls += 1;
        }

        let day = epoch_day(s.at_millis);
        if day >= first_day && day <= today {
            let slot = per_day.entry(day).or_default();
            slot.0 += s.input_tokens;
            slot.1 += s.output_tokens;
        }
    }

    let series = (0..days)
        .map(|i| {
            let day = first_day + i as i64;
            let (input_tokens, output_tokens) = per_day.get(&day).copied().unwrap_or_default();
            UsagePoint {
                date: iso_day(day),
                input_tokens,
                output_tokens,
            }
        })
        .collect();

    let mut by_agent: Vec<AgentTokens> = per_agent
        .into_iter()
        .map(|(id, (tokens, cost_usd))| AgentTokens {
            name: roster.get(&id).cloned().unwrap_or(id),
            tokens,
            cost_usd,
        })
        .collect();
    // Highest tokens first; name as a stable tie-breaker.
    by_agent.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));

    let connections = per_provider.len() as u64;
    let mut by_provider: Vec<ProviderCalls> = per_provider
        .into_iter()
        .map(|(provider, calls)| ProviderCalls { provider, calls })
        .collect();
    by_provider.sort_by(|a, b| {
        b.calls
            .cmp(&a.calls)
            .then_with(|| a.provider.cmp(&b.provider))
    });

    Usage {
        series,
        by_agent,
        by_provider,
        totals: UsageTotals {
            input_tokens: total_input,
            output_tokens: total_output,
            tokens: total_input + total_output,
            cost_usd: total_cost,
            oauth_calls,
            connections,
            search_calls,
        },
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
