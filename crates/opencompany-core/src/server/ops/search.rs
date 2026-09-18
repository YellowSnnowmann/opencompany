//! The search configuration write-plane: which providers a company has
//! connected, which one its agents search through, and the credential behind
//! each — **write-only**.
//!
//! `GET …/search` returns only [`SearchStatus`]: slugs, booleans, and the
//! non-secret endpoints. No API key is ever serialized into any response, by
//! construction. Keys live in [`SecretStore`](crate::ports::SecretStore) and
//! this module reads them back only to report *whether* they are there.
//!
//! # Why this is a list and not a field
//!
//! There used to be one `search/api_key` for the whole company and a separate
//! `search/provider` field selecting which API it was presented to. Switching
//! provider without re-pasting the key left the old key authenticating against
//! the new provider — and this route, the console badge and the harness all
//! agreed the company was correctly configured until an agent's first search
//! came back 401. One credential per provider slug makes that unrepresentable.
//! See [`crate::company::search::store`] and
//! `docs/modules/search/current-state.md`.
//!
//! # Why "not configured" is a working state and not an error
//!
//! Leaving this page alone is a legitimate choice: a company with no provider
//! configured searches through the platform's managed surface, which is metered
//! and daily-capped and needs no credential from the company at all. Connecting
//! a provider here moves those calls onto the company's own account — a change
//! of who is billed, and of which index answers, not a change of whether the
//! agents can search. [`SearchStatus::effective_provider`] is the field that
//! says which of the two is live, because "I connected Exa but pasted no key"
//! and "I connected Exa" must not read identically.
//!
//! # Three things can each be missing, and they fail differently
//!
//! A provider connected with no key; a `search` grant the manifest never made; a
//! build with no agent harness compiled in. The remedies are on three different
//! pages, so the status reports them separately rather than as one "connected"
//! flag.
//!
//! # Authority
//!
//! Reads are [`ScopedCompany`]; **every write and the probe are
//! [`AdminScopedCompany`]**. A search key is billed to whoever's account it
//! belongs to, and the provider choice decides which index — and which retention
//! policy — every agent's queries are handed to. The probe is an admin action
//! for two further reasons: it spends the company's money (no search provider
//! offers a free credential validator) and, for a self-hosted instance, it
//! fetches an operator-supplied address that is allowed to be on a private
//! network.

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::company::runtime::CompanyRuntime;
use crate::company::search::catalogue::{self, SearchProviderInfo};
use crate::company::search::copy::{self, ProviderGone};
use crate::company::search::probe::{self, ProbeClass};
use crate::company::search::resolve::Candidate;
use crate::company::search::store::{self, SearchProvider};
use crate::company::search::{
    API_KEY_SECRET, ENDPOINT_SECRET, MANAGED_PROVIDER, PROVIDER_SECRET, SUPPORTED_PROVIDERS,
    provider_requires_endpoint, provider_requires_key, resolve,
};
use crate::error::{OpenCompanyError, UsedBy};
use crate::ports::types::SecretValue;
use crate::server::error::ApiError;
use crate::server::ops::scope::{AdminScopedCompany, ScopedCompany, scoped};

/// One connected provider, as the console sees it.
///
/// Carries `key_configured` and **never** a key. The boolean is derived by
/// asking the store whether a non-empty value exists, never by reading a flag
/// that could go stale against a cleared secret.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchProviderView {
    /// The catalogue slug. Identity and address at once.
    pub slug: String,
    /// Display name, from the catalogue.
    pub label: String,
    /// `account` or `self-hosted` — which of the two questions it answers.
    pub category: String,
    /// Whether it is eligible to be the provider agents search through.
    pub enabled: bool,
    /// Whether a credential is stored. **Never the credential.**
    pub key_configured: bool,
    /// Whether this kind of provider takes a key at all. `false` for SearXNG,
    /// which is how the console knows not to offer "Remove key" on a row that
    /// has none.
    pub takes_key: bool,
    /// Whether this kind of provider takes an instance address.
    pub takes_endpoint: bool,
    /// The instance address, for a self-hosted provider. Not a secret.
    pub endpoint: Option<String>,
    /// Whether this provider has everything it needs to answer a search.
    pub complete: bool,
    /// Whether this is the provider agents search through.
    ///
    /// The **resolved** answer rather than the raw marker, so a row can say
    /// "Default" without the console knowing whether it was chosen or inherited
    /// — the operator sees the same answer either way.
    pub is_default: bool,
    /// Who still depends on this row (keys rework, issue #2306;
    /// `docs/key-reworks/in-use-guards.md` §1/§6): `default: true` iff the
    /// **bare stored** `search/default` marker names this slug — not the
    /// resolved `is_default` above, which can differ from the marker when
    /// the marked provider is disabled or incomplete (D-never-clear-default /
    /// X14: the marker is never silently moved off a provider the operator
    /// chose). Search has no agent pairs and is never itself a `surfaces`
    /// target for another guard, so `agents` and `surfaces` are always empty
    /// here — the only shape this ever carries is `{ "default": true }` or
    /// omitted entirely. Omitted (not `null`, not `{}`) when nothing depends
    /// on this row, so `"usedBy" in row` is itself the in-use check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_by: Option<UsedBy>,
}

/// The non-secret view of a company's search configuration.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchStatus {
    /// The provider the company **selected**, kept for compatibility with the
    /// single-slot shape: the active provider's slug, or `managed`.
    pub provider: String,
    /// The provider the agents actually search through. Differs from `provider`
    /// exactly when the active one is missing its credential.
    pub effective_provider: String,
    /// Every connected provider.
    pub providers: Vec<SearchProviderView>,
    /// Whether a key is stored for the active provider. Never the key.
    pub api_key_configured: bool,
    /// The active provider's instance URL, where it has one.
    pub endpoint: Option<String>,
    /// Whether the active provider still needs a key.
    pub needs_api_key: bool,
    /// Whether the active provider still needs an endpoint.
    pub needs_endpoint: bool,
    /// Whether this company's manifest **explicitly** grants `search`.
    pub granted: bool,
    /// Whether this build has the agent search tools compiled in at all.
    pub in_build: bool,
    /// Whether managed search resolves from this company's copied TinyHumans
    /// key or the deployment fallback.
    ///
    /// The company key bills the company's TinyHumans account. The deployment
    /// fallback instead bills the account of whoever runs this server; the
    /// per-company daily cap applies to both tiers.
    ///
    /// The Managed row is rendered from this rather than from a permanent
    /// "Always on" badge. Managed search is always the *fallback*, which is a
    /// different claim from always *working*: a self-hosted deployment with no
    /// platform credential falls back to a surface that answers nothing, and
    /// that badge would be the one claim on this page an operator most needs to
    /// be true.
    pub managed_configured: bool,
    /// Whether the managed credential belongs to this company rather than the
    /// deployment fallback. This is what makes the Managed row editable.
    pub managed_key_configured: bool,
    /// The company's daily managed-search ceiling.
    pub managed_daily_call_cap: u32,
    /// The providers a company can connect.
    pub supported_providers: Vec<String>,
    /// Set when `search/default` names a provider that no longer exists or is
    /// switched off (keys rework #2306, decision D-never-clear-default / X14:
    /// a disable or delete no longer clears the marker itself). An actionable
    /// sentence naming the provider and where to fix it, from
    /// `crate::company::search::copy::default_provider_unavailable`, for the
    /// console to show as a banner rather than the marker silently pointing at
    /// nothing. `None` when the default is unset, or names a connected,
    /// enabled provider — whether or not that provider is *complete* is a
    /// different, pre-existing condition, already covered by `needs_api_key`
    /// and `needs_endpoint` above.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_notice: Option<String>,
}

/// The ops router for search settings.
pub fn router() -> Router<AppState> {
    scoped("/search", get(get_search).put(put_search))
        .merge(scoped("/search/key", delete(delete_search)))
        .merge(scoped("/search/providers", post(connect_provider)))
        .merge(scoped(
            "/search/providers/{slug}",
            put(update_provider).delete(remove_provider),
        ))
        .merge(scoped("/search/providers/{slug}/key", put(replace_key)))
        .merge(scoped("/search/default", put(set_default)))
        .merge(scoped("/search/test", post(test_provider)))
}

/// Reads a stored secret, treating empty as absent.
async fn read(runtime: &CompanyRuntime, key: &str) -> Result<Option<String>, ApiError> {
    Ok(runtime
        .secrets()
        .get(runtime.id(), key)
        .await?
        .map(|value| value.expose().to_string())
        .filter(|value| !value.trim().is_empty()))
}

/// Writes several secrets, rolling back what already landed if one fails.
///
/// Written one `?` at a time, a store that took the key and then failed on the
/// provider would leave a company searching through one provider's index with
/// another provider's key — an authentication failure whose cause is invisible
/// from the settings page that caused it.
async fn write_all(runtime: &CompanyRuntime, writes: &[(&str, String)]) -> Result<(), ApiError> {
    let mut prior: Vec<(&str, String)> = Vec::new();
    for (key, value) in writes {
        let before = read(runtime, key).await?.unwrap_or_default();
        if let Err(err) = runtime
            .secrets()
            .set(runtime.id(), key, SecretValue(value.clone()))
            .await
        {
            for (done, restore) in &prior {
                if let Err(undo) = runtime
                    .secrets()
                    .set(runtime.id(), done, SecretValue(restore.clone()))
                    .await
                {
                    tracing::error!(
                        company = %runtime.id(),                        key = done,
                        "[search] a credential write failed and could not be rolled back; this \
                         company is now half configured: {undo}"
                    );
                }
            }
            return Err(ApiError(err));
        }
        prior.push((key, before));
    }
    Ok(())
}

/// Whether the deployment fallback for managed search resolves here.
fn deployment_managed_configured() -> bool {
    #[cfg(feature = "openhuman")]
    {
        use crate::app::config::ProcessEnv;
        crate::harness::built_in::provider::search_backend_from_env(&ProcessEnv).is_some()
    }
    #[cfg(not(feature = "openhuman"))]
    {
        false
    }
}

/// A bad-request error with `message`.
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError(crate::error::OpenCompanyError::InvalidRequest(
        message.into(),
    ))
}

/// The longest instance address this will store.
///
/// An address is an operator-supplied value that becomes a stored secret and is
/// rendered back on every page load. A sibling surface learned the hard way that
/// an unbounded operator-supplied string reaching the store is a way to leave a
/// row that cannot be deleted; a cap costs nothing and closes the class.
const MAX_ENDPOINT_LEN: usize = 2048;

/// Whether `slug` is safe to build a secret-store key out of.
///
/// Credential addresses are `search/provider/<slug>/key`, so a slug carrying a
/// slash, a control character or an unbounded run of text is a slug that writes
/// somewhere other than where it claims. Every route that *adds* something
/// checks catalogue membership, which is stricter; this exists for the routes
/// that address an existing row, where refusing a slug the catalogue no longer
/// knows would leave the operator unable to delete it.
fn slug_is_addressable(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 32
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The catalogue entry for `slug`, or a refusal naming what this build knows.
///
/// Refused here rather than stored and discovered later: a slug this build
/// cannot search through wires no tools at all, and the settings page would
/// still read as connected.
fn catalogue_entry(slug: &str) -> Result<&'static SearchProviderInfo, ApiError> {
    catalogue::entry(slug).ok_or_else(|| {
        invalid(format!(
            "`{slug}` is not a search provider this build supports — one of: {}",
            SUPPORTED_PROVIDERS.join(", ")
        ))
    })
}

/// Assembles the non-secret status.
async fn status_of(runtime: &CompanyRuntime) -> Result<SearchStatus, ApiError> {
    // The grant lives in the stored manifest, not on the runtime handle. A
    // company that cannot be loaded reports `granted: false` rather than failing
    // the whole status: the operator still needs to see what IS configured, and
    // a settings page that 500s tells them nothing.
    let record = runtime.store().load(runtime.id()).await.ok().flatten();
    let granted = record
        .as_ref()
        .map(|record| crate::company::grants_search_explicit(&record.manifest.tools.allow))
        .unwrap_or(false);
    let managed_daily_call_cap = record
        .as_ref()
        .and_then(|record| record.manifest.tools.search_daily_calls)
        .unwrap_or(crate::company::DEFAULT_SEARCH_DAILY_CALLS);
    let managed_key_configured =
        crate::company::search::load_managed_key(runtime.id(), runtime.secrets().as_ref())
            .await?
            .is_some();

    let candidates =
        crate::company::search::candidates(runtime.id(), runtime.secrets().as_ref()).await?;
    let marked = store::load_default_slug(runtime.id(), runtime.secrets().as_ref()).await?;
    let active = resolve::active(&candidates, marked.as_deref());
    let active_slug = active.map(|candidate| candidate.provider.slug.clone());

    let providers = candidates
        .iter()
        .map(|candidate| view_of(candidate, active_slug.as_deref(), marked.as_deref()))
        .collect();

    let default_notice = default_notice_for(&candidates, marked.as_deref());

    let effective = resolve::effective_slug(active).to_string();

    // `provider` is the SELECTION and `effective_provider` is what answers, and
    // the two must not be collapsed: "I connected Exa and pasted no key" has to
    // read differently from "I connected nothing". So the selection is the
    // marked slug when there is one — even when it resolves to nothing — and
    // only falls back to the active provider for a company that never marked
    // one.
    let selected = marked
        .clone()
        .filter(|slug| candidates.iter().any(|c| &c.provider.slug == slug))
        .or_else(|| active_slug.clone());
    let selected_candidate = selected
        .as_deref()
        .and_then(|slug| candidates.iter().find(|c| c.provider.slug == slug));

    let (api_key_configured, endpoint, needs_api_key, needs_endpoint) = match selected_candidate {
        Some(candidate) => (
            candidate.has_key,
            candidate.provider.endpoint.clone(),
            provider_requires_key(&candidate.provider.slug) && !candidate.has_key,
            provider_requires_endpoint(&candidate.provider.slug)
                && candidate.provider.endpoint.is_none(),
        ),
        None => (false, None, false, false),
    };

    Ok(SearchStatus {
        provider: selected.unwrap_or_else(|| MANAGED_PROVIDER.to_string()),
        effective_provider: effective,
        providers,
        api_key_configured,
        endpoint,
        needs_api_key,
        needs_endpoint,
        granted,
        in_build: cfg!(feature = "openhuman"),
        managed_configured: managed_key_configured || deployment_managed_configured(),
        managed_key_configured,
        managed_daily_call_cap,
        supported_providers: SUPPORTED_PROVIDERS
            .iter()
            .map(|provider| (*provider).to_string())
            .collect(),
        default_notice,
    })
}

/// The catalogue label for `slug`, or the slug itself when this build's
/// catalogue does not (or no longer does) know it — a row can outlive the
/// catalogue entry that named it, and a guard/status sentence still has to
/// name *something*.
fn label_for(slug: &str) -> String {
    catalogue::entry(slug)
        .map(|info| info.label.to_string())
        .unwrap_or_else(|| slug.to_string())
}

/// The `usedBy` a search provider's disable, removal, or key clear would
/// carry (`docs/key-reworks/in-use-guards.md` §1/§6, extended to search by
/// Agent A's scope note in §1): `default: true` iff the **bare stored**
/// `search/default` marker names this slug. Search has no agent pairs and is
/// never itself a `surfaces` target for another guard, so this is the only
/// shape the search guard ever produces.
fn used_by_for(marked: Option<&str>, slug: &str) -> Option<UsedBy> {
    (marked == Some(slug)).then(|| UsedBy {
        default: true,
        ..Default::default()
    })
}

/// `SearchStatus.default_notice`: `Some(sentence)` when `marked` names a slug
/// that either has no row at all (a confirmed delete) or has a row that is
/// switched off (a confirmed disable) — the two states decision
/// D-never-clear-default (X14) now lets the marker sit in indefinitely.
/// `None` when nothing is marked, or the marked provider is connected and
/// enabled (whether or not it is *complete* — see the doc on
/// [`SearchStatus::default_notice`] for why that is a different, pre-existing
/// condition this does not cover).
fn default_notice_for(candidates: &[Candidate], marked: Option<&str>) -> Option<String> {
    let slug = marked?;
    let why = match candidates.iter().find(|c| c.provider.slug == slug) {
        None => ProviderGone::Removed,
        Some(candidate) if !candidate.provider.enabled => ProviderGone::TurnedOff,
        Some(_) => return None,
    };
    Some(copy::default_provider_unavailable(&label_for(slug), why))
}

/// One row, from a candidate.
fn view_of(
    candidate: &Candidate,
    active_slug: Option<&str>,
    marked: Option<&str>,
) -> SearchProviderView {
    let slug = candidate.provider.slug.clone();
    let info = catalogue::entry(&slug);
    SearchProviderView {
        label: info
            .map(|info| info.label.to_string())
            .unwrap_or_else(|| slug.clone()),
        category: info
            .map(|info| info.category.as_str().to_string())
            .unwrap_or_else(|| "account".to_string()),
        enabled: candidate.provider.enabled,
        key_configured: candidate.has_key,
        takes_key: provider_requires_key(&slug),
        takes_endpoint: provider_requires_endpoint(&slug),
        used_by: used_by_for(marked, &slug),
        endpoint: candidate.provider.endpoint.clone(),
        complete: candidate.is_complete(),
        is_default: active_slug == Some(slug.as_str()),
        slug,
    }
}

/// `GET …/search` — non-secret status only.
async fn get_search(company: ScopedCompany) -> Result<Json<SearchStatus>, ApiError> {
    Ok(Json(status_of(&company.runtime).await?))
}

/// What a connect or test attempt came back with.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectOutcome {
    /// Whether the provider answered.
    pub ok: bool,
    /// The probe class when it did not. `null` on success.
    pub probe_class: Option<String>,
    /// One sentence for the operator. **Never carries the upstream body**, which
    /// can echo request material including fragments of the credential.
    pub message: Option<String>,
    /// Whether the record and the credential were kept. Only an auth-class
    /// failure is destructive.
    pub saved: bool,
    /// The status after the attempt.
    pub status: SearchStatus,
}

/// The body for connecting a provider.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectBody {
    /// The catalogue slug.
    slug: String,
    /// The API key (write-only). Required for an account provider.
    #[serde(default)]
    api_key: Option<String>,
    /// The instance URL. Required for a self-hosted provider.
    #[serde(default)]
    endpoint: Option<String>,
}

/// Trims a supplied value and treats empty as absent.
fn supplied(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Validates a draft against what its kind needs, returning the endpoint to use.
async fn validate_draft(
    info: &SearchProviderInfo,
    api_key: Option<&str>,
    endpoint: Option<&str>,
) -> Result<Option<String>, ApiError> {
    if info.needs_key() && api_key.is_none() {
        return Err(invalid(format!("{} needs an API key", info.label)));
    }
    if !info.needs_endpoint() {
        return Ok(None);
    }
    let endpoint =
        endpoint.ok_or_else(|| invalid(format!("{} needs an instance address", info.label)))?;
    validate_endpoint(endpoint).await?;
    Ok(Some(endpoint.to_string()))
}

/// Every check an operator-supplied instance address must pass before it is
/// stored or fetched.
///
/// **One function, because there are four write paths and they disagreed.**
/// `POST …/search/providers` ran all four of these; the two halves of the
/// legacy `PUT …/search` ran the last two, and `POST …/search/test` ran only
/// the last. So the compatibility route stored an address the modern route
/// refuses — including one past [`MAX_ENDPOINT_LEN`], which is the case that
/// comment was written about: an unbounded operator-supplied string reaching
/// the store is how a row that cannot be deleted gets made.
///
/// A validator that is a habit rather than a function is one that gets applied
/// unevenly, and unevenly is how this one was.
async fn validate_endpoint(endpoint: &str) -> Result<(), ApiError> {
    if endpoint.len() > MAX_ENDPOINT_LEN {
        return Err(invalid("that instance address is too long"));
    }
    if endpoint.chars().any(char::is_control) {
        return Err(invalid(
            "that instance address contains a control character",
        ));
    }
    // Checked at the door rather than turned into a connection error the
    // operator has to read a log to find.
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err(invalid(format!("`{endpoint}` is not an http(s) URL")));
    }
    probe::guard_instance_url(endpoint).map_err(invalid)?;
    // And what the name resolves to, not just how it is spelled. The literal
    // guard above judges `http://169.254.169.254/`; only this judges
    // `http://metadata.example/`, and this route may STORE the address without
    // ever probing it — after which the search tool resolves it at agent-turn
    // time and fetches whatever it points at. A name that does not resolve is
    // deliberately not refused; see `guard_resolved_host`.
    probe::guard_resolved_host(endpoint).await.map_err(invalid)
}

/// Runs the check and reports what it means, without touching the store.
async fn check(
    info: &SearchProviderInfo,
    api_key: Option<&str>,
    endpoint: Option<&str>,
) -> Option<(ProbeClass, String)> {
    match probe::probe(info, api_key, endpoint).await {
        Ok(()) => None,
        Err(failure) => {
            let class = probe::classify(info.slug, &failure);
            // Neither the response NOR the log gets the raw failure. The body
            // can echo request material including fragments of the credential,
            // and a log is a second durable copy of it — kept longer and read
            // by more people than the banner that reasoning was written about.
            // `log_detail` keeps the part a person acts on and withholds the
            // body; `class` beside it is what the body was read for.
            tracing::info!(
                provider = info.slug,
                class = class.as_str(),
                detail = %probe::log_detail(&failure),
                "[search] connectivity check failed"
            );
            Some((class, probe::describe(class, info.label)))
        }
    }
}

/// `POST …/search/providers` — connect one provider.
///
/// The ordering is not arbitrary. Local validation first, so nothing is written
/// for a draft that cannot work. Then the record, claimed test-and-set under
/// the store's lock so that two admins connecting the same provider at once
/// cannot both proceed — and before the credential, so the one that is refused
/// has written nothing to overwrite the winner's key with. Then the credential,
/// then the probe, which is handed the key rather than reading it back. Only an
/// auth-class failure rolls both back.
async fn connect_provider(
    company: AdminScopedCompany,
    State(_state): State<AppState>,
    Json(body): Json<ConnectBody>,
) -> Result<Json<ConnectOutcome>, ApiError> {
    let runtime = &company.runtime;
    let slug = body.slug.trim().to_ascii_lowercase();
    let info = catalogue_entry(&slug)?;

    let api_key = supplied(body.api_key.as_deref());
    let endpoint = validate_draft(
        info,
        api_key.as_deref(),
        supplied(body.endpoint.as_deref()).as_deref(),
    )
    .await?;

    // Test-and-set under the store's own lock, and **before** the credential.
    //
    // It used to be a read here and a write three awaits later, so two admins
    // connecting the same provider at once both got past it. The loser then did
    // real damage rather than duplicating work: an `Auth` probe failure rolls
    // back by deleting the row *and* the credential, so a request whose key was
    // rejected deleted the row and the working key the other request had just
    // stored — while that request answered `saved: true` from its own
    // request-local copy.
    //
    // Before the credential, because a loser must not have written anything by
    // the time it is refused. The order the comment above describes is
    // unaffected: the probe is handed the key rather than reading it back.
    if !store::claim_provider(
        runtime.id(),
        runtime.secrets().as_ref(),
        SearchProvider {
            slug: slug.clone(),
            enabled: true,
            endpoint: endpoint.clone(),
        },
    )
    .await?
    {
        return Err(invalid(format!(
            "{} is already connected — replace its key instead",
            info.label
        )));
    }

    if let Some(key) = api_key.as_deref() {
        // `if_connected`, because the claim above released the index lock and a
        // removal can land in the gap. Writing the credential anyway would put
        // it at an address the index does not hold — invisible in status,
        // skipped by Disconnect all — and this route would still answer
        // `saved: true` for a provider that is no longer connected.
        match store::store_key_if_connected(runtime.id(), runtime.secrets().as_ref(), &slug, key)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return Err(invalid(format!(
                    "{} was disconnected while it was being connected — try again",
                    info.label
                )));
            }
            // The claim above already wrote the row. Leaving it on a failed
            // credential write reported the connect as failed while keeping an
            // incomplete row behind — and the retry then failed with "already
            // connected". Undo the claim so the operation is retryable, and say
            // so loudly if even that fails.
            Err(err) => {
                if let Err(rollback) =
                    store::delete_provider(runtime.id(), runtime.secrets().as_ref(), &slug).await
                {
                    tracing::error!(
                        company = %runtime.id(),
                        provider = %slug,
                        "[search] a connect whose credential could not be stored also could not \
                         undo its claim; the row is left incomplete: {rollback}"
                    );
                }
                return Err(err.into());
            }
        }
    }

    let failure = check(info, api_key.as_deref(), endpoint.as_deref()).await;
    let (ok, probe_class, message, saved) = match failure {
        None => (true, None, None, true),
        Some((class, message)) if probe::destroys_credential(class) => {
            // Roll back both stores. A failed rollback is logged rather than
            // swallowed: a credential left behind for a provider with no record
            // is an orphaned secret.
            if let Err(err) =
                store::delete_provider(runtime.id(), runtime.secrets().as_ref(), &slug).await
            {
                tracing::error!(
                    company = %runtime.id(),                    provider = %slug,
                    "[search] a rejected credential could not be rolled back: {err}"
                );
            }
            (false, Some(class), Some(message), false)
        }
        Some((class, message)) => (false, Some(class), Some(message), true),
    };

    Ok(Json(ConnectOutcome {
        ok,
        probe_class: probe_class.map(|class| class.as_str().to_string()),
        message,
        saved,
        status: status_of(runtime).await?,
    }))
}

/// The `{slug}` capture, as a named struct.
///
/// **Not `Path<String>`.** Every route here is registered by
/// [`scoped`](super::scope::scoped), which serves both the platform form
/// (`…/companies/{id}/search/providers/{slug}`) and the single-company alias
/// (`…/company/search/providers/{slug}`). The platform form therefore captures
/// *two* parameters, and a `Path<String>` under it fails extraction with "wrong
/// number of path parameters" — a 400 on every call, from the console as well
/// as from a test. A named struct deserializes by key and works under both
/// shapes, which is why every other ops module with a path parameter uses one.
#[derive(Debug, Deserialize)]
struct SlugPath {
    slug: String,
}

/// The body for changing one provider.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateBody {
    /// Turn the provider on or off. Omit to leave it unchanged.
    #[serde(default)]
    enabled: Option<bool>,
    /// A new instance address, for a self-hosted provider.
    #[serde(default)]
    endpoint: Option<String>,
    /// Confirms a disable that the in-use guard would otherwise refuse
    /// (`docs/key-reworks/in-use-guards.md` §2). Ignored on an enable or a
    /// re-address alone — only a disable is guarded — and ignored when there
    /// is nothing to confirm. Defaults to `false`, so a caller that has never
    /// heard of this field gets the guarded path.
    #[serde(default)]
    confirm_in_use: bool,
}

/// `PUT …/search/providers/{slug}` — enable, disable, or re-address.
async fn update_provider(
    company: AdminScopedCompany,
    Path(SlugPath { slug }): Path<SlugPath>,
    State(_state): State<AppState>,
    Json(body): Json<UpdateBody>,
) -> Result<Json<SearchStatus>, ApiError> {
    let runtime = &company.runtime;
    let slug = slug.trim().to_ascii_lowercase();
    let info = catalogue_entry(&slug)?;

    if !store::list_providers(runtime.id(), runtime.secrets().as_ref())
        .await?
        .iter()
        .any(|provider| provider.slug == slug)
    {
        return Err(invalid(format!("{} is not connected", info.label)));
    }

    if let Some(endpoint) = supplied(body.endpoint.as_deref()) {
        let endpoint = validate_draft(info, Some("unused"), Some(&endpoint)).await?;
        // The read of the current row and the write back are one critical
        // section in the store. Split — as they were here, with the `enabled`
        // flag read above and written below — a removal landing between them
        // made this **recreate** the row: a provider the operator had just
        // disconnected came back enabled with a fresh address and started
        // receiving agent searches again, after the removal had answered 200.
        if !store::update_endpoint_if_present(
            runtime.id(),
            runtime.secrets().as_ref(),
            &slug,
            endpoint,
        )
        .await?
        {
            return Err(invalid(format!("{} is not connected", info.label)));
        }
    }
    if let Some(enabled) = body.enabled {
        // Only a disable is guarded (in-use-guards.md §1/§2): turning a
        // provider ON cannot strand anything this company already had. The
        // marked-default read, the in-use check, and the write are one
        // critical section under the store's index lock (KR review comment
        // 4012261309) — the row survives a disable (unlike a delete), so
        // `status_of`'s own `used_by` for this slug reports the same thing
        // afterwards.
        match store::set_enabled_guarded(
            runtime.id(),
            runtime.secrets().as_ref(),
            &slug,
            enabled,
            body.confirm_in_use,
        )
        .await?
        {
            store::GuardedWrite::Blocked => {
                return Err(ApiError(OpenCompanyError::InUse {
                    message: copy::provider_in_use_message(info.label),
                    used_by: used_by_for(Some(&slug), &slug)
                        .expect("Blocked only returned when slug is the marked default"),
                }));
            }
            store::GuardedWrite::NotConnected | store::GuardedWrite::Applied => {}
        }
    }

    Ok(Json(status_of(runtime).await?))
}

/// `?confirmInUse=true` on [`remove_provider`]
/// (`docs/key-reworks/in-use-guards.md` §2: "as the query parameter
/// `?confirmInUse=true` for DELETE, which has no body on this API").
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmInUseQuery {
    #[serde(default)]
    confirm_in_use: bool,
}

/// `DELETE …/search/providers/{slug}` — remove a provider and its credential.
async fn remove_provider(
    company: AdminScopedCompany,
    Path(SlugPath { slug }): Path<SlugPath>,
    Query(ConfirmInUseQuery { confirm_in_use }): Query<ConfirmInUseQuery>,
    State(_state): State<AppState>,
) -> Result<Json<SearchStatus>, ApiError> {
    let runtime = &company.runtime;
    let slug = slug.trim().to_ascii_lowercase();
    // Checked rather than looked up: a row whose slug this build no longer has
    // in its catalogue must still be removable, but a slug that could address
    // something other than its own credential must not reach the store.
    if !slug_is_addressable(&slug) {
        return Err(invalid("that is not a provider slug"));
    }
    // The marked-default read, the in-use check, and the removal are one
    // critical section under the store's index lock (KR review comment
    // 4012261309), so a confirmed removal echoes exactly what it would have
    // refused with rather than a stale read from before a concurrent
    // `set_default_if_connected` landed. The row is gone after this, so —
    // unlike a disable — there is no surviving row for `status_of`'s own
    // `used_by` to echo it through; the caller already saw this (from a prior
    // GET, or from a first unconfirmed 409) before confirming.
    match store::delete_provider_guarded(
        runtime.id(),
        runtime.secrets().as_ref(),
        &slug,
        confirm_in_use,
    )
    .await?
    {
        store::GuardedWrite::Blocked => {
            return Err(ApiError(OpenCompanyError::InUse {
                message: copy::provider_in_use_message(&label_for(&slug)),
                used_by: used_by_for(Some(&slug), &slug)
                    .expect("Blocked only returned when slug is the marked default"),
            }));
        }
        store::GuardedWrite::NotConnected | store::GuardedWrite::Applied => {}
    }
    Ok(Json(status_of(runtime).await?))
}

/// The body for replacing one provider's credential.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyBody {
    /// The new key (write-only). An empty value clears it.
    #[serde(default)]
    api_key: Option<String>,
    /// Confirms a clear that the in-use guard would otherwise refuse
    /// (`docs/key-reworks/in-use-guards.md` §2). Ignored on a set/rotate —
    /// only a clear is guarded — and ignored when there is nothing to
    /// confirm. Defaults to `false`.
    #[serde(default)]
    confirm_in_use: bool,
}

/// `PUT …/search/providers/{slug}/key` — replace or clear one credential.
///
/// Clearing is a write of the empty string: the store has no delete, and every
/// read site treats an empty value as unset.
async fn replace_key(
    company: AdminScopedCompany,
    Path(SlugPath { slug }): Path<SlugPath>,
    State(_state): State<AppState>,
    Json(body): Json<KeyBody>,
) -> Result<Json<SearchStatus>, ApiError> {
    let runtime = &company.runtime;
    let slug = slug.trim().to_ascii_lowercase();
    let key = supplied(body.api_key.as_deref()).unwrap_or_default();
    if slug == MANAGED_PROVIDER {
        let _guard = crate::company::company_key::slot_guard(runtime.id()).await;
        if key.is_empty() && !body.confirm_in_use {
            let current = status_of(runtime).await?;
            if current.effective_provider == MANAGED_PROVIDER && current.managed_key_configured {
                return Err(ApiError(OpenCompanyError::InUse {
                    message: "Managed Search is active for this company.".to_string(),
                    used_by: UsedBy {
                        default: true,
                        ..Default::default()
                    },
                }));
            }
        }
        runtime
            .secrets()
            .set(
                runtime.id(),
                crate::company::search::MANAGED_KEY_SECRET,
                SecretValue(key),
            )
            .await?;
        return Ok(Json(status_of(runtime).await?));
    }
    let info = catalogue_entry(&slug)?;
    if !info.needs_key() {
        return Err(invalid(format!("{} does not take an API key", info.label)));
    }
    // Only a clear is guarded (in-use-guards.md §1/§6): a clear is
    // functionally equivalent to disabling the row from a dependent's point
    // of view — a key-less row cannot serve the default — while a rotate
    // keeps serving whatever already depended on it.
    //
    // **Replace**, so there has to be something to replace. Without this a
    // direct `PUT …/search/providers/exa/key` for a provider with no row wrote
    // a credential to `search/provider/exa/key` that the status route never
    // reports and `DELETE …/search/key` never clears — its loop visits indexed
    // providers only. An invisible credential the operator cannot see and
    // cannot delete is the orphaned-secret shape this module keeps refusing
    // elsewhere, and it does not get an exception here.
    //
    // The marked-default read, the in-use check, the existence check, and the
    // write are one critical section under the store's index lock (KR review
    // comment 4012261309): a removal or a default change landing between them
    // used to be able to leave the credential at an address absent from the
    // index, or let an unconfirmed clear through against a marker that was
    // stale by the time it was read.
    match store::store_key_guarded(
        runtime.id(),
        runtime.secrets().as_ref(),
        &slug,
        &key,
        body.confirm_in_use,
    )
    .await?
    {
        store::GuardedWrite::Blocked => {
            return Err(ApiError(OpenCompanyError::InUse {
                message: copy::provider_in_use_message(info.label),
                used_by: used_by_for(Some(&slug), &slug)
                    .expect("Blocked only returned when slug is the marked default"),
            }));
        }
        store::GuardedWrite::NotConnected => {
            return Err(invalid(format!(
                "{} is not connected — connect it instead",
                info.label
            )));
        }
        store::GuardedWrite::Applied => {}
    }
    Ok(Json(status_of(runtime).await?))
}

/// The body for marking the default provider.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefaultBody {
    /// The slug to mark, or `null`/empty to unmark and fall back to the first
    /// enabled provider.
    #[serde(default)]
    slug: Option<String>,
}

/// `PUT …/search/default` — mark which provider the agents search through.
async fn set_default(
    company: AdminScopedCompany,
    State(_state): State<AppState>,
    Json(body): Json<DefaultBody>,
) -> Result<Json<SearchStatus>, ApiError> {
    let runtime = &company.runtime;
    match supplied(body.slug.as_deref()) {
        Some(slug) => {
            let slug = slug.to_ascii_lowercase();
            let info = catalogue_entry(&slug)?;
            // Checked and marked under the index lock that removal also takes,
            // so a removal cannot land between the check and the write and
            // leave a deleted slug marked as the default.
            if !store::set_default_if_connected(runtime.id(), runtime.secrets().as_ref(), &slug)
                .await?
            {
                return Err(invalid(format!("{} is not connected", info.label)));
            }
        }
        None => store::clear_default_slug(runtime.id(), runtime.secrets().as_ref()).await?,
    }
    Ok(Json(status_of(runtime).await?))
}

/// The body for a connectivity check.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestBody {
    /// The provider to check.
    slug: String,
    /// A key to check **without storing it** — testing a credential and
    /// committing to it are separate acts. Omit to check the stored one.
    #[serde(default)]
    api_key: Option<String>,
    /// An address to check without storing it.
    #[serde(default)]
    endpoint: Option<String>,
}

/// `POST …/search/test` — check a draft or a stored provider.
///
/// `AdminScopedCompany`, not `ScopedCompany`: the check spends the company's
/// money — no search provider publishes a free credential validator — and for a
/// self-hosted provider it fetches an operator-supplied address that is allowed
/// to be on a private network.
async fn test_provider(
    company: AdminScopedCompany,
    State(_state): State<AppState>,
    Json(body): Json<TestBody>,
) -> Result<Json<ConnectOutcome>, ApiError> {
    let runtime = &company.runtime;
    let slug = body.slug.trim().to_ascii_lowercase();
    let info = catalogue_entry(&slug)?;

    let api_key = match supplied(body.api_key.as_deref()) {
        Some(key) => Some(key),
        None => store::load_provider_key(runtime.id(), runtime.secrets().as_ref(), &slug).await?,
    };
    // **An address is only ever accepted for a provider that HAS one.**
    //
    // Without this an admin could check Brave against an address of their
    // choosing, and the probe would put the company's **stored Brave key** in a
    // header to it. The credential is write-only everywhere on this surface —
    // it is never returned by any route and never rendered back — and this
    // would have handed it straight back, to any destination, through a route
    // whose whole purpose is that it is safe to press. Neither the URL shape
    // check nor the DNS pin helps: both ask whether an address may be fetched,
    // and the question here is whether this provider has an address at all.
    //
    // The three account providers answer at constants in the catalogue.
    // `validate_draft` already drops an endpoint for them on every write path;
    // this is the read-only path that had no equivalent.
    let endpoint = match supplied(body.endpoint.as_deref()) {
        Some(endpoint) if !info.needs_endpoint() => {
            return Err(invalid(format!(
                "{} answers at its own address — `{endpoint}` cannot be checked against it",
                info.label
            )));
        }
        Some(endpoint) => Some(endpoint),
        None => store::list_providers(runtime.id(), runtime.secrets().as_ref())
            .await?
            .into_iter()
            .find(|provider| provider.slug == slug)
            .and_then(|provider| provider.endpoint),
    };
    if let Some(endpoint) = endpoint.as_deref() {
        validate_endpoint(endpoint).await?;
    }

    let failure = check(info, api_key.as_deref(), endpoint.as_deref()).await;
    Ok(Json(ConnectOutcome {
        ok: failure.is_none(),
        probe_class: failure
            .as_ref()
            .map(|(class, _)| class.as_str().to_string()),
        // A test never changes the store, so the save-flavoured sentences
        // `describe` produces would be wrong here. The class is what the console
        // renders from; this is the fallback for anything that does not.
        message: failure.as_ref().map(|(_, message)| message.clone()),
        saved: true,
        status: status_of(runtime).await?,
    }))
}

/// The write-only config body for the single-slot route. Every field is
/// optional; only fields present and non-empty are applied.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchConfigBody {
    /// The provider slug. Omit to leave it unchanged.
    #[serde(default)]
    provider: Option<String>,
    /// The provider API key (write-only). Omit to leave it unchanged.
    #[serde(default)]
    api_key: Option<String>,
    /// The instance URL, for SearXNG. Omit to leave it unchanged.
    #[serde(default)]
    endpoint: Option<String>,
}

/// `PUT …/search` — the single-slot route, kept and re-expressed over the list.
///
/// It predates the provider list and other callers may still hold it, so it
/// stays. What it must **not** do any more is write the flat keys directly: that
/// is the write half of the bug this rework exists to fix, and it would also
/// fight the convergence rule in [`store`]. So it is now sugar over the list —
/// connect-or-update the named provider, store its key at *its own* address, and
/// mark it as the default, which is what selecting a provider always meant.
async fn put_search(
    company: AdminScopedCompany,
    State(_state): State<AppState>,
    Json(body): Json<SearchConfigBody>,
) -> Result<Json<SearchStatus>, ApiError> {
    let runtime = &company.runtime;

    let Some(provider) = supplied(body.provider.as_deref()).map(|p| p.to_ascii_lowercase()) else {
        // No provider named: apply the key or address to the SELECTED provider,
        // not to the effective one. They differ precisely in the case this
        // branch exists to serve — "I picked SearXNG, now here is its address" —
        // where the selection resolves to nothing yet and the effective provider
        // is still managed. Reading the effective one here would refuse the
        // request that completes the configuration.
        //
        // Unmarked, it is the row the STATUS ROUTE NAMES — not simply the first
        // stored one. A client of this compatibility API reads `provider` from
        // the status and patches it, and the two disagreed: with a disabled Exa
        // stored before an enabled Brave, the status said `brave` and a bare
        // `{"apiKey": …}` silently updated exa. The same resolver answers both
        // now, so what was read is what is written.
        //
        // First-in-list survives as the last fallback, which is the case the
        // comment above is about: nothing resolves yet, so there is no active
        // row to name and the one the operator just picked is the answer.
        let selected = match store::load_default_slug(runtime.id(), runtime.secrets().as_ref())
            .await?
        {
            Some(slug) => Some(slug),
            None => {
                // The same call the status route makes, so the two cannot
                // drift: one derivation of "which row answers", not two.
                let candidates =
                    crate::company::search::candidates(runtime.id(), runtime.secrets().as_ref())
                        .await?;
                match resolve::active(&candidates, None) {
                    Some(active) => Some(active.provider.slug.clone()),
                    None => candidates
                        .into_iter()
                        .next()
                        .map(|candidate| candidate.provider.slug),
                }
            }
        };
        let Some(selected) = selected else {
            return Err(invalid("no provider is connected to apply that to"));
        };
        return apply_to(runtime, &selected, &body).await;
    };

    if provider == MANAGED_PROVIDER {
        // DEPRECATED(keys-rework #2306): the legacy single-slot `PUT
        // …/search` route's own "select managed" branch, clearing
        // `search/default` below; replaced by the indexed
        // `search/providers`/`search/default` flow, which never clears the
        // marker on a confirmed disable or removal
        // (X14/D-never-clear-default, `docs/key-reworks/in-use-guards.md`
        // §4). This branch still does, and — unlike every guarded route on
        // this page — that is an intentional, documented carve-out rather
        // than an oversight: the console has no caller for this route
        // (`saveSearch` in `frontend/src/api/search.ts` is defined but never
        // called; confirmed by grepping `frontend/src` for it), so nothing
        // reachable today exercises this clear. See in-use-guards.md §4 for
        // the full note. Removable when the route itself is removed, no
        // earlier — routes are not removed silently.
        //
        // Selecting managed has always meant "stop using my own account", and
        // the honest expression of that is still NOT to store `managed` as
        // though it were a connection.
        //
        // Unmarking the default is not it either, though, which is what this
        // branch used to do alone. [`resolve::active`] reads an absent marker as
        // "the first usable provider", so a company with any usable connection
        // kept searching through it while this route answered 200 — and worst
        // for exactly the configurations this route exists to serve, since an
        // upgraded legacy company has no marker for the clear to remove.
        //
        // So the connections are switched off. Managed search is what the
        // absence of everything else means, and leaving nothing in the way is
        // the only representation of that this model has. Nothing is destroyed:
        // every credential and address stays where it is, the rows stay on the
        // page reading as off, and naming one of them again turns it back on.
        for connected in store::list_providers(runtime.id(), runtime.secrets().as_ref()).await? {
            if connected.enabled {
                store::set_enabled(
                    runtime.id(),
                    runtime.secrets().as_ref(),
                    &connected.slug,
                    false,
                )
                .await?;
            }
        }
        store::clear_default_slug(runtime.id(), runtime.secrets().as_ref()).await?;
        return Ok(Json(status_of(runtime).await?));
    }

    let info = catalogue_entry(&provider)?;

    // Every supplied field is validated before anything is written.
    let endpoint = supplied(body.endpoint.as_deref());
    if let Some(endpoint) = endpoint.as_deref()
        && info.needs_endpoint()
    {
        validate_endpoint(endpoint).await?;
    }
    let key = supplied(body.api_key.as_deref());

    // Then one critical section in the store: row, credential, default marker.
    // As separate unlocked steps, a concurrent Change address was overwritten
    // with the address this handler had read, and a concurrent removal could
    // leave the deleted slug marked as the default. An omitted address is left
    // exactly as stored — nothing is carried over from a snapshot.
    store::select_provider(
        runtime.id(),
        runtime.secrets().as_ref(),
        &provider,
        endpoint.filter(|_| info.needs_endpoint()),
        key.as_deref(),
    )
    .await?;

    Ok(Json(status_of(runtime).await?))
}

/// [`put_search`]'s tail, for a provider that was not named in the body.
async fn apply_to(
    runtime: &CompanyRuntime,
    slug: &str,
    body: &SearchConfigBody,
) -> Result<Json<SearchStatus>, ApiError> {
    // Validate every supplied field BEFORE mutating anything. The key used to be
    // written first, so a request with a new key and an invalid address
    // answered 400 with the credential already replaced — and a client that
    // reasonably treats a failed patch as unapplied would be searching with a
    // key it believes it never set.
    let endpoint = supplied(body.endpoint.as_deref());
    if let Some(endpoint) = endpoint.as_deref() {
        validate_endpoint(endpoint).await?;
    }

    if let Some(key) = supplied(body.api_key.as_deref())
        && !store::store_key_if_connected(runtime.id(), runtime.secrets().as_ref(), slug, &key)
            .await?
    {
        // The slug was read from the index before this call; a removal since
        // would otherwise leave this key stored with no row to list it.
        return Err(invalid("that provider was disconnected — try again"));
    }
    if let Some(endpoint) = endpoint {
        // The same connected-only update the modern re-address route uses. This
        // read the row's `enabled` flag and then called `put_provider`, which
        // RECREATES a row — so a removal landing in between was undone, and a
        // provider the operator had just disconnected came back.
        if !store::update_endpoint_if_present(
            runtime.id(),
            runtime.secrets().as_ref(),
            slug,
            Some(endpoint),
        )
        .await?
        {
            return Err(invalid("that provider was disconnected — try again"));
        }
    }
    Ok(Json(status_of(runtime).await?))
}

/// `DELETE …/search/key` — clear every connection and fall back to managed.
///
/// The [`SecretStore`](crate::ports::SecretStore) port has no delete, so a
/// cleared credential is stored as the empty string; every read site treats an
/// empty value as unset, and resolution then falls back to managed search.
///
/// Guarded like every other destructive action on this page
/// (`docs/key-reworks/in-use-guards.md` §1/§2, keys rework #2306): refused
/// with `409 in_use` unless `?confirmInUse=true` when `search/default` is
/// currently set to **any** provider — this is the bulk form of the same
/// disable/remove/key-clear guard, so `usedBy` here is always exactly
/// `{ "default": true }` rather than naming one specific slug the way a
/// single-row guard's `usedBy` does. On a confirmed disconnect-all,
/// `search/default` is **not** cleared (X14/D-never-clear-default, §4): the
/// marker is left in place so `SearchStatus.default_notice` picks up that it
/// now names a removed provider, exactly as a confirmed single-row removal
/// already does.
async fn delete_search(
    company: AdminScopedCompany,
    Query(ConfirmInUseQuery { confirm_in_use }): Query<ConfirmInUseQuery>,
    State(_state): State<AppState>,
) -> Result<Json<SearchStatus>, ApiError> {
    let runtime = &company.runtime;
    // Computed before the delete, per in-use-guards.md §3, so a confirmed
    // disconnect-all echoes exactly what it would have refused with — named by
    // whichever provider is currently marked, the same label a single-row
    // guard would use for it. The bulk action just sweeps every row rather
    // than one.
    let marked = store::load_default_slug(runtime.id(), runtime.secrets().as_ref())
        .await
        .map_err(ApiError)?;
    if let Some(slug) = marked.as_deref()
        && !confirm_in_use
    {
        return Err(ApiError(OpenCompanyError::InUse {
            message: copy::provider_in_use_message(&label_for(slug)),
            used_by: UsedBy {
                default: true,
                ..Default::default()
            },
        }));
    }

    // Every provider goes, not just the active one: this route has always meant
    // "disconnect my own search", and leaving a second account's key behind
    // under a page that now says "managed" would be storing a credential the
    // operator believes they deleted.
    //
    // One hold of the index lock from the snapshot to the last removal. Taken
    // per row, a connect landing after the snapshot survived a disconnect that
    // reported success.
    store::delete_all_providers(runtime.id(), runtime.secrets().as_ref()).await?;
    let cleared: Vec<(&str, String)> = [API_KEY_SECRET, PROVIDER_SECRET, ENDPOINT_SECRET]
        .into_iter()
        .map(|key| (key, String::new()))
        .collect();
    write_all(runtime, &cleared).await?;
    // X14 (`in-use-guards.md` §4): never clear the marker, even confirmed —
    // `default_notice_for` is what turns a marker now naming nothing into the
    // console's banner.
    Ok(Json(status_of(runtime).await?))
}

#[cfg(test)]
#[path = "search_a_legacy_patch_that_tests.rs"]
mod tests_a_legacy_patch_that;
#[cfg(test)]
#[path = "search_an_unconfigured_company_reports_tests.rs"]
mod tests_an_unconfigured_company_reports;
#[cfg(test)]
#[path = "search_managed_key_tests.rs"]
mod tests_managed_key;
