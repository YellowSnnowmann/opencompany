pub(super) use super::*;

pub(super) fn call(id: &str, agent: &str, tool: &str, args: serde_json::Value) -> GrantedCall {
    GrantedCall {
        approval_id: ApprovalId::new(id),
        agent: agent.to_string(),
        tool: tool.to_string(),
        args,
        at_millis: 1_000,
        origin_thread: None,
        origin_parent: None,
        origin_task: None,
    }
}

// -----------------------------------------------------------------------
// Standing grants (issue #374)
// -----------------------------------------------------------------------

pub(super) fn operator() -> Actor {
    Actor {
        kind: crate::ports::types::ActorKind::User,
        id: "user-1".to_string(),
    }
}

pub(super) fn standing(id: &str, agent: &str, tool: &str, expires_at_millis: u64) -> StandingGrant {
    StandingGrant {
        id: GrantId::new(id),
        agent: agent.to_string(),
        workflow: None,
        tool: tool.to_string(),
        verdict: crate::ports::types::Verdict::Approve,
        granted_by: operator(),
        approval_id: ApprovalId::new(format!("approval-{id}")),
        at_millis: 1_000,
        expires_at_millis,
        origin_thread: None,
        origin_parent: None,
        origin_task: None,
        scope: None,
    }
}

/// A permission held by a workflow rather than a teammate (issue #1098).
pub(super) fn standing_workflow(
    id: &str,
    workflow: &str,
    tool: &str,
    scope: Option<&str>,
    expires_at_millis: u64,
) -> StandingGrant {
    StandingGrant {
        agent: String::new(),
        workflow: Some(workflow.to_string()),
        scope: scope.map(str::to_string),
        ..standing(id, "", tool, expires_at_millis)
    }
}

/// A grant confined to one Composio toolkit (issue #457).
pub(super) fn scoped(
    id: &str,
    agent: &str,
    tool: &str,
    scope: &str,
    expires_at_millis: u64,
) -> StandingGrant {
    StandingGrant {
        scope: Some(scope.to_string()),
        ..standing(id, agent, tool, expires_at_millis)
    }
}
