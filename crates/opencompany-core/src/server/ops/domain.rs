//! Custom domain + DNS verification.
//!
//! `PUT …/domain` sets the domain and returns a [`DomainStatus`] carrying the
//! records the operator must add (persisted as JSON at
//! [`DOMAIN_KEY`](super::DOMAIN_KEY)). `POST …/domain/verify` runs server-side
//! DNS lookups through the injected [`DnsResolver`](crate::company::dns::DnsResolver)
//! and returns the updated status. Without an injected resolver (default build /
//! no `dns` feature) verify is "not wired yet" (404).
//!
//! The domain config is non-secret — it shares the secret store only because
//! that is the per-company durable key/value seam.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::AppState;
use crate::company::dns::{self, DomainStatus};
use crate::company::runtime::CompanyRuntime;
use crate::error::OpenCompanyError;
use crate::ports::types::SecretValue;
use crate::server::error::ApiError;
use crate::server::ops::{AdminScopedCompany, DOMAIN_KEY, ScopedCompany, scoped};

/// Builds the domain route fragment.
pub fn router() -> Router<AppState> {
    scoped("/domain", get(get_domain).put(put_domain))
        .merge(scoped("/domain/verify", post(verify_domain)))
}

/// The set-domain request body.
#[derive(Debug, Deserialize)]
struct SetDomain {
    /// The custom domain to configure.
    domain: String,
}

/// Persists a fresh domain status and returns it.
async fn store_domain(
    runtime: Arc<CompanyRuntime>,
    domain: &str,
) -> Result<Json<DomainStatus>, ApiError> {
    let status = DomainStatus::fresh(domain);
    persist(&runtime, &status).await?;
    Ok(Json(status))
}

/// Writes the status JSON to the secret store.
async fn persist(runtime: &CompanyRuntime, status: &DomainStatus) -> Result<(), ApiError> {
    let json = serde_json::to_string(status)?;
    runtime
        .secrets()
        .set(runtime.id(), DOMAIN_KEY, SecretValue(json))
        .await?;
    Ok(())
}

/// Loads the stored domain config, if any.
///
/// `pub(crate)` and carrying the crate error rather than [`ApiError`] so the
/// GraphQL resolver for `Company.domain` can serve the same read instead of
/// keeping a second copy of it (issue #316) — an `ApiError` is an HTTP
/// response, and a resolver has nowhere to put one.
pub(crate) async fn load_domain(
    runtime: &CompanyRuntime,
) -> Result<Option<DomainStatus>, OpenCompanyError> {
    let Some(value) = runtime.secrets().get(runtime.id(), DOMAIN_KEY).await? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(value.expose())?))
}

/// `GET …/domain` (both scope forms) — the stored domain status, or `null`.
///
/// `ScopedCompany`, not `AdminScopedCompany`, and deliberately: the admin line
/// on this plane is the company's outward *identity*, which is the write. The
/// read carries no credential — a domain, its published DNS records, and
/// whether they resolved — so it stays open to any member, exactly as
/// `POST …/domain/verify` and `GET …/hosting` already are
/// (`docs/modules/server/authority.md`). Making it admin-only would `403` a
/// member on the Settings screen while the same domain, its records and its
/// verified flag stayed readable to them over GraphQL as `Company.domain`.
///
/// "The same", not "identical". Both surfaces read through [`load_domain`], so
/// neither can be staler than the other, but they do not answer the same
/// detail: this route returns the whole [`DomainStatus`], and `DomainStatusGql`
/// projects only `domain`, `verified` and `records`. The per-record `checks`
/// from the last verify pass are REST-only.
///
/// `null` rather than a synthesized empty status, matching the nullability
/// `Company.domain` already reports.
async fn get_domain(company: ScopedCompany) -> Result<Json<Option<DomainStatus>>, ApiError> {
    Ok(Json(load_domain(&company.runtime).await?))
}

/// `PUT …/domain` (both scope forms).
/// Requires authority over the company (issue #403) — the domain is the
/// company's mail identity. `POST …/domain/verify` deliberately stays open: it
/// re-checks DNS for a domain only an admin could have set, and changes nothing
/// a member could not already read.
async fn put_domain(
    company: AdminScopedCompany,
    Json(body): Json<SetDomain>,
) -> Result<Json<DomainStatus>, ApiError> {
    store_domain(company.runtime, &body.domain).await
}

/// Runs a verification pass through the injected resolver and persists it.
async fn run_verify(
    state: &AppState,
    runtime: Arc<CompanyRuntime>,
) -> Result<Json<DomainStatus>, crate::server::Rejection> {
    use axum::response::IntoResponse;
    let Some(resolver) = state.connections().dns.clone() else {
        return Err(super::not_wired("domain verification").into());
    };
    let stored = load_domain(&runtime).await?;
    let Some(stored) = stored else {
        return Err(ApiError(crate::error::OpenCompanyError::InvalidRequest(
            "no domain configured".to_string(),
        ))
        .into_response()
        .into());
    };
    let status = dns::verify(&stored.domain, resolver.as_ref()).await?;
    persist(&runtime, &status).await?;
    Ok(Json(status))
}

/// `POST …/domain/verify` (both scope forms).
async fn verify_domain(
    company: ScopedCompany,
    State(state): State<AppState>,
) -> Result<Json<DomainStatus>, crate::server::Rejection> {
    run_verify(&state, company.runtime).await
}
