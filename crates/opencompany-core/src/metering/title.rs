//! Emitting [`SampleKind::TitleCall`] usage samples — what naming a card costs,
//! and who it is charged to.
//!
//! # Why this is not a teammate's inference
//!
//! A titling pass is the tool-less model call that turns a request into a card's
//! headline. It happens while the card is being opened, before it has been
//! handed to anybody, so there is no agent to attribute it to. Like a triage
//! escalation and a responder selection, it is charged to the whole-company
//! bucket ([`UNATTRIBUTED_AGENT`]) with no `run_id`.
//!
//! It is deliberately not folded into
//! [`SampleKind::TriageCall`](crate::ports::usage::SampleKind). Triage runs on
//! every message the lexical layer abstained on; titling runs only on the ones
//! that become cards. Sharing a kind would make the triage line item move
//! whenever the board got busier, and neither number could be tuned against the
//! other.
//!
//! # Both writes are logged and swallowed
//!
//! Same rule as the triage and selector paths: the card has already been named
//! and the operator's turn is already running by the time this is called. A
//! ledger or meter hiccup must cost the accounting row and never the card. The
//! tokens were genuinely spent either way, which is why the failure is logged
//! rather than silently dropped.

use crate::ports::types::{CompanyId, TokenUsage};
use crate::ports::usage::{SampleKind, UsageMeter, UsageSample};
use crate::ports::{CompanyStore, now_millis};

use super::inference::{UNATTRIBUTED_AGENT, inference_ledger_entry};

/// Builds the [`SampleKind::TitleCall`] sample for one completed titling pass,
/// or `None` when it moved no tokens and cost nothing.
///
/// The `None` case is the offline/mock path, exactly as in
/// [`selector_sample`](super::selector_sample): a provider reporting no usage
/// yields a zero [`TokenUsage`], and a row for it would claim a call happened
/// that is indistinguishable from a real free one.
///
/// `agent` is not a parameter — attribution to [`UNATTRIBUTED_AGENT`] is the
/// rule this module holds, so no caller can bill a card's name to whoever the
/// card was then assigned to.
pub fn title_sample(
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
        kind: SampleKind::TitleCall,
        run_id: None,
        model,
    })
}

/// Records one completed titling pass: the Finances ledger entry (when it cost
/// USD) and, when a usage meter is wired, the usage sample.
///
/// The ledger entry goes through the same [`inference_ledger_entry`] the cycle's
/// inference spend uses, under the same `inference.spend` kind — titling spend
/// is inference spend as far as the money is concerned, and only the *usage*
/// breakdown cares about the distinction. The meter is deliberately optional: a
/// host with no usage meter still records the spend it can prove.
pub async fn record_title_usage(
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
        "[usage] recording a card titling pass"
    );
    if let Some(entry) = inference_ledger_entry(usage, UNATTRIBUTED_AGENT)
        && let Err(err) = store.append_ledger(company, entry).await
    {
        tracing::warn!(
            company = %company,
            error = %err,
            "[usage] failed to append the titling spend entry; the card still stands"
        );
    }
    if let Some(sample) = title_sample(usage, provider, model)
        && let Some(meter) = meter
        && let Err(err) = meter.record(company, &sample).await
    {
        tracing::warn!(
            company = %company,
            error = %err,
            "[usage] failed to record the titling usage sample; the card still stands"
        );
    }
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
