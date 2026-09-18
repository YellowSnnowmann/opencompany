//! The tool-catalog read surface: `GET {scope}/tools/catalog`.
//!
//! One list of everything this company can grant an agent — built-in tool
//! families, per-tenant MCP servers, and Composio toolkits — in a single
//! vocabulary, each row carrying the exact grant token an operator would write.
//!
//! Read-only, and open to any member who may address the company. Nothing here
//! decides anything: the catalog is a projection of the manifest through the
//! same matcher the roster build uses (see [`crate::company::tool_catalog`]), so
//! this route can only ever describe grants the gate already honours.

use axum::Json;
use axum::Router;
use axum::routing::get;
use serde::Serialize;

use crate::AppState;
use crate::company::tool_catalog::{CatalogEntry, catalog};
use crate::error::OpenCompanyError;
use crate::server::error::ApiError;
use crate::server::ops::{ScopedCompany, scoped};

/// Builds the tool-catalog route fragment.
pub fn router() -> Router<AppState> {
    scoped("/tools/catalog", get(get_catalog))
}

/// The company's tool catalog as the console renders it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolCatalogDto {
    /// The company-wide `[tools].allow` ceiling, echoed so a console can render
    /// the catalog beside the grant that produced each row's `granted` flag
    /// without a second request.
    company_allow: Vec<String>,
    /// Every grantable entry, built-ins first.
    entries: Vec<CatalogEntry>,
}

async fn get_catalog(company: ScopedCompany) -> Result<Json<ToolCatalogDto>, ApiError> {
    let record = company
        .runtime
        .store()
        .load(company.id())
        .await?
        .ok_or_else(|| OpenCompanyError::CompanyNotFound(company.id().to_string()))?;

    Ok(Json(ToolCatalogDto {
        company_allow: record.manifest.tools.allow.clone(),
        entries: catalog(&record.manifest),
    }))
}

#[cfg(test)]
#[path = "tool_catalog_tests.rs"]
mod tests;
