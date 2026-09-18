//! The Chargebee billing configuration write-plane (issue #788, UI in #527):
//! store the API key, the site identifier and the webhook credential — all
//! **write-only** — and surface the webhook URL an operator pastes into
//! Chargebee.
//!
//! `GET …/billing/chargebee` returns only [`BillingStatus`], which carries
//! booleans and the site slug. The API key and the webhook credential are never
//! serialized into any response, by construction: they live in
//! [`SecretStore`](crate::ports::SecretStore) and this module reads them back
//! only to *use* them, never to echo them.
//!
//! # Why the site identifier is a secret too
//!
//! It is not confidential, and it *is* returned by `GET` — a settings form has
//! to show what it is configured against, and "Connected ✓" beside the wrong
//! site is exactly the confusion this avoids. It shares the secret store with
//! the key only because the pair is meaningless apart: the tools need both or
//! neither, so keeping them in one place makes "half configured" impossible to
//! express by accident.
//!
//! # Three things can each be missing, and they fail differently
//!
//! [`BillingStatus`] reports them separately rather than as one "connected"
//! flag, because the remedies differ: no key or site means the agent has no
//! billing tools at all; no webhook credential means the tools work but nobody
//! is told when a customer pays; and a missing `chargebee` grant means both are
//! configured and still nothing reaches an agent. A single boolean would send an
//! operator looking in the wrong place for two of those three.

use axum::extract::State;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::company::billing::{API_KEY_SECRET, SITE_SECRET, WEBHOOK_SECRET_KEY};
use crate::company::paypal::{
    CLIENT_ID_SECRET, CLIENT_SECRET_SECRET, ENVIRONMENT_SECRET, PaypalEnvironment,
};
use crate::company::runtime::CompanyRuntime;
use crate::ports::types::{CompanyId, SecretValue};
use crate::server::error::ApiError;
use crate::server::ops::scope::{AdminScopedCompany, ScopedCompany, scoped};

/// The non-secret view of a company's Chargebee configuration.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BillingStatus {
    /// Whether an API key is stored and non-empty. Never the key itself.
    pub api_key_configured: bool,
    /// The Chargebee site slug, when set — shown so a settings form can say
    /// *which* site it is connected to rather than only that it is.
    pub site: Option<String>,
    /// Whether a webhook credential is stored, i.e. whether a delivery from
    /// Chargebee could be verified at all.
    pub webhook_configured: bool,
    /// The URL to paste into Chargebee's webhook settings. `None` on a host with
    /// no publicly reachable base URL — Chargebee cannot deliver to a loopback
    /// address, and showing one would send an operator to configure a webhook
    /// that silently never arrives (the shape of issue #203).
    pub webhook_url: Option<String>,
    /// Whether this company's manifest **explicitly** grants `chargebee`.
    /// Both credentials can be present and still wire no tools without it.
    pub granted: bool,
    /// Whether the `chargebee` feature is compiled into this build at all.
    pub in_build: bool,
}

/// The non-secret view of a company's PayPal connection (issue #789).
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PaypalStatus {
    /// Whether a client id is stored. Never the id itself — it is half a
    /// credential, and there is no reason to render it back.
    pub client_id_configured: bool,
    /// Whether a client secret is stored.
    pub client_secret_configured: bool,
    /// `sandbox` or `live`. Shown, because "Connected" against the wrong world
    /// is the confusion this exists to avoid.
    pub environment: String,
    /// Whether this company's manifest explicitly grants `paypal`.
    pub granted: bool,
    /// Whether the `paypal` feature is compiled into this build.
    pub in_build: bool,
}

/// Builds the billing configuration routes.
pub fn router() -> Router<AppState> {
    scoped("/billing/chargebee", get(get_billing).put(put_billing))
        .merge(scoped("/billing/chargebee/key", delete(delete_billing)))
        .merge(scoped("/billing/paypal", get(get_paypal).put(put_paypal)))
        .merge(scoped("/billing/paypal/key", delete(delete_paypal)))
}

/// Assembles the non-secret PayPal status.
async fn paypal_status_of(runtime: &CompanyRuntime) -> Result<PaypalStatus, ApiError> {
    let granted = runtime
        .store()
        .load(runtime.id())
        .await
        .ok()
        .flatten()
        .map(|record| crate::company::grants_paypal_explicit(&record.manifest.tools.allow))
        .unwrap_or(false);
    let environment = read(runtime, ENVIRONMENT_SECRET)
        .await?
        .map(|raw| PaypalEnvironment::parse(&raw))
        .unwrap_or_default();
    Ok(PaypalStatus {
        client_id_configured: read(runtime, CLIENT_ID_SECRET).await?.is_some(),
        client_secret_configured: read(runtime, CLIENT_SECRET_SECRET).await?.is_some(),
        environment: environment.as_str().to_string(),
        granted,
        in_build: cfg!(feature = "paypal"),
    })
}

/// `GET …/billing/paypal` — non-secret status only.
async fn get_paypal(company: ScopedCompany) -> Result<Json<PaypalStatus>, ApiError> {
    Ok(Json(paypal_status_of(&company.runtime).await?))
}

/// The write-only PayPal config body.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaypalConfigBody {
    /// REST app client id (write-only). Omit to leave unchanged.
    #[serde(default)]
    client_id: Option<String>,
    /// REST app secret (write-only). Omit to leave unchanged.
    #[serde(default)]
    client_secret: Option<String>,
    /// `sandbox` or `live`. Anything unrecognised stores `sandbox`.
    #[serde(default)]
    environment: Option<String>,
}

/// `PUT …/billing/paypal` — store any supplied credentials, return status.
///
/// Admin-only, like its Chargebee sibling: pointing a company at a different
/// PayPal account changes whose wallet its agents can read.
async fn put_paypal(
    company: AdminScopedCompany,
    Json(body): Json<PaypalConfigBody>,
) -> Result<Json<PaypalStatus>, ApiError> {
    let runtime = &company.runtime;
    let supplied = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };

    // Collected and then applied together — a client id stored without its
    // secret is exactly the half-configured state `write_all` exists to prevent.
    let mut writes: Vec<(&str, String)> = Vec::new();
    if let Some(client_id) = supplied(body.client_id.as_deref()) {
        writes.push((CLIENT_ID_SECRET, client_id));
    }
    if let Some(client_secret) = supplied(body.client_secret.as_deref()) {
        writes.push((CLIENT_SECRET_SECRET, client_secret));
    }
    if let Some(raw) = body.environment.as_deref() {
        // Normalised through the same parser the client uses, so an unrecognised
        // value is stored as `sandbox` rather than kept verbatim and re-parsed
        // differently somewhere else later.
        writes.push((
            ENVIRONMENT_SECRET,
            PaypalEnvironment::parse(raw).as_str().to_string(),
        ));
    }
    write_all(runtime, &writes).await?;

    Ok(Json(paypal_status_of(runtime).await?))
}

/// `DELETE …/billing/paypal/key` — clear the stored PayPal credentials.
///
/// The environment is cleared too, so a re-connect starts from the safe default
/// rather than silently inheriting `live` from a previous account.
async fn delete_paypal(company: AdminScopedCompany) -> Result<Json<PaypalStatus>, ApiError> {
    let runtime = &company.runtime;
    // Together, for the same reason as the write: a clear that dropped the
    // client id and then failed would leave a secret with no id — still "half
    // configured", and reported by `PaypalStatus` as such.
    let cleared: Vec<(&str, String)> = [CLIENT_ID_SECRET, CLIENT_SECRET_SECRET, ENVIRONMENT_SECRET]
        .into_iter()
        .map(|key| (key, String::new()))
        .collect();
    write_all(runtime, &cleared).await?;
    Ok(Json(paypal_status_of(runtime).await?))
}

/// The webhook URL for `company`, or `None` when this host has no publicly
/// reachable base URL.
///
/// Deliberately the same source as the telegram channel's — not the bind
/// address, which yields a `http://127.0.0.1:<port>/…` URL that is
/// syntactically fine and undeliverable in practice.
fn webhook_url(state: &AppState, company: &CompanyId) -> Option<String> {
    let base = state.config().public_webhook_base_url()?;
    Some(format!("{base}/hooks/{}/chargebee", company.as_ref()))
}

/// Applies a batch of credential writes so a failure part-way through cannot
/// leave a company half configured.
///
/// The module header says a half-configured company is "impossible to express by
/// accident". Sequential `set` calls, each with its own `?`, did not deliver
/// that: a store that accepted the API key and then failed on the webhook
/// credential returned an error to an operator whose key had nonetheless been
/// stored — and the pair is meaningless apart, which is the whole reason they
/// live in one place.
///
/// [`SecretStore`](crate::ports::SecretStore) has neither a transaction nor a
/// delete; `set` is its entire write surface. So atomicity is built here: every
/// key's prior value is read first, and a failure restores the ones already
/// written before the original error is returned. A key with no prior value is
/// restored to the empty string, which is how this module already spells "unset"
/// (see [`delete_billing`]) and what every read site already treats as absent.
///
/// **The rollback is best-effort, by necessity.** It is itself a sequence of
/// `set` calls against a store that has just failed one, so it can fail too.
/// What it cannot undo it logs at `error` with the key named, because an
/// operator told "save failed" who then finds a credential stored anyway has no
/// way to discover that on their own.
async fn write_all(runtime: &CompanyRuntime, writes: &[(&str, String)]) -> Result<(), ApiError> {
    // Snapshot first. Reading after a partial write would capture the value this
    // function itself just stored and roll back to it.
    let mut prior: Vec<(&str, String)> = Vec::with_capacity(writes.len());
    for (key, _) in writes {
        prior.push((
            key,
            runtime
                .secrets()
                .get(runtime.id(), key)
                .await?
                .map(|value| value.expose().to_string())
                .unwrap_or_default(),
        ));
    }

    for (index, (key, value)) in writes.iter().enumerate() {
        let Err(err) = runtime
            .secrets()
            .set(runtime.id(), key, SecretValue(value.clone()))
            .await
        else {
            continue;
        };
        for (done, before) in &prior[..index] {
            if let Err(undo) = runtime
                .secrets()
                .set(runtime.id(), done, SecretValue(before.clone()))
                .await
            {
                tracing::error!(
                    company = %runtime.id(),
                    key = done,
                    "[billing] a credential write failed and could not be rolled back; this \
                     company is now half configured: {undo}"
                );
            }
        }
        return Err(ApiError(err));
    }
    Ok(())
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

/// Assembles the non-secret status.
async fn status_of(state: &AppState, runtime: &CompanyRuntime) -> Result<BillingStatus, ApiError> {
    // The grant lives in the stored manifest, not on the runtime handle. A
    // company that cannot be loaded reports `granted: false` rather than
    // failing the whole status: the operator still needs to see what IS
    // configured, and a settings page that 500s tells them nothing.
    let granted = runtime
        .store()
        .load(runtime.id())
        .await
        .ok()
        .flatten()
        .map(|record| crate::company::grants_chargebee_explicit(&record.manifest.tools.allow))
        .unwrap_or(false);
    Ok(BillingStatus {
        api_key_configured: read(runtime, API_KEY_SECRET).await?.is_some(),
        site: read(runtime, SITE_SECRET).await?,
        webhook_configured: read(runtime, WEBHOOK_SECRET_KEY).await?.is_some(),
        webhook_url: webhook_url(state, runtime.id()),
        granted,
        in_build: cfg!(feature = "chargebee"),
    })
}

/// `GET …/billing/chargebee` — non-secret status only.
async fn get_billing(
    company: ScopedCompany,
    State(state): State<AppState>,
) -> Result<Json<BillingStatus>, ApiError> {
    Ok(Json(status_of(&state, &company.runtime).await?))
}

/// The write-only config body. Every field is optional; only fields present and
/// non-empty are applied, so the site can be corrected without re-entering the
/// key.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingConfigBody {
    /// The Chargebee API key (write-only). Omit to leave it unchanged.
    #[serde(default)]
    api_key: Option<String>,
    /// The site identifier — the `acme-test` in `acme-test.chargebee.com`.
    #[serde(default)]
    site: Option<String>,
    /// The `username:password` pair Chargebee is configured to present on its
    /// webhook deliveries (write-only). Omit to leave it unchanged.
    #[serde(default)]
    webhook_secret: Option<String>,
}

/// Normalises a site identifier an operator may paste in several shapes.
///
/// `acme-test`, `acme-test.chargebee.com` and `https://acme-test.chargebee.com/`
/// all mean the same site, and all three are what somebody actually pastes out
/// of a browser address bar. Storing the second or third produces a base URL of
/// `https://acme-test.chargebee.com.chargebee.com/api/v2`, whose failure names
/// DNS rather than the typo.
fn normalize_site(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `PUT …/billing/chargebee` — store any supplied credentials, return status.
///
/// Requires authority over the company: this key can raise invoices against
/// real customers in the company's name, so pointing it at a different
/// Chargebee site is not an ordinary member's edit.
async fn put_billing(
    company: AdminScopedCompany,
    State(state): State<AppState>,
    Json(body): Json<BillingConfigBody>,
) -> Result<Json<BillingStatus>, ApiError> {
    let runtime = &company.runtime;
    let supplied = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };

    // Collected and then applied together. Written one `?` at a time, a store
    // that took the API key and then failed on the webhook credential left the
    // key stored behind an error response — the half-configured state this
    // module's header claims cannot be reached by accident.
    let mut writes: Vec<(&str, String)> = Vec::new();
    if let Some(api_key) = supplied(body.api_key.as_deref()) {
        writes.push((API_KEY_SECRET, api_key));
    }
    if let Some(webhook_secret) = supplied(body.webhook_secret.as_deref()) {
        writes.push((WEBHOOK_SECRET_KEY, webhook_secret));
    }
    if let Some(site) = body
        .site
        .as_deref()
        .map(normalize_site)
        .filter(|s| !s.is_empty())
    {
        writes.push((SITE_SECRET, site));
    }
    write_all(runtime, &writes).await?;

    Ok(Json(status_of(&state, runtime).await?))
}

/// `DELETE …/billing/chargebee/key` — clear every stored credential.
///
/// The [`SecretStore`](crate::ports::SecretStore) port has no delete, so a
/// cleared credential is stored as the empty string; every read site treats an
/// empty value as unset (the tools fail closed, the webhook rejects).
async fn delete_billing(
    company: AdminScopedCompany,
    State(state): State<AppState>,
) -> Result<Json<BillingStatus>, ApiError> {
    let runtime = &company.runtime;
    // Together: a clear that dropped the key and then failed on the webhook
    // credential would report the integration as disconnected while leaving the
    // webhook endpoint live — the failure `clearing_removes_the_webhook_secret_
    // too_not_just_the_key` guards against, arrived at by a different route.
    let cleared: Vec<(&str, String)> = [API_KEY_SECRET, SITE_SECRET, WEBHOOK_SECRET_KEY]
        .into_iter()
        .map(|key| (key, String::new()))
        .collect();
    write_all(runtime, &cleared).await?;
    Ok(Json(status_of(&state, runtime).await?))
}

#[cfg(test)]
#[path = "billing_tests.rs"]
mod tests;
