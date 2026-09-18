pub(super) use super::*;

/// A continuation for a card's own single gate — the pre-#978 shape these
/// tests were written against, kept so they keep asserting what they were
/// written to assert. The run-scoped multi-gate form is exercised in
/// `crate::workflows::parallel_gate_fanout_test`.
pub(super) fn single_continuation_input(effect: &Effect) -> Result<Value> {
    let node_id = required_str(effect, PAYLOAD_NODE_ID)?.to_string();
    continuation_input(effect, std::slice::from_ref(&node_id), &[])
}

pub(super) fn effect(workflow: &str, node: &str, input: Value) -> Effect {
    gate_effect(workflow, node, &input, "run-1", &[], &[], None)
}

/// A gate whose call the host classified — the shape issue #846 writes and
/// issue #1098 reads back.
pub(super) fn gate_with_call(tool: &str, args: &Value) -> Effect {
    gate_effect(
        "sports_blog",
        "fetch_bbc",
        &json!({}),
        "run-1",
        &[],
        &[],
        Some(GateCall {
            tool,
            reason: None,
            args: Some(args),
            target: None,
        }),
    )
}

/// A delivery row with `status`, as `deliver_outputs` would have returned it.
pub(super) fn delivery(node: &str, kind: &str, status: DeliveryStatus) -> DeliveryReport {
    DeliveryReport {
        node: node.to_string(),
        kind: kind.to_string(),
        target: None,
        status,
        detail: String::new(),
        reason: crate::ports::DeliveryReason::Unspecified,
    }
}

/// The ledger rows a parked card carries.
pub(super) fn ledger(effect: &Effect) -> Vec<DeliveredReport> {
    serde_json::from_value(effect.payload[PAYLOAD_DELIVERED].clone()).expect("ledger parses")
}

// ── Issue #2005: the answered-blocker key ────────────────────────────────

pub(super) fn resolution(
    verdict: crate::ports::blockers::BlockerVerdict,
    answer: &str,
) -> crate::ports::blockers::BlockerResolution {
    crate::ports::blockers::BlockerResolution::answered(verdict, answer)
}

// --- issue #596: the pre-publish content preview -------------------------

pub(super) fn edge(from: &str, to: &str) -> crate::company::WorkflowEdgeDef {
    crate::company::WorkflowEdgeDef {
        from: from.to_string(),
        to: to.to_string(),
        label: None,
    }
}

pub(super) const FINGERPRINT_V1: &str = r#"
id = "editable"
name = "Editable"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "gate"
kind = "output"
name = "Gate"
requires_approval = true
[[edge]]
from = "start"
to = "gate"
"#;

/// Same graph, one node renamed — the shape of an in-place edit an author
/// makes to a workflow while one of its runs sits parked on an approval.
pub(super) const FINGERPRINT_V2: &str = r#"
id = "editable"
name = "Editable"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "gate"
kind = "output"
name = "Gate — renamed"
requires_approval = true
[[edge]]
from = "start"
to = "gate"
"#;
