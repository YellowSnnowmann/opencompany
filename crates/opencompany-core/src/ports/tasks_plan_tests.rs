use super::*;

// --- The plan brief (issue #337) ----------------------------------------

fn prereq(kind: PrereqKind, status: PrereqStatus) -> Prerequisite {
    Prerequisite {
        kind,
        name: "github".to_string(),
        status,
        note: "the work opens a pull request".to_string(),
    }
}

fn plan_with(prerequisites: Vec<Prerequisite>) -> TaskPlan {
    TaskPlan {
        description: "Open a PR that adds the changelog entry".to_string(),
        steps: vec![PlanStep {
            title: "Draft the entry".to_string(),
            detail: "Write it against the released version".to_string(),
            estimated_cost_usd: Some(0.02),
            estimated_minutes: Some(5),
        }],
        prerequisites,
        risks: vec!["the release may not be tagged yet".to_string()],
        verification: "the PR exists and CI is green".to_string(),
        scope: "the changelog only; no code changes".to_string(),
        proposed_assignee: Some("maya".to_string()),
        assignee_candidates: Vec::new(),
        planned_at_millis: 42,
    }
}

/// Only `missing` blocks. `needsApproval` and `unknown` ride on the brief
/// as warnings — an approval-gated tool is asked about at the moment it is
/// used, and an inventory the host could not reach is an admission, not a
/// verdict in either direction.
#[test]
fn only_a_missing_prerequisite_blocks_the_dispatch() {
    assert!(PrereqStatus::Missing.blocks());
    assert!(!PrereqStatus::Satisfied.blocks());
    assert!(!PrereqStatus::NeedsApproval.blocks());
    assert!(!PrereqStatus::Unknown.blocks());

    let clear = plan_with(vec![
        prereq(PrereqKind::Connection, PrereqStatus::Satisfied),
        prereq(PrereqKind::Permission, PrereqStatus::NeedsApproval),
        prereq(PrereqKind::Composio, PrereqStatus::Unknown),
    ]);
    assert!(clear.is_dispatchable());
    assert!(clear.blockers().is_empty());

    let blocked = plan_with(vec![
        prereq(PrereqKind::Connection, PrereqStatus::Satisfied),
        prereq(PrereqKind::Mcp, PrereqStatus::Missing),
    ]);
    assert!(!blocked.is_dispatchable());
    assert_eq!(blocked.blockers().len(), 1);
    assert_eq!(blocked.blockers()[0].kind, PrereqKind::Mcp);

    // A plan claiming nothing is dispatchable — "needs nothing" is a
    // legitimate answer, not a suspicious one.
    assert!(plan_with(Vec::new()).is_dispatchable());
}

/// A kind this host cannot check must not fail the parse and must not read
/// as satisfied. It deserializes to `Other`, which the verifier stamps
/// `unknown` — the model gets to be wrong without costing us the plan.
#[test]
fn an_unknown_prerequisite_kind_parses_as_other() {
    let raw = r#"{"kind":"quantum_flux","name":"x","status":"unknown","note":"n"}"#;
    let parsed: Prerequisite = serde_json::from_str(raw).expect("an odd kind still parses");
    assert_eq!(parsed.kind, PrereqKind::Other);
    assert_eq!(parsed.status, PrereqStatus::Unknown);

    // Every known kind still round-trips to its own variant.
    for (wire, kind) in [
        ("connection", PrereqKind::Connection),
        ("composio", PrereqKind::Composio),
        ("mcp", PrereqKind::Mcp),
        ("credential", PrereqKind::Credential),
        ("file", PrereqKind::File),
        ("permission", PrereqKind::Permission),
        ("assignee", PrereqKind::Assignee),
    ] {
        let raw = format!(r#"{{"kind":"{wire}","name":"x","status":"missing","note":"n"}}"#);
        let parsed: Prerequisite = serde_json::from_str(&raw).expect("known kind");
        assert_eq!(parsed.kind, kind);
        assert_eq!(parsed.kind.as_str(), wire);
    }
}

/// The additive-wire contract: a card persisted before #337 loads with no
/// plan, and a card that has never been planned serializes byte-identically
/// to the pre-#337 shape. This is what makes "no migration on any of the
/// three backends" true rather than hoped for.
#[test]
fn the_plan_field_is_additive_on_the_wire() {
    let legacy = r#"{
        "id": "t-1",
        "title": "Unplanned work",
        "column": "todo",
        "priority": "medium",
        "assignee": "maya",
        "updatedAtMillis": 7
    }"#;
    let card: TaskRecord = serde_json::from_str(legacy).expect("a pre-#337 card parses");
    assert!(card.plan.is_none());

    // Matched on the key, not the substring: the fixture's title contains
    // the word "unplanned", and a looser check passes for the wrong reason.
    let round_tripped = serde_json::to_string(&card).unwrap();
    assert!(
        !round_tripped.contains("\"plan\":"),
        "an unplanned card must not grow a key: {round_tripped}"
    );

    // And a planned card round-trips its whole brief, verdicts included.
    let planned = TaskRecord {
        plan: Some(plan_with(vec![prereq(
            PrereqKind::Connection,
            PrereqStatus::Missing,
        )])),
        ..card
    };
    let json = serde_json::to_string(&planned).unwrap();
    assert!(json.contains("\"status\":\"missing\""), "{json}");
    assert!(json.contains("\"estimatedCostUsd\":0.02"), "{json}");
    let back: TaskRecord = serde_json::from_str(&json).expect("round trip");
    assert_eq!(back, planned);
}
