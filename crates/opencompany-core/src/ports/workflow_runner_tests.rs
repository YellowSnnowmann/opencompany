use super::*;

fn row(node: &str, outcome: WorkflowApprovalOutcome) -> WorkflowRunApprovalRow {
    WorkflowRunApprovalRow {
        node_id: Some(node.to_string()),
        tool: Some("send_email".to_string()),
        outcome,
        approval_id: matches!(outcome, WorkflowApprovalOutcome::Parked)
            .then(|| "appr-1".to_string()),
    }
}

/// Codex review (#1865): a node with one live parked call and one failed
/// park is not stranded — an operator can still act on it — even though a
/// call-level count of unparkable rows would equal `pending_approvals.len()`
/// (1 node, 1 unparkable call) and wrongly report it as fully stranded.
#[test]
fn a_node_with_one_live_card_is_not_stranded_even_with_one_failed_park() {
    let pending = vec!["gate".to_string()];
    let approvals = vec![
        row("gate", WorkflowApprovalOutcome::Parked),
        row("gate", WorkflowApprovalOutcome::ParkFailed),
    ];
    assert_eq!(stranded_approvals(&pending, &approvals), 0);
}

/// The complementary case: a node whose every gated call failed to park
/// has no live card left, so it counts once — not twice, even though it
/// made two unparkable rows.
#[test]
fn a_node_with_every_call_unparkable_counts_once() {
    let pending = vec!["gate".to_string()];
    let approvals = vec![
        row("gate", WorkflowApprovalOutcome::ParkFailed),
        row("gate", WorkflowApprovalOutcome::Discarded),
    ];
    assert_eq!(stranded_approvals(&pending, &approvals), 1);
}

/// Never greater than `pending_approvals.len()` — the invariant
/// `RunVerdictFacts::stranded_approvals` documents. Two nodes, one fully
/// stranded and one with a live card, must read `1`, not `2` even though
/// three of the four rows are unparkable.
#[test]
fn mixed_nodes_stay_within_pending_approvals_count() {
    let pending = vec!["gate-a".to_string(), "gate-b".to_string()];
    let approvals = vec![
        row("gate-a", WorkflowApprovalOutcome::Parked),
        row("gate-a", WorkflowApprovalOutcome::ParkFailed),
        row("gate-b", WorkflowApprovalOutcome::ParkFailed),
        row("gate-b", WorkflowApprovalOutcome::Discarded),
    ];
    assert_eq!(stranded_approvals(&pending, &approvals), 1);
}

/// PR #1883 Codex review: a `requires_approval` gate `park_pending_gates`
/// parks — the ordinary authored/policy-raised gate shape, not a call
/// gated inside an agent turn — never gets an `approvals` row at all
/// (`park_pending_gates` writes straight to the approvals queue and
/// `WorkflowRun::approvals`, and never touches it). Before the fix this
/// read as `!approvals.iter().any(node_id && Parked)` — vacuously true
/// for a node with zero rows — so every ordinary gate reported stranded
/// on a run that never made a single failed park. A card is live and
/// waiting; `pending` alone, with no matching row, must count zero.
#[test]
fn a_node_with_no_approval_rows_at_all_is_not_stranded() {
    let pending = vec!["gate".to_string()];
    let approvals: Vec<WorkflowRunApprovalRow> = Vec::new();
    assert_eq!(stranded_approvals(&pending, &approvals), 0);
}

/// The same shape, mixed with a genuinely gated-and-unparkable node: the
/// receipt-less gate must still not count, while the node with real
/// failed-park rows does.
#[test]
fn a_receiptless_gate_beside_a_genuinely_stranded_node_counts_only_the_latter() {
    let pending = vec!["gate".to_string(), "agent-node".to_string()];
    let approvals = vec![row("agent-node", WorkflowApprovalOutcome::ParkFailed)];
    assert_eq!(stranded_approvals(&pending, &approvals), 1);
}

/// A manual `new(false)` reads back [`StartedBy::Operator`] — the coarse
/// default every call through `new`/`begin` gets unless overridden (issue
/// #1862 prerequisite).
#[test]
fn new_with_scheduled_false_defaults_started_by_to_operator() {
    let ctx = WorkflowRunContext::new(false);
    assert_eq!(ctx.started_by, StartedBy::Operator);
    assert!(!ctx.scheduled);
}

/// A cron-started `new(true)` reads back [`StartedBy::Schedule`] —
/// unambiguous, since only the scheduler ever sets `scheduled: true`.
#[test]
fn new_with_scheduled_true_defaults_started_by_to_schedule() {
    let ctx = WorkflowRunContext::new(true);
    assert_eq!(ctx.started_by, StartedBy::Schedule);
    assert!(ctx.scheduled);
}

/// [`WorkflowRunContext::with_started_by`] overrides the `scheduled`-derived
/// default — the lever a caller that knows the real triggering agent uses
/// instead of settling for `new`'s coarse reading.
#[test]
fn with_started_by_overrides_the_default() {
    let ctx = WorkflowRunContext::new(false).with_started_by(StartedBy::Agent("ceo".to_string()));
    assert_eq!(ctx.started_by, StartedBy::Agent("ceo".to_string()));
    // Overriding the sender does not retroactively flip `scheduled` — the
    // two are independent facts about the run.
    assert!(!ctx.scheduled);
}

/// `started_by` does not participate in [`WorkflowRunContext`] equality
/// (see the `impl PartialEq` doc) — two contexts sharing a run id and
/// `scheduled` flag are "the same context" regardless of who is credited
/// with starting it.
#[test]
fn started_by_is_excluded_from_equality() {
    let a = WorkflowRunContext::new(false).with_started_by(StartedBy::Operator);
    let mut b = WorkflowRunContext::new(false).with_started_by(StartedBy::Agent("ceo".to_string()));
    b.run_id = a.run_id.clone();
    assert_eq!(
        a, b,
        "differing started_by must not break the identity comparison"
    );
}
