//! The per-agent layer: it narrows, it never widens, and a document that has
//! never heard of it resolves exactly as it did before.
//!
//! Ungated: the whole `harness` tree is behind `feature = "openhuman"`, so a rule
//! proved only there is invisible to the default lane's bare `cargo test`.

use super::*;
use crate::company::mcp::{AuthMaterial, McpSource};
use crate::mcp::policy::{ToolTier, blocked_tool_names, mcp_allow_set};

const EVERY_MODE: [ApprovalMode; 3] = [
    ApprovalMode::AlwaysAllow,
    ApprovalMode::NeedsApproval,
    ApprovalMode::Blocked,
];

fn mode_only(mode: ApprovalMode) -> ToolPolicy {
    ToolPolicy {
        tier: None,
        mode: Some(mode),
    }
}

fn agent_entry(rows: &[(&str, ApprovalMode)]) -> AgentToolPolicies {
    AgentToolPolicies {
        overrides: rows
            .iter()
            .map(|(tool, mode)| ((*tool).to_string(), mode_only(*mode)))
            .collect(),
    }
}

fn inventory(rows: &[(&str, ToolTier)]) -> McpToolInventory {
    McpToolInventory {
        tools: rows
            .iter()
            .map(|(tool, tier)| ((*tool).to_string(), *tier))
            .collect(),
        discovered_at_millis: 7,
    }
}

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

// ---- the restriction order ------------------------------------------------

#[test]
fn max_restrictive_is_the_order_the_design_states() {
    use ApprovalMode::*;
    assert_eq!(AlwaysAllow.max_restrictive(NeedsApproval), NeedsApproval);
    assert_eq!(NeedsApproval.max_restrictive(AlwaysAllow), NeedsApproval);
    assert_eq!(NeedsApproval.max_restrictive(Blocked), Blocked);
    assert_eq!(Blocked.max_restrictive(AlwaysAllow), Blocked);
    for mode in EVERY_MODE {
        assert_eq!(mode.max_restrictive(mode), mode, "idempotent on {mode:?}");
    }
    // Commutative, so which side a caller puts the teammate's ask on cannot
    // change the answer.
    for a in EVERY_MODE {
        for b in EVERY_MODE {
            assert_eq!(a.max_restrictive(b), b.max_restrictive(a), "{a:?} {b:?}");
        }
    }
}

// ---- the wire form --------------------------------------------------------

#[test]
fn deserializing_a_document_without_an_agents_key_yields_an_empty_map() {
    let raw = r#"{"tierDefaults":{"read_only":"always_allow"},
                  "overrides":{"delete_page":{"mode":"blocked"}}}"#;
    let parsed: McpToolPolicies = serde_json::from_str(raw).expect("legacy document parses");
    assert!(parsed.agents.is_empty());
}

#[test]
fn a_per_agent_document_round_trips() {
    let raw = r#"{"tierDefaults":{},"overrides":{},
                  "agents":{"writer":{"overrides":{"write_page":{"mode":"blocked"}}}}}"#;
    let parsed: McpToolPolicies = serde_json::from_str(raw).expect("document parses");
    assert_eq!(
        parsed
            .agents
            .get("writer")
            .and_then(|entry| entry.overrides.get("write_page"))
            .and_then(|policy| policy.mode),
        Some(ApprovalMode::Blocked)
    );
    let round_tripped: McpToolPolicies =
        serde_json::from_str(&serde_json::to_string(&parsed).expect("serializes"))
            .expect("re-parses");
    assert_eq!(round_tripped, parsed);
}

/// Residue resolves identically and hashes differently, so a reset that left it
/// behind would move the effective-MCP fingerprint. Asserted on the stored JSON,
/// which is what the fingerprint folds.
#[test]
fn a_pruned_reset_leaves_no_agent_residue() {
    let mut policies = McpToolPolicies::default();
    policies.agents.insert(
        "writer".to_string(),
        agent_entry(&[("write_page", ApprovalMode::Blocked)]),
    );
    // The reset the route performs: the row's decision is cleared, not removed.
    policies
        .agents
        .get_mut("writer")
        .expect("entry")
        .overrides
        .insert("write_page".to_string(), ToolPolicy::default());

    policies.prune();

    let raw = serde_json::to_string(&policies).expect("serializes");
    assert!(
        !raw.contains("agents"),
        "a pruned reset must leave no agents key: {raw}"
    );
    assert_eq!(policies, McpToolPolicies::default());
}

#[test]
fn pruning_drops_an_emptied_teammate_but_keeps_a_deciding_one() {
    let mut policies = McpToolPolicies::default();
    policies
        .agents
        .insert("writer".to_string(), AgentToolPolicies::default());
    policies.agents.insert(
        "engineer".to_string(),
        agent_entry(&[("delete_page", ApprovalMode::Blocked)]),
    );

    policies.prune();

    assert!(!policies.agents.contains_key("writer"));
    assert!(policies.agents.contains_key("engineer"));
}

// ---- upgrade neutrality ---------------------------------------------------

/// Every shape of legacy document, against every shape of agent id — including
/// ids nobody has ever written a rule for. The per-agent answer must be the
/// company answer, and the deny list must match in contents *and* order, because
/// the attachment is built from that list and "byte-identical" is the promise.
#[test]
fn an_upgraded_document_resolves_identically_for_every_agent() {
    let inv = inventory(&[
        ("search_pages", ToolTier::ReadOnly),
        ("update_page", ToolTier::Interactive),
        ("delete_page", ToolTier::WriteDelete),
    ]);

    let mut documents: Vec<McpToolPolicies> = vec![McpToolPolicies::default()];

    let mut tiered = McpToolPolicies::default();
    tiered
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    tiered
        .tier_defaults
        .insert(ToolTier::WriteDelete, ApprovalMode::Blocked);
    documents.push(tiered);

    let mut pinned = McpToolPolicies::default();
    pinned
        .overrides
        .insert("delete_page".to_string(), mode_only(ApprovalMode::Blocked));
    pinned.overrides.insert(
        "search_pages".to_string(),
        ToolPolicy {
            tier: Some(ToolTier::ReadOnly),
            mode: Some(ApprovalMode::AlwaysAllow),
        },
    );
    pinned.overrides.insert(
        "wipe_all".to_string(),
        mode_only(ApprovalMode::NeedsApproval),
    );
    documents.push(pinned);

    let agents = [
        "engineer",
        "writer",
        "",
        "nobody-by-this-id",
        "1727000000000-3",
    ];
    let tools = [
        "search_pages",
        "update_page",
        "delete_page",
        "wipe_all",
        "never_seen",
    ];

    for policies in &documents {
        assert!(
            policies.agents.is_empty(),
            "the fixture is a legacy document"
        );
        for agent in agents {
            for tool in tools {
                let suggested = inv.suggested(tool);
                let server = resolve_policy(policies, tool, suggested);
                let resolved = resolve_policy_for_agent(policies, agent, tool, suggested);
                assert_eq!(
                    resolved.mode, server.mode,
                    "agent `{agent}` on `{tool}` must resolve to the company answer"
                );
                assert_eq!(resolved.asked, None);
                assert_eq!(
                    resolved.source,
                    PolicySource::for_server(server.is_override)
                );
            }
            assert_eq!(
                blocked_tool_names_for_agent(policies, &inv, agent),
                blocked_tool_names(policies, &inv),
                "the deny list for `{agent}` must match the company one exactly, in order"
            );
        }
    }
}

#[test]
fn an_upgraded_company_gives_every_agent_the_company_allow_set() {
    let mut server = decl("notion");
    server
        .tool_policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    server.tool_inventory = inventory(&[
        ("search_pages", ToolTier::ReadOnly),
        ("delete_page", ToolTier::WriteDelete),
    ]);
    let servers = [server];

    let company = mcp_allow_set(&servers);
    assert!(company.contains("notion", "search_pages"));
    for agent in ["engineer", "writer", "nobody"] {
        assert_eq!(
            mcp_allow_set_for_agent(&servers, agent, &grants(&["mcp:*"])),
            company,
            "`{agent}` must see the company allow set on a legacy document"
        );
    }
}

// ---- narrow-only, totally ------------------------------------------------

/// Every `ApprovalMode × ApprovalMode` pair, not a sample.
///
/// Two claims per pair: the resolved mode is never less restrictive than the
/// server's, and [`PolicySource::AgentClamped`] is reported on exactly the pairs
/// where the stored per-agent setting was discarded.
#[test]
fn the_per_agent_layer_never_widens() {
    let inv = inventory(&[("search_pages", ToolTier::ReadOnly)]);
    let mut pairs = 0;

    for server_mode in EVERY_MODE {
        for asked in EVERY_MODE {
            pairs += 1;
            let mut policies = McpToolPolicies::default();
            policies
                .overrides
                .insert("search_pages".to_string(), mode_only(server_mode));
            policies.agents.insert(
                "writer".to_string(),
                agent_entry(&[("search_pages", asked)]),
            );

            let resolved = resolve_policy_for_agent(
                &policies,
                "writer",
                "search_pages",
                inv.suggested("search_pages"),
            );

            assert_eq!(resolved.server.mode, server_mode);
            assert_eq!(resolved.asked, Some(asked));
            assert_eq!(
                resolved.mode,
                server_mode.max_restrictive(asked),
                "server {server_mode:?} + asked {asked:?}"
            );
            assert!(
                resolved.mode.restriction() >= server_mode.restriction(),
                "a per-agent rule must never widen: server {server_mode:?} + asked {asked:?} \
                 gave {:?}",
                resolved.mode
            );

            let discarded = resolved.mode != asked;
            assert_eq!(
                resolved.source == PolicySource::AgentClamped,
                discarded,
                "AgentClamped must be reported exactly on a discarded setting: server \
                 {server_mode:?} + asked {asked:?}"
            );
            if !discarded {
                assert_eq!(resolved.source, PolicySource::AgentPinned);
            }
        }
    }

    assert_eq!(pairs, 9, "the sweep must be total");
}

/// One row of the resolution table: a label, the shape the server document is
/// put in, the mode this teammate stored, and what the two must resolve to.
type Row = (
    &'static str,
    Box<dyn Fn(&mut McpToolPolicies)>,
    Option<ApprovalMode>,
    ApprovalMode,
    PolicySource,
);

/// The resolution ladder, row by row, as the design states it.
#[test]
fn the_resolution_table_holds_row_by_row() {
    use ApprovalMode::*;

    let inv = inventory(&[("search_pages", ToolTier::ReadOnly)]);
    let tier_allow = |policies: &mut McpToolPolicies| {
        policies
            .tier_defaults
            .insert(ToolTier::ReadOnly, AlwaysAllow);
    };

    let rows: Vec<Row> = vec![
        (
            "nothing stored anywhere",
            Box::new(|_| {}),
            None,
            NeedsApproval,
            PolicySource::ServerInherited,
        ),
        (
            "a bulk allow",
            Box::new(tier_allow),
            None,
            AlwaysAllow,
            PolicySource::ServerInherited,
        ),
        (
            "a bulk allow, this teammate must ask",
            Box::new(tier_allow),
            Some(NeedsApproval),
            NeedsApproval,
            PolicySource::AgentPinned,
        ),
        (
            "a bulk allow, this teammate refused",
            Box::new(tier_allow),
            Some(Blocked),
            Blocked,
            PolicySource::AgentPinned,
        ),
        (
            "a server pin is only a ceiling",
            Box::new(|policies| {
                policies.overrides.insert(
                    "search_pages".to_string(),
                    ToolPolicy {
                        tier: Some(ToolTier::ReadOnly),
                        mode: Some(AlwaysAllow),
                    },
                );
            }),
            Some(Blocked),
            Blocked,
            PolicySource::AgentPinned,
        ),
        (
            "a stored widening over a company block",
            Box::new(|policies| {
                policies
                    .overrides
                    .insert("search_pages".to_string(), mode_only(Blocked));
            }),
            Some(AlwaysAllow),
            Blocked,
            PolicySource::AgentClamped,
        ),
        (
            "a stored widening over a company ask",
            Box::new(|policies| {
                policies
                    .overrides
                    .insert("search_pages".to_string(), mode_only(NeedsApproval));
            }),
            Some(AlwaysAllow),
            NeedsApproval,
            PolicySource::AgentClamped,
        ),
        (
            "a company pin nobody narrows",
            Box::new(|policies| {
                policies
                    .overrides
                    .insert("search_pages".to_string(), mode_only(Blocked));
            }),
            None,
            Blocked,
            PolicySource::ServerPinned,
        ),
    ];

    for (label, shape, asked, expected, expected_source) in rows {
        let mut policies = McpToolPolicies::default();
        shape(&mut policies);
        if let Some(asked) = asked {
            policies.agents.insert(
                "writer".to_string(),
                agent_entry(&[("search_pages", asked)]),
            );
        }
        let resolved = resolve_policy_for_agent(
            &policies,
            "writer",
            "search_pages",
            inv.suggested("search_pages"),
        );
        assert_eq!(resolved.mode, expected, "{label}");
        assert_eq!(resolved.source, expected_source, "{label}");
    }
}

/// An entry present but deciding nothing is inert: the row is pruned on write,
/// and a hand-written document carrying one resolves as if it were absent.
#[test]
fn a_per_agent_entry_deciding_nothing_is_inert() {
    let mut policies = McpToolPolicies::default();
    policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    policies.agents.insert(
        "writer".to_string(),
        AgentToolPolicies {
            overrides: [("search_pages".to_string(), ToolPolicy::default())]
                .into_iter()
                .collect(),
        },
    );

    let resolved = resolve_policy_for_agent(
        &policies,
        "writer",
        "search_pages",
        Some(ToolTier::ReadOnly),
    );

    assert_eq!(resolved.mode, ApprovalMode::AlwaysAllow);
    assert_eq!(resolved.asked, None);
    assert_eq!(resolved.source, PolicySource::ServerInherited);
}

/// A teammate's rules do not reach another teammate.
#[test]
fn one_teammates_rule_does_not_reach_another() {
    let inv = inventory(&[("search_pages", ToolTier::ReadOnly)]);
    let mut policies = McpToolPolicies::default();
    policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    policies.agents.insert(
        "writer".to_string(),
        agent_entry(&[("search_pages", ApprovalMode::Blocked)]),
    );

    assert!(blocks_tool_for_agent(
        &policies,
        &inv,
        "writer",
        "search_pages"
    ));
    assert!(!blocks_tool_for_agent(
        &policies,
        &inv,
        "engineer",
        "search_pages"
    ));
}

// ---- the blocked set -----------------------------------------------------

/// A block on a tool no probe reached and no company override names still has to
/// reach the deny list, so the per-agent name set is the wider one.
#[test]
fn a_block_on_an_unprobed_tool_reaches_the_blocked_set() {
    let inv = McpToolInventory::default();
    let mut policies = McpToolPolicies::default();
    policies.agents.insert(
        "writer".to_string(),
        agent_entry(&[("write_page", ApprovalMode::Blocked)]),
    );

    assert_eq!(
        blocked_tool_names_for_agent(&policies, &inv, "writer"),
        vec!["write_page".to_string()]
    );
    assert!(blocked_tool_names(&policies, &inv).is_empty());
}

#[test]
fn the_per_agent_blocked_set_is_a_superset_and_stays_sorted() {
    let inv = inventory(&[
        ("alpha", ToolTier::ReadOnly),
        ("beta", ToolTier::Interactive),
        ("gamma", ToolTier::WriteDelete),
    ]);
    let mut policies = McpToolPolicies::default();
    policies
        .overrides
        .insert("gamma".to_string(), mode_only(ApprovalMode::Blocked));
    policies.agents.insert(
        "writer".to_string(),
        agent_entry(&[
            ("alpha", ApprovalMode::Blocked),
            ("zeta", ApprovalMode::Blocked),
        ]),
    );

    let company = blocked_tool_names(&policies, &inv);
    let writer = blocked_tool_names_for_agent(&policies, &inv, "writer");

    assert_eq!(company, vec!["gamma".to_string()]);
    assert_eq!(
        writer,
        vec!["alpha".to_string(), "gamma".to_string(), "zeta".to_string()]
    );
    for tool in &company {
        assert!(writer.contains(tool), "`{tool}` must stay blocked");
    }
}

// ---- the approval gate's read set ----------------------------------------

#[test]
fn the_per_agent_allow_set_is_a_subset_of_the_company_one() {
    let mut server = decl("notion");
    server
        .tool_policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    server.tool_inventory = inventory(&[
        ("search_pages", ToolTier::ReadOnly),
        ("get_page", ToolTier::ReadOnly),
    ]);
    server.tool_policies.agents.insert(
        "writer".to_string(),
        agent_entry(&[("search_pages", ApprovalMode::NeedsApproval)]),
    );
    let servers = [server];

    let company = mcp_allow_set(&servers);
    let writer = mcp_allow_set_for_agent(&servers, "writer", &grants(&["mcp:*"]));
    let engineer = mcp_allow_set_for_agent(&servers, "engineer", &grants(&["mcp:*"]));

    assert!(company.contains("notion", "search_pages"));
    assert!(
        !writer.contains("notion", "search_pages"),
        "a per-agent approval requirement must leave the gate's read set"
    );
    assert!(writer.contains("notion", "get_page"));
    assert_eq!(engineer, company);
}

/// Reach is decided before mode.
#[test]
fn a_server_the_grants_miss_contributes_no_read() {
    let mut server = decl("notion");
    server
        .tool_policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    server.tool_inventory = inventory(&[("search_pages", ToolTier::ReadOnly)]);
    let servers = [server];

    assert!(mcp_allow_set(&servers).contains("notion", "search_pages"));
    assert!(
        mcp_allow_set_for_agent(&servers, "engineer", &grants(&["mcp:other"])).is_empty(),
        "an ungranted server must contribute nothing"
    );
    assert!(
        mcp_allow_set_for_agent(&servers, "engineer", &grants(&["*"])).is_empty(),
        "MCP is an explicit opt-in: a bare wildcard reaches no server"
    );
}

#[test]
fn a_disabled_server_contributes_no_read_for_any_agent() {
    let mut server = decl("notion");
    server.enabled = false;
    server
        .tool_policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    server.tool_inventory = inventory(&[("search_pages", ToolTier::ReadOnly)]);

    assert!(mcp_allow_set_for_agent(&[server], "engineer", &grants(&["mcp:*"])).is_empty());
}

// ---- the exceptions a company-wide view is silent about ------------------

#[test]
fn differing_agents_names_only_the_teammates_that_differ() {
    let inv = inventory(&[("search_pages", ToolTier::ReadOnly)]);
    let mut policies = McpToolPolicies::default();
    policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    policies.agents.insert(
        "writer".to_string(),
        agent_entry(&[("search_pages", ApprovalMode::Blocked)]),
    );
    policies.agents.insert(
        "engineer".to_string(),
        agent_entry(&[("delete_page", ApprovalMode::Blocked)]),
    );
    policies.agents.insert(
        "designer".to_string(),
        agent_entry(&[("search_pages", ApprovalMode::AlwaysAllow)]),
    );

    assert_eq!(
        differing_agents(&policies, &inv, "search_pages"),
        vec!["writer".to_string()],
        "a clamped setting changes nothing, so its teammate does not differ"
    );
    assert_eq!(
        differing_agents(&policies, &inv, "delete_page"),
        vec!["engineer".to_string()]
    );
    assert!(differing_agents(&McpToolPolicies::default(), &inv, "search_pages").is_empty());
}

// ---- what a prompt may claim --------------------------------------------

#[test]
fn a_server_no_probe_reached_is_never_fully_refused() {
    let mut server = decl("notion");
    server.tool_policies.agents.insert(
        "writer".to_string(),
        agent_entry(&[("anything", ApprovalMode::Blocked)]),
    );

    assert!(
        !every_known_tool_refused(&server, "writer"),
        "no known tool is not the same fact as nothing callable"
    );
}

#[test]
fn a_server_with_every_known_tool_blocked_is_fully_refused() {
    let mut server = decl("notion");
    server.tool_inventory = inventory(&[
        ("search_pages", ToolTier::ReadOnly),
        ("delete_page", ToolTier::WriteDelete),
    ]);
    server.tool_policies.agents.insert(
        "writer".to_string(),
        agent_entry(&[
            ("search_pages", ApprovalMode::Blocked),
            ("delete_page", ApprovalMode::Blocked),
        ]),
    );

    assert!(every_known_tool_refused(&server, "writer"));
    assert!(!every_known_tool_refused(&server, "engineer"));
}

#[test]
fn one_callable_tool_keeps_a_server_from_being_fully_refused() {
    let mut server = decl("notion");
    server.tool_inventory = inventory(&[
        ("search_pages", ToolTier::ReadOnly),
        ("delete_page", ToolTier::WriteDelete),
    ]);
    server.tool_policies.agents.insert(
        "writer".to_string(),
        agent_entry(&[("delete_page", ApprovalMode::Blocked)]),
    );

    assert!(!every_known_tool_refused(&server, "writer"));
}

/// The declaration's own membership lists refuse a tool as surely as a policy
/// does, so a statement built from this cannot disagree with the attachment.
#[test]
fn the_declarations_membership_lists_count_as_a_refusal() {
    let mut server = decl("notion");
    server.tool_inventory = inventory(&[("search_pages", ToolTier::ReadOnly)]);
    server.disallowed_tools = vec!["search_pages".to_string()];
    assert!(every_known_tool_refused(&server, "engineer"));

    let mut narrowed = decl("notion");
    narrowed.tool_inventory = inventory(&[("search_pages", ToolTier::ReadOnly)]);
    narrowed.allowed_tools = vec!["write_page".to_string()];
    assert!(every_known_tool_refused(&narrowed, "engineer"));
    assert!(refuses_tool_for_agent(
        &narrowed,
        "engineer",
        "search_pages"
    ));
    assert!(!refuses_tool_for_agent(&narrowed, "engineer", "write_page"));
}
