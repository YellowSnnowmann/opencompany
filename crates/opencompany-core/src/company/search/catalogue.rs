//! The four search providers a company can connect, as data.
//!
//! # Where these facts come from
//!
//! The endpoint and the auth header are read off **the vendored OpenHuman tool
//! that makes the call**, not off the provider's published documentation,
//! because the vendored tool is what runs. Where the two could disagree — Exa
//! accepts `Authorization: Bearer` as well as `x-api-key`, and
//! [`crate::harness::built_in::search_byo`] sends the latter — this table
//! records what is actually sent.
//!
//! The failure shapes below come from the providers' own docs, and one of them
//! is the reason [`super::probe`] classifies per provider instead of matching
//! one regex over an error string: **Brave rejects a bad key with `422`**, and
//! its API reference documents no `401` and no `403` at all. A classifier that
//! maps "credential problem" to 401/403 would leave a dead Brave key stored and
//! would read Brave's only possible `403` — a WAF — as a rejected key.
//!
//! There is deliberately **no key-prefix field**. None of the three account
//! providers documents one, and a validation rule invented from a blog post
//! rejects a valid key for a reason the operator cannot see.
//!
//! `managed` is not in this table. It has no endpoint the company owns, no key
//! the company pastes and no row to add — it is rendered from resolution. See
//! `docs/modules/search/data-model.md`.

/// Which of the two questions a provider answers.
///
/// The split is the add dialog's, and it is the same argument the inference
/// dialog makes for its three: an account wants a key, a self-hosted instance
/// wants an address. One flat list would make the operator infer that from a
/// group heading; a select per category has a label and a line of helper text to
/// say it outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// A hosted search API behind an account. The operator supplies a key.
    Account,
    /// The operator's own instance. They supply the address; there is no key.
    SelfHosted,
}

impl Category {
    /// The wire spelling, shared with the console mirror.
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Account => "account",
            Category::SelfHosted => "self-hosted",
        }
    }
}

/// How a provider expects its credential presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStyle {
    /// A bare key in the named header — Brave's `X-Subscription-Token`, Exa's
    /// `x-api-key`.
    Header(&'static str),
    /// `Authorization: Bearer <key>`.
    Bearer,
}

/// One catalogue entry.
#[derive(Debug, Clone, Copy)]
pub struct SearchProviderInfo {
    /// The slug. Identity **and** address: the harness dispatches on it
    /// (`match config.provider.as_str()` in `search_byo`), so it is fixed here
    /// rather than chosen by an operator.
    pub slug: &'static str,
    /// Display name.
    pub label: &'static str,
    /// Which of the two questions this provider answers.
    pub category: Category,
    /// The API host, for the add dialog's detail line and for the probe.
    ///
    /// Not configurable for an account provider: Brave's base URL is a `const`
    /// in the vendored tool and its constructor takes no URL at all, while Exa's
    /// and Querit's constructors accept one that `search_byo` passes as `None`.
    /// A base-URL field on those rows would be a field the request ignores.
    pub endpoint: &'static str,
    /// How the credential is presented, or `None` where there is no credential.
    pub auth: Option<AuthStyle>,
    /// Where an operator gets a key, or `None` where no authoritative URL is
    /// documented.
    pub key_source: Option<&'static str>,
}

impl SearchProviderInfo {
    /// Whether this provider authenticates with a key the company pastes.
    pub fn needs_key(&self) -> bool {
        self.auth.is_some()
    }

    /// Whether this provider is addressed by an operator-supplied URL.
    pub fn needs_endpoint(&self) -> bool {
        matches!(self.category, Category::SelfHosted)
    }
}

/// Every provider that can appear as a row.
///
/// Mirrored in `frontend/src/search-providers/catalogue.ts`, with a test
/// asserting the two agree — the inference catalogue's fourth known defect is a
/// hand-duplicated table with nothing noticing when the copies drift.
pub const CATALOGUE: [SearchProviderInfo; 4] = [
    SearchProviderInfo {
        slug: "brave",
        label: "Brave Search",
        category: Category::Account,
        endpoint: "https://api.search.brave.com/res/v1",
        auth: Some(AuthStyle::Header("X-Subscription-Token")),
        key_source: Some("https://api-dashboard.search.brave.com/app/keys"),
    },
    SearchProviderInfo {
        slug: "exa",
        label: "Exa",
        category: Category::Account,
        endpoint: "https://api.exa.ai",
        auth: Some(AuthStyle::Header("x-api-key")),
        key_source: Some("https://dashboard.exa.ai/api-keys"),
    },
    SearchProviderInfo {
        slug: "querit",
        label: "Querit",
        category: Category::Account,
        endpoint: "https://api.querit.ai/v1",
        auth: Some(AuthStyle::Bearer),
        // Querit's docs say to copy the key "from the Dashboard" and never print
        // a URL for it. Linking a guess is worse than linking nothing.
        key_source: Some("https://www.querit.ai/"),
    },
    SearchProviderInfo {
        slug: "searxng",
        label: "SearXNG",
        category: Category::SelfHosted,
        // Replaced by the company's own instance URL. Present so the add
        // dialog's detail line has something to say about what SearXNG is.
        endpoint: "",
        auth: None,
        key_source: None,
    },
];

/// The catalogue entry for `slug`, if this build knows it.
pub fn entry(slug: &str) -> Option<&'static SearchProviderInfo> {
    CATALOGUE.iter().find(|info| info.slug == slug)
}

#[cfg(test)]
#[path = "catalogue_tests.rs"]
mod tests;
