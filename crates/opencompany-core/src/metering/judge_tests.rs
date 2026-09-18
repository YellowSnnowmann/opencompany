use std::sync::Mutex;

use async_trait::async_trait;

use super::*;
use crate::ports::types::{CompanyRecord, CompanySummary, LedgerEntry};

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

#[test]
fn judge_usage_is_company_owned_and_runless() {
    let usage = TokenUsage {
        input: 80,
        output: 4,
        cached_input: 20,
        cost_usd: 0.001,
    };
    let sample = judge_sample(&usage, "openrouter", None).expect("non-zero usage");
    assert_eq!(sample.agent, UNATTRIBUTED_AGENT);
    assert_eq!(sample.kind, SampleKind::JudgeCall);
    assert_eq!(sample.run_id, None);
}

#[test]
fn zero_usage_is_not_an_invented_call() {
    assert!(judge_sample(&TokenUsage::default(), "managed", None).is_none());
}

#[tokio::test]
async fn completed_usage_is_recorded_even_when_the_judgment_is_discarded() {
    let usage = TokenUsage {
        input: 80,
        output: 4,
        cached_input: 20,
        cost_usd: 0.001,
    };
    let store = RecordingStore::default();
    let meter = RecordingMeter::default();
    record_judge_usage(
        &usage,
        "openrouter",
        None,
        &CompanyId::new("acme"),
        &store,
        Some(&meter),
    )
    .await;
    assert_eq!(meter.samples.lock().unwrap().len(), 1);
    assert_eq!(store.ledger.lock().unwrap().len(), 1);
}
