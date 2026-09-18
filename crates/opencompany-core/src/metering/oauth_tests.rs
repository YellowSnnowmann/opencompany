use super::*;

use std::sync::Mutex;

use async_trait::async_trait;

use crate::error::OpenCompanyError;

#[derive(Default)]
struct RecordingMeter {
    samples: Mutex<Vec<UsageSample>>,
}

#[async_trait]
impl UsageMeter for RecordingMeter {
    async fn record(&self, _company: &CompanyId, sample: &UsageSample) -> crate::Result<()> {
        self.samples.lock().unwrap().push(sample.clone());
        Ok(())
    }
    async fn query(&self, _company: &CompanyId, _since: u64) -> crate::Result<Vec<UsageSample>> {
        Ok(self.samples.lock().unwrap().clone())
    }
}

/// A meter whose writes always fail — proves metering cannot fail the work.
struct FailingMeter;

#[async_trait]
impl UsageMeter for FailingMeter {
    async fn record(&self, _company: &CompanyId, _sample: &UsageSample) -> crate::Result<()> {
        Err(OpenCompanyError::Store("disk on fire".to_string()))
    }
    async fn query(&self, _company: &CompanyId, _since: u64) -> crate::Result<Vec<UsageSample>> {
        Ok(Vec::new())
    }
}

#[test]
fn sample_is_a_zero_token_zero_cost_oauth_call() {
    let s = oauth_call_sample("ceo", "gmail", 1_700);
    assert_eq!(s.kind, SampleKind::OauthCall);
    assert_eq!(s.agent, "ceo");
    assert_eq!(s.provider, "gmail");
    assert_eq!(s.at_millis, 1_700);
    // An OAuth call is counted, never token-billed — zeros here keep it out
    // of the token series it shares a stream with.
    assert_eq!(s.input_tokens, 0);
    assert_eq!(s.output_tokens, 0);
    assert_eq!(s.cached_input_tokens, 0);
    assert_eq!(s.cost_usd, 0.0);
}

#[test]
fn provider_is_normalized_so_one_account_is_one_connection() {
    assert_eq!(normalize_provider("GitHub"), "github");
    assert_eq!(normalize_provider("  gmail  "), "gmail");
    assert_eq!(normalize_provider("SLACK"), "slack");
}

#[test]
fn blank_provider_falls_back_rather_than_dropping_the_call() {
    assert_eq!(normalize_provider(""), UNKNOWN_PROVIDER);
    assert_eq!(normalize_provider("   "), UNKNOWN_PROVIDER);
    assert_eq!(oauth_call_sample("ceo", "", 1).provider, UNKNOWN_PROVIDER);
}

#[tokio::test]
async fn record_writes_one_sample_per_call() {
    let meter = RecordingMeter::default();
    let company = CompanyId::new("acme");
    record_oauth_call(&meter, &company, "ceo", "GMAIL", 1_000).await;
    record_oauth_call(&meter, &company, "ceo", "gmail", 2_000).await;

    let samples = meter.samples.lock().unwrap();
    assert_eq!(samples.len(), 2);
    assert!(samples.iter().all(|s| s.kind == SampleKind::OauthCall));
    // Both spellings collapse onto one provider, so this reads as one
    // connection with two calls rather than two connections with one each.
    assert!(samples.iter().all(|s| s.provider == "gmail"));
}

#[tokio::test]
async fn a_meter_failure_never_surfaces_to_the_caller() {
    // The tool call already reached the provider; failing here would make
    // the agent believe a completed action failed, and retry it.
    record_oauth_call(&FailingMeter, &CompanyId::new("acme"), "ceo", "gmail", 1).await;
}

/// The emitted sample must survive the aggregation that reads it — the
/// whole point of the issue is that these three counters stop being zero.
#[test]
fn emitted_samples_reach_the_console_counters() {
    use std::collections::HashMap;

    use crate::metering::{UsageRange, bucket_usage};

    let now = 1_700_000_000_000u64;
    let samples = vec![
        oauth_call_sample("ceo", "gmail", now),
        oauth_call_sample("ceo", "gmail", now),
        oauth_call_sample("ops", "github", now),
    ];
    let usage = bucket_usage(&samples, UsageRange::D7, now, &HashMap::new());

    assert_eq!(usage.totals.oauth_calls, 3);
    assert_eq!(usage.totals.connections, 2);
    assert_eq!(usage.by_provider.len(), 2);
    assert_eq!(usage.by_provider[0].provider, "gmail");
    assert_eq!(usage.by_provider[0].calls, 2);
    assert_eq!(usage.by_provider[1].provider, "github");
    assert_eq!(usage.by_provider[1].calls, 1);
    // OAuth calls carry no tokens, so they never distort the token series.
    assert_eq!(usage.totals.tokens, 0);
}

#[test]
fn an_mcp_server_is_namespaced_away_from_composio_toolkits() {
    assert_eq!(mcp_provider("linear"), "mcp:linear");
    // Same normalisation as every other provider: the prefix does not
    // exempt a server name from the case folding that stops `Linear` and
    // `linear` becoming two connections.
    assert_eq!(mcp_provider("  LINEAR "), "mcp:linear");
}

#[test]
fn an_unnamed_server_does_not_mint_a_phantom_mcp_connection() {
    // `mcp:unknown` would be a connection row for a server nobody can
    // point at. Falling back to the bare slug keeps the call counted in
    // `oauthCalls` without inventing an MCP connection in `byProvider`.
    assert_eq!(mcp_provider(""), UNKNOWN_PROVIDER);
    assert_eq!(mcp_provider("   "), UNKNOWN_PROVIDER);
}

/// The collision issue #698 asks to prevent, asserted through the
/// aggregation rather than on the string, because the string is not the
/// claim — "two connections, counted separately" is.
#[test]
fn a_composio_toolkit_and_an_mcp_server_of_one_name_stay_two_connections() {
    use std::collections::HashMap;

    use crate::metering::{UsageRange, bucket_usage};

    let now = 1_700_000_000_000u64;
    let samples = vec![
        // The Composio toolkit: bare slug, as `composio_execute` emits it.
        oauth_call_sample("ceo", "gmail", now),
        // An MCP server the operator also named `gmail`.
        oauth_call_sample("ceo", &mcp_provider("gmail"), now),
    ];
    let usage = bucket_usage(&samples, UsageRange::D7, now, &HashMap::new());

    assert_eq!(usage.totals.oauth_calls, 2);
    assert_eq!(
        usage.totals.connections, 2,
        "an MCP server and a Composio toolkit sharing a name are two \
         connections; merging them under-reports the KPI"
    );
    let providers: Vec<&str> = usage
        .by_provider
        .iter()
        .map(|p| p.provider.as_str())
        .collect();
    assert!(providers.contains(&"gmail"), "{providers:?}");
    assert!(providers.contains(&"mcp:gmail"), "{providers:?}");
}
