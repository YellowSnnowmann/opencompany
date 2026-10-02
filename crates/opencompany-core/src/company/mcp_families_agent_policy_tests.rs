//! What the server-family brief says about a server one teammate is refused
//! everything on.
//!
//! Ungated, like the renderer, so the default lane runs it.

use super::*;
use crate::company::mcp::{AuthMaterial, McpSource};
use crate::company::mcp_policy::{
    AgentToolPolicies, ApprovalMode, McpToolInventory, McpToolPolicies, ToolPolicy, ToolTier,
};

const REFUSAL: &str = "refused to you";

fn decl(name: &str) -> McpServerDecl {
    McpServerDecl {
        name: name.to_string(),
        endpoint: format!("https://{name}.example/mcp"),
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

fn probed(rows: &[(&str, ToolTier)]) -> McpToolInventory {
    McpToolInventory {
        tools: rows
            .iter()
            .map(|(tool, tier)| ((*tool).to_string(), *tier))
            .collect(),
        discovered_at_millis: 3,
    }
}

fn blocked_for(agent: &str, tools: &[&str]) -> McpToolPolicies {
    let mut policies = McpToolPolicies::default();
    policies.agents.insert(
        agent.to_string(),
        AgentToolPolicies {
            overrides: tools
                .iter()
                .map(|tool| {
                    (
                        (*tool).to_string(),
                        ToolPolicy {
                            tier: None,
                            mode: Some(ApprovalMode::Blocked),
                        },
                    )
                })
                .collect(),
        },
    );
    policies
}

/// The state per-agent policy creates: reaches a server, can call nothing on it.
/// Named **with** the refusal.
#[test]
fn a_server_every_tool_of_which_is_refused_is_named_as_refused() {
    let mut server = decl("notion");
    server.tool_inventory = probed(&[
        ("search_pages", ToolTier::ReadOnly),
        ("delete_page", ToolTier::WriteDelete),
    ]);
    server.tool_policies = blocked_for("writer", &["search_pages", "delete_page"]);

    let brief = server_family_brief(&[server], &[], &grants(&["mcp:notion"]), "writer");

    assert!(
        brief.contains("`notion`"),
        "dropping the server answers \"no\" to \"do you have notion?\": {brief}"
    );
    assert!(brief.contains(REFUSAL), "{brief}");
}

/// The same company, the teammate without the rule: named normally, no clause.
#[test]
fn another_teammate_sees_the_server_named_normally() {
    let mut server = decl("notion");
    server.tool_inventory = probed(&[("search_pages", ToolTier::ReadOnly)]);
    server.tool_policies = blocked_for("writer", &["search_pages"]);

    let brief = server_family_brief(&[server], &[], &grants(&["mcp:notion"]), "engineer");

    assert!(brief.contains("`notion`"), "{brief}");
    assert!(!brief.contains(REFUSAL), "{brief}");
}

/// One callable tool is not nothing.
#[test]
fn one_callable_tool_leaves_the_server_named_normally() {
    let mut server = decl("notion");
    server.tool_inventory = probed(&[
        ("search_pages", ToolTier::ReadOnly),
        ("delete_page", ToolTier::WriteDelete),
    ]);
    server.tool_policies = blocked_for("writer", &["delete_page"]);

    let brief = server_family_brief(&[server], &[], &grants(&["mcp:notion"]), "writer");

    assert!(!brief.contains(REFUSAL), "{brief}");
}

/// A server no probe has reached knows no tool, so nothing about it is refused.
#[test]
fn an_unprobed_server_is_never_called_refused() {
    let mut server = decl("notion");
    server.tool_policies = blocked_for("writer", &["search_pages"]);
    assert!(server.tool_inventory.tools.is_empty());

    let brief = server_family_brief(&[server], &[], &grants(&["mcp:notion"]), "writer");

    assert!(brief.contains("`notion`"), "{brief}");
    assert!(!brief.contains(REFUSAL), "{brief}");
}

/// A company that has written no per-agent rule renders the brief it rendered
/// before, for every teammate — including ids on no roster.
#[test]
fn an_upgraded_company_renders_the_same_brief_for_every_agent() {
    let mut server = decl("notion");
    server.tool_inventory = probed(&[("search_pages", ToolTier::ReadOnly)]);
    let servers = [server];
    let reach = grants(&["mcp:notion"]);

    let expected = server_family_brief(&servers, &[], &reach, "engineer");
    assert!(!expected.contains(REFUSAL));
    for agent in ["writer", "", "nobody-by-this-id"] {
        assert_eq!(
            server_family_brief(&servers, &[], &reach, agent),
            expected,
            "`{agent}` must be told what every teammate was told before"
        );
    }
}

/// The clause rides the pairing suffix rather than replacing it: an agent still
/// needs to know both ways to address the server it cannot currently use.
#[test]
fn the_refusal_clause_does_not_displace_the_pairing_note() {
    let mut server = decl("notion");
    server.endpoint = "https://shared.example/mcp".to_string();
    server.tool_inventory = probed(&[("search_pages", ToolTier::ReadOnly)]);
    server.tool_policies = blocked_for("writer", &["search_pages"]);
    let install = RegistryServerRow {
        server_id: "notion-install".to_string(),
        display_name: "Notion".to_string(),
        endpoint: Some("https://shared.example/mcp".to_string()),
        enabled: true,
    };

    let brief = server_family_brief(
        &[server],
        &[install],
        &grants(&["mcp:notion", "mcp_registry"]),
        "writer",
    );

    assert!(
        brief.contains("the same \nserver") || brief.contains("the same server"),
        "{brief}"
    );
    assert!(brief.contains("notion-install"), "{brief}");
    assert!(brief.contains(REFUSAL), "{brief}");
}
