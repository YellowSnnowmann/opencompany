//! The `?agent=` lens on the tool-permission route: what it merges, what it
//! refuses, and that the two scopes never write over each other.

use super::*;

use crate::company::mcp_policy::{
    AgentToolPolicies, ApprovalMode, McpToolInventory, McpToolPolicies, PolicySource, ToolPolicy,
    ToolTier, inventory_from_discovery,
};

fn entry(tool: &str, tier: Option<ToolTier>, mode: Option<ApprovalMode>) -> PutToolPolicyEntry {
    PutToolPolicyEntry {
        tool: tool.to_string(),
        tier,
        mode,
    }
}

fn patch(tools: Vec<PutToolPolicyEntry>) -> PutToolPolicy {
    PutToolPolicy {
        tier_defaults: None,
        tools: Some(tools),
    }
}

fn mode_only(mode: ApprovalMode) -> ToolPolicy {
    ToolPolicy {
        tier: None,
        mode: Some(mode),
    }
}

/// A company document with one pin, so a per-agent write has something it could
/// have clobbered.
fn company_document() -> McpToolPolicies {
    let mut stored = McpToolPolicies::default();
    stored
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    stored
        .overrides
        .insert("delete_page".into(), mode_only(ApprovalMode::Blocked));
    stored
}

// ---- the merge ------------------------------------------------------------

/// The isolation claim: a per-agent write lands in that teammate's entry and
/// leaves the company half byte-for-byte alone.
#[test]
fn a_per_agent_patch_does_not_touch_the_company_document() {
    let before = company_document();
    let merged = apply_tool_policy_patch(
        before.clone(),
        patch(vec![entry(
            "search_pages",
            None,
            Some(ApprovalMode::Blocked),
        )]),
        Some("writer"),
    )
    .expect("merged");

    assert_eq!(merged.tier_defaults, before.tier_defaults);
    assert_eq!(merged.overrides, before.overrides);
    assert_eq!(
        merged
            .agents
            .get("writer")
            .and_then(|rules| rules.overrides.get("search_pages"))
            .and_then(|row| row.mode),
        Some(ApprovalMode::Blocked)
    );
}

/// …and the reverse: a company write leaves every teammate's entry standing.
#[test]
fn a_company_patch_does_not_touch_a_teammates_entry() {
    let mut before = company_document();
    before.agents.insert(
        "writer".into(),
        AgentToolPolicies {
            overrides: [("search_pages".to_string(), mode_only(ApprovalMode::Blocked))]
                .into_iter()
                .collect(),
        },
    );

    let merged = apply_tool_policy_patch(
        before.clone(),
        patch(vec![entry(
            "update_page",
            None,
            Some(ApprovalMode::NeedsApproval),
        )]),
        None,
    )
    .expect("merged");

    assert_eq!(merged.agents, before.agents);
}

/// An entry naming no mode resets that teammate's row only, and an emptied
/// teammate is pruned away rather than left as residue.
#[test]
fn an_agent_entry_naming_no_field_resets_that_row_only() {
    let mut before = company_document();
    before.agents.insert(
        "writer".into(),
        AgentToolPolicies {
            overrides: [
                ("search_pages".to_string(), mode_only(ApprovalMode::Blocked)),
                ("get_page".to_string(), mode_only(ApprovalMode::Blocked)),
            ]
            .into_iter()
            .collect(),
        },
    );

    let merged = apply_tool_policy_patch(
        before,
        patch(vec![entry("search_pages", None, None)]),
        Some("writer"),
    )
    .expect("merged");

    let rules = merged.agents.get("writer").expect("the teammate survives");
    assert!(!rules.overrides.contains_key("search_pages"));
    assert!(rules.overrides.contains_key("get_page"));

    let emptied = apply_tool_policy_patch(
        merged,
        patch(vec![entry("get_page", None, None)]),
        Some("writer"),
    )
    .expect("merged");
    assert!(
        emptied.agents.is_empty(),
        "an emptied teammate must not be left as residue: {emptied:?}"
    );
}

/// Resetting a row nobody decided about is a no-op rather than an error, and it
/// must not conjure an entry for the teammate.
#[test]
fn resetting_an_undecided_agent_row_leaves_no_entry() {
    let merged = apply_tool_policy_patch(
        company_document(),
        patch(vec![entry("search_pages", None, None)]),
        Some("writer"),
    )
    .expect("merged");
    assert!(merged.agents.is_empty());
}

// ---- what the scope refuses ----------------------------------------------

/// A tier classifies the tool, not the teammate. Refused rather than dropped: a
/// body the host half-applies is worse than one it rejects.
#[test]
fn a_per_agent_tier_is_refused() {
    let err = apply_tool_policy_patch(
        company_document(),
        patch(vec![entry(
            "search_pages",
            Some(ToolTier::ReadOnly),
            Some(ApprovalMode::Blocked),
        )]),
        Some("writer"),
    )
    .expect_err("refused");
    assert!(err.contains("tier"), "{err}");
    assert!(err.contains("teammate"), "{err}");
}

#[test]
fn per_agent_tier_defaults_are_refused() {
    let err = apply_tool_policy_patch(
        company_document(),
        PutToolPolicy {
            tier_defaults: Some(
                [("read_only".to_string(), Some(ApprovalMode::Blocked))]
                    .into_iter()
                    .collect(),
            ),
            tools: None,
        },
        Some("writer"),
    )
    .expect_err("refused");
    assert!(err.contains("everyone"), "{err}");
}

/// A refused body must change nothing, including the half it did name.
#[test]
fn a_refused_per_agent_tier_writes_nothing() {
    let before = company_document();
    let err = apply_tool_policy_patch(
        before.clone(),
        patch(vec![
            entry("get_page", None, Some(ApprovalMode::Blocked)),
            entry("search_pages", Some(ToolTier::ReadOnly), None),
        ]),
        Some("writer"),
    );
    assert!(err.is_err());
    // The refusal is returned before any entry is applied, so the caller still
    // holds the untouched document it passed in.
    assert!(before.agents.is_empty());
}

#[test]
fn an_agent_entry_still_needs_a_tool_name() {
    let err = apply_tool_policy_patch(
        company_document(),
        patch(vec![entry("   ", None, Some(ApprovalMode::Blocked))]),
        Some("writer"),
    )
    .expect_err("refused");
    assert!(err.contains("`tool` name"), "{err}");
}

/// A blank `?agent=` is the company document, not a teammate named "". Without
/// this a console that always appends the parameter would write rules under an
/// empty id that no roster read could ever surface.
#[test]
fn a_blank_agent_parameter_is_the_company_document() {
    let scope = AgentScope {
        agent: Some("   ".to_string()),
    };
    assert_eq!(scope.agent(), None);
    assert_eq!(AgentScope::default().agent(), None);
    assert_eq!(
        AgentScope {
            agent: Some(" writer ".to_string())
        }
        .agent(),
        Some("writer")
    );
}

// ---- what the row says ---------------------------------------------------

/// Every `PolicySource` an agent-scoped read can produce, and the stored mode
/// carried alongside a clamped one so the console can name what was set.
#[test]
fn an_agent_scoped_row_names_the_rule_that_won() {
    let mut policies = McpToolPolicies::default();
    policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    policies
        .overrides
        .insert("delete_page".into(), mode_only(ApprovalMode::Blocked));
    policies.agents.insert(
        "writer".into(),
        AgentToolPolicies {
            overrides: [
                // narrows: pinned for this teammate
                ("get_page".to_string(), mode_only(ApprovalMode::Blocked)),
                // widens over a company block: discarded, and named as such
                (
                    "delete_page".to_string(),
                    mode_only(ApprovalMode::AlwaysAllow),
                ),
            ]
            .into_iter()
            .collect(),
        },
    );
    let inventory = inventory_from_discovery(
        [
            ("search_pages", None),
            ("get_page", None),
            ("delete_page", None),
        ],
        1,
    );

    let dto = tool_policy_dto("notion", &policies, &inventory, Some("writer"));
    assert_eq!(dto.agent.as_deref(), Some("writer"));
    let row = |tool: &str| {
        dto.tools
            .iter()
            .find(|row| row.tool == tool)
            .unwrap_or_else(|| panic!("row for {tool}"))
    };

    assert_eq!(row("search_pages").source, PolicySource::ServerInherited);
    assert_eq!(row("search_pages").mode, ApprovalMode::AlwaysAllow);
    assert_eq!(row("search_pages").agent_mode, None);

    assert_eq!(row("get_page").source, PolicySource::AgentPinned);
    assert_eq!(row("get_page").mode, ApprovalMode::Blocked);

    assert_eq!(row("delete_page").source, PolicySource::AgentClamped);
    assert_eq!(row("delete_page").mode, ApprovalMode::Blocked);
    assert_eq!(
        row("delete_page").agent_mode,
        Some(ApprovalMode::AlwaysAllow),
        "a discarded setting must still be named"
    );
}

/// The company-wide lens is true about the document and silent about its
/// exceptions, so it carries the count of them.
#[test]
fn the_company_lens_reports_the_teammates_that_differ() {
    let mut policies = McpToolPolicies::default();
    policies
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::AlwaysAllow);
    policies.agents.insert(
        "writer".into(),
        AgentToolPolicies {
            overrides: [("search_pages".to_string(), mode_only(ApprovalMode::Blocked))]
                .into_iter()
                .collect(),
        },
    );
    let inventory = inventory_from_discovery([("search_pages", None), ("get_page", None)], 1);

    let dto = tool_policy_dto("notion", &policies, &inventory, None);
    assert_eq!(dto.agent, None);
    let row = |tool: &str| dto.tools.iter().find(|row| row.tool == tool).expect("row");
    assert_eq!(row("search_pages").differing_agents, vec!["writer"]);
    assert!(row("get_page").differing_agents.is_empty());
    // The company-wide mode is unchanged by the exception.
    assert_eq!(row("search_pages").mode, ApprovalMode::AlwaysAllow);
    assert_eq!(row("search_pages").source, PolicySource::ServerInherited);
}

/// A teammate can hold a rule about a tool no probe reached and no company
/// override names. The agent lens has to show that row, or the only way to clear
/// it would be hand-editing the store.
#[test]
fn the_agent_lens_shows_a_row_only_that_teammate_has() {
    let mut policies = McpToolPolicies::default();
    policies.agents.insert(
        "writer".into(),
        AgentToolPolicies {
            overrides: [("write_page".to_string(), mode_only(ApprovalMode::Blocked))]
                .into_iter()
                .collect(),
        },
    );
    let inventory = McpToolInventory::default();

    let writer = tool_policy_dto("notion", &policies, &inventory, Some("writer"));
    let company = tool_policy_dto("notion", &policies, &inventory, None);

    assert!(writer.tools.iter().any(|row| row.tool == "write_page"));
    assert!(company.tools.is_empty());
}

/// Tiers are never per-agent, so the grouping is identical in both lenses — the
/// property the console's shared row component depends on.
#[test]
fn both_lenses_report_the_same_tier_defaults() {
    let policies = company_document();
    let inventory = inventory_from_discovery([("search_pages", None)], 1);

    let company = tool_policy_dto("notion", &policies, &inventory, None);
    let writer = tool_policy_dto("notion", &policies, &inventory, Some("writer"));

    let tiers = |dto: &ToolPolicyDto| {
        dto.tier_defaults
            .iter()
            .map(|(tier, row)| (tier.clone(), row.mode, row.stored))
            .collect::<Vec<_>>()
    };
    assert_eq!(tiers(&company), tiers(&writer));
    let grouped = |dto: &ToolPolicyDto| {
        dto.tools
            .iter()
            .map(|row| (row.tool.clone(), row.effective_tier))
            .collect::<Vec<_>>()
    };
    assert_eq!(grouped(&company), grouped(&writer));
}
