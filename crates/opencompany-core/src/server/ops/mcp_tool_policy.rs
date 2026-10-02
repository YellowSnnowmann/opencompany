//! Per-tool permission routes for MCP servers.
//!
//! Ungated, like the `/mcp/servers` management routes this sits beside: editing
//! what a tool is allowed to do is configuration, not a runtime capability, and
//! a build without the harness must still be able to express it. On such a
//! build no probe runs, so the inventory is empty and a read honestly returns
//! only the rows an operator already decided about.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::company::mcp::resolve_effective;
use crate::company::mcp_policy;
use crate::company::runtime::CompanyRuntime;
use crate::server::error::ApiError;
use crate::server::ops::mcp::{NamePath, manifest_servers};
use crate::server::ops::{AdminScopedCompany, ScopedCompany, scoped};

/// One tool's permission row as the console renders it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicyRowDto {
    pub tool: String,
    /// The tier this row is grouped and defaulted under.
    pub effective_tier: crate::company::mcp_policy::ToolTier,
    /// What discovery suggested, when it reached this tool. May legitimately
    /// disagree with `effectiveTier` — an operator can reclassify a row, and the
    /// console must render that without looking broken.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_tier: Option<crate::company::mcp_policy::ToolTier>,
    /// The mode enforced in this row's scope — the company's when the read is
    /// company-wide, this teammate's when it is scoped to one.
    pub mode: crate::company::mcp_policy::ApprovalMode,
    /// Whether an operator decided anything about this row, as opposed to it
    /// inheriting. Derived here; never stored.
    pub is_override: bool,
    /// Which rule decided [`Self::mode`]. Host-resolved — one of its values names
    /// a discarded per-agent setting, which no client can derive from the mode.
    pub source: crate::company::mcp_policy::PolicySource,
    /// In an agent-scoped read, this teammate's stored mode — present even when
    /// the narrow-only clamp discarded it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_mode: Option<crate::company::mcp_policy::ApprovalMode>,
    /// The teammates whose resolved mode for this tool differs from the company's.
    pub differing_agents: Vec<String>,
}

/// One tier's bulk default, and whether an operator actually wrote it.
///
/// `stored` is what stops the console presenting a nominal value as a live one.
/// An unstored tier carries [`default_mode_for`]'s nominal mode for reference,
/// but `resolve_policy` does not apply it to a merely-suggested tier — so a row
/// under that tier reads `NeedsApproval` while this says `AlwaysAllow`. Naming
/// which of the two an operator is looking at is the difference between
/// confirming a displayed value and unknowingly granting a bulk allow.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierDefaultDto {
    pub mode: crate::company::mcp_policy::ApprovalMode,
    pub stored: bool,
}

/// A server's whole permission document as the console reads it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicyDto {
    pub server: String,
    /// The teammate this read is scoped to, or absent for the company document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Every tier's bulk default — **total**, so the console never ships its own
    /// copy of the fallbacks and cannot drift from them.
    pub tier_defaults: std::collections::BTreeMap<String, TierDefaultDto>,
    pub tools: Vec<ToolPolicyRowDto>,
    /// When discovery last succeeded, if ever. `0` reads as never.
    pub discovered_at_millis: u64,
    /// The rebuild reminder, on a mutating response only. Absent on a read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl ToolPolicyDto {
    /// Attaches the next-turn reminder to a mutating response.
    fn with_note(mut self) -> Self {
        self.note = Some(super::mcp::NEXT_TURN_NOTE.to_string());
        self
    }
}

/// Renders a resolved policy document for one server, company-wide or as it
/// stands for one teammate.
///
/// The tier grouping is identical in either scope — tiers are never per-agent —
/// and only the mode, the source and the stored per-agent value change.
pub fn tool_policy_dto(
    server: &str,
    policies: &crate::company::mcp_policy::McpToolPolicies,
    inventory: &crate::company::mcp_policy::McpToolInventory,
    agent: Option<&str>,
) -> ToolPolicyDto {
    use crate::company::mcp_policy::{
        PolicySource, ToolTier, agent_policy_tool_names, default_mode_for, differing_agents,
        policy_tool_names, resolve_policy, resolve_policy_for_agent,
    };

    let tier_defaults = ToolTier::ALL
        .iter()
        .map(|tier| {
            let stored = policies.tier_defaults.get(tier).copied();
            let dto = TierDefaultDto {
                mode: stored.unwrap_or_else(|| default_mode_for(*tier)),
                stored: stored.is_some(),
            };
            (tier.as_str().to_string(), dto)
        })
        .collect();

    // An agent-scoped read enumerates the wider set: a teammate can hold a rule
    // about a tool no probe reached and no company override names.
    let names: Vec<String> = match agent {
        Some(agent) => agent_policy_tool_names(policies, inventory, agent).collect(),
        None => policy_tool_names(policies, inventory).collect(),
    };

    let tools = names
        .into_iter()
        .map(|tool| {
            let suggested = inventory.suggested(&tool);
            let differing = differing_agents(policies, inventory, &tool);
            match agent {
                Some(agent) => {
                    let resolved = resolve_policy_for_agent(policies, agent, &tool, suggested);
                    ToolPolicyRowDto {
                        tool,
                        effective_tier: resolved.server.tier,
                        suggested_tier: suggested,
                        mode: resolved.mode,
                        is_override: resolved.server.is_override,
                        source: resolved.source,
                        agent_mode: resolved.asked,
                        differing_agents: differing,
                    }
                }
                None => {
                    let resolved = resolve_policy(policies, &tool, suggested);
                    ToolPolicyRowDto {
                        tool,
                        effective_tier: resolved.tier,
                        suggested_tier: suggested,
                        mode: resolved.mode,
                        is_override: resolved.is_override,
                        source: PolicySource::for_server(resolved.is_override),
                        agent_mode: None,
                        differing_agents: differing,
                    }
                }
            }
        })
        .collect();

    ToolPolicyDto {
        server: server.to_string(),
        agent: agent.map(str::to_string),
        tier_defaults,
        tools,
        discovered_at_millis: inventory.discovered_at_millis,
        note: None,
    }
}

/// The `?agent=` lens a policy read or write is scoped to.
#[derive(Debug, Default, Deserialize)]
pub struct AgentScope {
    #[serde(default)]
    pub agent: Option<String>,
}

impl AgentScope {
    /// The teammate this request is about, or `None` for the company document.
    /// A blank value is the company document, not a teammate named "".
    pub fn agent(&self) -> Option<&str> {
        self.agent
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }
}

/// The body of a tool-policy PUT.
///
/// **A partial merge, at two levels.** A field this body does not name is left
/// as stored; an entry naming no field at all is the reset for that tool. The
/// second level is the one that matters: `{tool, mode}` sets the mode and
/// leaves any tier reclassification intact, because replace-semantics would
/// make every press of a three-way control silently revert the operator's tier
/// decision.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutToolPolicy {
    /// A named tier is set; a tier named with `null` is cleared back to unset.
    /// Same shape as a `tools` entry naming neither field — naming a thing is
    /// how it is changed, and naming it as nothing is how it is undone.
    #[serde(default)]
    pub tier_defaults:
        Option<std::collections::HashMap<String, Option<crate::company::mcp_policy::ApprovalMode>>>,
    #[serde(default)]
    pub tools: Option<Vec<PutToolPolicyEntry>>,
}

/// One tool's patch. Absent is "leave alone"; an entry with neither field is
/// the reset.
///
/// The wire form and the stored form are different objects, deliberately.
/// `{tool}` is a meaningful instruction — reset this row — while the
/// `ToolPolicy` it produces, with both fields `None`, is the meaningless result
/// that gets pruned. One word covering both is how this contract reads as
/// self-contradictory.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutToolPolicyEntry {
    pub tool: String,
    #[serde(default)]
    pub tier: Option<crate::company::mcp_policy::ToolTier>,
    #[serde(default)]
    pub mode: Option<crate::company::mcp_policy::ApprovalMode>,
}

/// Applies a PUT body to a stored document, returning the merged result or the
/// operator-facing reason it cannot be applied.
///
/// `agent` picks which half of the document the `tools` entries land in. In an
/// agent scope the merge is one level shallower — a teammate has modes, not
/// tiers — and a per-agent tier is refused in either shape it could take.
pub fn apply_tool_policy_patch(
    mut stored: crate::company::mcp_policy::McpToolPolicies,
    patch: PutToolPolicy,
    agent: Option<&str>,
) -> Result<crate::company::mcp_policy::McpToolPolicies, String> {
    use crate::company::mcp_policy::ToolTier;

    if patch.tier_defaults.is_none() && patch.tools.is_none() {
        return Err(
            "name `tierDefaults`, `tools`, or both — a body naming neither changes nothing."
                .to_string(),
        );
    }

    if let Some(agent) = agent {
        if patch.tier_defaults.is_some() {
            return Err(
                "tier defaults are set for everyone, not per teammate — write them without \
                 `?agent=`."
                    .to_string(),
            );
        }
        if let Some(entries) = &patch.tools
            && entries.iter().any(|entry| entry.tier.is_some())
        {
            return Err(
                "a tier classifies the tool, not the teammate — reclassify it for everyone \
                 without `?agent=`."
                    .to_string(),
            );
        }
        for entry in patch.tools.unwrap_or_default() {
            let tool = entry.tool.trim();
            if tool.is_empty() {
                return Err("every entry in `tools` needs a `tool` name.".to_string());
            }
            match entry.mode {
                // Names no mode: the reset for this teammate's row only.
                None => {
                    if let Some(rules) = stored.agents.get_mut(agent) {
                        rules.overrides.remove(tool);
                    }
                }
                Some(mode) => {
                    stored
                        .agents
                        .entry(agent.to_string())
                        .or_default()
                        .overrides
                        .insert(
                            tool.to_string(),
                            crate::company::mcp_policy::ToolPolicy {
                                tier: None,
                                mode: Some(mode),
                            },
                        );
                }
            }
        }
        stored.prune();
        return Ok(stored);
    }

    if let Some(defaults) = patch.tier_defaults {
        for (tier, mode) in defaults {
            let parsed = ToolTier::ALL
                .iter()
                .find(|candidate| candidate.as_str() == tier.trim())
                .copied()
                .ok_or_else(|| format!("`{tier}` is not a tool tier."))?;
            match mode {
                Some(mode) => stored.tier_defaults.insert(parsed, mode),
                None => stored.tier_defaults.remove(&parsed),
            };
        }
    }

    for entry in patch.tools.unwrap_or_default() {
        let tool = entry.tool.trim();
        if tool.is_empty() {
            return Err("every entry in `tools` needs a `tool` name.".to_string());
        }
        match (entry.tier, entry.mode) {
            // Names no field: the reset.
            (None, None) => {
                stored.overrides.remove(tool);
            }
            (tier, mode) => {
                let row = stored.overrides.entry(tool.to_string()).or_default();
                if tier.is_some() {
                    row.tier = tier;
                }
                if mode.is_some() {
                    row.mode = mode;
                }
            }
        }
    }

    stored.prune();
    Ok(stored)
}

/// Builds the per-tool permission route fragment.
pub fn router() -> Router<AppState> {
    scoped(
        "/mcp/servers/{name}/tools/policy",
        get(read_policy).put(write_policy).delete(reset_policy),
    )
}

/// Resolves the named server, or the response explaining why it could not be.
async fn decl_for(
    runtime: &CompanyRuntime,
    name: &str,
) -> Result<crate::company::mcp::McpServerDecl, Box<Response>> {
    let manifest = manifest_servers(runtime)
        .await
        .map_err(|err| Box::new(err.into_response()))?;
    let decls = resolve_effective(
        runtime.id(),
        runtime.default_mcp_servers(),
        &manifest,
        runtime.secrets().as_ref(),
    )
    .await
    .map_err(|err| Box::new(ApiError(err).into_response()))?;
    decls
        .into_iter()
        .find(|d| d.name == name)
        .ok_or_else(|| Box::new(not_found(name)))
}

fn not_found(name: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": format!("no MCP server named `{name}`"),
            "code": "not_found",
        })),
    )
        .into_response()
}

/// The one repair a merge cannot perform: a document that will not parse has no
/// fields to merge into, so the reset is the only way back.
pub fn policy_unreadable(name: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": format!(
                "the stored tool permissions for `{name}` cannot be read. Clearing them restores \
                 the defaults; every tool parks for approval until you do."
            ),
            "code": "policy_unreadable",
        })),
    )
        .into_response()
}

/// Confirms `agent` names a roster teammate, so a write cannot store rules
/// under an id no teammate uses. Read-only routes and agent-scoped resets
/// skip this — a reset must still be able to clean up a departed teammate's
/// rules.
pub fn unknown_agent(agent_id: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": format!("no teammate named `{agent_id}` on this company's roster"),
            "code": "not_found",
        })),
    )
        .into_response()
}

/// Loads the company record and checks `agent` against its roster, returning
/// the `404` to send back when it is not there.
pub async fn require_roster_agent(
    runtime: &CompanyRuntime,
    agent: &str,
) -> Result<(), Box<Response>> {
    match runtime.store().load(runtime.id()).await {
        Ok(Some(record)) if record.is_roster_agent(agent) => Ok(()),
        Ok(Some(_)) => Err(Box::new(unknown_agent(agent))),
        Ok(None) => Err(Box::new(
            ApiError(crate::error::OpenCompanyError::CompanyNotFound(
                runtime.id().to_string(),
            ))
            .into_response(),
        )),
        Err(err) => Err(Box::new(ApiError(err).into_response())),
    }
}

/// Reads the stored document strictly, so an unreadable one is a `409` rather
/// than silently rendered as "no overrides".
///
/// The gate's own loader degrades instead. The two disagree on purpose: a
/// console that rendered the degraded document would show permissions nobody
/// chose, and an edit saved from that view would make them real.
async fn stored_strict(
    runtime: &CompanyRuntime,
    name: &str,
) -> Result<crate::company::mcp_policy::McpToolPolicies, Box<Response>> {
    mcp_policy::load_tool_policies_strict(
        runtime.id(),
        runtime.secrets().as_ref(),
        &mcp_policy::tool_policies_key(name),
    )
    .await
    .map_err(|_| Box::new(policy_unreadable(name)))
    .map(Option::unwrap_or_default)
}

async fn read_policy(
    company: ScopedCompany,
    Path(NamePath { name }): Path<NamePath>,
    Query(scope): Query<AgentScope>,
) -> Response {
    let runtime = company.runtime.as_ref();
    let name = name.trim().to_string();
    let decl = match decl_for(runtime, &name).await {
        Ok(decl) => decl,
        Err(response) => return *response,
    };
    let stored = match stored_strict(runtime, &name).await {
        Ok(stored) => stored,
        Err(response) => return *response,
    };
    let policies = mcp_policy::effective_policies(
        &decl.read_only_tools,
        mcp_policy::StoredPolicies::Stored(stored),
    );
    Json(tool_policy_dto(
        &name,
        &policies,
        &decl.tool_inventory,
        scope.agent(),
    ))
    .into_response()
}

async fn write_policy(
    company: AdminScopedCompany,
    Path(NamePath { name }): Path<NamePath>,
    Query(scope): Query<AgentScope>,
    body: Option<Json<PutToolPolicy>>,
) -> Response {
    let runtime = company.runtime.as_ref();
    let name = name.trim().to_string();
    let decl = match decl_for(runtime, &name).await {
        Ok(decl) => decl,
        Err(response) => return *response,
    };
    if let Some(agent) = scope.agent()
        && let Err(response) = require_roster_agent(runtime, agent).await
    {
        return *response;
    }
    let stored = match stored_strict(runtime, &name).await {
        Ok(stored) => stored,
        Err(response) => return *response,
    };

    let patch = match body {
        Some(Json(patch)) => patch,
        None => return bad_request("send a JSON body naming `tierDefaults`, `tools`, or both."),
    };
    let merged = match apply_tool_policy_patch(stored, patch, scope.agent()) {
        Ok(merged) => merged,
        Err(reason) => return bad_request(&reason),
    };

    if let Err(err) = mcp_policy::save_tool_policies(
        runtime.id(),
        runtime.secrets().as_ref(),
        &mcp_policy::tool_policies_key(&name),
        &merged,
    )
    .await
    {
        return ApiError(err).into_response();
    }

    // The echoed document is the resolved one, not the patch: every field the
    // console renders is host-resolved, and predicting them there would be a
    // second implementation of a rule this crate owns.
    let policies = mcp_policy::effective_policies(
        &decl.read_only_tools,
        mcp_policy::StoredPolicies::Stored(merged),
    );
    Json(tool_policy_dto(&name, &policies, &decl.tool_inventory, scope.agent()).with_note())
        .into_response()
}

async fn reset_policy(
    company: AdminScopedCompany,
    Path(NamePath { name }): Path<NamePath>,
    Query(scope): Query<AgentScope>,
) -> Response {
    let runtime = company.runtime.as_ref();
    let name = name.trim().to_string();
    let decl = match decl_for(runtime, &name).await {
        Ok(decl) => decl,
        Err(response) => return *response,
    };
    // An agent-scoped reset clears one teammate's rules and leaves the company
    // document alone. It has to read first, so an unreadable document is a `409`
    // here; the company-scoped reset stays the repair for that.
    let merged = match scope.agent() {
        Some(agent) => {
            let mut stored = match stored_strict(runtime, &name).await {
                Ok(stored) => stored,
                Err(response) => return *response,
            };
            stored.agents.remove(agent);
            stored.prune();
            if let Err(err) = mcp_policy::save_tool_policies(
                runtime.id(),
                runtime.secrets().as_ref(),
                &mcp_policy::tool_policies_key(&name),
                &stored,
            )
            .await
            {
                return ApiError(err).into_response();
            }
            mcp_policy::StoredPolicies::Stored(stored)
        }
        None => {
            let replacement = match mcp_policy::reset_company_policy(
                runtime.id(),
                runtime.secrets().as_ref(),
                &mcp_policy::tool_policies_key(&name),
            )
            .await
            {
                Ok(replacement) => replacement,
                Err(err) => return ApiError(err).into_response(),
            };
            mcp_policy::StoredPolicies::Stored(replacement)
        }
    };
    let policies = mcp_policy::effective_policies(&decl.read_only_tools, merged);
    Json(tool_policy_dto(&name, &policies, &decl.tool_inventory, scope.agent()).with_note())
        .into_response()
}

fn bad_request(reason: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": reason, "code": "invalid_request" })),
    )
        .into_response()
}

#[cfg(test)]
#[path = "mcp_tool_policy_tests.rs"]
mod tests;

/// The `?agent=` lens: what it merges, what it refuses, and what a row says.
#[cfg(test)]
#[path = "mcp_tool_policy_agent_tests.rs"]
mod agent_tests;

/// The roster check on write, and the company reset preserving `agents` —
/// driven over the real router.
#[cfg(test)]
#[path = "mcp_tool_policy_route_tests.rs"]
mod route_tests;
