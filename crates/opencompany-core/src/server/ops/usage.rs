//! REST usage read surface (Phase 1): `GET …/usage?range=…`.
//!
//! A REST twin of the `Company.usage(range)` GraphQL resolver
//! (`graphql/usage.rs`). The operator console is REST-only and ships no GraphQL
//! client, so the Usage view could only render sample data; this exposes the
//! same pure projection ([`bucket_usage`](crate::metering::bucket_usage)) over
//! the write-plane's dual-scope router. No new business logic — it queries the
//! [`UsageMeter`](crate::ports::UsageMeter) for the window, resolves the roster
//! display-name map, and buckets.
//!
//! Data maturity: token/cost samples only populate on a harness
//! (`openhuman`-feature) build via `src/harness/cost.rs`. The offline build has
//! no cost hook, so the meter is empty and the series is (correctly) zero-filled
//! — that is the true value, not a stub.
//!
//! The same holds for OAuth-call samples
//! ([`SampleKind::OauthCall`](crate::ports::usage::SampleKind)): they are now
//! emitted per completed connected-tool call (see
//! [`metering::oauth`](crate::metering::oauth)), but only a `composio`-feature
//! build wires a connected-tool surface at all — so on a default build
//! `byProvider` / `oauthCalls` still read zero, and that zero is now the honest
//! answer ("no connected tools in this build") rather than a missing hook.

use axum::Json;
use axum::Router;
use axum::extract::Query;
use axum::routing::get;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::company::runtime::CompanyRuntime;
use crate::metering::{
    ProviderCalls, Usage, UsagePoint, UsageRange, bucket_usage, roster_display_names,
};
use crate::ports::now_millis;
use crate::server::error::ApiError;
use crate::server::ops::{ScopedCompany, scoped};

/// Milliseconds in a UTC day — the meter is queried `range.days()` of these back.
const MILLIS_PER_DAY: u64 = 86_400_000;

/// Builds the usage read route fragment (both scope forms).
pub fn router() -> Router<AppState> {
    scoped("/usage", get(get_usage))
}

/// The `?range=` selector. Accepts the console's `7d` / `30d` / `90d` keys (and
/// the bare-number / `D7` forms); anything else falls back to the 30-day default,
/// matching the GraphQL resolver's default window.
#[derive(Debug, Deserialize)]
struct UsageQuery {
    range: Option<String>,
}

/// Parses a `?range=` value into a [`UsageRange`], defaulting to 30 days.
fn parse_range(raw: Option<&str>) -> UsageRange {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("7d" | "7" | "d7") => UsageRange::D7,
        Some("90d" | "90" | "d90") => UsageRange::D90,
        _ => UsageRange::D30,
    }
}

/// The Usage read as the console renders it (camelCase). Wraps the pure
/// [`Usage`] projection; the inner types already serialize camelCase, but
/// [`Usage`] itself does not rename its fields, so this DTO carries the
/// `byAgent` / `byProvider` casing the frontend keys off.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageDto {
    /// Zero-filled daily token series over the range, oldest first.
    series: Vec<UsagePoint>,
    /// Tokens per teammate (desk), highest first.
    by_agent: Vec<AgentUsageDto>,
    /// OAuth calls per provider, highest first. Empty on a build with no
    /// connected-tool surface compiled in — see the module docs.
    by_provider: Vec<ProviderCalls>,
    /// Window totals.
    totals: UsageTotalsDto,
    /// Money exists in this response but is withheld from this principal.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    cost_hidden: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentUsageDto {
    name: String,
    tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageTotalsDto {
    input_tokens: u64,
    output_tokens: u64,
    tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    oauth_calls: u64,
    connections: u64,
    search_calls: u64,
}

impl UsageDto {
    fn new(usage: Usage, may_read_cost: bool) -> Self {
        let cost_hidden = !may_read_cost && usage.totals.cost_usd > 0.0;
        Self {
            series: usage.series,
            by_agent: usage
                .by_agent
                .into_iter()
                .map(|agent| AgentUsageDto {
                    name: agent.name,
                    tokens: agent.tokens,
                    cost_usd: may_read_cost.then_some(agent.cost_usd),
                })
                .collect(),
            by_provider: usage.by_provider,
            totals: UsageTotalsDto {
                input_tokens: usage.totals.input_tokens,
                output_tokens: usage.totals.output_tokens,
                tokens: usage.totals.tokens,
                cost_usd: may_read_cost.then_some(usage.totals.cost_usd),
                oauth_calls: usage.totals.oauth_calls,
                connections: usage.totals.connections,
                search_calls: usage.totals.search_calls,
            },
            cost_hidden,
        }
    }
}

/// Queries the meter and projects a company's usage for the window.
async fn project_usage(
    runtime: &CompanyRuntime,
    range: UsageRange,
    may_read_cost: bool,
) -> Result<UsageDto, ApiError> {
    let now = now_millis();
    let since = now.saturating_sub(range.days().saturating_mul(MILLIS_PER_DAY));
    let samples = runtime
        .usage()
        .query(runtime.id(), since)
        .await
        .map_err(ApiError)?;

    let record = runtime.store().load(runtime.id()).await.map_err(ApiError)?;
    let roster = record
        .as_ref()
        .map(|record| roster_display_names(&record.effective_agents(), &record.overlay_agents))
        .unwrap_or_default();

    Ok(UsageDto::new(
        bucket_usage(&samples, range, now, &roster),
        may_read_cost,
    ))
}

/// `GET …/usage?range=` — the company's usage read for the window.
async fn get_usage(
    company: ScopedCompany,
    Query(query): Query<UsageQuery>,
) -> Result<Json<UsageDto>, ApiError> {
    let range = parse_range(query.range.as_deref());
    Ok(Json(
        project_usage(company.runtime.as_ref(), range, company.may_read_contents).await?,
    ))
}

#[cfg(test)]
#[path = "usage_visibility_tests.rs"]
mod visibility_tests;

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
