pub(super) use super::*;

/// The company shape issue #272 was observed on: real desks, plus a
/// teammate (`writer`) the orchestrator mistook for one.
pub(super) fn record() -> CompanyRecord {
    let manifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "ceo"
role = "Chief Executive"

[[agent]]
id = "writer"
role = "Writer"

[[group_chat]]
id = "engineering"
name = "Engineering desk"
members = ["ceo"]

[[group_chat]]
id = "content"
name = "Content desk"
members = ["writer"]

[[group_chat]]
id = "legal"
name = "Legal desk"
members = ["counsel"]
"#,
    )
    .expect("valid manifest");
    CompanyRecord {
        general_channel: Default::default(),
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        id: crate::ports::types::CompanyId::new("acme"),
        manifest,
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        overlay_agents: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
        setup: None,
        activation_completed_at: None,
        created_at_millis: None,
        name_confirmed: false,
        overlay_tool_grants: Default::default(),
    }
}

// --- Teammate hand-off (issue #884) ------------------------------------
