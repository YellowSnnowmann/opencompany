//! The managed search backend's response, as it comes off the wire.
//!
//! [`search`](crate::harness::search) posts to the platform's own
//! `/agent-integrations/parallel/search` route and reads the response rather
//! than a rendered string, because the charge (`costUsd`) is the one figure a
//! *metered* tool needs and rendering drops it. These two types are that
//! response.
//!
//! # Why they live here and not upstream
//!
//! They were `oh::search::tools::{SearchResponse, SearchResultItem}` until
//! OpenHuman v0.64.10 moved the search domain out to the `tinysearch` module:
//! the per-engine tools became one `TinySearchTool` whose results arrive as
//! `tinysearch_bus::SearchResult` after the module has already normalized them,
//! and the raw managed envelope stopped being a public type anywhere.
//!
//! The *wire form* did not change — `tinysearch`'s own managed provider still
//! reads `searchId` and `costUsd` off that route, and its normalizer still names
//! those fields. What changed is only who owns a Rust type for them. So this is
//! a re-declaration of an unchanged contract, not a fork of a moving one: the
//! route and the field names are the platform backend's, and the backend is not
//! what the version bump moved.
//!
//! Borrowing `tinysearch_bus::SearchResult` instead would not do: it is the
//! *normalized* shape, downstream of the module's own truncation and citation
//! handling, and `cost_usd` is not on it.

use serde::{Deserialize, Serialize};

/// One managed search call's answer.
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchResponse {
    /// The backend's id for the call. Carried so a support request can name one
    /// search; nothing on the belt reads it.
    #[serde(rename = "searchId")]
    #[allow(dead_code)]
    pub search_id: String,
    /// The results, best first.
    pub results: Vec<SearchResultItem>,
    /// What the call cost the platform, in USD. This is the sample the
    /// [`UsageMeter`](crate::ports::usage::UsageMeter) records.
    #[serde(rename = "costUsd")]
    pub cost_usd: f64,
    /// The upstream engine the managed backend resolved to (e.g. `"Exa"`).
    ///
    /// Optional: an older backend omits it, and the caller then falls back to
    /// the managed default. Aliased so a rename on the backend side keeps
    /// deserializing.
    #[serde(
        default,
        alias = "resolvedProvider",
        alias = "searchProvider",
        skip_serializing_if = "Option::is_none"
    )]
    pub provider: Option<String>,
}

/// One result in a [`SearchResponse`].
#[derive(Debug, Deserialize, Serialize)]
pub struct SearchResultItem {
    /// The page's URL. The only field the backend always sends.
    pub url: String,
    /// The page's title, empty when the backend could not read one.
    #[serde(default)]
    pub title: String,
    /// When the page says it was published, if it says so.
    #[serde(default)]
    pub publish_date: Option<String>,
    /// Extracts the backend judged relevant to the query.
    #[serde(default)]
    pub excerpts: Vec<String>,
}

#[cfg(test)]
#[path = "search_wire_tests.rs"]
mod tests;
