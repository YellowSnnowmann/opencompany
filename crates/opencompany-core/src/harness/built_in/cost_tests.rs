use super::*;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::ports::types::{CompanyRecord, CompanySummary};
use crate::ports::usage::SampleKind;

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

fn turn_with(cost: f64) -> TurnUsage {
    TurnUsage {
        input_tokens: 100,
        output_tokens: 50,
        cached_input_tokens: 10,
        cost_usd: cost,
    }
}

#[test]
fn zero_usage_turn_produces_no_entry_or_sample() {
    let turn = TurnUsage::default();
    assert!(ledger_entry_for(&turn, "ceo").is_none());
    assert!(usage_sample_for(&turn, "ceo", "managed", None, None).is_none());
}

/// The `/openai/v1` passthrough reports tokens but no USD (billing happens
/// backend-side, off the wire). A token-bearing, zero-cost turn is still
/// real usage — it must produce a sample so the Usage surface is not blind.
#[test]
fn token_only_zero_cost_turn_is_not_zero_usage() {
    let turn = TurnUsage {
        input_tokens: 22,
        output_tokens: 2,
        cached_input_tokens: 0,
        cost_usd: 0.0,
    };
    assert!(usage_sample_for(&turn, "ceo", "managed", None, None).is_some());
    // No USD ⇒ no ledger entry, but the token sample still lands.
    assert!(ledger_entry_for(&turn, "ceo").is_none());
}

#[test]
fn spend_maps_to_inference_ledger_entry() {
    let turn = turn_with(0.42);
    let entry = ledger_entry_for(&turn, "ceo").unwrap();
    assert_eq!(entry.kind, "inference.spend");
    // Negative: an outflow, per the ledger convention (issue #1047).
    assert_eq!(entry.amount_usd, -0.42);
    assert_eq!(entry.memo, "ceo");
}

#[tokio::test]
async fn record_turn_cost_writes_ledger_and_sample() {
    let store = RecordingStore::default();
    let meter = RecordingMeter::default();
    let turn = turn_with(1.5);
    record_turn_cost(
        &turn,
        "ceo",
        "managed",
        None,
        &CompanyId::new("acme"),
        &store,
        Some(&meter),
        None,
    )
    .await
    .unwrap();

    let ledger = store.ledger.lock().unwrap();
    assert_eq!(ledger.len(), 1);
    // Negative: an outflow, per the ledger convention (issue #1047).
    assert_eq!(ledger[0].amount_usd, -1.5);
    let samples = meter.samples.lock().unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].kind, SampleKind::Inference);
    assert_eq!(samples[0].output_tokens, 50);
}

/// Issue #242: a turn that ran under a dispatched attempt attributes its
/// sample to that attempt, and one that did not stays unattributed. The
/// ledger entry is untouched either way — money still moves through the same
/// `inference.spend` line, so no accounting semantics change here.
#[tokio::test]
async fn a_sample_carries_the_attempt_its_turn_ran_under() {
    let store = RecordingStore::default();
    let meter = RecordingMeter::default();
    let turn = turn_with(0.5);

    record_turn_cost(
        &turn,
        "ceo",
        "managed",
        None,
        &CompanyId::new("acme"),
        &store,
        Some(&meter),
        Some("run-7"),
    )
    .await
    .unwrap();
    // …and a chat turn, which belongs to no attempt.
    record_turn_cost(
        &turn,
        "ceo",
        "managed",
        None,
        &CompanyId::new("acme"),
        &store,
        Some(&meter),
        None,
    )
    .await
    .unwrap();

    let samples = meter.samples.lock().unwrap();
    assert_eq!(samples[0].run_id.as_deref(), Some("run-7"));
    assert_eq!(samples[1].run_id, None);
    // Attribution only — the spend itself is unchanged.
    let ledger = store.ledger.lock().unwrap();
    assert_eq!(ledger.len(), 2);
    assert!(ledger.iter().all(|e| e.amount_usd == -0.5));

    // The field is additive on the wire: an unattributed sample serializes
    // exactly as it did before #242.
    let plain = serde_json::to_string(&samples[1]).expect("serialize");
    assert!(!plain.contains("runId"), "{plain}");
    let tagged = serde_json::to_string(&samples[0]).expect("serialize");
    assert!(tagged.contains(r#""runId":"run-7""#), "{tagged}");
}

/// Issue #1749: the turn's sample names the model it ran on.
///
/// This is the seam the issue is about — the richest one in the tree, with
/// agent, provider, run id and totals already in scope — so a model that
/// does not arrive here arrives nowhere.
#[tokio::test]
async fn a_turns_sample_names_the_model_it_ran_on() {
    let store = RecordingStore::default();
    let meter = RecordingMeter::default();
    let turn = turn_with(0.25);
    record_turn_cost(
        &turn,
        "ceo",
        "byok",
        Some(ModelSlug::classify("anthropic/claude-sonnet-4-6")),
        &CompanyId::new("acme"),
        &store,
        Some(&meter),
        None,
    )
    .await
    .unwrap();

    let samples = meter.samples.lock().unwrap();
    assert_eq!(
        samples[0].model.map(|m| m.as_str()),
        Some("anthropic-sonnet"),
        "`provider` says who served the tokens; only `model` says what ran"
    );
}

/// The BYOK containment, end to end at this seam: an operator-named model
/// is folded onto the fallback, and its raw name is nowhere in the sample
/// the meter is handed.
#[tokio::test]
async fn an_operator_named_model_never_reaches_the_meter() {
    let store = RecordingStore::default();
    let meter = RecordingMeter::default();
    let turn = turn_with(0.25);
    record_turn_cost(
        &turn,
        "ceo",
        "byok",
        Some(ModelSlug::classify("northwind-legal-review-v2")),
        &CompanyId::new("acme"),
        &store,
        Some(&meter),
        None,
    )
    .await
    .unwrap();

    let samples = meter.samples.lock().unwrap();
    assert_eq!(samples[0].model, Some(ModelSlug::OTHER));
    let persisted = serde_json::to_string(&samples[0]).expect("serialize");
    assert!(
        !persisted.to_ascii_lowercase().contains("northwind"),
        "the operator's model name reached the meter: {persisted}"
    );
}

/// The gate is `cost_usd == 0.0` on a value openhuman derives, not a
/// token-count threshold — so the smallest representable positive `f64`
/// still posts a ledger entry, however invisible its rendered amount
/// would be, and there is no rounding tolerance between "exactly zero"
/// and "not".
#[test]
fn the_smallest_representable_positive_cost_still_posts_a_ledger_entry() {
    // The smallest positive `f64` there is: the least subnormal, which is
    // smaller than `MIN_POSITIVE` (the smallest *normal*). Nothing positive
    // can sit closer to the gate than this.
    let smallest = f64::from_bits(1);
    assert!(smallest.is_subnormal() && smallest > 0.0);
    let turn = TurnUsage {
        input_tokens: 3,
        output_tokens: 1,
        cached_input_tokens: 0,
        cost_usd: smallest,
    };
    let entry = ledger_entry_for(&turn, "ceo")
        .expect("a nonzero cost, however small, still posts — the gate is exact equality to zero");
    assert_eq!(entry.amount_usd, -smallest);

    let exactly_zero = TurnUsage {
        cost_usd: 0.0,
        ..turn
    };
    assert!(
        ledger_entry_for(&exactly_zero, "ceo").is_none(),
        "and exactly zero is still the one value that does not post"
    );
}

/// A session that only ever runs managed-passthrough turns has real cost
/// (openhuman billed backend-side) this seam can never see — `cost_usd`
/// arrives `0.0` on every turn. The acknowledged limit is that Finances
/// stays silent about it forever, not just on one turn: repeated turns
/// keep producing zero ledger entries while their tokens keep
/// accumulating on the Usage surface, so the two surfaces drift apart by
/// design rather than by omission.
#[tokio::test]
async fn a_session_of_only_zero_cost_turns_never_posts_a_ledger_entry() {
    let store = RecordingStore::default();
    let meter = RecordingMeter::default();
    let turn = TurnUsage {
        input_tokens: 500,
        output_tokens: 120,
        cached_input_tokens: 0,
        cost_usd: 0.0,
    };
    for _ in 0..5 {
        record_turn_cost(
            &turn,
            "ceo",
            "managed",
            None,
            &CompanyId::new("acme"),
            &store,
            Some(&meter),
            None,
        )
        .await
        .unwrap();
    }

    assert!(
        store.ledger.lock().unwrap().is_empty(),
        "five real turns, still zero ledger entries — this seam has no visibility into \
         managed-passthrough spend at any scale"
    );
    assert_eq!(
        meter.samples.lock().unwrap().len(),
        5,
        "but every turn's tokens still land on the Usage surface"
    );
}

#[tokio::test]
async fn record_turn_cost_is_a_noop_for_zero_usage() {
    let store = RecordingStore::default();
    let meter = RecordingMeter::default();
    record_turn_cost(
        &TurnUsage::default(),
        "ceo",
        "managed",
        None,
        &CompanyId::new("acme"),
        &store,
        Some(&meter),
        None,
    )
    .await
    .unwrap();
    assert!(store.ledger.lock().unwrap().is_empty());
    assert!(meter.samples.lock().unwrap().is_empty());
}
