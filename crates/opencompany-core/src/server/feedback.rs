//! Operator HTTP surface for the feedback loop.
//!
//! `POST /api/v1/companies/{id}/feedback` (and the single-company alias
//! `POST /api/v1/company/feedback`) captures a feedback item and runs the
//! scrub-then-preview gate. The response never leaks a blocked value: a scrub
//! abort returns `{ blocked: true, reason }`, a `preview` returns the byte-exact
//! final body, and a satisfied consent returns the filed issue URL (or a
//! prefilled manual link when no token is configured). `destination` reports
//! where the report actually went.
//!
//! The matching `GET` routes list this company's reports for the console, as
//! the [`FeedbackSummary`] projection — never the raw item, whose
//! `operator_words` are local-only.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;

use crate::AppState;
use crate::company::runtime::CompanyRuntime;
use crate::error::OpenCompanyError;
use crate::feedback::service::FeedbackResponse;
use crate::feedback::types::{FeedbackInput, FeedbackSummary};
use crate::ports::types::CompanyId;
use crate::server::error::ApiError;
use crate::server::platform_auth::{CompanyAuth, authorize_address, refuse_until_password_changed};

/// Builds the feedback route fragment, merged into the main router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/companies/{id}/feedback", post(submit).get(list))
        .route(
            "/api/v1/company/feedback",
            post(submit_single).get(list_single),
        )
}

/// The feedback submission body: the capture input plus a `preview` flag and an
/// optional `item_id` confirming a previewed item.
#[derive(Debug, Deserialize)]
struct FeedbackRequest {
    /// The capture fields (category, note, work_ref, template).
    #[serde(flatten)]
    input: FeedbackInput,
    /// When true, return the exact final body instead of filing.
    #[serde(default)]
    preview: bool,
    /// When confirming (Send after Preview), the previewed item's id — finalize
    /// that item instead of capturing a second one.
    #[serde(default)]
    item_id: Option<String>,
}

/// Resolves a company runtime by id. Shared with the board routes in
/// [`super::feedback_board`], which address companies exactly the same way.
pub(crate) fn lookup(state: &AppState, id: &str) -> Result<Arc<CompanyRuntime>, ApiError> {
    state
        .registry()
        .get(&CompanyId::new(id))
        .ok_or_else(|| ApiError(OpenCompanyError::CompanyNotFound(id.to_string())))
}

/// The sole company on a single-company host, for the `/company/...` aliases.
pub(crate) fn sole(state: &AppState) -> Result<Arc<CompanyRuntime>, ApiError> {
    state.registry().sole().ok_or_else(|| {
        ApiError(OpenCompanyError::CompanyNotFound(
            "single-company".to_string(),
        ))
    })
}

async fn run(
    runtime: Arc<CompanyRuntime>,
    body: FeedbackRequest,
) -> Result<Json<FeedbackResponse>, ApiError> {
    runtime.ensure_running().await?;
    let response = runtime
        .submit_feedback(body.input, body.preview, body.item_id)
        .await?;
    Ok(Json(response))
}

/// `POST /api/v1/companies/{id}/feedback`.
///
/// A per-company route: like every other `/companies/{id}/…` handler it takes
/// platform-or-operator auth and enforces tenant ownership, so one tenant can
/// never file feedback (or trigger issue-filing) against another's company.
async fn submit(
    CompanyAuth(auth): CompanyAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<FeedbackResponse>, crate::server::Rejection> {
    let company = CompanyId::new(&id);
    if let Some(resp) = authorize_address(&state, &auth, &company) {
        return Err(resp.into());
    }
    let runtime = lookup(&state, &id)?;
    run(runtime, body)
        .await
        .map_err(|error| IntoResponse::into_response(error).into())
}

/// `POST /api/v1/company/feedback` (single-company alias).
async fn submit_single(
    CompanyAuth(auth): CompanyAuth,
    State(state): State<AppState>,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<FeedbackResponse>, crate::server::Rejection> {
    let runtime = sole(&state)?;
    // The sole company IS the addressed one, so the principal is checked
    // against it exactly as on the `{id}` form.
    if let Some(resp) = authorize_address(&state, &auth, runtime.id()) {
        return Err(resp.into());
    }
    if let Some(resp) = refuse_until_password_changed(&auth) {
        return Err(resp.into());
    }
    run(runtime, body)
        .await
        .map_err(|error| IntoResponse::into_response(error).into())
}

/// `GET /api/v1/companies/{id}/feedback` — this company's reports, newest first.
///
/// Returns the [`FeedbackSummary`] projection, never the raw item: the
/// operator's own words are local-only and must not cross this boundary.
async fn list(
    CompanyAuth(auth): CompanyAuth,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<FeedbackSummary>>, crate::server::Rejection> {
    let company = CompanyId::new(&id);
    if let Some(resp) = authorize_address(&state, &auth, &company) {
        return Err(resp.into());
    }
    let runtime = lookup(&state, &id)?;
    runtime
        .list_feedback()
        .await
        .map(Json)
        .map_err(|e| ApiError(e).into_response().into())
}

/// `GET /api/v1/company/feedback` (single-company alias).
async fn list_single(
    CompanyAuth(auth): CompanyAuth,
    State(state): State<AppState>,
) -> Result<Json<Vec<FeedbackSummary>>, crate::server::Rejection> {
    let runtime = sole(&state)?;
    if let Some(resp) = authorize_address(&state, &auth, runtime.id()) {
        return Err(resp.into());
    }
    runtime
        .list_feedback()
        .await
        .map(Json)
        .map_err(|e| ApiError(e).into_response().into())
}

#[cfg(test)]
#[path = "feedback_tests.rs"]
mod tests;
