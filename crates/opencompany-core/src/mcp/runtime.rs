//! Company-scoped persistence and access to OpenHuman's live MCP registry:
//! the directory search, installs, connections and calls behind the console's
//! registry routes and the agents' `mcp_registry_*` tools.
//!
//! Compiled only under `feature = "openhuman"`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use openhuman_core as oh;

use oh::mcp::registry::types::{ConnStatus, InstalledServer, McpTool};

use crate::error::OpenCompanyError;

/// The transport filter every directory search is pinned to — upstream's
/// vocabulary for "has a hosted HTTP endpoint" (`registry::apply_transport`
/// keeps `is_deployed` rows for `"hosted"`, drops them for `"stdio"`).
///
/// **Hardcoded, not a parameter the console may set.** A stdio entry launches a
/// local subprocess through `npx` / `uvx`, and the tenant image is
/// `debian:bookworm-slim` plus `ca-certificates`, `curl`, `libssl3` and X11 —
/// no Node, no Python, no package manager (issue #1270). So there is no caller
/// for whom `"stdio"` or `"all"` would produce an installable row, and offering
/// the knob would only let the console show an operator servers that fail at
/// install time. Widening it is one edit here, on the day a sidecar that can
/// actually run stdio servers exists; until then the honest surface is the one
/// that cannot express the broken request.
const HOSTED_TRANSPORT: &str = "hosted";

/// Company-home-scoped persistence and access to OpenHuman's live MCP registry.
pub struct McpRuntime {
    config: oh::config::Config,
}

impl McpRuntime {
    /// Creates a runtime whose MCP SQLite store lives beneath `workspace_dir`.
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            config: Self::config_for(workspace_dir),
        }
    }

    /// The config that selects the MCP store beneath `workspace_dir`.
    ///
    /// Public because the agent toolbelt needs the *same* one: OpenHuman's
    /// `mcp_registry_*` tools take a config now rather than reading a process
    /// global, and a tool built over a different config would quietly read a
    /// different SQLite store than REST does — the installs would be there in
    /// the console and absent from the turn.
    #[must_use]
    pub fn config_for(workspace_dir: PathBuf) -> oh::config::Config {
        oh::config::Config {
            workspace_dir,
            ..Default::default()
        }
    }

    /// The config the three **directory** calls run against.
    ///
    /// Upstream carries a `registry_auth.smithery_api_key`, and this deployment
    /// deliberately never sets one. A per-company Smithery key was a credential
    /// slot on a console tab — to store, rotate, revoke and explain — and what
    /// it bought was one vendor's hosted listings; the open
    /// `modelcontextprotocol/registry` is queried without any credential at all.
    /// Left unset, upstream still falls back to the host's `SMITHERY_API_KEY`
    /// where an operator has set one on the process, which is the whole of the
    /// Smithery story now.
    fn directory_config(&self) -> oh::config::Config {
        self.config.clone()
    }

    /// Search the upstream MCP directory — the official
    /// `modelcontextprotocol/registry` — paged and SQLite-cached upstream
    /// (issue #1270).
    ///
    /// The console's only way to answer "what could I add?". The static server
    /// list cannot: an operator has to arrive already knowing an endpoint, so
    /// that surface is empty until somebody pastes a URL into it.
    ///
    /// The transport filter is fixed at [`HOSTED_TRANSPORT`] rather than exposed
    /// as a parameter — see that constant for why.
    pub async fn search(
        &self,
        query: Option<String>,
        page: Option<u32>,
        page_size: Option<u32>,
    ) -> crate::Result<serde_json::Value> {
        oh::mcp::registry::ops::mcp_clients_registry_search(
            &self.directory_config(),
            query,
            Some(HOSTED_TRANSPORT.to_string()),
            page,
            page_size,
        )
        .await
        .map(|outcome| outcome.value)
        .map_err(|e| OpenCompanyError::Harness(format!("mcp registry search failed: {e}")))
    }

    /// One directory entry in full, routed back to the registry it came from.
    pub async fn registry_get(&self, qualified_name: String) -> crate::Result<serde_json::Value> {
        oh::mcp::registry::ops::mcp_clients_registry_get(&self.directory_config(), qualified_name)
            .await
            .map(|outcome| outcome.value)
            .map_err(|e| OpenCompanyError::Harness(format!("mcp registry lookup failed: {e}")))
    }

    /// Rotate an install's environment values (write-only, never read back).
    pub async fn update_env(
        &self,
        server_id: String,
        env: HashMap<String, String>,
    ) -> crate::Result<()> {
        oh::mcp::registry::ops::mcp_clients_update_env(&self.config, server_id, env)
            .await
            .map(|_| ())
            .map_err(|e| OpenCompanyError::Harness(format!("mcp env update failed: {e}")))
    }

    /// Reconnects enabled installed servers. Failures are logged by OpenHuman
    /// per server and never prevent the company runtime from booting.
    pub async fn boot(&self) {
        oh::mcp::registry::boot::spawn_installed_servers(&self.config).await;
    }

    /// The `tinymcp` service backing this runtime's registry.
    ///
    /// The store and connection map used to be reachable as free functions on
    /// `oh::mcp::registry`; the registry moved into `tinymcp` and both are now
    /// accessors on the one service the process holds for a config. Opening is
    /// per-config and cached upstream, so this is a lookup rather than a build.
    fn host(&self) -> crate::Result<std::sync::Arc<oh::mcp::host::McpHost>> {
        oh::mcp::host::for_config(&self.config).map_err(store_error)
    }

    /// Returns every persisted install without loading secret environment values.
    pub fn list(&self) -> crate::Result<Vec<InstalledServer>> {
        self.host()?
            .dynamic()
            .store()
            .list_servers()
            .map_err(store_error)
    }

    /// Persists an install and its write-only environment values.
    pub fn install(
        &self,
        server: &InstalledServer,
        env: &HashMap<String, String>,
    ) -> crate::Result<()> {
        let store = self.host()?;
        let store = store.dynamic().store();
        store.insert_server(server).map_err(store_error)?;
        // `set_env_values` takes an ordered map now; the write is the same one.
        let env: std::collections::BTreeMap<String, String> =
            env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        if let Err(error) = store.set_env_values(&server.server_id, &env) {
            let _ = store.delete_server(&server.server_id);
            return Err(store_error(error));
        }
        Ok(())
    }

    /// Loads an installed server, establishing the company-store membership
    /// check before touching OpenHuman's process-global connection registry.
    pub fn get(&self, server_id: &str) -> crate::Result<InstalledServer> {
        self.host()?
            .dynamic()
            .store()
            .get_server(server_id)
            // Only a genuinely absent install is "not found". A store that
            // fails to read must not be reported as a missing server — the
            // caller would be told to reinstall something that is there.
            .map_err(|error| match error {
                tinymcp::Error::UnknownServer { .. } => {
                    OpenCompanyError::McpServerNotFound(server_id.to_string())
                }
                other => store_error(other),
            })
    }

    /// Connects an installed server and returns its advertised tools.
    pub async fn connect(&self, server_id: &str) -> crate::Result<Vec<McpTool>> {
        let server = self.get(server_id)?;
        oh::mcp::registry::connections::connect(&self.config, &server)
            .await
            .map_err(harness_error)
    }

    /// Disconnects an installed server after verifying it belongs to this store.
    ///
    /// Goes through this runtime's own service rather than the
    /// `oh::mcp::registry::connections` free function, which reads the
    /// *process-global* one. `connect` above is per-config, so the free
    /// function would look for the connection in a service that never holds it
    /// — this runtime never calls `host::init` — and answer a truthful-looking
    /// `false` for a server that is in fact connected.
    pub async fn disconnect(&self, server_id: &str) -> crate::Result<bool> {
        self.get(server_id)?;
        Ok(self
            .host()?
            .dynamic()
            .connections()
            .disconnect(server_id)
            .await)
    }

    /// Disconnects and deletes an installed server and its environment values.
    pub async fn uninstall(&self, server_id: &str) -> crate::Result<bool> {
        self.get(server_id)?;
        // Same per-config service as `disconnect`, for the same reason.
        let host = self.host()?;
        host.dynamic().connections().disconnect(server_id).await;
        host.dynamic()
            .store()
            .delete_server(server_id)
            .map_err(store_error)
    }

    /// Returns connection state joined by OpenHuman against this runtime's store.
    ///
    /// Reporting status must not fail a caller that is only rendering it, so a
    /// service that will not open — or a store that will not list — reports
    /// "nothing installed" rather than an error, which is what the free
    /// function this replaced did.
    pub async fn status(&self) -> Vec<ConnStatus> {
        let Ok(host) = self.host() else {
            return Vec::new();
        };
        let registry = host.dynamic();
        match registry.connections().all_status(registry.store()).await {
            Ok(statuses) => statuses,
            Err(error) => {
                log::warn!("[mcp] could not summarize connection status: {error}");
                Vec::new()
            }
        }
    }

    /// Returns the cached tool list for a connected installed server.
    pub async fn tools(&self, server_id: &str) -> crate::Result<Vec<McpTool>> {
        self.get(server_id)?;
        self.host()?
            .dynamic()
            .connections()
            .tools_for(server_id)
            .await
            .ok_or_else(|| {
                OpenCompanyError::InvalidRequest(format!(
                    "MCP server '{server_id}' is not connected"
                ))
            })
    }

    /// Calls one tool after verifying the server belongs to this runtime's store.
    pub async fn call_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: Value,
    ) -> crate::Result<Value> {
        self.get(server_id)?;
        // The transport returns a structured result now; the raw JSON payload
        // is the field this surface has always handed back.
        self.host()?
            .dynamic()
            .connections()
            .call_tool(server_id, tool_name, arguments)
            .await
            .map(|result| result.raw_result)
            .map_err(harness_error)
    }
}

fn store_error(error: impl std::fmt::Display) -> OpenCompanyError {
    OpenCompanyError::Store(format!("MCP registry: {error}"))
}

fn harness_error(error: impl std::fmt::Display) -> OpenCompanyError {
    OpenCompanyError::Harness(format!("MCP registry: {error}"))
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
