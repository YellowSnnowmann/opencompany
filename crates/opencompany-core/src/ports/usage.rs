//! The [`UsageMeter`] port: durable per-company usage samples.
//!
//! Every metered event — a model inference turn, or an OAuth-connected tool
//! call — is recorded as one [`UsageSample`]. The WS4 cost hook writes samples
//! here; the WS5 Usage/Finances reads aggregate them (`query` returns the
//! window a console chart renders). Samples are non-secret accounting rows;
//! money still resolves from the ledger and `[budget]`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::metering::ModelSlug;
use crate::ports::types::CompanyId;

/// How long a [`UsageMeter`] backend retains samples: the console's maximum
/// window (`UsageRange::D90`). Backends evict samples older than this on write.
pub const RETENTION_DAYS: u64 = 90;

/// [`RETENTION_DAYS`] expressed in milliseconds.
pub const RETENTION_MILLIS: u64 = RETENTION_DAYS * 86_400_000;

/// The oldest `at_millis` a backend keeps, given the newest sample it has seen
/// (typically ~now). Samples strictly older than this are evicted; a sample
/// exactly [`RETENTION_DAYS`] old is still inside the window and kept.
///
/// Anchoring to the newest observed sample (rather than wall-clock now) keeps
/// eviction deterministic and testable, and never discards a company's only
/// recent data just because the process clock moved.
pub fn retention_cutoff(newest_at_millis: u64) -> u64 {
    newest_at_millis.saturating_sub(RETENTION_MILLIS)
}

/// What produced a [`UsageSample`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SampleKind {
    /// Tokens consumed by a model inference call.
    Inference,
    /// An OAuth-connected tool invocation (populates the calls-by-provider
    /// chart). Wired by the runtime when a connected tool runs.
    OauthCall,
    /// One completed metered web search (issue #238).
    ///
    /// Deliberately **not** [`Self::OauthCall`]. That kind is defined as
    /// zero-cost — the money for a connected tool moves at the provider, not
    /// through our meter — and it is what mints the calls-by-provider /
    /// connections chart, so reusing it would both drop the cost the search
    /// backend actually charged and invent a "connection" for a company that
    /// has connected no account. A search is a *priced* call on the managed
    /// platform, so it carries a real `cost_usd` and gets its own counter.
    SearchCall,
    /// One completed planning pass — the single tool-less model call a card
    /// makes on entering `planning` (issue #337).
    ///
    /// Deliberately **not** [`Self::Inference`], even though it is literally an
    /// inference call, because the two answer different questions. Every
    /// `Inference` sample is a *teammate's* turn: it is attributed to an agent,
    /// it may carry a `run_id`, and it is what the per-teammate token chart and
    /// the daily-spend cap are reasoning about. A planning pass belongs to no
    /// teammate — planning is frequently what *picks* the teammate — so it is
    /// charged to the whole-company bucket
    /// ([`UNATTRIBUTED_AGENT`](crate::metering::UNATTRIBUTED_AGENT)) with no
    /// `run_id`, and a separate kind is what lets an operator later ask "how
    /// much are we spending on planning?" without that answer being tangled up
    /// in "how much did Maya spend?".
    ///
    /// It **does** count toward the capability-tier token budget (issue #108):
    /// see [`tokens_in`](crate::metering::tokens_in). Excluding it would let a
    /// company plan indefinitely after its tier budget was exhausted, which is
    /// exactly the leak the tier exists to close.
    PlanningCall,
    /// One tool-less sufficiency judgment after a workflow node's deterministic
    /// postcondition passes (issue #1866). The company owns this call; it is not
    /// part of the agent turn and has no run-row attribution.
    JudgeCall,
    /// One completed triage escalation — the tool-less model call an operator
    /// message makes when the lexical classifier abstained (issue #678).
    ///
    /// Its own kind for the same reason [`Self::PlanningCall`] is: it belongs to
    /// no teammate, so it is charged to the whole-company bucket with no
    /// `run_id`, and an operator asking "what is triage costing us?" should not
    /// have to read that answer out of a teammate's column. Distinct from
    /// `PlanningCall` too — conflating them would make the planning line item
    /// move whenever chat volume moved, and the two are tuned independently.
    ///
    /// Counts toward the capability-tier token budget, exactly as a planning
    /// pass does — see [`tokens_in`](crate::metering::tokens_in). It is
    /// company-driven model spend, and excluding it would let chat keep paying
    /// for classification after the tier budget was exhausted.
    TriageCall,
    /// One completed responder selection — the tool-less model call an
    /// unmentioned message in an `auto` channel makes to pick its best-fit
    /// answerer (issue #1835).
    ///
    /// Its own kind for the reason [`Self::TriageCall`] is its own kind: it
    /// belongs to no teammate — selection is what *picks* the teammate — so it
    /// is charged to the whole-company bucket with no `run_id`, and "what is
    /// routing costing us?" should be answerable without reading a teammate's
    /// column. Distinct from `TriageCall` too: triage is driven by raw chat
    /// volume everywhere, selection only by unmentioned messages in `auto`
    /// channels, and conflating them would make the triage line move whenever
    /// an operator created a channel.
    ///
    /// Counts toward the capability-tier token budget, exactly as triage does
    /// — see [`tokens_in`](crate::metering::tokens_in) — and for the same
    /// leak: selection is per-message spend, so a company past its ceiling
    /// must not keep paying to route.
    SelectorCall,
    /// One completed card-titling pass — the single tool-less model call that
    /// names the work a request asks for.
    ///
    /// Charged to the whole-company bucket with no `run_id`, on
    /// [`SampleKind::TriageCall`]'s exact terms: the pass runs while a card is
    /// being opened, before any teammate has been handed it, so there is no
    /// agent whose budget it could belong to.
    ///
    /// Its own kind rather than folded into `TriageCall` because the two are
    /// driven by different things — triage by every abstained-on message,
    /// titling only by messages that become cards — so a shared kind would move
    /// the triage line whenever the board got busier, and neither number could
    /// be tuned against the other.
    ///
    /// Counts toward the capability-tier token budget: it is per-card spend a
    /// company past its ceiling must stop paying.
    TitleCall,
    /// One completed oversized-tool-result extraction — the single tool-less
    /// model call that turns a payload too large to inline into the part of it
    /// that answers the turn (issue #6014).
    ///
    /// Deliberately **not** [`Self::Inference`], on the same reasoning as
    /// [`Self::TitleCall`] and [`Self::PlanningCall`]: an `Inference` sample is
    /// a teammate's turn, attributed to an agent and counted against the
    /// per-teammate chart and the daily cap. An extraction happens *inside*
    /// another agent's turn, on a tool result, and is charged to the
    /// whole-company bucket with no `run_id`.
    ///
    /// It gets its own kind because its cost curve is unlike anything else
    /// here: it fires once per oversized tool result, its input is the payload
    /// rather than a prompt, and a turn that hits several large results pays it
    /// several times. Folded into any existing kind, that spend would be
    /// invisible exactly when it mattered — which is how it shipped unmetered
    /// and was caught in review on tinyhumansai/opencompany#2153.
    ExtractionCall,
    /// One completed first-run setup pass — the single tool-less model call
    /// that turns three answers into a starting roster
    /// (`docs/spec/runtime/company-setup.md`).
    ///
    /// A sibling of [`Self::PlanningCall`] and not of [`Self::Inference`], for
    /// the same reason: this call belongs to no teammate. It runs *before the
    /// roster exists*, so there is not yet an agent it could be attributed to,
    /// and it mints no attempt row — [`UNATTRIBUTED_AGENT`](crate::metering::UNATTRIBUTED_AGENT)
    /// with no `run_id` is the truth rather than a gap.
    ///
    /// Its own kind rather than a reused `PlanningCall` because the two are
    /// asked about separately: setup runs once per company and is a
    /// first-impression cost we are actively measuring, while planning recurs
    /// for the life of the company. Folding them together would make "what does
    /// onboarding a company cost?" unanswerable.
    ///
    /// Counted toward the capability-tier token budget exactly like planning —
    /// see [`tokens_in`](crate::metering::tokens_in).
    SetupCall,
    /// One completed authoring assist — the single tool-less model call that
    /// drafts text an operator will read and then keep or throw away
    /// (issue #1776: a teammate's mandate or persona).
    ///
    /// A sibling of [`Self::PlanningCall`] and [`Self::SetupCall`] rather than
    /// of [`Self::Inference`], for the reason those two are: the call belongs to
    /// no teammate's turn. It is *about* a teammate, which is not the same
    /// thing — the teammate did not run, and attributing the draft to it would
    /// both corrupt "how much did Maya spend?" and let an operator's drafting
    /// eat the daily cap of the very teammate they are describing. So it is
    /// charged to the whole-company bucket
    /// ([`UNATTRIBUTED_AGENT`](crate::metering::UNATTRIBUTED_AGENT)) with no
    /// `run_id`.
    ///
    /// Its own kind rather than a reused [`Self::SetupCall`] because that one is
    /// explicitly the once-per-company onboarding cost, and folding a recurring
    /// authoring assist into it would make "what does onboarding a company
    /// cost?" unanswerable — the exact question `SetupCall` was split out to
    /// keep answerable.
    ///
    /// Counted toward the capability-tier token budget exactly like planning and
    /// setup — see [`tokens_in`](crate::metering::tokens_in). Drafting is
    /// company-driven model spend, and excluding it would let an operator keep
    /// drafting after the tier budget was exhausted.
    AuthoringCall,
}

/// One metered usage event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSample {
    /// Epoch-millis timestamp the event happened.
    pub at_millis: u64,
    /// The agent that produced the usage.
    pub agent: String,
    /// The inference/tool provider slug (e.g. `managed`, `github`).
    pub provider: String,
    /// Input/prompt tokens consumed.
    pub input_tokens: u64,
    /// Output/completion tokens produced.
    pub output_tokens: u64,
    /// Input tokens served from the KV cache.
    pub cached_input_tokens: u64,
    /// USD cost attributed to the sample.
    pub cost_usd: f64,
    /// What produced the sample.
    pub kind: SampleKind,
    /// The task **attempt** ([`RunRecord`](crate::ports::runs::RunRecord)) whose
    /// turn produced this usage, when it ran under one (issue #242).
    ///
    /// Purely an attribution key: it lets "what did this attempt cost?" be
    /// answered from the meter as well as from the run row, and "which attempts
    /// burned this teammate's budget?" be answered at all. It changes **no**
    /// ledger semantics — money still moves through the same `inference.spend`
    /// entry, and [`UNATTRIBUTED_AGENT`](crate::metering::UNATTRIBUTED_AGENT)
    /// still owns whole-company cycle usage.
    ///
    /// `None` for every chat turn, every workflow node, every OAuth/search call,
    /// and every sample written before this field existed — so a backend's
    /// stored rows need no migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Which model this sample's spend went to, folded onto the closed
    /// [`ModelSlug`] vocabulary (issue #1749).
    ///
    /// Without it, "which model is this company's spend going to?" — the first
    /// question either the Usage console or product analytics is asked about
    /// cost — cannot be answered from the meter at all, because `provider`
    /// names *who served* the tokens and never *what ran*. On the subscription
    /// proxy every sample says `subscription`, and on a BYOK tenant every
    /// sample says `byok`, whichever of four workloads produced it.
    ///
    /// A [`ModelSlug`] and **not** a `String`: a BYOK or `openai_compatible`
    /// deployment can name a model anything at all, including a customer's
    /// name, so the raw name is classified at the harness and never stored.
    /// See the [`model`](crate::metering::model) module docs for the
    /// vocabulary, the rule for extending it, and why the raw name is not kept
    /// alongside the slug.
    ///
    /// `None` for a sample with no model to name — every
    /// [`SampleKind::OauthCall`] and [`SampleKind::SearchCall`], a cognition
    /// path that cannot identify one, and **every sample written before this
    /// field existed**. `#[serde(default)]` is what makes that last case a
    /// non-event: the three backends persist a sample as its own JSON/BSON
    /// document, so a row from before this change deserializes with `model:
    /// None` and needs no migration — the same contract
    /// [`run_id`](Self::run_id) shipped under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSlug>,
}

/// Durable per-company usage samples. Company A's usage MUST be invisible to
/// company B.
#[async_trait]
pub trait UsageMeter: Send + Sync {
    /// Records a single usage sample.
    async fn record(&self, company: &CompanyId, sample: &UsageSample) -> Result<()>;
    /// Returns every sample at or after `since_millis`, oldest first.
    async fn query(&self, company: &CompanyId, since_millis: u64) -> Result<Vec<UsageSample>>;
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
