//! One agent's MCP surface, resolved once from the company's declarations,
//! that agent's effective grants and the company's registry store.
//!
//! [`resolve_for_agent`] answers every MCP question the agent builder asks —
//! which declared servers ride on the agent's spec, which registry tools go on
//! its belt, which native bridge names its tool scope lists, and what its
//! persona brief says — from the same inputs, so the four answers cannot
//! drift apart.

use std::path::PathBuf;
use std::sync::Arc;

use openhuman_core as oh;
use tinytools::Tool;

use crate::company::mcp::McpServerDecl;
use crate::mcp::decl::families::{RegistryServerRow, server_family_brief};
use crate::mcp::runtime::McpRuntime;
use crate::ports::SecretStore;
use crate::ports::types::CompanyId;

use super::{
    OcMcpRegistryInstalledListTool, OcMcpRegistryScopedTool, capability_brief, embed_servers,
    granted_decls, registry_from_refs,
};

/// OpenHuman's own bridge tools over the servers attached to an agent's spec.
pub const DECLARED_BRIDGE_TOOLS: [&str; 2] = ["mcp_list_tools", "mcp_call_tool"];

/// What one agent reaches over MCP.
///
/// Built by [`resolve_for_agent`]. Holds the declarations and grants it was
/// resolved from, borrowed, plus the two wiring decisions derived from them.
pub struct AgentMcp<'a> {
    decls: &'a [McpServerDecl],
    granted: Vec<&'a McpServerDecl>,
    agent: &'a str,
    grants: &'a [String],
    declared_wired: bool,
    registry_home: Option<PathBuf>,
}

/// Resolves one agent's MCP surface.
///
/// `grants` must be the agent's effective grants (an empty manifest request
/// inherits the company belt). `mcp_home` is the company's registry store, when
/// one is configured.
///
/// The declared family is wired when at least one enabled server the grants
/// cover builds into a non-empty registry. The registry family is wired only
/// on an explicit `mcp_registry` grant — the catch-all `*` does not confer it —
/// and only when a registry store exists.
pub fn resolve_for_agent<'a>(
    decls: &'a [McpServerDecl],
    agent: &'a str,
    grants: &'a [String],
    mcp_home: Option<PathBuf>,
) -> AgentMcp<'a> {
    let granted = granted_decls(decls, grants);
    let declared_wired =
        !granted.is_empty() && !registry_from_refs(granted.iter().copied()).is_empty();
    let registry_home = mcp_home.filter(|_| crate::company::grants_mcp_registry_explicit(grants));
    AgentMcp {
        decls,
        granted,
        agent,
        grants,
        declared_wired,
        registry_home,
    }
}

impl AgentMcp<'_> {
    /// Whether the agent reaches any declared server.
    pub fn declared_wired(&self) -> bool {
        self.declared_wired
    }

    /// Whether the agent holds the `mcp_registry_*` tools.
    pub fn registry_wired(&self) -> bool {
        self.registry_home.is_some()
    }

    /// The declared servers attached to the agent's spec, each carrying its
    /// per-agent deny list. Empty unless [`Self::declared_wired`].
    pub fn embed_servers(&self) -> Vec<openhuman_embed::McpServer> {
        if !self.declared_wired {
            return Vec::new();
        }
        embed_servers(&self.granted, self.agent)
    }

    /// The OpenHuman-native bridge names the agent's tool scope must list for
    /// the attached servers to be callable. Empty unless
    /// [`Self::declared_wired`].
    pub fn native_tool_names(&self) -> &'static [&'static str] {
        if self.declared_wired {
            &DECLARED_BRIDGE_TOOLS
        } else {
            &[]
        }
    }

    /// The registry tools for the agent's belt: enumeration, schema discovery
    /// and the call tool, each scoped to the installs the grants reach.
    ///
    /// An explicit `mcp_registry` grant with no registry store wires nothing
    /// and warns.
    pub fn registry_tools(
        &self,
        company: &CompanyId,
        secrets: Option<Arc<dyn SecretStore>>,
    ) -> Vec<Box<dyn Tool>> {
        let Some(mcp_home) = self.registry_home.clone() else {
            if crate::company::grants_mcp_registry_explicit(self.grants) {
                tracing::warn!(
                    company = %company,
                    agent = %self.agent,
                    "[build] agent explicitly grants `mcp_registry` but no MCP registry home is \
                     configured; mcp_registry tools NOT wired (fail-closed)"
                );
            }
            return Vec::new();
        };
        let config = Arc::new(McpRuntime::config_for(mcp_home.clone()));
        vec![
            Box::new(OcMcpRegistryInstalledListTool::new(
                Arc::new(McpRuntime::new(mcp_home)),
                self.grants.to_vec(),
            )),
            Box::new(OcMcpRegistryScopedTool::new(
                Box::new(oh::mcp::registry::tools::McpRegistryListToolsTool::new(
                    config.clone(),
                )),
                self.agent.to_string(),
                self.grants.to_vec(),
                company.clone(),
                secrets.clone(),
            )),
            Box::new(OcMcpRegistryScopedTool::new(
                Box::new(oh::mcp::registry::tools::McpRegistryToolCallTool::new(
                    config,
                )),
                self.agent.to_string(),
                self.grants.to_vec(),
                company.clone(),
                secrets,
            )),
        ]
    }

    /// The registry installs the persona brief names, read under the same
    /// condition that wires the registry tools.
    ///
    /// A store that cannot be read yields no rows and warns, so the brief is
    /// shorter rather than wrong.
    #[cfg_attr(not(feature = "mcp"), allow(dead_code))]
    pub(crate) fn registry_installs(&self, company: &CompanyId) -> Vec<RegistryServerRow> {
        let Some(mcp_home) = self.registry_home.clone() else {
            return Vec::new();
        };
        match McpRuntime::new(mcp_home).list() {
            Ok(installs) => installs
                .iter()
                .map(|install| RegistryServerRow {
                    server_id: install.server_id.clone(),
                    display_name: install.display_name.clone(),
                    endpoint: install.transport.deployment_url().map(str::to_string),
                    enabled: install.enabled,
                })
                .collect(),
            Err(error) => {
                tracing::warn!(
                    company = %company,
                    agent = %self.agent,
                    error = %error,
                    "[build] MCP registry installs unreadable; the server-family \
                     brief names the declared servers only"
                );
                Vec::new()
            }
        }
    }

    /// The MCP part of the agent's persona: the live-enumeration directive for
    /// whichever families are wired, then the server-family brief naming each
    /// reachable server and the tool that addresses it.
    #[cfg_attr(not(feature = "mcp"), allow(dead_code))]
    pub(crate) fn persona_brief(&self, installs: &[RegistryServerRow]) -> String {
        let mut brief = capability_brief(self.declared_wired, self.registry_wired());
        brief.push_str(&server_family_brief(
            self.decls,
            installs,
            self.grants,
            self.agent,
        ));
        brief
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
