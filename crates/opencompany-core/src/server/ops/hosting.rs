//! The hosting configuration write-plane: store a company's hosting provider
//! API key — **write-only** — and report what is configured without echoing it.
//!
//! `GET …/hosting` returns only [`HostingStatus`]: booleans, the provider slug,
//! and the team scope. The API key is never serialized into any response, by
//! construction. It lives in [`SecretStore`](crate::ports::SecretStore) and this
//! module reads it back only to report *whether* it is there.
//!
//! # Why the team is stored beside the key but is not confidential
//!
//! A team scope is not a secret, and it *is* returned by `GET` — a settings form
//! has to show which account it deploys to, and "Connected ✓" beside the wrong
//! team is exactly the confusion this avoids. It shares the secret store with
//! the key only because the pair is used together.
//!
//! # Three things can each be missing, and they fail differently
//!
//! [`HostingStatus`] reports them separately rather than as one "connected"
//! flag, because the remedies differ: no key means the agents have no hosting
//! tools at all; a missing `hosting` grant means the key is configured and still
//! nothing reaches an agent; and a build without the `openhuman` feature has no
//! hosting tools to wire in the first place. A single boolean would send an
//! operator looking in the wrong place for two of those three.

use axum::extract::State;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::company::hosting::{API_KEY_SECRET, DEFAULT_PROVIDER, PROVIDER_SECRET, TEAM_SECRET};
use crate::company::runtime::CompanyRuntime;
use crate::ports::types::SecretValue;
use crate::server::error::ApiError;
use crate::server::ops::scope::{AdminScopedCompany, ScopedCompany, scoped};

/// The providers this build can deploy to.
///
/// Kept here rather than derived from the crate so a settings form can render a
/// picker in a build with no hosting tools compiled in at all.
pub const SUPPORTED_PROVIDERS: [&str; 1] = ["vercel"];

/// The non-secret view of a company's hosting configuration.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostingStatus {
    /// Whether an API key is stored and non-empty. Never the key itself.
    pub api_key_configured: bool,
    /// The provider the key belongs to — shown so a settings form can say
    /// *which* provider it is connected to rather than only that it is.
    pub provider: String,
    /// The team, organization, or account scope, when one is set. `None` means
    /// the provider's personal account.
    pub team: Option<String>,
    /// Whether this company's manifest **explicitly** grants `hosting`. A key
    /// can be present and still wire no tools without it.
    pub granted: bool,
    /// Whether this build has the hosting tools compiled in at all.
    pub in_build: bool,
    /// The providers a key can be stored for.
    pub supported_providers: Vec<String>,
}

/// The ops router for hosting settings.
pub fn router() -> Router<AppState> {
    scoped("/hosting", get(get_hosting).put(put_hosting))
        .merge(scoped("/hosting/key", delete(delete_hosting)))
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
/// provider would leave the key stored behind an error response — a company
/// whose settings page reads "not connected" while an agent deploys with the
/// key anyway.
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
                        company = %runtime.id(),
                        key = done,
                        "[hosting] a credential write failed and could not be rolled back; this \
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

/// Assembles the non-secret status.
async fn status_of(runtime: &CompanyRuntime) -> Result<HostingStatus, ApiError> {
    // The grant lives in the stored manifest, not on the runtime handle. A
    // company that cannot be loaded reports `granted: false` rather than failing
    // the whole status: the operator still needs to see what IS configured, and
    // a settings page that 500s tells them nothing.
    let granted = runtime
        .store()
        .load(runtime.id())
        .await
        .ok()
        .flatten()
        .map(|record| crate::company::grants_hosting_explicit(&record.manifest.tools.allow))
        .unwrap_or(false);

    Ok(HostingStatus {
        api_key_configured: read(runtime, API_KEY_SECRET).await?.is_some(),
        provider: read(runtime, PROVIDER_SECRET)
            .await?
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_string()),
        team: read(runtime, TEAM_SECRET).await?,
        granted,
        in_build: cfg!(feature = "openhuman"),
        supported_providers: SUPPORTED_PROVIDERS
            .iter()
            .map(|p| (*p).to_string())
            .collect(),
    })
}

/// `GET …/hosting` — non-secret status only.
async fn get_hosting(company: ScopedCompany) -> Result<Json<HostingStatus>, ApiError> {
    Ok(Json(status_of(&company.runtime).await?))
}

/// The write-only config body. Every field is optional; only fields present and
/// non-empty are applied, so the team can be corrected without re-entering the
/// key.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostingConfigBody {
    /// The provider API key (write-only). Omit to leave it unchanged.
    #[serde(default)]
    api_key: Option<String>,
    /// The provider slug. Omit to leave it unchanged.
    #[serde(default)]
    provider: Option<String>,
    /// The team, organization, or account scope. Omit to leave it unchanged.
    #[serde(default)]
    team: Option<String>,
}

/// `PUT …/hosting` — store any supplied credentials, return status.
///
/// Requires authority over the company: this key deploys the company's files to
/// the public internet under its own account and can provision a database it is
/// billed for, so pointing it at a different hosting account is not an ordinary
/// member's edit.
async fn put_hosting(
    company: AdminScopedCompany,
    State(_state): State<AppState>,
    Json(body): Json<HostingConfigBody>,
) -> Result<Json<HostingStatus>, ApiError> {
    let runtime = &company.runtime;
    let supplied = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };

    let mut writes: Vec<(&str, String)> = Vec::new();
    if let Some(api_key) = supplied(body.api_key.as_deref()) {
        writes.push((API_KEY_SECRET, api_key));
    }
    if let Some(provider) = supplied(body.provider.as_deref()) {
        let provider = provider.to_ascii_lowercase();
        // Refused here rather than stored and discovered later: a slug this
        // build cannot connect to would wire no tools at all, and the settings
        // page would still read "Connected".
        if !SUPPORTED_PROVIDERS.contains(&provider.as_str()) {
            return Err(ApiError(crate::error::OpenCompanyError::InvalidRequest(
                format!(
                    "`{provider}` is not a hosting provider this build supports — one of: {}",
                    SUPPORTED_PROVIDERS.join(", ")
                ),
            )));
        }
        writes.push((PROVIDER_SECRET, provider));
    }
    if let Some(team) = supplied(body.team.as_deref()) {
        writes.push((TEAM_SECRET, team));
    }
    write_all(runtime, &writes).await?;

    Ok(Json(status_of(runtime).await?))
}

/// `DELETE …/hosting/key` — clear every stored hosting credential.
///
/// The [`SecretStore`](crate::ports::SecretStore) port has no delete, so a
/// cleared credential is stored as the empty string; every read site treats an
/// empty value as unset, and the tools then fail closed.
async fn delete_hosting(
    company: AdminScopedCompany,
    State(_state): State<AppState>,
) -> Result<Json<HostingStatus>, ApiError> {
    let runtime = &company.runtime;
    // Together, and including the team: a clear that dropped the key but left a
    // team behind would report the integration as disconnected while a later
    // key, saved without a team, silently inherited the old account scope.
    let cleared: Vec<(&str, String)> = [API_KEY_SECRET, PROVIDER_SECRET, TEAM_SECRET]
        .into_iter()
        .map(|key| (key, String::new()))
        .collect();
    write_all(runtime, &cleared).await?;
    Ok(Json(status_of(runtime).await?))
}

#[cfg(test)]
#[path = "hosting_tests.rs"]
mod tests;
