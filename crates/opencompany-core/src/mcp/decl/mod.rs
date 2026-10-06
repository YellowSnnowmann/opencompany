//! Per-tenant MCP tool servers: the inert, ungated data model plus the pure
//! merge/validation and the async secret-resolution used to materialize a
//! company's *effective* MCP servers (issue #50).
//!
//! A company's effective MCP servers are the union of three sources, merged
//! lowest-precedence first by [`effective_mcp_servers`]:
//!
//! 1. **Default** — the install-wide `[[default_mcp_server]]` entries in the
//!    instance `config.toml`, shipped enabled by a packaged Open Company so a
//!    fresh install has working tools with no user setup (issue #527). They
//!    apply to *every* company on the install and are normalized once, at the
//!    config boundary, by [`normalize_default_servers`].
//! 2. **Manifest** — the `[[mcp_server]]` entries committed in `company.toml`
//!    ([`McpServer`]). Declarative intent; never a credential. A manifest entry
//!    shadows a default of the same name: the company said something specific.
//! 3. **Runtime** — servers the operator adds through the console, persisted as
//!    a single JSON index in the [`SecretStore`](crate::ports::SecretStore)
//!    under [`RUNTIME_INDEX_KEY`]. A runtime entry with the *same name* as a
//!    manifest **or default** server is an **override** (enable/disable, tool
//!    allow-list) — the body wins, the lower layer keeps the provenance badge.
//!
//! Credentials live apart from the declarations: a server's outbound token is
//! written to its own per-server key ([`auth_key`]) — never inline in the index
//! or the manifest — and is resolved into [`AuthMaterial`] only at harness build
//! time by [`resolve_effective`]. Nothing here ever serializes a credential into
//! an API response, log line, or agent-visible output.
//!
//! Hosted v1 boundary: **HTTP transport only**. A server that declares a stdio
//! `command` is rejected by [`validate_servers`].

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::company::McpServer;
use crate::error::OpenCompanyError;
use crate::ports::SecretStore;
use crate::ports::types::{CompanyId, SecretValue};

/// The one rule that decides whether two MCP records name the same server,
/// shared by the console's server list and the agent prompt that tells a model
/// which dispatch tool reaches which server.
pub(crate) mod endpoint;
/// Which of an agent's two MCP dispatch tools reaches which connected server,
/// rendered for its system prompt. Ungated: the prompt is composed from company
/// data, and the rule is worth testing without a harness build.
#[cfg_attr(not(all(feature = "openhuman", feature = "mcp")), allow(dead_code))]
pub(crate) mod families;
/// The bundle's MCP declaration file: `companies/<name>/mcp.json`. A vertical
/// ships the tool servers its work needs the way it already ships its ledgers,
/// rather than starting with an empty tool surface somebody has to fill in by
/// hand from the console before the company can do anything.
pub mod file;
/// What an MCP server says about itself — its own title, description, website
/// and icon, read off the `serverInfo` block of its `initialize` reply and kept
/// beside its health record.
pub mod server_info;
mod store;
mod validate;

pub use store::{
    auth_configured, clear_auth, clear_health, load_auth, load_health, load_runtime_index,
    resolve_effective, save_health, save_runtime_index, store_auth, store_bearer,
};
pub(crate) use validate::has_query_credential;
use validate::normalize_tools;
pub use validate::{
    RESERVED_SERVER_NAMES, endpoint_secret_advisory, normalize_default_servers, validate_one,
    validate_servers,
};

/// The [`SecretStore`](crate::ports::SecretStore) key holding the JSON runtime
/// server index (a `Vec<McpServer>` of console-added servers + manifest
/// overrides).
pub const RUNTIME_INDEX_KEY: &str = "mcp/servers";

/// The per-request timeout a server declaration gets when it names none.
///
/// Lives here rather than beside [`McpServer`] because every declaration path
/// defaults it — the manifest through serde, `mcp.json` through
/// [`file`] — and two spellings of "30" is how they come to disagree.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// The canonical per-server credential key. A server's outbound token is stored
/// here (write-only via the console); the value is a JSON [`StoredAuth`].
pub fn auth_key(name: &str) -> String {
    format!("mcp/{name}/auth")
}

/// The per-server health key. Holds the last probe outcome as a JSON
/// [`McpHealth`]. **Invariant**: the value written here is always scrubbed — it
/// is a non-secret status record and MUST NEVER carry a credential (see
/// [`save_health`]). Distinct from [`auth_key`], which holds the write-only
/// credential and is never read back out.
pub fn health_key(name: &str) -> String {
    format!("mcp/{name}/health")
}

/// Where an effective server declaration came from — drives the console's source
/// badge and the delete-guard (a manifest or default server cannot be deleted,
/// only disabled/overridden).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpSource {
    /// Declared in `company.toml`'s `[[mcp_server]]`.
    Manifest,
    /// Added at runtime through the console.
    Runtime,
    /// Shipped enabled by the packaged install: a `[[default_mcp_server]]` entry
    /// in the instance `config.toml` (issue #527). Present for every company on
    /// the install, which is what makes it a distinct provenance rather than a
    /// flavour of [`Self::Manifest`] — nobody wrote it into *this* company, and
    /// the console must not label it as operator-added.
    Default,
    /// Installed from an upstream MCP directory — Smithery.ai or the official
    /// `modelcontextprotocol/registry` — through the console's browse surface
    /// (issue #1270).
    ///
    /// Distinct from [`Self::Runtime`] because the two are keyed differently and
    /// deleted differently: a runtime server is addressed by `name` and lives in
    /// this company's runtime index, while a registry install is addressed by a
    /// stable `server_id` and lives in OpenHuman's own store, so removing one
    /// means uninstalling it there rather than dropping an index row.
    Registry,
}

/// The operator-facing refusal for a directory entry that can only run as a
/// local stdio subprocess (issue #1270).
///
/// Says *why* rather than "unsupported": the blocker is not the read-only root
/// filesystem (tenants mount a writable `/data` and an emptyDir `/tmp`) but that
/// the tenant image carries no Node, Python or package manager to launch one
/// with — a stdio install would fail on `npx: not found`. One function so the
/// two places that can refuse an install — the catalogue pre-check at the route
/// and the post-install belt in
/// [`McpRuntime`](crate::harness::mcp::McpRuntime) — say the same sentence, and
/// so the day a sidecar makes stdio runnable there is one message to retire.
pub fn stdio_install_refusal(qualified_name: &str) -> String {
    format!(
        "`{qualified_name}` offers no hosted HTTP endpoint — it can only run as a local \
         subprocess, and this deployment ships no Node, Python or package manager to launch \
         one. Pick a server with a hosted endpoint, or add it by URL if you host it yourself."
    )
}

/// Resolved outbound auth material for one MCP server.
///
/// This is the *in-process* resolved credential, filled from the
/// [`SecretStore`](crate::ports::SecretStore) at harness-build time. It defaults
/// to [`AuthMaterial::None`] and is **never** serialized anywhere agent- or
/// operator-visible (it derives no `Serialize`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AuthMaterial {
    /// No outbound auth.
    #[default]
    None,
    /// `Authorization: Bearer <token>`.
    Bearer(String),
    /// A single custom request header.
    Header { name: String, value: String },
    /// A credential carried as a URL query parameter (`?<name>=<value>`), the
    /// BrowserBase / Parallel-Search style. The upstream transport already
    /// applies this via `request.query()` (`mcp_client/client.rs`), so wiring it
    /// needs zero vendor changes — but it means the credential ends up in the
    /// request URL, which is exactly why the error-surfacing seams strip query
    /// strings before persisting or emitting anything (see
    /// [`crate::redact::scrub`]).
    QueryParam { name: String, value: String },
    /// An OAuth 2.0 (authorization-code + PKCE) credential obtained through the
    /// console's browser sign-in flow ([`crate::company::mcp_oauth`]). The
    /// resolved `access_token` is sent to the transport as an
    /// `Authorization: Bearer` (the harness `auth_config` mapping); the
    /// remaining fields are the bookkeeping the OAuth refresh path needs to mint
    /// a fresh access token without another browser round-trip.
    ///
    /// **Security**: every token field is enumerated by [`Self::secret_values`],
    /// so the access token, the refresh token, and any confidential
    /// `client_secret` all feed the scrubber and can never survive into an
    /// agent-visible error, health record, or API response.
    OAuth {
        /// The bearer access token sent to the server (short-lived, ≈1h).
        access_token: String,
        /// The refresh token, when the authorization server issued one. Absent
        /// servers force a fresh browser sign-in once the access token expires.
        refresh_token: Option<String>,
        /// The dynamically-registered client id (RFC 7591).
        client_id: String,
        /// The confidential client secret, when the server issued one.
        client_secret: Option<String>,
        /// The token endpoint a refresh POSTs to.
        token_endpoint: String,
        /// Unix seconds when `access_token` expires (best-effort).
        expires_at: u64,
    },
}

impl AuthMaterial {
    /// Whether any credential is configured (for the non-secret
    /// `auth_configured` status field). Never reveals the value.
    pub fn is_configured(&self) -> bool {
        !matches!(self, AuthMaterial::None)
    }

    /// The concrete credential substrings this material carries, for the
    /// scrubber's known-secret set. Never surfaced to any caller that
    /// serializes — used only to feed [`crate::redact::scrub`].
    pub fn secret_values(&self) -> Vec<String> {
        match self {
            AuthMaterial::None => Vec::new(),
            AuthMaterial::Bearer(token) => vec![token.clone()],
            AuthMaterial::Header { value, .. } => vec![value.clone()],
            AuthMaterial::QueryParam { value, .. } => vec![value.clone()],
            // CRITICAL: every OAuth token substring must feed the scrubber —
            // the access token (sent as the bearer), the refresh token, and any
            // confidential client secret. Missing one would let it survive into
            // an error/health/agent-visible surface.
            AuthMaterial::OAuth {
                access_token,
                refresh_token,
                client_secret,
                ..
            } => {
                let mut out = vec![access_token.clone()];
                if let Some(refresh) = refresh_token {
                    out.push(refresh.clone());
                }
                if let Some(secret) = client_secret {
                    out.push(secret.clone());
                }
                out
            }
        }
    }
}

/// The on-disk credential envelope stored under [`auth_key`]. Kept private —
/// only [`resolve_effective`] / [`store_bearer`] cross this boundary.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredAuth {
    Bearer {
        token: String,
    },
    Header {
        name: String,
        value: String,
    },
    QueryParam {
        name: String,
        value: String,
    },
    /// The persisted OAuth bundle: the access token plus everything a silent
    /// refresh needs. Written by the callback exchange and the refresh path;
    /// read only by [`load_auth`] at harness-build / probe time.
    Oauth {
        access_token: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        client_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_secret: Option<String>,
        token_endpoint: String,
        expires_at: u64,
    },
}

impl From<StoredAuth> for AuthMaterial {
    fn from(stored: StoredAuth) -> Self {
        match stored {
            StoredAuth::Bearer { token } => AuthMaterial::Bearer(token),
            StoredAuth::Header { name, value } => AuthMaterial::Header { name, value },
            StoredAuth::QueryParam { name, value } => AuthMaterial::QueryParam { name, value },
            StoredAuth::Oauth {
                access_token,
                refresh_token,
                client_id,
                client_secret,
                token_endpoint,
                expires_at,
            } => AuthMaterial::OAuth {
                access_token,
                refresh_token,
                client_id,
                client_secret,
                token_endpoint,
                expires_at,
            },
        }
    }
}

/// One effective MCP server declaration for a company — the merge of a manifest
/// [`McpServer`] and any runtime override, with auth resolved to
/// [`AuthMaterial`] at harness-build time.
#[derive(Clone, Debug)]
pub struct McpServerDecl {
    /// Stable slug used by the bridge tools + console.
    pub name: String,
    /// HTTP(S) endpoint URL.
    pub endpoint: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Allow-list of remote tool names (empty = all, minus `disallowed_tools`).
    pub allowed_tools: Vec<String>,
    /// Deny-list of remote tool names (takes precedence).
    pub disallowed_tools: Vec<String>,
    /// Remote tool names the operator declares **read-only** on this server
    /// (issue #1124). Merged through [`effective_mcp_servers`] and editable
    /// through the runtime-override layer exactly as the two lists above are, so
    /// an operator expresses it without hand-editing the manifest. A call to a
    /// tool named here does not park under `auto`; every other bridge call does.
    pub read_only_tools: Vec<String>,
    /// Per-request timeout in seconds.
    pub timeout_secs: u64,
    /// Whether this server is exposed to agents.
    pub enabled: bool,
    /// Manifest vs runtime provenance.
    pub source: McpSource,
    /// Resolved outbound credential (`None` until [`resolve_effective`] fills it).
    pub auth: AuthMaterial,
    /// The operator's stored per-tool approval policy, layered over
    /// [`read_only_tools`](Self::read_only_tools) by
    /// [`effective_policies`](crate::mcp::policy::effective_policies). Empty
    /// until [`resolve_effective`] fills it.
    pub tool_policies: crate::mcp::policy::McpToolPolicies,
    /// The tools discovery last saw on this server and the tier each was
    /// suggested under. Empty until [`resolve_effective`] fills it, and empty
    /// for a server discovery has never reached.
    pub tool_inventory: crate::mcp::policy::McpToolInventory,
}

impl McpServerDecl {
    fn from_server(server: &McpServer, source: McpSource) -> Self {
        Self {
            name: server.name.trim().to_string(),
            endpoint: server.endpoint.trim().to_string(),
            description: server.description.clone(),
            allowed_tools: normalize_tools(&server.allowed_tools),
            disallowed_tools: normalize_tools(&server.disallowed_tools),
            read_only_tools: normalize_tools(&server.read_only_tools),
            timeout_secs: server.timeout_secs,
            enabled: server.enabled,
            source,
            auth: AuthMaterial::None,
            tool_policies: crate::mcp::policy::McpToolPolicies::default(),
            tool_inventory: crate::mcp::policy::McpToolInventory::default(),
        }
    }
}

/// Merges the install defaults, the manifest servers and the runtime index into
/// the effective set.
///
/// Three layers, lowest to highest: **default** (install-wide, issue #527) →
/// **manifest** (this company's `company.toml`) → **runtime** (console edits).
/// A higher layer overriding a lower one replaces the body — its
/// enable/disable + tool lists win — but the declaration keeps the **lowest**
/// layer's badge, so the console still shows where the server came from and
/// still refuses to delete it. That is the rule the manifest/runtime pair
/// already followed; defaults join it rather than introducing a second one.
///
/// A name in both the defaults and the manifest resolves to the manifest: a
/// company that declares a server has said something specific about it, and the
/// install-wide default is the fallback it overrides.
///
/// Order is manifest first (in declared order), then defaults the manifest did
/// not shadow, then runtime-only additions. Manifest stays first so an install
/// with no defaults configured produces a byte-identical list to before.
///
/// Auth is left [`AuthMaterial::None`]; [`resolve_effective`] fills it.
pub fn effective_mcp_servers(
    defaults: &[McpServer],
    manifest: &[McpServer],
    runtime: &[McpServer],
) -> Vec<McpServerDecl> {
    let mut out: Vec<McpServerDecl> = Vec::new();

    // The body that actually applies for `name`: the runtime override when the
    // console has one, else the layer's own declaration. Shared by the manifest
    // and default passes so the override rule cannot drift between them.
    let with_override = |own: &McpServer, name: &str, source: McpSource| match runtime
        .iter()
        .find(|r| r.name.trim() == name)
    {
        Some(override_entry) => McpServerDecl::from_server(override_entry, source),
        None => McpServerDecl::from_server(own, source),
    };

    for m in manifest {
        let name = m.name.trim();
        if name.is_empty() {
            continue;
        }
        out.push(with_override(m, name, McpSource::Manifest));
    }

    for d in defaults {
        let name = d.name.trim();
        if name.is_empty() || manifest.iter().any(|m| m.name.trim() == name) {
            continue;
        }
        out.push(with_override(d, name, McpSource::Default));
    }

    for r in runtime {
        let name = r.name.trim();
        if name.is_empty()
            || manifest.iter().any(|m| m.name.trim() == name)
            || defaults.iter().any(|d| d.name.trim() == name)
        {
            continue;
        }
        out.push(McpServerDecl::from_server(r, McpSource::Runtime));
    }

    out
}

/// Flattens a company's effective MCP servers into the `(server, tool)` pairs
/// their `read_only_tools` declarations name.
///
/// Test-only: it ignores the stored tool policy, so it is the differential
/// oracle for [`mcp_allow_set`](crate::mcp::policy::mcp_allow_set), which is
/// what every gate reads. A disabled server contributes nothing.
#[cfg(test)]
pub fn mcp_read_set(servers: &[McpServerDecl]) -> crate::policy::McpReadSet {
    crate::policy::McpReadSet::from_pairs(servers.iter().filter(|s| s.enabled).flat_map(|server| {
        server
            .read_only_tools
            .iter()
            .map(move |tool| (server.name.clone(), tool.clone()))
    }))
}

/// The coarse health status of an MCP server, shown as the console badge.
///
/// Serialized `snake_case`; the frontend maps it to a green/amber/red tier. Kept
/// deliberately small — a single actionable status plus an operator-facing
/// (scrubbed) message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpStatus {
    /// Reached the server and listed its tools — fully working.
    Ok,
    /// The server is reachable but needs a credential the operator hasn't
    /// supplied (401 with no/rejected credential, or an OAuth challenge). A
    /// valid, expected resting state for a just-added server — never a rollback.
    NeedsConfig,
    /// The server could not be used: unreachable, wrong URL, not an MCP
    /// endpoint, a 5xx, a TLS failure, or a rejected call.
    Error,
    /// The probe did not run (non-`openhuman` build, or never attempted).
    Unknown,
}

impl McpStatus {
    /// The stable wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            McpStatus::Ok => "ok",
            McpStatus::NeedsConfig => "needs_config",
            McpStatus::Error => "error",
            McpStatus::Unknown => "unknown",
        }
    }
}

/// The last probe outcome for one MCP server.
///
/// **Security invariant**: `message` is always scrubbed before it reaches this
/// struct (via [`crate::redact::scrub`]) and this struct is the only
/// thing [`save_health`] persists — so a credential can never land in the health
/// key, the console, or an API response. `auth_hint` is a stable reason code
/// (`oauth_required` / `token_rejected` / `credential_required`), never a URL or
/// raw challenge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpHealth {
    /// The coarse status tier.
    pub status: McpStatus,
    /// A short, scrubbed, operator-facing message.
    pub message: String,
    /// How many tools the server advertised on a successful probe.
    pub tool_count: u32,
    /// Epoch-millis timestamp of the probe.
    pub checked_at_millis: u64,
    /// Stable auth-failure reason code, when the status is a credential problem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_hint: Option<String>,
}

#[cfg(test)]
#[path = "decl_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "decl_store_tests.rs"]
mod store_tests;
