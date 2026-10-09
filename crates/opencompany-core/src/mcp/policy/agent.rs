//! The per-agent layer of a server's tool policy: one teammate's own decisions
//! about the tools on a server the company already reaches.
//!
//! This layer may only narrow. A stored per-agent mode moves the resolved mode
//! along `AlwaysAllow < NeedsApproval < Blocked` in the restricting direction
//! only: [`ApprovalMode::max_restrictive`] is the only way it reaches the
//! result, and the transport's filter consults deny before allow, so a widening
//! rule could not be enforced. A stored setting the clamp discards is reported
//! as [`PolicySource::AgentClamped`] rather than dropped.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    ApprovalMode, McpToolInventory, McpToolPolicies, ResolvedPolicy, ToolPolicy, policy_tool_names,
    resolve_policy,
};
use crate::company::mcp::McpServerDecl;
use crate::runtime::tools::grants_cover_server;

/// One teammate's own tool decisions on one server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolPolicies {
    /// This teammate's per-tool decisions. Ordered so the document is
    /// byte-stable and the fingerprint over it canonical.
    #[serde(default)]
    pub overrides: BTreeMap<String, ToolPolicy>,
}

impl AgentToolPolicies {
    /// Whether this entry carries any decision. An empty entry is pruned on
    /// write, so the fingerprint does not move on a reset.
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }

    /// Drops per-tool rows that decide nothing.
    pub fn prune(&mut self) {
        self.overrides.retain(|_, policy| !policy.is_empty());
    }
}

/// Which rule decided a tool's mode for one teammate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    /// Nothing is pinned anywhere: the tier default or the fallback decided.
    ServerInherited,
    /// The company document pins this tool, and the teammate adds nothing.
    ServerPinned,
    /// The teammate's own mode decided.
    AgentPinned,
    /// The teammate's stored mode would widen, so it was discarded.
    AgentClamped,
}

impl PolicySource {
    /// The source of a mode no per-agent rule touched.
    pub fn for_server(is_override: bool) -> Self {
        if is_override {
            PolicySource::ServerPinned
        } else {
            PolicySource::ServerInherited
        }
    }
}

/// One tool's policy as it stands for one teammate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedPolicyForAgent {
    /// What [`resolve_policy`] answers for the server, byte-for-byte.
    pub server: ResolvedPolicy,
    /// The mode enforced for this teammate.
    pub mode: ApprovalMode,
    /// The teammate's stored mode, present even when the clamp discarded it.
    pub asked: Option<ApprovalMode>,
    /// Which rule decided [`Self::mode`].
    pub source: PolicySource,
}

/// Resolves one tool's mode for one teammate.
///
/// A document with no `agents` key, or an agent id nobody has written a rule
/// for, resolves to the server's own answer unchanged.
pub fn resolve_policy_for_agent(
    policies: &McpToolPolicies,
    agent: &str,
    tool: &str,
    suggested: Option<crate::mcp::policy::ToolTier>,
) -> ResolvedPolicyForAgent {
    let server = resolve_policy(policies, tool, suggested);
    let asked = policies
        .agents
        .get(agent)
        .and_then(|entry| entry.overrides.get(tool))
        .and_then(|policy| policy.mode);
    let mode = asked.map_or(server.mode, |a| server.mode.max_restrictive(a));
    let source = match asked {
        None => PolicySource::for_server(server.is_override),
        Some(a) if a == mode => PolicySource::AgentPinned,
        Some(_) => PolicySource::AgentClamped,
    };
    ResolvedPolicyForAgent {
        server,
        mode,
        asked,
        source,
    }
}

/// Every tool name a decision can be stated about for one teammate: the server's
/// own set plus the tools only this teammate has a rule for.
///
/// The per-agent-only names are included so a block on a tool no probe has
/// reached still reaches the deny list.
pub fn agent_policy_tool_names(
    policies: &McpToolPolicies,
    inventory: &McpToolInventory,
    agent: &str,
) -> impl Iterator<Item = String> {
    let mut names: BTreeSet<String> = policy_tool_names(policies, inventory).collect();
    if let Some(entry) = policies.agents.get(agent) {
        names.extend(entry.overrides.keys().cloned());
    }
    names.into_iter()
}

/// Whether this teammate is refused `tool` on this server outright.
pub fn blocks_tool_for_agent(
    policies: &McpToolPolicies,
    inventory: &McpToolInventory,
    agent: &str,
    tool: &str,
) -> bool {
    resolve_policy_for_agent(policies, agent, tool, inventory.suggested(tool)).mode
        == ApprovalMode::Blocked
}

/// Every tool this server refuses this teammate outright, sorted.
///
/// A superset of [`blocked_tool_names`](super::blocked_tool_names), sorted the
/// same way.
pub fn blocked_tool_names_for_agent(
    policies: &McpToolPolicies,
    inventory: &McpToolInventory,
    agent: &str,
) -> Vec<String> {
    let mut names: Vec<String> = agent_policy_tool_names(policies, inventory, agent)
        .filter(|tool| blocks_tool_for_agent(policies, inventory, agent, tool))
        .collect();
    names.sort();
    names
}

/// The `(server, tool)` pairs one teammate's approval gate lets run without
/// parking.
///
/// A subset of [`mcp_allow_set`](super::mcp_allow_set), narrowed by this
/// teammate's own modes and by the servers its grants reach.
pub fn mcp_allow_set_for_agent(
    servers: &[McpServerDecl],
    agent: &str,
    grants: &[String],
) -> crate::policy::McpReadSet {
    crate::policy::McpReadSet::from_pairs(
        servers
            .iter()
            .filter(|server| server.enabled && grants_cover_server(grants, &server.name))
            .flat_map(|server| {
                policy_tool_names(&server.tool_policies, &server.tool_inventory)
                    .filter(|tool| {
                        resolve_policy_for_agent(
                            &server.tool_policies,
                            agent,
                            tool,
                            server.tool_inventory.suggested(tool),
                        )
                        .mode
                            == ApprovalMode::AlwaysAllow
                    })
                    .map(move |tool| (server.name.clone(), tool))
            }),
    )
}

/// The teammates whose resolved mode for `tool` differs from the server's,
/// sorted by agent id.
pub fn differing_agents(
    policies: &McpToolPolicies,
    inventory: &McpToolInventory,
    tool: &str,
) -> Vec<String> {
    let suggested = inventory.suggested(tool);
    let server = resolve_policy(policies, tool, suggested);
    policies
        .agents
        .keys()
        .filter(|agent| {
            resolve_policy_for_agent(policies, agent, tool, suggested).mode != server.mode
        })
        .cloned()
        .collect()
}

/// Whether `tool` cannot be called on this server by this teammate at all.
///
/// Composes the same three refusals the transport applies: the declaration's
/// allow list, its deny list, and the resolved policy.
pub fn refuses_tool_for_agent(decl: &McpServerDecl, agent: &str, tool: &str) -> bool {
    if !decl.allowed_tools.is_empty() && !decl.allowed_tools.iter().any(|t| t == tool) {
        return true;
    }
    if decl.disallowed_tools.iter().any(|t| t == tool) {
        return true;
    }
    blocks_tool_for_agent(&decl.tool_policies, &decl.tool_inventory, agent, tool)
}

/// Whether every tool this server is *known* to offer is refused this teammate.
///
/// Never true for a server no probe has reached.
pub fn every_known_tool_refused(decl: &McpServerDecl, agent: &str) -> bool {
    !decl.tool_inventory.tools.is_empty()
        && decl
            .tool_inventory
            .tools
            .keys()
            .all(|tool| refuses_tool_for_agent(decl, agent, tool))
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
