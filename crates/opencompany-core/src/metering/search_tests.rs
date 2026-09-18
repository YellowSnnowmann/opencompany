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
fn a_search_sample_is_token_less_but_never_cost_less() {
    let s = search_call_sample("ceo", "Exa", 0.013, 1_700);
    assert_eq!(s.kind, SampleKind::SearchCall);
    assert_eq!(s.agent, "ceo");
    // Provider is normalised the same way OAuth providers are, so `Exa` and
    // `exa` are one row rather than two.
    assert_eq!(s.provider, "exa");
    assert_eq!(s.at_millis, 1_700);
    assert_eq!(s.input_tokens, 0);
    assert_eq!(s.output_tokens, 0);
    assert!((s.cost_usd - 0.013).abs() < 1e-9);
}

/// The headline invariant: a completed search can never be recorded free.
#[test]
fn a_backend_that_reports_no_cost_still_yields_a_priced_sample() {
    for reported in [0.0, -1.0, f64::NAN] {
        let s = search_call_sample("ceo", "", reported, 1);
        assert!(
            s.cost_usd > 0.0,
            "a paid call recorded at {reported} cost must still be priced"
        );
        assert!((s.cost_usd - FALLBACK_SEARCH_COST_USD).abs() < 1e-9);
    }
}

/// An unnamed provider attributes to the managed platform, not `unknown` —
/// the search *did* run on the managed surface, and `managed` is the slug
/// the inference samples already use.
#[test]
fn an_unnamed_provider_attributes_to_the_managed_platform() {
    assert_eq!(
        search_call_sample("ceo", "   ", 0.01, 1).provider,
        MANAGED_SEARCH_PROVIDER
    );
}

#[tokio::test]
async fn record_writes_one_sample_per_completed_search() {
    let meter = RecordingMeter::default();
    let company = CompanyId::new("acme");
    record_search_call(&meter, &company, "ceo", "Exa", 0.01, 1_000).await;
    record_search_call(&meter, &company, "ceo", "Exa", 0.01, 2_000).await;

    let samples = meter.samples.lock().unwrap();
    assert_eq!(samples.len(), 2);
    assert!(samples.iter().all(|s| s.kind == SampleKind::SearchCall));
}

#[tokio::test]
async fn a_meter_failure_never_surfaces_to_the_caller() {
    // The backend already charged for this search; failing here would make
    // the agent believe a completed search failed, and pay for a retry.
    record_search_call(
        &FailingMeter,
        &CompanyId::new("acme"),
        "ceo",
        "exa",
        0.01,
        1,
    )
    .await;
}

/// The emitted sample must survive the aggregation that reads it: it counts
/// in `searchCalls`, its cost rolls into the window total, and it neither
/// mints a connection nor distorts the token charts.
#[test]
fn emitted_samples_reach_the_console_counters_without_faking_a_connection() {
    use std::collections::HashMap;

    use crate::metering::{UsageRange, bucket_usage};

    let now = 1_700_000_000_000u64;
    let samples = vec![
        search_call_sample("ceo", "Exa", 0.01, now),
        search_call_sample("ceo", "Exa", 0.02, now),
    ];
    let usage = bucket_usage(&samples, UsageRange::D7, now, &HashMap::new());

    assert_eq!(usage.totals.search_calls, 2);
    assert!((usage.totals.cost_usd - 0.03).abs() < 1e-9);
    // Searches are not connected accounts: the connections KPI and the
    // calls-by-provider chart must stay untouched.
    assert_eq!(usage.totals.oauth_calls, 0);
    assert_eq!(usage.totals.connections, 0);
    assert!(usage.by_provider.is_empty());
    // And they carry no tokens, so no teammate appears as a zero-token bar.
    assert_eq!(usage.totals.tokens, 0);
    assert!(usage.by_agent.is_empty());
}
