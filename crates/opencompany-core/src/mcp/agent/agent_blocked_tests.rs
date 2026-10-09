//! A blocked tool is denied on the server attached to the agent's spec.

use super::*;

use crate::mcp::policy::{ApprovalMode, McpToolPolicies, ToolPolicy};

use super::tests::{decl, grants};

/// An endpoint nothing listens on: a call that reaches the transport fails
/// loudly, so "was it dialled" is observable without a live server.
const DEAD_ENDPOINT: &str = "http://127.0.0.1:1/mcp";

/// The teammate every case here is resolved for. These fixtures write no
/// per-agent rule, so the company answer is what each one must produce.
const AGENT: &str = "engineer";

fn blocked_server(name: &str, tool: &str) -> McpServerDecl {
    let mut server = decl(name, DEAD_ENDPOINT);
    let mut policies = McpToolPolicies::default();
    policies.overrides.insert(
        tool.to_string(),
        ToolPolicy {
            tier: None,
            mode: Some(ApprovalMode::Blocked),
        },
    );
    server.tool_policies = policies;
    server
}

/// What `AgentSpec::mcp` will carry, read back the only way the type allows:
/// its redacting `Debug`, which prints `disallowed_tools` verbatim. Asserting
/// on the attachment itself rather than on a helper's return value is the
/// point — the question is what the spec receives.
fn attachment(server: McpServerDecl) -> String {
    let attached =
        crate::mcp::agent::embed_servers_for_agent(&[server], AGENT, &grants(&["mcp:*"]));
    assert_eq!(attached.len(), 1);
    format!("{attached:?}")
}

/// A company agent reaches a declared server through OpenHuman's own native
/// `mcp_call_tool` over the servers `AgentSpec::mcp` carries, so the attached
/// server's deny list is the enforcement, and the transport's own filter puts
/// deny above allow.
#[test]
fn a_blocked_tool_is_denied_on_the_attached_server() {
    let debug = attachment(blocked_server("notion", "delete_page"));

    assert!(
        debug.contains(r#"disallowed_tools: ["delete_page"]"#),
        "a blocked tool must not be reachable natively: {debug}"
    );
}

/// The declaration's own deny list survives: the policy adds to it rather than
/// replacing it, or an operator's `disallowed_tools` would be dropped the
/// moment they blocked something else.
#[test]
fn the_declarations_own_deny_list_is_kept() {
    let mut server = blocked_server("notion", "delete_page");
    server.disallowed_tools = vec!["debug_dump".to_string()];

    let debug = attachment(server);

    assert!(
        debug.contains(r#"disallowed_tools: ["debug_dump", "delete_page"]"#),
        "{debug}"
    );
}

/// A server nobody has blocked anything on is attached exactly as it was.
#[test]
fn an_unblocked_server_is_attached_unchanged() {
    let debug = attachment(decl("notion", DEAD_ENDPOINT));

    assert!(debug.contains("disallowed_tools: []"), "{debug}");
}

/// A tool the operator left at `needs_approval` is not denied — the deny list
/// is the enforcement for `blocked` alone, and denying anything else would take
/// away a tool the approval gate exists to let through.
#[test]
fn a_tool_that_merely_parks_is_not_denied() {
    let mut server = decl("notion", DEAD_ENDPOINT);
    let mut policies = McpToolPolicies::default();
    policies.overrides.insert(
        "update_page".to_string(),
        ToolPolicy {
            tier: None,
            mode: Some(ApprovalMode::NeedsApproval),
        },
    );
    server.tool_policies = policies;

    let debug = attachment(server);

    assert!(debug.contains("disallowed_tools: []"), "{debug}");
}

/// A tier default blocks the tools discovery found, not only the ones an
/// operator has already named — the widening the persisted inventory exists
/// for, asserted where it has to hold: the attachment.
#[test]
fn a_blocked_tier_default_denies_the_inventoried_tools() {
    let mut server = decl("notion", DEAD_ENDPOINT);
    let mut policies = McpToolPolicies::default();
    policies.tier_defaults.insert(
        crate::mcp::policy::ToolTier::WriteDelete,
        ApprovalMode::Blocked,
    );
    server.tool_policies = policies;
    let mut inventory = crate::mcp::policy::McpToolInventory::default();
    inventory.tools.insert(
        "delete_page".to_string(),
        crate::mcp::policy::ToolTier::WriteDelete,
    );
    inventory.tools.insert(
        "read_page".to_string(),
        crate::mcp::policy::ToolTier::ReadOnly,
    );
    server.tool_inventory = inventory;

    let debug = attachment(server);

    assert!(
        debug.contains(r#"disallowed_tools: ["delete_page"]"#),
        "{debug}"
    );
}
