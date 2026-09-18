//! Metering for the workflow sufficiency judge (issue #1866).
//!
//! A judgment is a single company-owned, tool-less model call. It is not part
//! of the agent's turn, so it carries the company bucket and no run id. Calls
//! are recorded even when their answer is malformed and discarded; only a
//! provider-reported zero usage produces no accounting row.

use crate::ports::types::{CompanyId, TokenUsage};
use crate::ports::usage::{SampleKind, UsageMeter, UsageSample};
use crate::ports::{CompanyStore, now_millis};

use super::inference::{UNATTRIBUTED_AGENT, inference_ledger_entry};

pub fn judge_sample(
    usage: &TokenUsage,
    provider: &str,
    model: Option<crate::metering::ModelSlug>,
) -> Option<UsageSample> {
    if usage.is_zero() {
        return None;
    }
    Some(UsageSample {
        at_millis: now_millis(),
        agent: UNATTRIBUTED_AGENT.to_string(),
        provider: super::oauth::normalize_provider(provider),
        input_tokens: usage.input,
        output_tokens: usage.output,
        cached_input_tokens: usage.cached_input,
        cost_usd: usage.cost_usd,
        kind: SampleKind::JudgeCall,
        run_id: None,
        model,
    })
}

pub async fn record_judge_usage(
    usage: &TokenUsage,
    provider: &str,
    model: Option<crate::metering::ModelSlug>,
    company: &CompanyId,
    store: &dyn CompanyStore,
    meter: Option<&dyn UsageMeter>,
) {
    if usage.is_zero() {
        return;
    }
    if let Some(entry) = inference_ledger_entry(usage, UNATTRIBUTED_AGENT)
        && let Err(err) = store.append_ledger(company, entry).await
    {
        tracing::warn!(company = %company, error = %err, "[usage] failed to record judge spend");
    }
    if let Some(sample) = judge_sample(usage, provider, model)
        && let Some(meter) = meter
        && let Err(err) = meter.record(company, &sample).await
    {
        tracing::warn!(company = %company, error = %err, "[usage] failed to record judge sample");
    }
}

#[cfg(test)]
#[path = "judge_tests.rs"]
mod tests;
