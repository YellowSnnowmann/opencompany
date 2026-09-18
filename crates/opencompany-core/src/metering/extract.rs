//! Emitting [`SampleKind::ExtractionCall`] usage samples — what reading an
//! oversized tool result costs, and who it is charged to.
//!
//! # Why this is not a teammate's inference
//!
//! An extraction is the tool-less model call that turns a tool result too large
//! to inline into the part of it that answers the turn. It happens *inside*
//! another agent's turn, against a payload rather than a prompt, so there is no
//! separate agent to attribute it to and no run of its own. Like a titling pass
//! and a triage escalation, it is charged to the whole-company bucket
//! ([`UNATTRIBUTED_AGENT`]) with no `run_id`.
//!
//! # Why it is not folded into `Inference`
//!
//! Its cost curve is unlike a turn's. It fires once per oversized result, its
//! input is the payload — potentially hundreds of kilobytes — and a turn that
//! gathers several large results pays it several times. Folded into the
//! teammate's own `Inference` line, that spend would move whenever a tool got
//! chattier, and no operator could separate "this agent is expensive" from
//! "this agent's tools return a lot".
//!
//! # Both writes are logged and swallowed
//!
//! Same rule as the titling and selector paths: by the time this is called the
//! extraction has already happened and the turn is already carrying its result.
//! A ledger or meter hiccup must cost the accounting row, never the turn. The
//! tokens were genuinely spent either way, which is why the failure is logged
//! rather than silently dropped.

use crate::ports::types::{CompanyId, TokenUsage};
use crate::ports::usage::{SampleKind, UsageMeter, UsageSample};
use crate::ports::{CompanyStore, now_millis};

use super::inference::{UNATTRIBUTED_AGENT, inference_ledger_entry};

/// Builds the [`SampleKind::ExtractionCall`] sample for one completed
/// extraction, or `None` when it moved no tokens and cost nothing.
///
/// The `None` case is the offline/mock path: a provider reporting no usage
/// yields a zero [`TokenUsage`], and a row for it would claim a call happened
/// that is indistinguishable from a real free one.
pub fn extraction_sample(
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
        kind: SampleKind::ExtractionCall,
        run_id: None,
        model,
    })
}

/// Records one extraction's spend against the company ledger and the usage
/// meter. Both writes are best-effort and logged on failure.
pub async fn record_extraction_usage(
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
    tracing::debug!(
        company = %company,
        provider = %provider,
        input = usage.input,
        output = usage.output,
        cached_input = usage.cached_input,
        cost_usd = usage.cost_usd,
        "[usage] recording an oversized-tool-result extraction"
    );
    if let Some(entry) = inference_ledger_entry(usage, UNATTRIBUTED_AGENT)
        && let Err(err) = store.append_ledger(company, entry).await
    {
        tracing::warn!(
            company = %company,
            error = %err,
            "[usage] failed to append the extraction spend entry; the turn still stands"
        );
    }
    if let Some(sample) = extraction_sample(usage, provider, model)
        && let Some(meter) = meter
        && let Err(err) = meter.record(company, &sample).await
    {
        tracing::warn!(
            company = %company,
            error = %err,
            "[usage] failed to record the extraction usage sample; the turn still stands"
        );
    }
}
