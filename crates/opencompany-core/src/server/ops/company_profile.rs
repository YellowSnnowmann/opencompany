//! `PATCH {scope}` — the conscious naming step of the account-activation
//! funnel (issue #1844): sets the company's display name and stamps
//! [`CompanyRecord::name_confirmed`], the first of the three activation steps
//! [`crate::company::activation`] derives.
//!
//! ## Why this writes the manifest directly, unlike every other console write
//!
//! Every other operator write in `ops` lands in an overlay field on
//! [`CompanyRecord`] — never `record.manifest` — because a rebuild
//! re-persists the manifest from the seed on every boot (`RuntimeBuilder::build`),
//! which would silently wipe a direct manifest write on the next redeploy. This
//! route is the one deliberate exception: the display name genuinely **is**
//! `[company].name`, the same field `company.toml` seeds, so there is no
//! separate "confirmed name" value to keep in an overlay — writing the
//! manifest field IS the write. What makes this safe rather than the same trap
//! every overlay doc warns about is `RuntimeBuilder::build`'s own carry-forward
//! (issue #1844, beside `merge_enabled_workflows`): once `name_confirmed` is
//! `true`, a rebuild copies the *existing record's* name back onto the freshly
//! parsed seed manifest before either is used, exactly the way `[workflows].enabled`
//! is merged instead of re-derived. Before confirmation, no such carry exists —
//! the seed name is a provisional default and stays seed-authoritative, which is
//! the correct behaviour for an operator still editing `company.toml` pre-launch.
//!
//! ## Attribution and authority
//!
//! Admin-only, like the sibling [`policy`](super::policy) and per-teammate
//! budget writes — renaming the company's own identity is at least as sharp a
//! boundary as either. [`ScopedCompany`] resolves addressing and the
//! temporary-password gate; [`require_admin`] adds the authority check.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::patch;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::OpenCompanyError;
use crate::ports::store::company_write_lock;
use crate::ports::types::{CompanyEvent, CompanyRecord, OnboardingStep};
use crate::server::error::ApiError;
use crate::server::graphql::auth::MaybePeer;
use crate::server::ops::{ScopedCompany, scoped};
use crate::server::users::admin::require_admin;

/// Builds the company-profile route fragment.
pub fn router() -> Router<AppState> {
    scoped("", patch(patch_company))
}

/// The `PATCH {scope}` request body. `name` is the only field this route
/// accepts today — a company's identity has exactly one console-writable
/// piece, and a body that named anything else would be silently ignored,
/// which is worse than a route that simply does not exist yet for it.
#[derive(Debug, Deserialize)]
struct PatchCompanyInput {
    #[serde(default)]
    name: Option<String>,
}

/// Max length of the company's display name, matching the convention set by
/// `MAX_WORKFLOW_NAME_LEN` (`src/company/workflow_create.rs`).
///
/// PR #1875 review finding: this name is embedded verbatim into every
/// agent's system prompt (`persona_prompt`, `src/company/prompt.rs`), with no
/// bound before this — an accidental pasted document is enough to inflate
/// every model request until context limits are exceeded and workflows stop
/// running.
const COMPANY_NAME_MAX_CHARS: usize = 200;

/// What the console gets back after a successful rename.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchCompanyDto {
    name: String,
    name_confirmed: bool,
}

fn refusal(message: &str) -> Response {
    (
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        message.to_string(),
    )
        .into_response()
}

/// `PATCH {scope}` — set the company's display name and stamp
/// [`CompanyRecord::name_confirmed`].
///
/// Idempotent in the sense that matters for its own contract: renaming an
/// already-confirmed company (any operator, any time — this is an ordinary
/// rename, not only the first-run step) still succeeds and keeps
/// `name_confirmed` set. What is *not* re-emitted on a later rename is the
/// [`CompanyEvent::OnboardingStepCompleted`] audit line — see the `first`
/// guard below — because that event marks the funnel step's first completion,
/// not every subsequent edit.
async fn patch_company(
    company: ScopedCompany,
    State(state): State<AppState>,
    headers: HeaderMap,
    MaybePeer(peer): MaybePeer,
    Json(body): Json<PatchCompanyInput>,
) -> Result<Json<PatchCompanyDto>, crate::server::Rejection> {
    require_admin(&headers, &state, &company.runtime, peer).await?;

    let Some(name) = body.name else {
        return Err(refusal("Nothing to set. Send `name`.").into());
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        // Same rule and the same words `CompanyManifest::validate` uses for
        // `[company].name` — a console rejection and a `company.toml`
        // rejection must not describe the same requirement differently.
        return Err(refusal("`name` cannot be empty — give your company a name.").into());
    }
    if name.chars().count() > COMPANY_NAME_MAX_CHARS {
        return Err(refusal(&format!(
            "a company name can be at most {COMPANY_NAME_MAX_CHARS} characters."
        ))
        .into());
    }

    let write_lock = company_write_lock(company.id());
    let _lock = write_lock.lock().await;

    let mut record = load_record(&company).await?;
    let first = !record.name_confirmed;
    record.manifest.company.name = name.clone();
    record.name_confirmed = true;

    company.runtime.store().save(&record).await?;

    // Only the transition into confirmation is journaled — see the handler's
    // own doc comment. Best-effort: the record write above already landed, so
    // a journal failure here never leaves the name unset, only the audit
    // trail thinner (the same trade-off `compute_and_latch` makes for
    // `OnboardingCompleted`).
    if first
        && let Err(err) = company
            .runtime
            .events()
            .append(
                company.id(),
                CompanyEvent::OnboardingStepCompleted {
                    step: OnboardingStep::NameConfirmed,
                },
            )
            .await
    {
        tracing::warn!(
            company = %company.id(),
            %err,
            "company confirmed its name but the OnboardingStepCompleted audit event could not be journaled"
        );
    }

    Ok(Json(PatchCompanyDto {
        name: record.manifest.company.name,
        name_confirmed: record.name_confirmed,
    }))
}

async fn load_record(company: &ScopedCompany) -> Result<CompanyRecord, crate::server::Rejection> {
    company
        .runtime
        .store()
        .load(company.id())
        .await?
        .ok_or_else(|| {
            ApiError(OpenCompanyError::CompanyNotFound(company.id().to_string()))
                .into_response()
                .into()
        })
}

#[cfg(test)]
#[path = "company_profile_tests.rs"]
mod tests;
