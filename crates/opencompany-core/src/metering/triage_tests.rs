use std::sync::Mutex;

use async_trait::async_trait;

use super::*;
use crate::ports::types::{CompanyRecord, CompanySummary, LedgerEntry};

fn usage() -> TokenUsage {
    TokenUsage {
        input: 120,
        output: 3,
        cached_input: 0,
        cost_usd: 0.0004,
    }
}

#[derive(Default)]
struct RecordingStore {
    ledger: Mutex<Vec<LedgerEntry>>,
}

#[async_trait]
impl CompanyStore for RecordingStore {
    async fn load(&self, _id: &CompanyId) -> crate::Result<Option<CompanyRecord>> {
        Ok(None)
    }
    async fn save(&self, _record: &CompanyRecord) -> crate::Result<()> {
        Ok(())
    }
    async fn list(&self) -> crate::Result<Vec<CompanySummary>> {
        Ok(Vec::new())
    }
    async fn append_ledger(&self, _id: &CompanyId, entry: LedgerEntry) -> crate::Result<()> {
        self.ledger.lock().unwrap().push(entry);
        Ok(())
    }
}

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

/// The meter is optional, the ledger is not: a host with no usage meter must
/// still record the spend it can prove (same contract as
/// [`record_turn_cost`](crate::harness::cost::record_turn_cost)).
#[tokio::test]
async fn meter_none_still_records_the_ledger_row() {
    let store = RecordingStore::default();
    let company = CompanyId::new("acme");
    record_triage_usage(&usage(), "openrouter", None, &company, &store, None).await;

    let ledger = store.ledger.lock().unwrap();
    assert_eq!(
        ledger.len(),
        1,
        "the spend row must survive without a meter"
    );
    let entry = &ledger[0];
    assert_eq!(entry.kind, super::super::inference::INFERENCE_SPEND_KIND);
    assert_eq!(entry.memo, UNATTRIBUTED_AGENT);
    assert!(
        (entry.amount_usd - (-0.0004)).abs() < 1e-9,
        "an outflow posts negative (issue #1047)"
    );
}

/// A wired meter receives the same sample `triage_sample` builds — the
/// `record` call and the shape the aggregation reads are one contract.
#[tokio::test]
async fn a_wired_meter_records_the_sample_and_the_ledger() {
    let store = RecordingStore::default();
    let meter = RecordingMeter::default();
    let company = CompanyId::new("acme");
    record_triage_usage(&usage(), "openrouter", None, &company, &store, Some(&meter)).await;

    let samples = meter.samples.lock().unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].kind, SampleKind::TriageCall);
    assert_eq!(samples[0].agent, UNATTRIBUTED_AGENT);
    assert_eq!(samples[0].provider, "openrouter");
    assert_eq!(samples[0].input_tokens, 120);
    assert_eq!(store.ledger.lock().unwrap().len(), 1);
}

/// The offline path is a no-op at the record level too — nothing to charge
/// and nothing to meter.
#[tokio::test]
async fn a_zero_usage_escalation_records_nothing() {
    let store = RecordingStore::default();
    let meter = RecordingMeter::default();
    let company = CompanyId::new("acme");
    record_triage_usage(
        &TokenUsage::default(),
        "managed",
        None,
        &company,
        &store,
        Some(&meter),
    )
    .await;
    assert!(store.ledger.lock().unwrap().is_empty());
    assert!(meter.samples.lock().unwrap().is_empty());
}

#[test]
fn a_completed_escalation_is_charged_to_the_company_not_a_teammate() {
    let sample = triage_sample(&usage(), "managed", None).expect("a sample for real spend");
    assert_eq!(sample.kind, SampleKind::TriageCall);
    assert_eq!(
        sample.agent, UNATTRIBUTED_AGENT,
        "triage runs before a teammate is chosen, so no teammate may be billed"
    );
    assert!(
        sample.run_id.is_none(),
        "an escalation belongs to no attempt"
    );
}

/// The offline path. A mock provider reports nothing, and a zero row would be
/// indistinguishable from a real call that happened to be free.
#[test]
fn an_escalation_that_moved_nothing_writes_no_row() {
    assert!(triage_sample(&TokenUsage::default(), "managed", None).is_none());
}

/// Distinct from planning, on purpose: the two are driven by different
/// things and tuned separately.
#[test]
fn triage_is_not_filed_as_planning() {
    let sample = triage_sample(&usage(), "managed", None).expect("a sample");
    assert_ne!(sample.kind, SampleKind::PlanningCall);
}
