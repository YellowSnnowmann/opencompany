//! What one company agent reaches over MCP.
//!
//! A declared server reaches an agent as an [`openhuman_embed::McpServer`]
//! attached to its spec ([`embed_servers_for_agent`]), called through
//! OpenHuman's own `mcp_list_tools` / `mcp_call_tool`. That `mcp_call_tool`
//! scrubs failed and successful results against the server's own credentials
//! ([`tinymcp::tools::SecretScrubber`]), and the attachment's deny list carries
//! every tool the agent's per-tool policy blocks. A registry install reaches an
//! agent through [`OcMcpRegistryScopedTool`] on its belt. [`resolve_for_agent`]
//! decides both from the same inputs.
//!
//! `mcp_list_servers` is never in a company agent's scope: it reports every
//! configured server, not only the ones the agent was granted.
//!
//! Compiled only under `feature = "openhuman"`.

use serde_json::Value;

use openhuman_core as oh;

use oh::config::{Config, McpAuthConfig, McpServerConfig};
use oh::mcp::config_servers::McpServerRegistry;

use crate::company::mcp::{AuthMaterial, McpServerDecl};
use crate::runtime::tools::grants_cover_server;

mod registry_list;
pub(crate) mod registry_outcome;
mod registry_scoped;
mod resolve;

pub use registry_list::OcMcpRegistryInstalledListTool;
pub use registry_scoped::OcMcpRegistryScopedTool;
pub use resolve::{AgentMcp, DECLARED_BRIDGE_TOOLS, resolve_for_agent};

/// Builds a registry from a set of decls, keeping only the enabled ones.
///
/// Sets `gitbooks.enabled = false` — **critical**: OpenHuman's `Config::default`
/// seeds a `gitbooks` MCP server, which would otherwise leak into every tenant
/// agent's server list. `command` is always empty, so the registry always
/// selects the HTTP transport (hosted-v1 boundary). Returns an empty registry
/// when nothing survives.
pub fn registry_from_decls(decls: &[McpServerDecl]) -> McpServerRegistry {
    let mut config = Config::default();
    // Do NOT inherit upstream's default gitbooks server.
    config.gitbooks.enabled = false;
    config.mcp_client.enabled = true;
    config.mcp_client.servers = decls
        .iter()
        .filter(|decl| decl.enabled)
        .map(server_config)
        .collect();
    // `from_config` takes `tinymcp`'s own client config now, not OpenHuman's
    // `Config`. `host::static_registry` is the conversion, and it already
    // degrades an unbuildable set to an empty one rather than failing.
    oh::mcp::host::static_registry(&config)
}

/// A persona brief appended when an agent is granted MCP tools: a stale-memory
/// mitigation directing the agent to answer capability questions from a **live**
/// enumeration call, never from memory (the effective server set can change
/// between turns — the MCP-freshness path). The root fix for stale answers lives
/// in the Memory cell; this is the mitigation.
///
/// Names no server itself. The server-family brief carries the names, and with
/// them the tool and key that address each one; this says only what to call to
/// see what a server currently offers.
///
/// The two families inspect through different tools, so the brief names only the
/// ones the agent was actually wired. No company agent is scoped to list the
/// configured servers — that tool answers with their credentials — so a declared
/// server is inspected by name and never discovered. Empty when neither family is
/// wired.
pub fn capability_brief(declared: bool, registry: bool) -> String {
    let enumerate = match (declared, registry) {
        (true, true) => {
            "`mcp_list_tools` with a server's name, and `mcp_registry_installed_list` (then \
             `mcp_registry_list_tools` for a specific install)"
        }
        (true, false) => "`mcp_list_tools` with the server's name",
        (false, true) => {
            "`mcp_registry_installed_list` (and `mcp_registry_list_tools` for a specific install)"
        }
        (false, false) => return String::new(),
    };
    format!(
        " When you are asked what tools, integrations, or MCP servers you have — or whether you \
         can do something that would use one — ALWAYS call {enumerate} to check what is available \
         right now. Never answer such questions from memory: your available servers and tools can \
         change between turns."
    )
}

/// The company's granted MCP servers, rendered as [`openhuman_embed::McpServer`]
/// attachments an [`openhuman_embed::AgentSpec`] carries via
/// [`AgentSpec::mcp`](openhuman_embed::AgentSpec::mcp), alongside the internal
/// `opencompany` server.
///
/// Every tool the agent's per-tool policy blocks is added to the attachment's
/// deny list, which the transport checks before dialling.
pub fn embed_servers_for_agent(
    decls: &[McpServerDecl],
    agent: &str,
    grants: &[String],
) -> Vec<openhuman_embed::McpServer> {
    decls
        .iter()
        .filter(|decl| decl.enabled && grants_cover_server(grants, &decl.name))
        .map(|decl| {
            // Deny outranks allow in the transport, so a server with an allow
            // list cannot re-admit a blocked tool.
            let mut denied = decl.disallowed_tools.clone();
            for tool in crate::mcp::policy::blocked_tool_names_for_agent(
                &decl.tool_policies,
                &decl.tool_inventory,
                agent,
            ) {
                if !denied.contains(&tool) {
                    denied.push(tool);
                }
            }
            openhuman_embed::McpServer::http(decl.name.clone(), decl.endpoint.clone())
                .auth(auth_config(&decl.auth))
                .allow_tools(decl.allowed_tools.clone())
                .deny_tools(denied)
                .timeout_secs(decl.timeout_secs)
                .description(decl.description.clone().unwrap_or_default())
        })
        .collect()
}

/// Projects a [`McpServerDecl`] onto an OpenHuman [`McpServerConfig`], mapping
/// the resolved [`AuthMaterial`] onto the transport's auth config. `command`
/// stays empty so the registry always builds the HTTP transport.
fn server_config(decl: &McpServerDecl) -> McpServerConfig {
    let mut config = McpServerConfig::default();
    config.server.name.clone_from(&decl.name);
    config.server.endpoint.clone_from(&decl.endpoint);
    config.server.description.clone_from(&decl.description);
    config.server.enabled = true;
    config.server.allowed_tools.clone_from(&decl.allowed_tools);
    config
        .server
        .disallowed_tools
        .clone_from(&decl.disallowed_tools);
    config.server.timeout_secs = decl.timeout_secs;
    config.server.auth = auth_config(&decl.auth);
    config
}

/// Maps resolved [`AuthMaterial`] onto the transport's [`McpAuthConfig`].
fn auth_config(material: &AuthMaterial) -> McpAuthConfig {
    match material {
        AuthMaterial::None => McpAuthConfig::None,
        AuthMaterial::Bearer(token) => McpAuthConfig::BearerToken {
            token: token.clone(),
        },
        AuthMaterial::Header { name, value } => McpAuthConfig::Header {
            name: name.clone(),
            value: value.clone(),
        },
        // The upstream HTTP transport already applies this via `request.query()`
        // (`mcp_client/client.rs`), so BrowserBase-style URL auth needs zero
        // vendor changes — just this mapping.
        AuthMaterial::QueryParam { name, value } => McpAuthConfig::QueryParam {
            name: name.clone(),
            value: value.clone(),
        },
        // The whole trick behind console OAuth: an OAuth credential resolves to
        // exactly the bearer path the static registry already knows how to send.
        // The freshness of `access_token` is the caller's responsibility — the
        // harness builder refreshes an expired token before this mapping runs
        // (see `crate::company::mcp_oauth::refresh` + `resolve_effective`).
        AuthMaterial::OAuth { access_token, .. } => McpAuthConfig::BearerToken {
            token: access_token.clone(),
        },
    }
}

/// One remote tool advertised by an MCP server, projected for the console's
/// live-discovery view. Sanitized: the `title`/`description` are read through
/// OpenHuman's `display_*` accessors (control-char strip + injection fence +
/// length cap), never the raw remote fields.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Value,
}

/// Live-discovers the tools a single server exposes, through a one-server
/// registry built from `decls`. Inherits the registry's per-server allow-list
/// and the input-validation safety filter. `server` names the decl to query.
pub async fn discover_tools(
    decls: &[McpServerDecl],
    server: &str,
) -> anyhow::Result<Vec<McpToolInfo>> {
    let registry = registry_from_decls(decls);
    let tools = registry.list_tools(server).await?;
    Ok(tools
        .iter()
        .map(|tool| McpToolInfo {
            name: tool.name.clone(),
            title: tool.display_title(),
            description: tool.display_description(),
            input_schema: tool.input_schema.clone(),
        })
        .collect())
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_blocked_tests.rs"]
mod blocked_tests;

#[cfg(all(test, feature = "mcp"))]
#[path = "agent_turn_tests.rs"]
mod turn_tests;
