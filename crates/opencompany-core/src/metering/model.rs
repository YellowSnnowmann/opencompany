//! [`ModelSlug`] — the closed vocabulary a metered sample names its model in.
//!
//! # Why this is not a `String`
//!
//! The model that reaches an inference endpoint is **operator-authored free
//! text**. A company on `openai_compatible` or `ollama` points at a server it
//! runs and calls the model whatever it likes, and `[inference].models` maps a
//! workload tier onto that name verbatim — the legacy arm's
//! [`configured_model_for_tier`](crate::company::inference::legacy_tiers::configured_model_for_tier)
//! deliberately honours an operator's entry without rewriting it (keys rework,
//! issue #2306, slice 2d). So the raw name can be a customer's name, an
//! internal project code, or anything else a person typed.
//!
//! Two consequences follow, and they are the whole reason this type exists
//! rather than a `model: String` field on [`UsageSample`]:
//!
//! * **Telemetry.** The product-analytics design (#1739, landing in #1751 as
//!   `docs/spec/runtime/analytics.md`) requires every textual property to be a
//!   `&'static str` written in this repository, precisely so a runtime string
//!   cannot become a payload. A `String` on the sample would be one
//!   `sample.model.clone()` away from an outbound body, and the metered event
//!   that PR adds reads exactly this sample.
//! * **Cardinality.** Samples are retained for
//!   [`RETENTION_DAYS`](crate::ports::usage::RETENTION_DAYS) — 90 days on every
//!   backend. A free-text column on every sample is unbounded-cardinality data
//!   an aggregation cannot group by and a store cannot index.
//!
//! This mirrors what `provider` already does
//! ([`provider_slug`](crate::company::inference::provider_slug), documented as
//! "the stable telemetry slug"): fold onto a fixed list, and send anything
//! unrecognised to a fallback. The dangerous direction is a value nobody
//! anticipated, so that is the direction it fails in.
//!
//! # The raw model name is deliberately **not** stored
//!
//! A design where the sample keeps the operator's own model name alongside the
//! slug was considered and rejected. It is defensible — it is the operator's
//! own data on the operator's own instance, and a Usage view showing them the
//! name they typed is a nicer view. It was rejected because the guarantee would
//! then be a *rule call sites obey* ("never put `raw_model` on an outbound
//! payload") rather than a *type that cannot hold runtime text*, and it would
//! have to be re-obeyed at every seam that leaves the box: the analytics
//! envelope, a hosted support bundle, an error report, a future export. One of
//! them eventually forgets.
//!
//! The operator's own model name is not lost by this: it is in
//! `[inference].models` and on the console's Inference card, which is where a
//! *current* configuration belongs. Copying it onto 90 days of accounting rows
//! is a different thing, and a worse one.
//!
//! # The vocabulary, and how it is extended
//!
//! A slug is `<vendor>` or `<vendor>-<line>`, plus this repo's own workload
//! tiers, plus [`ModelSlug::OTHER`]. A line is broken out only where the vendor
//! prices that line separately — which is exactly when an operator asks "what
//! is Sonnet costing us versus Haiku?" and a vendor-level answer cannot reply.
//!
//! **Add a slug when one of these becomes true, and not otherwise:**
//!
//! 1. a workload tier this repo defines starts classifying to `other` — the
//!    test `a_workload_tier_is_named_rather_than_other` fails when that
//!    happens, which is what stops `EXACT` rotting silently as the tier set
//!    moves;
//! 2. a vendor is common enough among BYOK tenants that `other` has stopped
//!    being a useful answer for them. That is a judgement call, and it is meant
//!    to be: the alternative — adding a slug for every model that exists — is
//!    not a closed vocabulary, it is a copy of the world's model catalogue that
//!    goes stale weekly.
//!
//! A model outside the list reports [`ModelSlug::OTHER`]. That is the honest
//! answer and it is a *bounded* one; growing the list to avoid ever seeing it
//! would defeat the point of the type.
//!
//! [`UsageSample`]: crate::ports::usage::UsageSample

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A model identity folded onto the closed vocabulary described in the
/// [module docs](self).
///
/// The inner value is a private `&'static str`, so a `ModelSlug` **cannot** be
/// constructed from runtime data except through [`ModelSlug::classify`], which
/// only ever returns a compiled-in literal. That is what makes "the raw model
/// name never leaves the harness" a property of the type rather than a
/// convention a call site has to keep remembering — and it is the same
/// construction argument the analytics payload vocabulary in #1751 makes, so
/// [`as_str`](ModelSlug::as_str) drops straight into a `&'static str` property
/// with no second classifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelSlug(&'static str);

impl ModelSlug {
    /// A model ran, but it is not one this build can name — a BYOK or
    /// self-hosted model, or a vendor the vocabulary has not been extended to.
    ///
    /// Deliberately not "unknown": nothing is unknown here. The endpoint was
    /// asked for a specific model and answered; what is missing is a *name for
    /// it that is safe to keep*, which is a different fact.
    pub const OTHER: Self = Self("other");

    /// The literal recorded on a sample and read by an aggregation.
    pub fn as_str(&self) -> &'static str {
        self.0
    }

    /// Folds a raw model string onto the vocabulary.
    ///
    /// Matching is case-insensitive and ignores an OpenRouter-style
    /// `author/` prefix, so `deepseek/deepseek-v4-pro`, `DeepSeek-V4-Pro` and
    /// `deepseek-v4-pro` all land on the same slug.
    ///
    /// A marker match is a **substring** test within the vendor's line, not an
    /// exact one, because vendors version their ids (`claude-sonnet-4-6`,
    /// `gpt-5.2-mini`) and pinning the exact id would send every point release
    /// to `OTHER`. The cost is that an operator who names a self-hosted model
    /// `our-sonnet-clone` is classified as `anthropic-sonnet`: a
    /// misclassification, not a leak, and the leak is the thing this type is
    /// defending against.
    pub fn classify(raw: &str) -> Self {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.is_empty() {
            return Self::OTHER;
        }

        // This repo's own workload tiers. On the subscription-proxied path the
        // tier *is* what goes on the wire — the platform's registry resolves it
        // upstream — so the tier is the most specific true answer available
        // here, and substituting the model it resolves to by default would be
        // a guess printed as a fact.
        //
        // Matched on the whole string, before the `author/` split, because a
        // tier has no author. The sentinels round-trip to themselves so that
        // `Deserialize` (which re-classifies) is idempotent.
        for slug in EXACT {
            if lower == *slug {
                return Self(slug);
            }
        }

        // OpenRouter and most catalogues namespace as `author/slug`. Keep both
        // halves: the author is the strongest vendor signal, and the slug is
        // where the line marker lives.
        let (author, line) = match lower.split_once('/') {
            Some((author, line)) => (author, line),
            None => ("", lower.as_str()),
        };

        for vendor in VENDORS {
            let matched = vendor
                .authors
                .iter()
                .any(|candidate| author == *candidate || line.starts_with(candidate));
            if !matched {
                continue;
            }
            for (marker, slug) in vendor.lines {
                if line.contains(marker) {
                    return Self(slug);
                }
            }
            return Self(vendor.slug);
        }

        Self::OTHER
    }
}

/// Slugs matched on the whole model string, before any `author/` split.
///
/// The four workload tiers this repo defines (`crate::company::types::INFERENCE_TIERS`),
/// plus [`ModelSlug::OTHER`]'s own literal so that re-classifying an
/// already-folded value is a fixed point.
const EXACT: &[&str] = &[
    "chat-v1",
    "reasoning-v1",
    "agentic-v1",
    "vision-v1",
    "other",
];

/// One vendor's entry in the vocabulary.
struct Vendor {
    /// The slug reported when the vendor matches but no line marker does.
    slug: &'static str,
    /// Author namespaces and id prefixes that identify this vendor.
    authors: &'static [&'static str],
    /// `(marker, slug)` pairs, tried in order, matched as a substring of the
    /// model id. Present only where the vendor prices its lines separately.
    lines: &'static [(&'static str, &'static str)],
}

/// The vendor table. See the [module docs](self) for the rule that governs
/// what may be added to it.
const VENDORS: &[Vendor] = &[
    // DeepSeek's flash and pro lines are priced separately, so retain the line
    // split for operators who select either from the OpenRouter catalog.
    Vendor {
        slug: "deepseek",
        authors: &["deepseek"],
        lines: &[
            ("v4-flash", "deepseek-v4-flash"),
            ("v4-pro", "deepseek-v4-pro"),
        ],
    },
    // `qwen3.8-max` is a real upstream id an operator can point a `vision-v1`
    // mapping at. The dot is not carried into the slug — a telemetry value is
    // read by people and grouped by machines, and a point release must not
    // mint a new one.
    Vendor {
        slug: "qwen",
        authors: &["qwen"],
        lines: &[
            ("3.8-max", "qwen3-max"),
            ("3-max", "qwen3-max"),
            ("3.7-plus", "qwen3-plus"),
            ("3-plus", "qwen3-plus"),
        ],
    },
    // The shipped chat and agentic defaults are Anthropic Sonnet and Opus.
    // Anthropic prices opus, sonnet and haiku separately and by an order of
    // magnitude; "what is Sonnet costing us versus Haiku?" is the question
    // issue #1749 is named after, and a vendor-level slug cannot answer it.
    Vendor {
        slug: "anthropic",
        authors: &["anthropic", "claude"],
        lines: &[
            ("opus", "anthropic-opus"),
            ("sonnet", "anthropic-sonnet"),
            ("haiku", "anthropic-haiku"),
        ],
    },
    // The shipped reasoning default is an OpenAI GPT model. OpenAI's reasoning
    // (`o`-series) and chat (`gpt`) families are priced on different rate cards,
    // which is the same split as above.
    Vendor {
        slug: "openai",
        authors: &["openai", "gpt", "o1", "o3", "o4"],
        lines: &[("gpt", "openai-gpt")],
    },
    // Google prices Gemini Pro and Gemini Flash separately.
    Vendor {
        slug: "google",
        authors: &["google", "gemini"],
        lines: &[
            ("pro", "google-gemini-pro"),
            ("flash", "google-gemini-flash"),
        ],
    },
    // Open-weight vendors a self-hosted `ollama` or `openai_compatible` tenant
    // most often runs. One slug each: nobody is billed per line for a model
    // they host themselves, so there is nothing for a line split to answer.
    Vendor {
        slug: "meta-llama",
        authors: &["meta-llama", "llama"],
        lines: &[],
    },
    Vendor {
        slug: "mistral",
        authors: &["mistralai", "mistral"],
        lines: &[],
    },
];

impl fmt::Display for ModelSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for ModelSlug {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for ModelSlug {
    /// Re-classifies on the way in.
    ///
    /// A stored row is not trusted to already hold a vocabulary member: a
    /// hand-edited document, a row written by an older or a forked build, or a
    /// shared-single-DB tenant's collection can all present arbitrary text
    /// here. Re-folding it means a raw model name **cannot** survive a
    /// round-trip through a store and reach a reader — the classifier is on
    /// both boundaries, not just the write one.
    ///
    /// Serialising a vocabulary member and reading it back is a fixed point:
    /// every slug the classifier can emit classifies to itself.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::classify(&raw))
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
