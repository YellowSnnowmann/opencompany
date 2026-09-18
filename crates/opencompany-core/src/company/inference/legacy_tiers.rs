//! Legacy tier lookups — decision F3 (keys rework, issue #2306, slice 2d).
//! **A tier name is never a model.**
//!
//! All that survives of tiers on the request path is one lookup: an agent's
//! tier hint selects an entry in a map an operator configured (entry zero's
//! `inference/config.models`, a manifest `[inference].models`, or a route's
//! pinned model until slice 5b), and only a real id found there is ever sent.
//! Everything that used to *guess* a substitute — the shipped
//! `DEFAULT_TIER_MODELS` table, the discovered `TierVocabulary`, and
//! `model_for_tier`'s fallback to sending the bare tier — is deleted along
//! with the fallback chains that read them
//! ([`super::model_on_the_wire`](crate::company::inference::model_on_the_wire)
//! is the one place this module is read from now).
//!
//! Kept here, quarantined, rather than deleted outright: the legacy arm
//! (a company with no agent pair and no full default) still has real
//! per-tier maps to read, and this is the one lookup that stays honest about
//! them — a stored map is honoured verbatim, and a tier name stored as a
//! "model" is recognised as not one, never re-guessed into something else.

use std::collections::BTreeMap;

use crate::company::types::INFERENCE_TIERS;

/// Whether `id`, trimmed, is one of [`INFERENCE_TIERS`].
pub fn is_tier_name(id: &str) -> bool {
    INFERENCE_TIERS.contains(&id.trim())
}

/// The operator's real model id for `tier`, or `None` when the map has no
/// entry, a blank one, or one that is itself a tier name.
///
/// **Never returns a tier name.** A stored map can itself hold `{"chat-v1":
/// "chat-v1"}` — nothing on the write path forbade that before 2c's
/// `check_model_id`, and an old record can still carry it — and this is the
/// one place that refuses to pass it through as if it were a real id.
pub fn configured_model_for_tier(tier: &str, models: &BTreeMap<String, String>) -> Option<String> {
    models
        .get(tier.trim())
        .map(|m| m.trim())
        .filter(|m| !m.is_empty() && !is_tier_name(m))
        .map(str::to_string)
}

#[cfg(test)]
#[path = "legacy_tiers_tests.rs"]
mod tests;
