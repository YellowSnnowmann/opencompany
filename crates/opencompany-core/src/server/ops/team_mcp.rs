//! What one teammate can actually call on this company's MCP servers.
//!
//! `GET {scope}/team/{agent_id}/mcp/permissions` — one host-resolved read
//! covering every configured server.
//!
//! Covers declared servers only. A directory install carries its policy under
//! its own key; its per-agent rules are authored and read through the registry
//! route. Writes go to the per-server policy route with `?agent=`.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::company::mcp::{McpServerDecl, resolve_effective};
use crate::company::runtime::CompanyRuntime;
use crate::error::OpenCompanyError;
use crate::mcp::policy as mcp_policy;
use crate::runtime::tools::grants_cover_server;
use crate::server::error::ApiError;
use crate::server::ops::mcp::manifest_servers;
use crate::server::ops::mcp_tool_policy::{ToolPolicyRowDto, tool_policy_dto};
use crate::server::ops::{ScopedCompany, scoped};

/// The teammate a permissions read is about.
#[derive(Debug, Deserialize)]
pub struct AgentPath {
    pub agent_id: String,
}

/// One configured server, as it stands for this teammate.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentServerPermissionsDto {
    pub server: String,
    /// Whether this teammate's grants reach the server at all.
    pub reached: bool,
    /// The grant that would make it reachable, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_needed: Option<String>,
    pub enabled: bool,
    /// Every tool a decision can be stated about, resolved for this teammate.
    /// Empty when the document is unreadable.
    pub tools: Vec<ToolPolicyRowDto>,
    /// When discovery last succeeded, if ever. `0` reads as never.
    pub discovered_at_millis: u64,
    /// Whether every tool this server is *known* to offer is refused this
    /// teammate. Never true for a server no probe has reached.
    pub fully_refused: bool,
    /// Whether this server's stored document could not be read. Degrades this
    /// block only.
    pub unreadable: bool,
}

/// A tool name more than one reached server offers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedToolNameDto {
    pub tool: String,
    pub servers: Vec<String>,
}

/// One teammate's whole MCP picture.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMcpPermissionsDto {
    pub agent: String,
    /// The grant this teammate asks for: `null` inherits the company's standard
    /// grant, `[]` is a no-tools grant, `[globs]` narrows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested: Option<Vec<String>>,
    /// The grants this teammate is actually built with.
    pub effective_grants: Vec<String>,
    pub servers: Vec<AgentServerPermissionsDto>,
    /// Tool names reachable on more than one server this teammate holds.
    /// Matching is on the name, not the capability.
    pub shared_tool_names: Vec<SharedToolNameDto>,
    /// Whether approval parking is live on this host. `false` means a
    /// `needs_approval` mode behaves as allow.
    ///
    /// Read from [`crate::policy::approvals_park`], which asks the policy the
    /// roster is actually built from.
    pub approvals_park: bool,
}

/// Builds the per-teammate MCP permissions route fragment.
pub fn router() -> Router<AppState> {
    scoped("/team/{agent_id}/mcp/permissions", get(read_permissions))
}

/// Every declared server this company has configured.
async fn configured_servers(runtime: &CompanyRuntime) -> Result<Vec<McpServerDecl>, ApiError> {
    let manifest = manifest_servers(runtime).await?;
    resolve_effective(
        runtime.id(),
        runtime.default_mcp_servers(),
        &manifest,
        runtime.secrets().as_ref(),
    )
    .await
    .map_err(ApiError)
}

async fn read_permissions(
    company: ScopedCompany,
    Path(AgentPath { agent_id }): Path<AgentPath>,
) -> Response {
    let runtime = company.runtime.as_ref();
    let agent_id = agent_id.trim().to_string();

    let record = match runtime.store().load(company.id()).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return ApiError(OpenCompanyError::CompanyNotFound(company.id().to_string()))
                .into_response();
        }
        Err(err) => return ApiError(err).into_response(),
    };
    if !record
        .effective_agents()
        .iter()
        .any(|agent| agent.id == agent_id)
    {
        return agent_not_found(&agent_id);
    }

    let grants = super::team_agent::effective_grants(&record, &agent_id);
    let decls = match configured_servers(runtime).await {
        Ok(decls) => decls,
        Err(err) => return err.into_response(),
    };

    let mut servers = Vec::with_capacity(decls.len());
    for decl in &decls {
        // Reach, exactly as `registry_for_agent` decides it.
        let reached = decl.enabled && grants_cover_server(&grants, &decl.name);
        let stored = mcp_policy::load_tool_policies_strict(
            runtime.id(),
            runtime.secrets().as_ref(),
            &mcp_policy::tool_policies_key(&decl.name),
        )
        .await;
        let (tools, fully_refused, unreadable) = match stored {
            Err(_) => (Vec::new(), false, true),
            Ok(stored) => {
                let policies = mcp_policy::effective_policies(
                    &decl.read_only_tools,
                    mcp_policy::StoredPolicies::Stored(stored.unwrap_or_default()),
                );
                let dto =
                    tool_policy_dto(&decl.name, &policies, &decl.tool_inventory, Some(&agent_id));
                let mut resolved = decl.clone();
                resolved.tool_policies = policies;
                let refused = mcp_policy::every_known_tool_refused(&resolved, &agent_id);
                (dto.tools, refused, false)
            }
        };
        servers.push(AgentServerPermissionsDto {
            server: decl.name.clone(),
            reached,
            grant_needed: (!reached).then(|| format!("mcp:{}", decl.name)),
            enabled: decl.enabled,
            tools,
            discovered_at_millis: decl.tool_inventory.discovered_at_millis,
            fully_refused,
            unreadable,
        });
    }

    let shared_tool_names = shared_names(&servers);
    let approvals_park = crate::policy::approvals_park(&runtime.approval_gate.policy());

    Json(AgentMcpPermissionsDto {
        requested: super::team_agent::requested_grants(&record, &agent_id),
        agent: agent_id,
        effective_grants: grants,
        servers,
        shared_tool_names,
        approvals_park,
    })
    .into_response()
}

fn agent_not_found(agent_id: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": format!("no teammate named `{agent_id}` on this company's roster"),
            "code": "not_found",
        })),
    )
        .into_response()
}

/// Tool names offered by more than one server this teammate reaches.
fn shared_names(servers: &[AgentServerPermissionsDto]) -> Vec<SharedToolNameDto> {
    let mut by_tool: std::collections::BTreeMap<&str, Vec<String>> =
        std::collections::BTreeMap::new();
    for server in servers.iter().filter(|s| s.reached && s.enabled) {
        for row in &server.tools {
            by_tool
                .entry(row.tool.as_str())
                .or_default()
                .push(server.server.clone());
        }
    }
    by_tool
        .into_iter()
        .filter(|(_, servers)| servers.len() > 1)
        .map(|(tool, servers)| SharedToolNameDto {
            tool: tool.to_string(),
            servers,
        })
        .collect()
}

#[cfg(test)]
#[path = "team_mcp_tests.rs"]
mod tests;
