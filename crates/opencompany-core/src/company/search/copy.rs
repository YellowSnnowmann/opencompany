//! Shared search-surface sentences (keys rework, issue #2306).
//!
//! Mirrors the LLM surface's `src/company/inference/copy.rs` in spirit
//! (D-copy / X9: one shared sentence table, defined once, asserted by exact
//! text in tests, rather than formatted ad hoc per call site) — but is its
//! own small module rather than a shared one, because Agent B's `copy.rs` is
//! out of this dispatch's scope (`docs/key-reworks/README.md`'s per-agent
//! file ownership) and the two surfaces' sentences differ in a load-bearing
//! way: see [`default_provider_unavailable`].

/// The two ways a search default's provider can stop being able to answer.
///
/// Mirrors `crate::company::inference::copy::ProviderGone` in shape (not
/// reused — that type lives in Agent B's module) because the two surfaces
/// describe the same idea with the same two words, and a reader moving
/// between them should not have to learn a second vocabulary for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderGone {
    /// The provider's row no longer exists (a confirmed delete).
    Removed,
    /// The provider's row exists but is switched off (a confirmed disable).
    TurnedOff,
}

/// "The search default uses {Provider}, which is {removed|turned off}.
/// Choose a new search default in Connections → API Keys → Search."
///
/// The nav path is verified against `frontend/src/views/connection-pages.ts`:
/// the `search` page (label "Search") is filed under the `keys` group
/// (label "API Keys").
///
/// # Why this is a status banner, not a turn-failure error (unlike copy.rs's
/// `default_broken` on the LLM surface)
///
/// A search turn never fails closed. `crate::company::search::resolve::active`
/// already falls through a marked-but-gone provider to the first usable one,
/// or to [`super::MANAGED_PROVIDER`] when nothing is usable — managed search
/// is a universal fallback with no credential of its own to be missing, so
/// there is no "no model is chosen"-shaped turn failure to hook a sentence
/// into the way the LLM surface has one. What decision D-never-clear-default
/// (X14, 2026-09-15, `docs/key-reworks/README.md`) actually changes for
/// search is that this fallback, which already happened silently before this
/// rework (nothing told an operator their SearXNG box got skipped because
/// they flipped a switch — the bill just quietly moved to the platform's
/// managed account), now persists indefinitely instead of self-correcting the
/// next time the marker was written. This sentence is the fix for that
/// silence: surfaced on `GET …/search` as `SearchStatus.defaultNotice`, for
/// the console to render as a banner rather than saying nothing at all.
pub fn default_provider_unavailable(provider_label: &str, why: ProviderGone) -> String {
    let state = match why {
        ProviderGone::Removed => "removed",
        ProviderGone::TurnedOff => "turned off",
    };
    format!(
        "The search default uses {provider_label}, which is {state}. Choose a \
         new search default in Connections \u{2192} API Keys \u{2192} Search."
    )
}

/// §2's fixed sentence for "default alone": `"<Label> is the search
/// default."` (`docs/key-reworks/in-use-guards.md` §2's table, adapted: search
/// has no agents or surfaces of its own — Agent A's scope note in that file's
/// §1 — so this is the *only* shape the search in-use guard ever produces,
/// unlike the LLM/Composio sentence tables which also cover an agents or
/// surfaces branch).
pub fn provider_in_use_message(provider_label: &str) -> String {
    format!("{provider_label} is the search default.")
}

#[cfg(test)]
#[path = "copy_tests.rs"]
mod tests;
