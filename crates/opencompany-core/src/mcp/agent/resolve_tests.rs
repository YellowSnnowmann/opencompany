use super::*;

use crate::company::mcp::{AuthMaterial, McpSource};

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
        tool_policies: Default::default(),
        tool_inventory: Default::default(),
    }
}

fn grants(g: &[&str]) -> Vec<String> {
    g.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_granted_enabled_server_wires_the_declared_family() {
    let decls = [decl("notion", "https://mcp.notion.example/mcp")];
    let grants = grants(&["mcp:notion"]);
    let mcp = resolve_for_agent(&decls, "writer", &grants, None);
    assert!(mcp.declared_wired());
    assert_eq!(mcp.native_tool_names(), DECLARED_BRIDGE_TOOLS);
    let servers = mcp.embed_servers();
    assert_eq!(servers.len(), 1);
}

#[test]
fn an_ungranted_server_wires_nothing() {
    let decls = [decl("notion", "https://mcp.notion.example/mcp")];
    let grants = grants(&["mcp:github", "email.send"]);
    let mcp = resolve_for_agent(&decls, "writer", &grants, None);
    assert!(!mcp.declared_wired());
    assert!(mcp.native_tool_names().is_empty());
    assert!(mcp.embed_servers().is_empty());
}

#[test]
fn a_disabled_server_wires_nothing() {
    let mut server = decl("notion", "https://mcp.notion.example/mcp");
    server.enabled = false;
    let decls = [server];
    let grants = grants(&["mcp:*"]);
    let mcp = resolve_for_agent(&decls, "writer", &grants, None);
    assert!(!mcp.declared_wired());
    assert!(mcp.embed_servers().is_empty());
}

#[test]
fn no_declarations_wire_nothing() {
    let grants = grants(&["mcp:*"]);
    let mcp = resolve_for_agent(&[], "writer", &grants, None);
    assert!(!mcp.declared_wired());
    assert!(mcp.native_tool_names().is_empty());
}

#[test]
fn the_registry_family_needs_an_explicit_grant_and_a_store() {
    let home = tempfile::tempdir().expect("tempdir");
    let company = CompanyId::new("acme");

    let wildcard = grants(&["*"]);
    let mcp = resolve_for_agent(&[], "writer", &wildcard, Some(home.path().to_path_buf()));
    assert!(!mcp.registry_wired());
    assert!(mcp.registry_tools(&company, None).is_empty());

    let explicit = grants(&["mcp_registry"]);
    let unconfigured = resolve_for_agent(&[], "writer", &explicit, None);
    assert!(!unconfigured.registry_wired());
    assert!(unconfigured.registry_tools(&company, None).is_empty());

    let wired = resolve_for_agent(&[], "writer", &explicit, Some(home.path().to_path_buf()));
    assert!(wired.registry_wired());
    let names: Vec<String> = wired
        .registry_tools(&company, None)
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();
    assert_eq!(
        names,
        [
            "mcp_registry_installed_list",
            "mcp_registry_list_tools",
            "mcp_registry_tool_call"
        ]
    );
}

#[test]
fn the_persona_brief_is_the_capability_brief_then_the_family_brief() {
    let decls = [decl("notion", "https://mcp.notion.example/mcp")];
    let grants = grants(&["mcp:notion"]);
    let mcp = resolve_for_agent(&decls, "writer", &grants, None);
    let expected = format!(
        "{}{}",
        capability_brief(true, false),
        server_family_brief(&decls, &[], &grants, "writer")
    );
    assert_eq!(mcp.persona_brief(&[]), expected);
    assert!(mcp.persona_brief(&[]).contains("`mcp_list_tools`"));
}

#[test]
fn an_agent_with_no_mcp_gets_no_persona_brief() {
    let grants = grants(&["email.send"]);
    let mcp = resolve_for_agent(&[], "writer", &grants, None);
    assert_eq!(mcp.persona_brief(&[]), "");
}

#[test]
fn a_wildcard_grant_attaches_every_enabled_server_and_a_named_one_only_its_own() {
    let mut off = decl("off", "https://off.example/mcp");
    off.enabled = false;
    let decls = [
        decl("notion", "https://notion.example/mcp"),
        decl("linear", "https://linear.example/mcp"),
        off,
    ];

    let wildcard = grants(&["mcp:*"]);
    let all = resolve_for_agent(&decls, "writer", &wildcard, None).embed_servers();
    let debug = format!("{all:?}");
    assert_eq!(all.len(), 2, "{debug}");
    assert!(debug.contains("notion") && debug.contains("linear") && !debug.contains("\"off\""));

    let named = grants(&["mcp:notion"]);
    let one = resolve_for_agent(&decls, "writer", &named, None).embed_servers();
    let debug = format!("{one:?}");
    assert_eq!(one.len(), 1, "{debug}");
    assert!(debug.contains("notion") && !debug.contains("linear"));
}
