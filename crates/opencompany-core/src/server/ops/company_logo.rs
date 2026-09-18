//! Company-logo settings, persisted in the company manifest.

use axum::routing::put;
use axum::{Json, Router};
use serde::Deserialize;

use crate::AppState;
use crate::error::OpenCompanyError;
use crate::ports::store::company_write_lock;
use crate::runtime::CompanyStatus;
use crate::server::error::ApiError;
use crate::server::ops::{AdminScopedCompany, scoped};

const COMPANY_LOGO_MAX_CHARS: usize = 1_000_000;
const ALLOWED_IMAGE_MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompanyLogoBody {
    #[serde(default)]
    logo_url: Option<String>,
}

/// Builds both `PUT /api/v1/company/logo` addressing variants.
pub fn router() -> Router<AppState> {
    scoped("/logo", put(put_logo))
}

fn invalid_logo(message: impl Into<String>) -> ApiError {
    ApiError(OpenCompanyError::InvalidRequest(message.into()))
}

/// Accepts only bounded, self-contained image data URLs. `None` clears the logo.
fn company_logo_value(value: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() > COMPANY_LOGO_MAX_CHARS {
        return Err(invalid_logo(format!(
            "company logo exceeds the {COMPANY_LOGO_MAX_CHARS}-character limit"
        )));
    }

    let (header, payload) = value
        .split_once(',')
        .ok_or_else(|| invalid_logo("company logo must be a base64 image data URL"))?;
    let mime = header
        .strip_prefix("data:")
        .and_then(|header| header.strip_suffix(";base64"))
        .filter(|mime| ALLOWED_IMAGE_MIMES.contains(mime))
        .ok_or_else(|| {
            invalid_logo("company logo must be a base64 PNG, JPEG, GIF, or WebP data URL")
        })?;
    let padding = payload.len() - payload.trim_end_matches('=').len();
    if payload.is_empty()
        || payload.len() % 4 != 0
        || padding > 2
        || !payload[..payload.len() - padding]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    {
        return Err(invalid_logo(format!(
            "company logo contains invalid base64 data for {mime}"
        )));
    }

    Ok(Some(value))
}

/// `PUT …/logo` — replace or clear the company logo and return fresh status.
///
/// PR #1875 review finding: this is a load-modify-save cycle over the whole
/// [`CompanyRecord`] manifest, exactly the shape `company_write_lock` exists
/// to serialize (see its own doc comment) — every sibling console write
/// (`company_profile::patch_company`, `team.rs`, `team_agent.rs`,
/// `tool_grants.rs`, `policy.rs`, `setup.rs`) already takes it. This one did
/// not, so a rename landing between this handler's `load` and `save` was
/// silently reverted: this handler's `save` writes back the whole manifest it
/// loaded, including the pre-rename `name`, even though it only meant to
/// change `logo_url`.
async fn put_logo(
    company: AdminScopedCompany,
    Json(body): Json<CompanyLogoBody>,
) -> Result<Json<CompanyStatus>, ApiError> {
    let logo_url = company_logo_value(body.logo_url)?;

    let write_lock = company_write_lock(company.id());
    let _lock = write_lock.lock().await;

    let mut record = company
        .runtime
        .store()
        .load(company.id())
        .await?
        .ok_or_else(|| OpenCompanyError::CompanyNotFound(company.id().to_string()))?;
    record.manifest.company.logo_url = logo_url;
    company.runtime.store().save(&record).await?;
    Ok(Json(company.runtime.status().await?))
}

#[cfg(test)]
#[path = "company_logo_tests.rs"]
mod tests;
