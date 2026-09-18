//! The TinyHumans proxy's paged model catalog:
//! `{"success":true,"data":{"object":"list","data":[…],"total":N,"limit":L,"offset":O}}`.
//! Pure; no I/O. Readers: [`super::probe::probe_models`],
//! [`crate::server::inference_models::discover_models`].
//!
//! Keys rework, issue #2306, slice 2a. Every id this parser reads is kept
//! exactly as given — nothing here hardcodes, filters, prefers or rejects a
//! model id by vendor or name; any id the endpoint returns is valid.
//!
//! [`catalogue_offer`] (slice 4a, moved here in the P3-7 layering-violation
//! review) is the one place "sort, dedupe, cap" a published id list is
//! decided, read by both [`crate::server::ops::inference::providers`] and the
//! account-key fan-out ([`crate::company::company_key::fan_out`]) — a
//! `company`-layer caller, which is why this pure helper lives here rather
//! than under `server`.

use std::collections::HashSet;

/// Page size requested. The backend clamps `limit` to `[1, 500]`.
pub const PAGE_LIMIT: usize = 500;
/// Most pages one read follows, so a `total` never reached cannot loop.
pub const MAX_PAGES: usize = 20;
/// Largest success body read for one page.
pub const PAGE_BODY_CAP: usize = 4 * 1024 * 1024;

/// The path (with query) for one page at `offset`.
pub fn page_path(offset: usize) -> String {
    format!("/models?limit={PAGE_LIMIT}&offset={offset}")
}

/// One model the proxy's catalog listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    /// The id exactly as the endpoint sent it. Any id it returns is valid.
    pub id: String,
    /// `display_name`, else `name`, when present.
    pub name: Option<String>,
    /// The context window, when the endpoint publishes one.
    pub context_length: Option<u64>,
}

/// One parsed page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPage {
    /// Entries this page carried, after dropping malformed ones.
    pub entries: Vec<CatalogEntry>,
    /// How many entries the page carried, usable or not. Paging advances by
    /// this, not by `entries.len()`, so a malformed entry still moves the
    /// offset forward instead of being re-requested forever.
    pub raw_len: usize,
    /// The envelope's `total`, when it parses as a non-negative integer.
    pub total: Option<usize>,
}

/// What to do after one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextPage {
    /// Request the next page at this offset.
    At(usize),
    /// Stop: an empty page, `total` reached, or no `total` at all.
    Done,
    /// Stop: [`MAX_PAGES`] was reached before `total` was.
    Truncated {
        /// How many entries were read before stopping.
        read: usize,
        /// The envelope's own `total`.
        total: usize,
    },
}

/// Parses one page. `Err` (never an empty page) on: not JSON; `success:
/// false`; no object `data`; no array `data.data` — so a plain
/// `{"data":[…]}` body (the OpenAI shape) is an error, not a page with no
/// entries. Every id is kept exactly as given. Unknown fields are ignored.
pub fn parse_page(body: &str) -> Result<CatalogPage, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("the model catalog was not JSON: {e}"))?;
    if value.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        let reason = value
            .get("error")
            .or_else(|| value.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("no reason given");
        return Err(format!("the model catalog reported a failure: {reason}"));
    }
    let Some(data) = value.get("data").filter(|d| d.is_object()) else {
        return Err("the model catalog was not in the `{success, data}` envelope".to_string());
    };
    let Some(raw) = data.get("data").and_then(serde_json::Value::as_array) else {
        return Err("the model catalog envelope carried no `data` list".to_string());
    };
    let text = |e: &serde_json::Value, k: &str| {
        e.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let entries = raw
        .iter()
        .filter_map(|e| {
            Some(CatalogEntry {
                id: text(e, "id")?,
                name: text(e, "display_name").or_else(|| text(e, "name")),
                context_length: e.get("context_length").and_then(serde_json::Value::as_u64),
            })
        })
        .collect();
    let total = data
        .get("total")
        .and_then(serde_json::Value::as_u64)
        .and_then(|t| usize::try_from(t).ok());
    Ok(CatalogPage {
        entries,
        raw_len: raw.len(),
        total,
    })
}

/// Pages collected, deduplicated by id, in listing order.
#[derive(Debug, Default)]
pub struct Collector {
    seen: HashSet<String>,
    entries: Vec<CatalogEntry>,
    offset: usize,
    pages: usize,
}

impl Collector {
    /// The offset the next page should request.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Folds one page in and says what to do next. Stops on: an empty page;
    /// reaching `total`; no `total` at all; [`MAX_PAGES`].
    pub fn push(&mut self, page: CatalogPage) -> NextPage {
        self.pages += 1;
        for entry in page.entries {
            if self.seen.insert(entry.id.clone()) {
                self.entries.push(entry);
            }
        }
        if page.raw_len == 0 {
            return NextPage::Done;
        }
        self.offset += page.raw_len;
        match page.total {
            Some(total) if self.offset < total && self.pages >= MAX_PAGES => NextPage::Truncated {
                read: self.offset,
                total,
            },
            Some(total) if self.offset < total => NextPage::At(self.offset),
            _ => NextPage::Done,
        }
    }

    /// The entries collected so far, consuming the collector.
    pub fn finish(self) -> Vec<CatalogEntry> {
        self.entries
    }
}

/// The published ids to offer, sorted and deduplicated.
///
/// Sorted because a catalog's own order is whatever the endpoint felt like,
/// and a select an operator has to scan is worth putting in one order.
///
/// **Never capped below what the paged read returned** (keys rework #2306,
/// round-3a review P2-5). A `500`-id truncation used to sit here, filtering
/// by name the moment a catalog's alphabetically-first 500 entries were not
/// its whole content — reachable ever since [`super::probe::probe_models`]
/// started following `total` across pages instead of reading one page and
/// calling it the catalog (KR-L1-01): OpenRouter's ~600 ids and TinyHumans'
/// paged envelope (up to [`MAX_PAGES`] × [`PAGE_LIMIT`] = 10,000) both read in
/// full, then had everything from roughly `x-ai/…` onward silently missing
/// from the add dialog's model step while `modelCount` still reported the
/// true total. The bound that actually applies is [`Collector`]'s own —
/// [`MAX_PAGES`] pages of at most [`PAGE_LIMIT`] ids each — which is a read
/// limit, not a display filter, and is never hit by a vendor catalog this
/// small.
///
/// Moved here from `server::ops::inference::providers` (keys rework #2306,
/// P3-7 review): the account-key fan-out (`company::company_key::fan_out`)
/// offers the same catalog shape on its own `needsModel` answer as this
/// module's other readers ([`super::probe::probe_models`];
/// `server::ops::inference::providers`'s probe route) already do — and
/// `company` must never import from `server`, so the one place "sort, dedupe"
/// a published id list is decided has to live somewhere both layers can
/// already reach. This module already is that place.
pub fn catalogue_offer(models: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = models.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[cfg(test)]
#[path = "paged_catalog_tests.rs"]
mod tests;
