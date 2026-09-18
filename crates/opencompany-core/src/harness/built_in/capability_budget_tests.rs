use super::*;
use crate::ports::{SampleKind, UsageSample};
use async_trait::async_trait;
use std::collections::HashSet;

fn inference_sample(input: u64, output: u64) -> UsageSample {
    UsageSample {
        at_millis: 0,
        agent: "ceo".into(),
        provider: "managed".into(),
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: 0,
        cost_usd: 0.0,
        kind: SampleKind::Inference,
        run_id: None,
        model: None,
    }
}

/// A meter whose `query` always errors — proves the fail-closed path.
struct FailingMeter;

#[async_trait]
impl UsageMeter for FailingMeter {
    async fn record(&self, _c: &CompanyId, _s: &UsageSample) -> crate::Result<()> {
        Ok(())
    }
    async fn query(&self, _c: &CompanyId, _since: u64) -> crate::Result<Vec<UsageSample>> {
        Err(crate::error::OpenCompanyError::Harness("boom".into()))
    }
}

/// A meter serving a fixed sample set.
struct FixedMeter(Vec<UsageSample>);

#[async_trait]
impl UsageMeter for FixedMeter {
    async fn record(&self, _c: &CompanyId, _s: &UsageSample) -> crate::Result<()> {
        Ok(())
    }
    async fn query(&self, _c: &CompanyId, _since: u64) -> crate::Result<Vec<UsageSample>> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn resolve_filter_no_meter_denies_all() {
    let plan = plan_named("pro").unwrap();
    let filter = resolve_filter(&plan, None, &CompanyId::new("acme"), 0).await;
    match filter {
        CapabilityFilter::DenyNamespaces(set) => {
            for ns in GATEABLE_NAMESPACES {
                assert!(set.contains(ns), "fail-closed must deny {ns}");
            }
        }
        other => panic!("expected DenyNamespaces, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_filter_query_error_denies_all() {
    let plan = plan_named("pro").unwrap();
    let meter = FailingMeter;
    let filter = resolve_filter(&plan, Some(&meter), &CompanyId::new("acme"), 0).await;
    match filter {
        CapabilityFilter::DenyNamespaces(set) => {
            assert_eq!(set.len(), GATEABLE_NAMESPACES.len());
        }
        other => panic!("expected DenyNamespaces, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_filter_under_budget_allows_all_when_plan_covers_all() {
    let plan = plan_named("unlimited").unwrap();
    let meter = FixedMeter(vec![inference_sample(100, 100)]);
    let filter = resolve_filter(&plan, Some(&meter), &CompanyId::new("acme"), 10).await;
    assert!(
        matches!(filter, CapabilityFilter::AllowAll),
        "unlimited under budget is identity"
    );
}

#[tokio::test]
async fn resolve_filter_partial_plan_denies_uncovered_even_under_budget() {
    let plan = plan_named("starter").unwrap();
    let meter = FixedMeter(vec![inference_sample(10, 10)]);
    let filter = resolve_filter(&plan, Some(&meter), &CompanyId::new("acme"), 10).await;
    match filter {
        CapabilityFilter::DenyNamespaces(set) => {
            assert!(set.contains("web") && set.contains("subagent"));
            assert!(!set.contains("shell") && !set.contains("code"));
        }
        other => panic!("expected DenyNamespaces, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_filter_over_budget_denies_exhausted_tier() {
    let plan = plan_named("starter").unwrap();
    // 150k + 150k = 300k > 200k → shell + code exhausted.
    let meter = FixedMeter(vec![inference_sample(150_000, 150_000)]);
    let filter = resolve_filter(&plan, Some(&meter), &CompanyId::new("acme"), 10).await;
    match filter {
        CapabilityFilter::DenyNamespaces(set) => {
            for ns in GATEABLE_NAMESPACES {
                assert!(set.contains(ns), "everything denied once exhausted: {ns}");
            }
        }
        other => panic!("expected DenyNamespaces, got {other:?}"),
    }
}

// --- fingerprint --------------------------------------------------------

#[test]
fn fingerprint_is_order_independent_and_stable() {
    let a: HashSet<&'static str> = ["shell", "web"].into_iter().collect();
    let b: HashSet<&'static str> = ["web", "shell"].into_iter().collect();
    assert_eq!(
        filter_fingerprint(&CapabilityFilter::DenyNamespaces(a)),
        filter_fingerprint(&CapabilityFilter::DenyNamespaces(b)),
    );
}

#[test]
fn fingerprint_diverges_on_different_denied_sets() {
    let allow = filter_fingerprint(&CapabilityFilter::AllowAll);
    let deny_shell = filter_fingerprint(&CapabilityFilter::DenyNamespaces(
        ["shell"].into_iter().collect(),
    ));
    let deny_web = filter_fingerprint(&CapabilityFilter::DenyNamespaces(
        ["web"].into_iter().collect(),
    ));
    assert_ne!(allow, deny_shell);
    assert_ne!(deny_shell, deny_web);
}

#[test]
fn allow_all_fingerprints_as_empty_deny() {
    let allow = filter_fingerprint(&CapabilityFilter::AllowAll);
    let empty = filter_fingerprint(&CapabilityFilter::DenyNamespaces(HashSet::new()));
    assert_eq!(allow, empty);
}
