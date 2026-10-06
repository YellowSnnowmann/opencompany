//! Per-agent MCP tool permissions where they are enforced: the attachment a
//! company agent actually dials through, the registry decorator, and the gate's
//! read set.
//!
//! The rule itself is proved in the default lane
//! (`company::mcp_policy::agent::tests`). What needs the gated lane is that the
//! rule reaches the five seams — a rule nothing consults is a rule that does not
//! exist.

use std::sync::Arc;

use serde_json::json;

use super::built_in_test_fixtures::*;
use super::*;

use crate::company::mcp::{AuthMaterial, McpSource};
use crate::mcp::policy::{
    AgentToolPolicies, ApprovalMode, McpToolInventory, McpToolPolicies, ToolPolicy, ToolTier,
};

/// An endpoint nothing listens on, so a call that reached the transport would
/// fail loudly. Local fixtures for the same reason `mcp_families_tests` keeps its
/// own: the ones beside `mcp.rs` are private to that module.
const DEAD_ENDPOINT: &str = "http://127.0.0.1:1/mcp";

fn decl(name: &str, endpoint: &str) -> McpServerDecl {
    McpServerDecl {
        name: name.to_string(),
        endpoint: endpoint.to_string(),
        description: None,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        read_only_tools: Vec::new(),
        timeout_secs: 30,
        enabled: true,
        source: McpSource::Runtime,
        auth: AuthMaterial::None,
        tool_policies: McpToolPolicies::default(),
        tool_inventory: McpToolInventory::default(),
    }
}

fn grants(globs: &[&str]) -> Vec<String> {
    globs.iter().map(|g| (*g).to_string()).collect()
}

fn mode_only(mode: ApprovalMode) -> ToolPolicy {
    ToolPolicy {
        tier: None,
        mode: Some(mode),
    }
}

fn inventory(rows: &[(&str, ToolTier)]) -> McpToolInventory {
    McpToolInventory {
        tools: rows
            .iter()
            .map(|(tool, tier)| ((*tool).to_string(), *tier))
            .collect(),
        discovered_at_millis: 11,
    }
}

/// A notion-shaped server whose company document allows every read, with the
/// writer refused one of them.
fn server_with_a_writer_rule() -> McpServerDecl {
    let mut server = decl("notion", DEAD_ENDPOINT);
    server.tool_inventory = inventory(&[
        ("search_pages", ToolTier::ReadOnly),
        ("get_page", ToolTier::ReadOnly),
    ]);
    server
        .tool_policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    server.tool_policies.agents.insert(
        "writer".to_string(),
        AgentToolPolicies {
            overrides: [("search_pages".to_string(), mode_only(ApprovalMode::Blocked))]
                .into_iter()
                .collect(),
        },
    );
    server
}

/// What `AgentSpec::mcp` will carry, read back the only way the type allows: its
/// redacting `Debug`, which prints `disallowed_tools` verbatim.
fn attachment(servers: &[McpServerDecl], agent: &str) -> String {
    let attached = crate::mcp::agent::embed_servers_for_agent(servers, agent, &grants(&["mcp:*"]));
    assert_eq!(attached.len(), 1);
    format!("{attached:?}")
}

/// The headline claim, asserted where it has to hold: the attachment. One
/// teammate's refusal reaches only that teammate's attached server, and the
/// other teammate's is untouched.
#[test]
fn a_blocked_for_one_agent_tool_is_denied_only_on_that_agents_attachment() {
    let servers = vec![server_with_a_writer_rule()];

    let writer = attachment(&servers, "writer");
    let engineer = attachment(&servers, "engineer");

    assert!(
        writer.contains(r#"disallowed_tools: ["search_pages"]"#),
        "the writer's own refusal must reach its attachment: {writer}"
    );
    assert!(
        engineer.contains("disallowed_tools: []"),
        "one teammate's rule must not reach another's attachment: {engineer}"
    );
}

/// A company document nobody has written a per-agent rule into builds the
/// attachment it built before, for every teammate — including ids that are not on
/// any roster.
#[test]
fn an_upgraded_company_builds_the_same_attachment() {
    let mut server = decl("notion", DEAD_ENDPOINT);
    server.disallowed_tools = vec!["debug_dump".to_string()];
    server.tool_inventory = inventory(&[
        ("search_pages", ToolTier::ReadOnly),
        ("delete_page", ToolTier::WriteDelete),
    ]);
    server
        .tool_policies
        .tier_defaults
        .insert(ToolTier::WriteDelete, ApprovalMode::Blocked);
    let servers = vec![server];

    let expected = r#"disallowed_tools: ["debug_dump", "delete_page"]"#;
    for agent in ["engineer", "writer", "", "nobody-by-this-id"] {
        let debug = attachment(&servers, agent);
        assert!(
            debug.contains(expected),
            "`{agent}` must get the company deny list, in order: {debug}"
        );
    }
}

/// The bridge tool's own set is built for one teammate, so it cannot answer for
/// another.
#[test]
fn the_bridge_tools_policy_set_is_built_for_one_teammate() {
    let servers = vec![server_with_a_writer_rule()];
    let reach = grants(&["mcp:*"]);

    let writer = crate::mcp::agent::granted_policies(&servers, "writer", &reach);
    let engineer = crate::mcp::agent::granted_policies(&servers, "engineer", &reach);

    assert!(writer.is_blocked("notion", "search_pages"));
    assert!(!engineer.is_blocked("notion", "search_pages"));
    assert!(!writer.is_blocked("notion", "get_page"));
}

/// The gate's read set follows the per-agent mode, and an episode seat resolves
/// the same one the chat agent does.
///
/// Asserted on the resolved read set, never on parking: `ApprovalPolicy::check`
/// returns `Allow` at the `policy_hitl_enabled` bypass on this build, so a
/// per-agent `NeedsApproval` behaves as Allow and only the read set and the deny
/// list are observable.
#[test]
fn a_per_agent_approval_requirement_leaves_the_gates_read_set() {
    let mut fx = fixture();
    let mut server = decl("notion", DEAD_ENDPOINT);
    server.tool_inventory = inventory(&[("search_pages", ToolTier::ReadOnly)]);
    server
        .tool_policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    server.tool_policies.agents.insert(
        "engineer".to_string(),
        AgentToolPolicies {
            overrides: [(
                "search_pages".to_string(),
                mode_only(ApprovalMode::NeedsApproval),
            )]
            .into_iter()
            .collect(),
        },
    );
    fx.deps.mcp_servers = vec![server];
    let company = record();
    let roster = company.effective_agents();
    let engineer = roster
        .iter()
        .find(|agent| agent.id == "engineer")
        .expect("engineer is on the roster");
    let ceo = roster
        .iter()
        .find(|agent| agent.id == "ceo")
        .expect("ceo is on the roster");

    let engineer_reads = seat_policy(&company, &fx.deps, engineer)
        .mcp_reads()
        .clone();
    let ceo_reads = seat_policy(&company, &fx.deps, ceo).mcp_reads().clone();
    let engineer_chat = agent_policy_for(
        &company,
        &fx.deps,
        engineer,
        &company.effective_policy(),
        company.effective_budget(&engineer.id),
    );

    assert!(
        !engineer_reads.contains("notion", "search_pages"),
        "a per-agent approval requirement must leave this teammate's read set"
    );
    assert!(
        ceo_reads.contains("notion", "search_pages"),
        "and must not leave anybody else's"
    );
    assert_eq!(
        engineer_chat.mcp_reads(),
        &engineer_reads,
        "a seat and a chat agent must carry the same read set"
    );
}

// ---- what the prompt may claim -------------------------------------------

/// The state per-agent policy creates and company-wide policy barely could: a
/// teammate that reaches a server and can call nothing on it. Both halves
/// asserted together, because the prompt and the permission are the two things
/// that must not disagree — the attachment refuses every tool, and the brief says
/// so instead of describing a server the agent will only be refused by.
#[test]
fn a_hive_agents_prompt_names_no_server_it_cannot_call() {
    let mut server = decl("notion", DEAD_ENDPOINT);
    server.tool_inventory = inventory(&[
        ("search_pages", ToolTier::ReadOnly),
        ("delete_page", ToolTier::WriteDelete),
    ]);
    server.tool_policies.agents.insert(
        "writer".to_string(),
        AgentToolPolicies {
            overrides: [
                ("search_pages".to_string(), mode_only(ApprovalMode::Blocked)),
                ("delete_page".to_string(), mode_only(ApprovalMode::Blocked)),
            ]
            .into_iter()
            .collect(),
        },
    );
    let servers = vec![server];
    let reach = grants(&["mcp:notion"]);

    let writer_attachment = attachment(&servers, "writer");
    let engineer_attachment = attachment(&servers, "engineer");
    assert!(
        writer_attachment.contains(r#"disallowed_tools: ["delete_page", "search_pages"]"#),
        "{writer_attachment}"
    );
    assert!(
        engineer_attachment.contains("disallowed_tools: []"),
        "{engineer_attachment}"
    );

    let writer_brief =
        crate::mcp::decl::families::server_family_brief(&servers, &[], &reach, "writer");
    let engineer_brief =
        crate::mcp::decl::families::server_family_brief(&servers, &[], &reach, "engineer");

    assert!(writer_brief.contains("`notion`"), "{writer_brief}");
    assert!(
        writer_brief.contains("refused to you"),
        "the writer must be told, not left to discover it from an empty listing: {writer_brief}"
    );
    assert!(engineer_brief.contains("`notion`"), "{engineer_brief}");
    assert!(
        !engineer_brief.contains("refused to you"),
        "{engineer_brief}"
    );
}

/// The boundary the refusal clause must not cross: the brief names servers and
/// the key that addresses each, never an individual remote tool. A half-written
/// remote name is one a model may pass back verbatim and be refused for, and the
/// tool-name channel in a system prompt belongs to the internal `opencompany`
/// server alone.
#[test]
fn the_server_family_brief_never_names_a_remote_mcp_tool() {
    let mut refused = decl("notion", DEAD_ENDPOINT);
    refused.tool_inventory = inventory(&[
        ("search_pages", ToolTier::ReadOnly),
        ("delete_page", ToolTier::WriteDelete),
    ]);
    refused.tool_policies.agents.insert(
        "writer".to_string(),
        AgentToolPolicies {
            overrides: [
                ("search_pages".to_string(), mode_only(ApprovalMode::Blocked)),
                ("delete_page".to_string(), mode_only(ApprovalMode::Blocked)),
            ]
            .into_iter()
            .collect(),
        },
    );
    let remote_names: Vec<String> = refused.tool_inventory.tools.keys().cloned().collect();
    let servers = vec![refused];

    for agent in ["writer", "engineer"] {
        let brief = crate::mcp::decl::families::server_family_brief(
            &servers,
            &[],
            &grants(&["mcp:notion"]),
            agent,
        );
        for name in &remote_names {
            assert!(
                !brief.contains(name.as_str()),
                "the brief named the remote tool `{name}` for `{agent}`: {brief}"
            );
        }
        assert!(
            crate::harness::build::tools_named_in_mcp_brief(&brief).is_empty(),
            "the server-family brief must not parse as the opencompany tool brief: {brief}"
        );
    }
}

// ---- the registry sibling ------------------------------------------------

/// A minimal store the test seeds, so the policy the decorator reads is the one
/// the test wrote.
#[derive(Default)]
struct MemSecrets {
    map: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[async_trait::async_trait]
impl crate::ports::SecretStore for MemSecrets {
    async fn get(
        &self,
        _company: &crate::ports::types::CompanyId,
        key: &str,
    ) -> crate::Result<Option<crate::ports::types::SecretValue>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(key)
            .map(|raw| crate::ports::types::SecretValue(raw.clone())))
    }
    async fn set(
        &self,
        _company: &crate::ports::types::CompanyId,
        key: &str,
        value: crate::ports::types::SecretValue,
    ) -> crate::Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.0);
        Ok(())
    }
}

/// A registry install's tool is addressed by an argument at call time, so the
/// decorator reads the document itself — for the teammate it was wired for.
#[tokio::test]
async fn a_registry_install_blocked_for_one_agent_refuses_through_the_scoped_tool() {
    use tinytools::Tool;

    let mut policies = McpToolPolicies::default();
    policies.agents.insert(
        "writer".to_string(),
        AgentToolPolicies {
            overrides: [("delete_page".to_string(), mode_only(ApprovalMode::Blocked))]
                .into_iter()
                .collect(),
        },
    );
    let secrets = Arc::new(MemSecrets::default());
    let company = crate::ports::types::CompanyId::new("acme");
    crate::ports::SecretStore::set(
        secrets.as_ref(),
        &company,
        &crate::mcp::policy::registry_tool_policies_key("notion-install"),
        crate::ports::types::SecretValue(serde_json::to_string(&policies).expect("serializes")),
    )
    .await
    .expect("seeded");

    let args = json!({ "server_id": "notion-install", "tool_name": "delete_page" });
    let refusals: Vec<(&str, bool)> = {
        let mut out = Vec::new();
        for agent in ["writer", "engineer"] {
            let tool = crate::mcp::agent::OcMcpRegistryScopedTool::new(
                Box::new(NeverDials),
                agent.to_string(),
                grants(&["mcp_registry"]),
                company.clone(),
                Some(secrets.clone()),
            );
            let result = tool.execute(args.clone()).await.expect("executed");
            out.push((agent, result.output().contains("blocked")));
        }
        out
    };

    assert_eq!(
        refusals,
        vec![("writer", true), ("engineer", false)],
        "the install's per-agent block must refuse the writer and only the writer"
    );
}

/// An inner tool that records nothing and dials nothing: reaching it at all is
/// the observation that the decorator let the call through.
struct NeverDials;

#[async_trait::async_trait]
impl tinytools::Tool for NeverDials {
    fn name(&self) -> &str {
        "mcp_registry_tool_call"
    }
    fn description(&self) -> &str {
        "test double"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<tinytools::ToolResult> {
        Ok(tinytools::ToolResult::success("delegated"))
    }
    fn permission_level(&self) -> tinytools::PermissionLevel {
        tinytools::PermissionLevel::Execute
    }
}

/// The policy a roster agent runs under, resolved from the company's policy
/// and budget in force (what the retired episode seat was built with).
fn seat_policy(
    company: &CompanyRecord,
    deps: &HarnessDeps,
    agent: &ManifestAgent,
) -> crate::harness::policy::ApprovalPolicy {
    agent_policy_for(
        company,
        deps,
        agent,
        &company.effective_policy(),
        company.effective_budget(&agent.id),
    )
}
