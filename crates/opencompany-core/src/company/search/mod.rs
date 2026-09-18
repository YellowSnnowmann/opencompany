//! Secret-store keys and the provider vocabulary for a company's **own** web
//! search connection (the follow-up issue #238 deferred).
//!
//! Managed search — `web_search` over the platform's backend, metered and
//! daily-capped — uses the company's copied TinyHumans key before the instance
//! identity and is the effective default. A company can instead bring its own
//! search-provider account: an operator opens Settings → Search, picks a
//! provider, pastes the key, and every agent that already holds the `search`
//! grant gets that provider's tools on its next turn.
//!
//! The keys live here, always compiled, rather than beside the harness wiring in
//! [`crate::harness::search_byo`]: that module is gated on the `openhuman`
//! feature but the **configuration surface** is not, so an operator on a build
//! with no agent harness still sees "this build has no search tools" rather than
//! a 404. It is the same split — and the same argument — as
//! [`crate::company::hosting`].
//!
//! # Why credentials are company-scoped before the environment fallback
//!
//! A BYO provider key is billed directly by that provider. Managed search may
//! instead use [`MANAGED_KEY_SECRET`], a copy of the company's own TinyHumans
//! account key, before falling through to the instance identity. That does not
//! reintroduce an ambient credential paid for by somebody else: the company key
//! belongs to, and is billed to, the same company whose agents present it.

pub mod catalogue;
pub mod copy;
pub mod probe;
pub mod resolve;
pub mod store;

/// Holds the company's chosen search provider slug — one of
/// [`SUPPORTED_PROVIDERS`].
///
/// # Entry zero
///
/// This is now the **legacy** address, and it is deliberately still read. A
/// company that configured search before the provider list existed has its
/// selection here and its key at [`API_KEY_SECRET`], and
/// [`store::list_providers`] synthesises that pair into the first row rather
/// than migrating it — the [`SecretStore`](crate::ports::SecretStore) port has
/// no rename and no delete, so a flag-day migration on a store with no
/// transaction can leave a company with neither configuration. The first save
/// through the new path moves it. See `docs/modules/search/data-model.md`.
///
/// Stored rather than inferred from which key happens to be present: the slug is
/// what decides which API the key is presented to, and a key sent to the wrong
/// provider fails in a way that reads like a bad key.
pub const PROVIDER_SECRET: &str = "search/provider";

/// Deprecated flat entry-zero search key. New writes converge onto
/// `search/provider/{slug}/key`; this fallback remains readable until a
/// dedicated migration can prove every company has converged, so an untouched
/// pre-provider-list company does not silently lose search.
pub const API_KEY_SECRET: &str = "search/api_key";

/// The company's managed-search TinyHumans credential.
///
/// Filled by the account-key fan-out and intentionally not indexed as a search
/// provider: Managed is the fallback row, has no company-owned endpoint, and
/// always proxies through the TinyHumans backend.
pub const MANAGED_KEY_SECRET: &str = "search/managed/key";

/// Reads the company-scoped managed-search key, treating blank as unset.
pub async fn load_managed_key(
    company: &crate::ports::types::CompanyId,
    secrets: &dyn crate::ports::SecretStore,
) -> crate::Result<Option<String>> {
    Ok(secrets
        .get(company, MANAGED_KEY_SECRET)
        .await?
        .map(|value| value.0.trim().to_string())
        .filter(|value| !value.is_empty()))
}

/// Holds the provider's base URL, for the one provider that is an address
/// rather than an account: a self-hosted SearXNG instance. Not a secret — a
/// settings form has to show which instance it queries.
pub const ENDPOINT_SECRET: &str = "search/endpoint";

/// The provider used when a company has configured nothing: the platform's own
/// metered, daily-capped managed search.
pub const MANAGED_PROVIDER: &str = "managed";

/// Every provider slug the console accepts.
///
/// Kept here rather than derived from the harness so a settings form can render
/// the picker in a build with no search tools compiled in at all — the same
/// reason [`crate::server::ops::hosting::SUPPORTED_PROVIDERS`] lives beside its
/// keys.
pub const SUPPORTED_PROVIDERS: [&str; 5] = ["managed", "brave", "exa", "querit", "searxng"];

/// Whether `slug` names a provider this build knows how to wire.
pub fn provider_supported(slug: &str) -> bool {
    SUPPORTED_PROVIDERS.contains(&slug)
}

/// Whether `slug` is a BYO provider — one that needs the company's own
/// credentials, as opposed to the managed platform surface.
pub fn provider_is_byo(slug: &str) -> bool {
    provider_supported(slug) && slug != MANAGED_PROVIDER
}

/// Whether `slug` authenticates with an API key.
///
/// SearXNG is the exception: it is a self-hosted instance addressed by URL, with
/// no account behind it, so a key is neither required nor used.
pub fn provider_requires_key(slug: &str) -> bool {
    provider_is_byo(slug) && slug != "searxng"
}

/// Whether `slug` needs a base URL rather than a key.
pub fn provider_requires_endpoint(slug: &str) -> bool {
    slug == "searxng"
}

/// Whether the pair (`provider`, what is stored) is complete enough to wire
/// tools for. An incomplete BYO configuration is not an error — it falls back to
/// the managed surface, exactly as OpenHuman's registry does.
pub fn configuration_complete(provider: &str, has_key: bool, has_endpoint: bool) -> bool {
    if !provider_is_byo(provider) {
        return false;
    }
    if provider_requires_key(provider) && !has_key {
        return false;
    }
    if provider_requires_endpoint(provider) && !has_endpoint {
        return false;
    }
    true
}

/// The provider that actually answers, given what is stored: the selection when
/// it is complete, and [`MANAGED_PROVIDER`] otherwise.
///
/// The **one** derivation of that answer. The console's Search page and the
/// capabilities panel both report it, and two surfaces that merely mirrored each
/// other's rule would drift the first time a provider was added — leaving one
/// page saying a company searches through Exa while its agents search through
/// the platform.
pub fn effective_provider(provider: &str, has_key: bool, has_endpoint: bool) -> &str {
    if configuration_complete(provider, has_key, has_endpoint) {
        provider
    } else {
        MANAGED_PROVIDER
    }
}

/// Every connected provider, paired with whether its credential is present.
///
/// The credential itself never leaves the store here — only the boolean, asked
/// of the store per provider rather than read from a flag that could go stale
/// against a cleared secret.
///
/// # Errors
///
/// Returns an error when the secret store cannot be read.
pub async fn candidates(
    company: &crate::ports::types::CompanyId,
    secrets: &dyn crate::ports::SecretStore,
) -> crate::Result<Vec<resolve::Candidate>> {
    let mut candidates = Vec::new();
    for provider in store::list_providers(company, secrets).await? {
        let has_key = store::provider_key_configured(company, secrets, &provider.slug).await?;
        candidates.push(resolve::Candidate { provider, has_key });
    }
    Ok(candidates)
}

/// The provider a company's agents actually search through, over its store.
///
/// **The one derivation**, unchanged in name and contract from the single-slot
/// version it replaces — the console's Search page, the capabilities panel and
/// [`crate::harness::built_in::search_byo`] all call this rather than restating
/// the rule. What changed is only what it reads: a list of connected providers
/// and a default marker, instead of three flat keys.
///
/// That "only" hides the one hazard in the rework. Every reader must move
/// together with the store's convergence write: a console that writes a
/// credential to `search/provider/<slug>/key` while some reader is still on
/// `search/api_key` sees an unconfigured company and silently drops it to
/// managed search. The agents keep searching; they just quietly stop using the
/// account the operator pays for.
///
/// # Errors
///
/// Returns an error when the secret store cannot be read.
pub async fn resolve_effective_provider(
    company: &crate::ports::types::CompanyId,
    secrets: &dyn crate::ports::SecretStore,
) -> crate::Result<String> {
    let candidates = candidates(company, secrets).await?;
    let marked = store::load_default_slug(company, secrets).await?;
    Ok(resolve::effective_slug(resolve::active(&candidates, marked.as_deref())).to_string())
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
