//! The analytics [`Event`] enum: the closed set of things this crate reports.
//!
//! Split out of `types.rs` (which was at the 750-line cap) along the one seam
//! that file had: everything above it is the vocabulary an event is built from,
//! and this is the event itself. Re-exported from [`crate::analytics::types`]
//! and [`crate::analytics`], so every existing path still resolves.

use super::{FailureCode, Outcome, Prop, PropValue, Trigger, provider_slug, sample_kind_slug};
use crate::ports::usage::UsageSample;

/// One analytics event.
///
/// Every field is a number, a bool, or a value from a closed vocabulary. There
/// is no variant carrying a `String`, and adding one would defeat the whole
/// module — see [`PropValue`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// The host finished booting and is serving.
    InstanceStarted {
        /// How many companies this host serves.
        companies: u64,
        /// The storage backend kind (`fs`, `sqlite`, `mongodb`).
        storage: &'static str,
        /// Whether first-run setup has been completed.
        setup_complete: bool,
    },
    /// One cycle finished — the product's unit of work.
    TurnFinished {
        /// What started it.
        trigger: Trigger,
        /// Whether it returned a report or an error.
        outcome: Outcome,
        /// The coarse failure class, `None` on success.
        failure: Option<FailureCode>,
        /// Wall-clock milliseconds from bracket open to bracket close. The only
        /// duration the runtime measures for a cycle; nothing else times one.
        duration_ms: u64,
        /// How many effects the cycle executed.
        effects_executed: u64,
        /// How many effects parked for an operator decision.
        approvals_parked: u64,
    },
    /// One usage sample was metered — tokens or a counted call.
    TurnMetered {
        /// What produced the sample.
        kind: &'static str,
        /// The provider, folded onto the closed vocabulary.
        provider: &'static str,
        /// The model, as the closed [`ModelSlug`](crate::metering::ModelSlug)
        /// vocabulary already folded it (issue #1749).
        ///
        /// A `&'static str` and not a `String` because a `ModelSlug`'s inner
        /// value is a compiled-in literal and `as_str` hands one back — so the
        /// raw model name a BYOK tenant configured cannot reach a payload even
        /// in principle, and this module needs no classifier of its own.
        ///
        /// `None` when the sample named no model: an OAuth or search call, a
        /// cognition path that cannot identify one, or a row written before
        /// [`UsageSample::model`] existed. Absent rather than folded onto
        /// [`OTHER`], so "no model ran" stays a different answer from "a model
        /// ran that this build cannot name" — the property is **omitted** from
        /// the payload rather than sent as `other` or as a null.
        model: Option<&'static str>,
        /// Prompt tokens.
        input_tokens: u64,
        /// Completion tokens.
        output_tokens: u64,
        /// Prompt tokens served from cache.
        cached_input_tokens: u64,
        /// USD attributed to the sample.
        cost_usd: f64,
        /// Whether the sample belongs to a task attempt. The attempt *id* is
        /// deliberately absent: it is a correlation key into this company's own
        /// data and buys no segmentation here.
        attributed_to_run: bool,
    },
}

impl Event {
    /// Builds a [`Self::TurnMetered`] from a sample, folding every free-form
    /// field away. This is the only constructor, so no call site can choose to
    /// pass the agent name or the raw provider through.
    pub fn metered(sample: &UsageSample) -> Self {
        Self::TurnMetered {
            kind: sample_kind_slug(sample.kind),
            provider: provider_slug(&sample.provider),
            // No `provider_slug`-style fold here on purpose: `ModelSlug` is
            // itself the closed vocabulary, classified once at the harness, and
            // `as_str` is already a `&'static str`. Re-classifying a folded
            // value would be a second place for the two lists to disagree.
            model: sample.model.map(|slug| slug.as_str()),
            input_tokens: sample.input_tokens,
            output_tokens: sample.output_tokens,
            cached_input_tokens: sample.cached_input_tokens,
            cost_usd: sample.cost_usd,
            attributed_to_run: sample.run_id.is_some(),
        }
    }

    /// The event's stable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::InstanceStarted { .. } => "instance_started",
            Self::TurnFinished { .. } => "turn_finished",
            Self::TurnMetered { .. } => "turn_metered",
        }
    }

    /// The event's own properties, excluding the envelope's.
    pub fn props(&self) -> Vec<Prop> {
        match *self {
            Self::InstanceStarted {
                companies,
                storage,
                setup_complete,
            } => vec![
                ("companies", PropValue::Count(companies)),
                ("storage", PropValue::Word(storage)),
                ("setup_complete", PropValue::Flag(setup_complete)),
            ],
            Self::TurnFinished {
                trigger,
                outcome,
                failure,
                duration_ms,
                effects_executed,
                approvals_parked,
            } => vec![
                ("trigger", PropValue::Word(trigger.as_str())),
                ("outcome", PropValue::Word(outcome.as_str())),
                (
                    "failure",
                    PropValue::Word(failure.map_or("none", FailureCode::as_str)),
                ),
                ("duration_ms", PropValue::Count(duration_ms)),
                ("effects_executed", PropValue::Count(effects_executed)),
                ("approvals_parked", PropValue::Count(approvals_parked)),
            ],
            Self::TurnMetered {
                kind,
                provider,
                model,
                input_tokens,
                output_tokens,
                cached_input_tokens,
                cost_usd,
                attributed_to_run,
            } => {
                let mut props = vec![
                    ("sample_kind", PropValue::Word(kind)),
                    ("provider", PropValue::Word(provider)),
                    ("input_tokens", PropValue::Count(input_tokens)),
                    ("output_tokens", PropValue::Count(output_tokens)),
                    ("cached_input_tokens", PropValue::Count(cached_input_tokens)),
                    ("cost_usd", PropValue::Amount(cost_usd)),
                    ("attributed_to_run", PropValue::Flag(attributed_to_run)),
                ];
                // Pushed only when the sample named a model. A `map_or("none",
                // …)` here would spend a vocabulary slot on the same fact the
                // property's absence already states, and an operator segmenting
                // spend by model would have to know that `none` and `other` are
                // different kinds of nothing.
                if let Some(model) = model {
                    props.push(("model", PropValue::Word(model)));
                }
                props
            }
        }
    }
}
