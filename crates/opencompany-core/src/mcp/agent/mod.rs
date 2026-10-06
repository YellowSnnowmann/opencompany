//! Per-agent MCP registry assembly and the hardened `mcp_call_tool` (issue #50).
//!
//! [`registry_for_agent`] folds a company's effective [`McpServerDecl`]s into an
//! OpenHuman [`McpServerRegistry`](oh::mcp::config_servers::McpServerRegistry) scoped to
//! one agent's `mcp:*` tool grants. The registry reuses upstream's HTTP
//! transport and its input-validation safety filter (`apply_safety_filter`),
//! so remote tool metadata is scanned for prompt-injection before an agent ever
//! sees it.
//!
//! **Security**: `mcp_list_servers` is upstream's own
//! [`McpListServersTool`](tinymcp::tools::McpListServersTool), which reports only
//! a non-secret `auth_configured` / `auth_kind` per server — this module used to
//! carry a redacting replacement from when upstream serialized `server.auth`.
//! [`OcMcpCallTool`] scrubs both failed *and* successful results against the
//! server's own credentials ([`tinymcp::tools::SecretScrubber`]), since a
//! server can reflect its credential into a normal response.
//!
//! Compiled only under `feature = "openhuman"`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use openhuman_core as oh;

use oh::config::{Config, McpAuthConfig, McpServerConfig};
use oh::mcp::config_servers::McpServerRegistry;
use oh::security::{SecurityPolicy, ToolOperation};
use tinytools::{PermissionLevel, Tool, ToolCallOptions, ToolResult};

use crate::company::mcp::{AuthMaterial, McpServerDecl};
use crate::mcp::probe::{McpFailure, McpFailureQueue, classify_mcp_error, operator_message};
use crate::ports::types::CompanyId;
use crate::ports::usage::UsageMeter;
use crate::redact::scrub;
use crate::runtime::tools::grants_cover_server;

mod registry_list;
mod registry_scoped;

pub use registry_list::OcMcpRegistryInstalledListTool;
pub use registry_scoped::OcMcpRegistryScopedTool;

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

/// The MCP registry scoped to one agent, or `None` when the agent is granted no
/// (enabled) MCP servers.
///
/// An agent reaches a server named `<slug>` only when its effective `grants`
/// (already narrowed by [`agent_effective_grants`]) match `mcp:<slug>` (a bare
/// `mcp:*` grants all). Disabled servers are excluded. Returns `None` (not an
/// empty registry) so the caller can skip pushing the MCP bridge tools entirely
/// for an agent with no MCP surface.
///
/// [`agent_effective_grants`]: crate::runtime::builder::agent_effective_grants
pub fn registry_for_agent(
    decls: &[McpServerDecl],
    grants: &[String],
) -> Option<Arc<McpServerRegistry>> {
    let granted: Vec<McpServerDecl> = decls
        .iter()
        .filter(|decl| decl.enabled && grants_cover_server(grants, &decl.name))
        .cloned()
        .collect();
    if granted.is_empty() {
        return None;
    }
    let registry = registry_from_decls(&granted);
    if registry.is_empty() {
        None
    } else {
        Some(Arc::new(registry))
    }
}

/// The credential substrings from the (enabled, grant-matched) servers this
/// agent reaches — the known-secret set fed to
/// [`scrub`](crate::redact::scrub) so no configured credential can
/// survive into an agent-visible error. `grants` must be the same effective
/// grants passed to [`registry_for_agent`], rather than the raw manifest
/// request, because an empty request inherits the company belt and therefore
/// reaches every server that belt grants.
/// Never serialized anywhere.
pub fn granted_secrets(decls: &[McpServerDecl], grants: &[String]) -> Vec<String> {
    decls
        .iter()
        .filter(|decl| decl.enabled && grants_cover_server(grants, &decl.name))
        .flat_map(|decl| decl.auth.secret_values())
        .collect()
}

/// The per-tool policies for the servers an agent's grants reach, resolved for
/// that agent.
pub fn granted_policies(
    decls: &[McpServerDecl],
    agent: &str,
    grants: &[String],
) -> crate::mcp::policy::McpToolPolicySet {
    crate::mcp::policy::McpToolPolicySet::from_declarations(
        agent,
        decls
            .iter()
            .filter(|decl| grants_cover_server(grants, &decl.name)),
    )
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
/// attachments an [`openhuman_embed::AgentSpec`] can carry directly (plan
/// hive-desks, Phase 2 follow-up).
///
/// # Why this exists alongside [`registry_for_agent`]
///
/// [`host_loop`](crate::harness::host_loop)'s module doc says it plainly:
/// "the company agents run on the embedded OpenHuman runtime, whose tool set
/// is its own (plus MCP servers) — there is no seam for a `Tool` this crate
/// built" for a company AGENT (as opposed to an in-process auxiliary pass).
/// [`OcMcpCallTool`] and upstream's `McpListToolsTool` are exactly such
/// tools — pushed onto
/// [`AgentBlueprint::tools`](crate::harness::built_in::build::AgentBlueprint::tools)
/// under the reserved names `mcp_call_tool` / `mcp_list_tools` so the OLD native-dispatch builder (`tool_dispatcher.rs`,
/// removed when the runtime moved to the hosted pipeline) would run OC's
/// decorator instead of OpenHuman's own implementation of those names.
///
/// That dispatch seam is gone. A name in
/// [`OPENHUMAN_NATIVE_TOOLS`](crate::harness::built_in::build::OPENHUMAN_NATIVE_TOOLS)
/// is now *always* OpenHuman's own implementation — reaching only whatever
/// [`McpServer`](openhuman_embed::McpServer)s were attached to the spec via
/// [`AgentSpec::mcp`](openhuman_embed::AgentSpec::mcp) — so `OcMcpCallTool`'s
/// registry (built from these same `decls`/`grants`) was never being called at
/// all: a company's own registered servers were unreachable, and
/// `mcp_call_tool` only ever found the internal `opencompany` hive server
/// (issue tracked alongside plan hive-desks Phase 3/4). This function is the
/// other half of that fix: it hands the SAME granted servers to
/// `agent_spec_for` so they reach the spec the way `AgentSpec::mcp` (plural —
/// "call repeatedly to add several") is meant to be used, alongside the
/// `opencompany` attachment.
///
/// **Known gap left open by this fix**: OpenHuman's own `mcp_call_tool` does
/// not scrub credentials the way `OcMcpCallTool`'s `handle_failure` does (see
/// this module's security note above) — a transport failure can surface a
/// configured bearer/token verbatim to the agent for a directly-attached
/// company server. `mcp_list_servers` is kept out of every company agent's
/// tool scope for the same reason. Restoring that hardening needs a real
/// seam into the hosted pipeline (a job for hive-desks Phase 4), not a
/// band-aid here; it is called out rather than silently reintroduced.
pub fn embed_servers_for_agent(
    decls: &[McpServerDecl],
    agent: &str,
    grants: &[String],
) -> Vec<openhuman_embed::McpServer> {
    decls
        .iter()
        .filter(|decl| decl.enabled && grants_cover_server(grants, &decl.name))
        .map(|decl| {
            // A blocked tool is denied here, not only in `OcMcpCallTool`: this
            // attachment is the path a company agent actually takes, and the
            // deny list is what the transport filters on. Deny outranks allow
            // there, so a server with an allow list cannot re-admit one.
            //
            // Resolved for `agent`, so one teammate's refusal reaches only that
            // teammate's attachment.
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

/// What `mcp_call_tool` needs to record an `OauthCall` usage sample.
///
/// Mirrors [`ComposioMetering`](crate::harness::composio::ComposioMetering):
/// the company and agent the sample is scoped to, and a meter that may be
/// absent because the harness wires none in some embeddings — in which case the
/// tool still works and simply is not metered.
#[derive(Clone)]
pub struct McpMetering {
    /// The company the sample is scoped to.
    pub company: CompanyId,
    /// The agent whose turn made the call.
    pub agent: String,
    /// The usage meter. `None` leaves metering off entirely.
    pub meter: Option<Arc<dyn UsageMeter>>,
}

impl McpMetering {
    /// A handle that records nothing — for embeddings and tests that wire no
    /// meter. Named rather than spelled out at each call site so "unmetered" is
    /// a visible decision instead of a `None` a reader has to interpret.
    pub fn off() -> Self {
        Self {
            company: CompanyId::new("unmetered"),
            agent: String::new(),
            meter: None,
        }
    }
}

/// A hardening decorator around upstream's [`McpCallTool`](oh::tools::McpCallTool)
/// that keeps the same tool name + schema but turns a raw transport failure into
/// a **scrubbed, actionable** result and records it on a shared
/// [`McpFailureQueue`] the brain drains after the turn.
///
/// Upstream's tool surfaces `mcp_call_tool failed: {err}` verbatim — which can
/// carry a response body or (with query-parameter auth) the full request URL
/// including the credential. This decorator classifies the error, scrubs it
/// against the granted servers' known credentials, rewrites the agent-facing
/// text into a "don't retry blindly, tell the operator" directive, and pushes an
/// [`McpFailure`] so the operator sees a warning after the turn.
pub struct OcMcpCallTool {
    registry: Arc<McpServerRegistry>,
    security: Arc<SecurityPolicy>,
    /// Known credential substrings from the agent's granted servers, fed to
    /// [`scrub`] so no configured secret can survive into agent-visible output.
    secrets: Vec<String>,
    /// The shared failure queue the brain drains after the turn.
    failures: McpFailureQueue,
    /// Where a completed call is counted (issue #698). See
    /// [`McpMetering`].
    metering: McpMetering,
    /// The granted servers' per-tool policies, consulted before dialling.
    policies: crate::mcp::policy::McpToolPolicySet,
}

impl OcMcpCallTool {
    /// Builds the decorator over the agent's registry, the (permissive) MCP
    /// security policy, the granted servers' credential substrings, the shared
    /// failure queue, and the metering handle.
    pub fn new(
        registry: Arc<McpServerRegistry>,
        security: Arc<SecurityPolicy>,
        secrets: Vec<String>,
        failures: McpFailureQueue,
        metering: McpMetering,
        policies: crate::mcp::policy::McpToolPolicySet,
    ) -> Self {
        Self {
            registry,
            security,
            secrets,
            failures,
            metering,
            policies,
        }
    }

    /// Whether the named server has a credential configured (drives the
    /// 401-vs-rejected classification without reading the credential).
    fn auth_configured(&self, server: &str) -> bool {
        self.registry
            .get(server)
            .map(|s| !matches!(s.auth, tinymcp::McpAuthConfig::None))
            .unwrap_or(false)
    }

    /// Classify + scrub + record a failed call, returning the agent-facing error
    /// result. The pushed [`McpFailure`] and the returned text are both scrubbed.
    fn handle_failure(&self, server: &str, tool: &str, err: &anyhow::Error) -> ToolResult {
        let class = classify_mcp_error(err, self.auth_configured(server), true);
        let scrubbed = scrub(&operator_message(server, &class, err), &self.secrets);
        self.failures.push(McpFailure {
            server: server.to_string(),
            tool: tool.to_string(),
            status: class.code(),
            hint: class.auth_hint.clone(),
            scrubbed_message: scrubbed.clone(),
        });
        // The agent-facing directive: don't retry blindly, surface to operator.
        let agent_text = scrub(
            &format!(
                "The MCP call to '{server}' (tool '{tool}') did not succeed. {scrubbed} Do not retry blindly — surface this to the operator."
            ),
            &self.secrets,
        );
        ToolResult::error(agent_text)
    }
}

#[async_trait]
impl Tool for OcMcpCallTool {
    fn name(&self) -> &str {
        "mcp_call_tool"
    }

    fn description(&self) -> &str {
        "Call a tool on a named remote MCP server. First inspect available tools with `mcp_list_tools`, then pass the remote tool name and its JSON arguments here."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Registered MCP server name, from the granted servers named in your persona brief."
                },
                "tool": {
                    "type": "string",
                    "description": "Remote MCP tool name from `mcp_list_tools`."
                },
                "arguments": {
                    "type": "object",
                    "description": "Arguments object passed through to the remote MCP tool."
                }
            },
            "required": ["server", "tool", "arguments"],
            "additionalProperties": false
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Execute
    }

    fn supports_markdown(&self) -> bool {
        true
    }

    async fn execute_with_options(
        &self,
        args: Value,
        options: ToolCallOptions,
    ) -> anyhow::Result<ToolResult> {
        self.security
            .enforce_tool_operation(ToolOperation::Act, self.name())
            .map_err(|err| anyhow::anyhow!(err))?;

        let server = required_string_arg(&args, "server")?;
        let tool = required_string_arg(&args, "tool")?;
        // Gated on the cleaned names, which are the ones that would be
        // dispatched. Placed here rather than on one of the other two entry
        // points because both default to this one.
        if self.policies.is_blocked(&server, &tool) {
            return Ok(ToolResult::error(crate::mcp::policy::blocked_refusal(
                &server, &tool,
            )));
        }
        let arguments = args
            .get("arguments")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing required `arguments` object"))?;
        if !arguments.is_object() {
            return Ok(ToolResult::error("`arguments` must be an object"));
        }

        match self.registry.call_tool(&server, &tool, arguments).await {
            Ok(result) => {
                // Metered on success only, mirroring `composio_execute`: a call
                // that actually reached the server. `connections` in the read
                // model is the count of providers seen, so counting a failed
                // call would mint a connection row for a server that never
                // answered (issue #698). One line by design — see the module
                // docs on `crate::metering::oauth` for why the shape and the
                // swallow live there rather than here.
                if let Some(meter) = &self.metering.meter {
                    crate::metering::record_oauth_call(
                        meter.as_ref(),
                        &self.metering.company,
                        &self.metering.agent,
                        &crate::metering::mcp_provider(&server),
                        crate::ports::now_millis(),
                    )
                    .await;
                }
                // Use tinymcp's shared conversion rather than reimplementing
                // the mapping of output text, metadata, and error state.
                let result: ToolResult = tinymcp::tools::tool_result(result.rendered);
                // A successful response can still reflect the server's own
                // credential (an echoed header, a URL with its query-string
                // key); scrub it the same way a failure is scrubbed.
                let mut result =
                    tinymcp::tools::SecretScrubber::for_server(&self.registry, &server)
                        .scrub_result(result);
                if options.prefer_markdown && result.markdown_formatted.is_none() {
                    result.markdown_formatted = Some(result.output());
                }
                Ok(result)
            }
            Err(err) => Ok(self.handle_failure(&server, &tool, &anyhow::Error::new(err))),
        }
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        self.execute_with_options(args, ToolCallOptions::default())
            .await
    }
}

/// Pulls a required, non-empty string argument (mirrors upstream's private
/// helper of the same name).
fn required_string_arg(args: &Value, key: &str) -> anyhow::Result<String> {
    let value = args
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing required `{key}`"))?;
    // Models routinely wrap identifiers in markdown emphasis when they answer
    // in prose style (`server: \`werkplaats\``). A trailing backtick is part of
    // the markdown, not the name: strip wrapping / trailing fence characters
    // so the registry lookup matches the configured server name. Only *leading
    // and trailing* occurrences are removed — a legitimate name never starts
    // or ends with one of these, so stripping cannot mangle a real id.
    let cleaned = value
        .trim_start_matches(['`', '*', '_'])
        .trim_end_matches(['`', '*', '_', '.', ',', ';', ':', '!']);
    if cleaned.is_empty() {
        return Err(anyhow::anyhow!("missing required `{key}`"));
    }
    Ok(cleaned.to_string())
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_blocked_tests.rs"]
mod blocked_tests;
