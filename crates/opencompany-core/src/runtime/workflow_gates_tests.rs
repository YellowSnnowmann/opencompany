use super::*;
use crate::ports::types::EffectGroup;
use crate::runtime::workflow_resume::gate_effect;
use serde_json::json;

fn emergency_gate_effect(node: &str) -> Effect {
    Effect {
        kind: crate::runtime::workflow_resume::WORKFLOW_APPROVE_KIND.to_string(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::json!({
            crate::runtime::workflow_resume::PAYLOAD_NODE_ID: node,
        }),
        agent: None,
        run_id: Some("wr-1".to_string()),
    }
}

fn gate(workflow: &str, node: &str) -> Effect {
    gate_effect(workflow, node, &json!({}), "run-1", &[], &[], None)
}

fn non_gate() -> Effect {
    Effect {
        kind: "payment.send".to_string(),
        group: EffectGroup::Spend,
        amount_usd: Some(10.0),
        established_thread: false,
        first_time_counterparty: false,
        payload: json!({}),
        agent: None,
        run_id: None,
    }
}

/// **Codex review finding on PR #2140 (`3955615141`).** Before this,
/// `release` removed a fully-decided batch unconditionally, and only the
/// caller two `.await`s downstream (`RunSupervisor::begin`) could refuse
/// it — by which point the batch was already gone and its checkpoint got
/// pruned as a terminal failure. This proves the batch survives a refusal
/// here instead: still queryable, every verdict still banked.
#[test]
fn release_refuses_and_preserves_a_decided_batch_while_stopped() {
    let gate = Arc::new(crate::policy::gate::ManifestApprovalGate::new(
        crate::company::Policy {
            mode: "full".to_string(),
            always_approve: Vec::new(),
            auto_approve_under_usd: None,
            approval_ttl_hours: None,
        },
    ));
    let queue = WorkflowGateQueue::default().with_emergency_gate(gate.clone());

    let a = ApprovalId::new("appr-a");
    let b = ApprovalId::new("appr-b");
    queue.arm("turn-1", &a, &emergency_gate_effect("node-a"));
    queue.arm("turn-1", &b, &emergency_gate_effect("node-b"));
    queue.decide("turn-1", &a, Verdict::Approve);
    queue.decide("turn-1", &b, Verdict::Deny);
    assert_eq!(
        queue.undecided("turn-1"),
        0,
        "both gates on the turn are decided"
    );

    gate.set_emergency(true);
    match queue.release("turn-1") {
        Err(ReleaseRefusal::EmergencyStop) => {}
        Ok(_) => panic!("a decided batch must be refused, not released, while stopped"),
    }

    assert!(
        queue.is_armed("turn-1"),
        "the refused release must leave the batch in the queue, not destroy it"
    );
    assert_eq!(
        queue.ready_for_release(),
        vec!["turn-1".to_string()],
        "the preserved, fully-decided batch is discoverable for a later redrive"
    );

    gate.set_emergency(false);
    let released = queue
        .release("turn-1")
        .expect("not stopped, so release succeeds")
        .expect("the batch is still there");
    assert_eq!(released.approved, vec!["node-a".to_string()]);
    assert_eq!(released.denied, vec!["node-b".to_string()]);
    assert!(
        !queue.is_armed("turn-1"),
        "a successful release still takes the batch out of the queue"
    );
}

/// A queue built with no [`with_emergency_gate`](WorkflowGateQueue::with_emergency_gate)
/// call — every construction site with no company to ask — releases
/// regardless of any flag, exactly as before this refusal existed.
#[test]
fn release_ignores_emergency_state_with_no_gate_installed() {
    let queue = WorkflowGateQueue::default();
    let a = ApprovalId::new("appr-a");
    queue.arm("turn-1", &a, &emergency_gate_effect("node-a"));
    queue.decide("turn-1", &a, Verdict::Approve);

    queue
        .release("turn-1")
        .expect("no gate installed, so nothing here can refuse on that basis")
        .expect("the batch is there to release");
}

/// The core loop the module exists for: two gates park on one run, one
/// approved and one denied, and release hands back exactly that split on
/// the one representative effect.
#[test]
fn arm_decide_release_splits_approved_and_denied() {
    let q = WorkflowGateQueue::default();
    let id_a = ApprovalId::new("a");
    let id_b = ApprovalId::new("b");
    q.arm("turn-1", &id_a, &gate("wf", "node-a"));
    q.arm("turn-1", &id_b, &gate("wf", "node-b"));
    assert_eq!(q.undecided("turn-1"), 2);

    q.decide("turn-1", &id_a, Verdict::Approve);
    assert_eq!(q.undecided("turn-1"), 1);
    q.decide("turn-1", &id_b, Verdict::Deny);
    assert_eq!(q.undecided("turn-1"), 0);

    let released = q
        .release("turn-1")
        .expect("no emergency gate installed")
        .expect("a batch was armed");
    assert_eq!(released.approved, vec!["node-a".to_string()]);
    assert_eq!(released.denied, vec!["node-b".to_string()]);
}

/// `arm` only ever accepts a `workflow.approve` effect — the kind check is
/// what stops a native effect (say, a payment) from silently being treated
/// as a gate this queue must eventually release.
#[test]
fn arm_ignores_a_non_gate_effect() {
    let q = WorkflowGateQueue::default();
    let id = ApprovalId::new("a");
    q.arm("turn-1", &id, &non_gate());
    assert!(
        !q.is_armed("turn-1"),
        "a non-gate effect must not arm a batch"
    );
    assert_eq!(q.undecided("turn-1"), 0);
}

/// Deciding an id/turn this queue never armed — the shape of a stale or
/// forged approval id — must be a silent no-op, not a panic and not a
/// phantom entry that later corrupts a real release.
#[test]
fn decide_on_an_unarmed_turn_or_unknown_id_is_a_no_op() {
    let q = WorkflowGateQueue::default();
    // Unknown turn entirely.
    q.decide("ghost-turn", &ApprovalId::new("x"), Verdict::Approve);
    assert!(!q.is_armed("ghost-turn"));

    // Known turn, unknown id.
    let id = ApprovalId::new("a");
    q.arm("turn-1", &id, &gate("wf", "node-a"));
    q.decide("turn-1", &ApprovalId::new("not-armed"), Verdict::Deny);
    assert_eq!(
        q.undecided("turn-1"),
        1,
        "a decision on an id this batch never armed must not consume the real one"
    );
}

/// Deciding the same id twice — a retried resolve, or two callers racing
/// one approval — must not double-count the node into the ledger the
/// second time, since the first `decide` already removed it from
/// `undecided`.
#[test]
fn deciding_the_same_id_twice_does_not_double_count() {
    let q = WorkflowGateQueue::default();
    let id = ApprovalId::new("a");
    q.arm("turn-1", &id, &gate("wf", "node-a"));
    q.decide("turn-1", &id, Verdict::Approve);
    q.decide("turn-1", &id, Verdict::Approve);
    let released = q
        .release("turn-1")
        .expect("no emergency gate installed")
        .expect("armed");
    assert_eq!(released.approved, vec!["node-a".to_string()]);
}

/// TTL expiry is documented to "contribute no event at all" beyond the
/// sweep's own `decide` call — there is no `ApprovalResolved` behind it.
/// This proves `decide` alone, with no prior continuation event, still
/// moves the node into `denied` so the run does not replay into it.
#[test]
fn a_ttl_expiry_decide_with_no_prior_event_still_lands_in_denied() {
    let q = WorkflowGateQueue::default();
    let id = ApprovalId::new("a");
    q.arm("turn-1", &id, &gate("wf", "node-a"));
    // The sweep calls decide() directly; nothing else touched this queue.
    q.decide("turn-1", &id, Verdict::Deny);
    let released = q
        .release("turn-1")
        .expect("no emergency gate installed")
        .expect("armed");
    assert!(released.denied.contains(&"node-a".to_string()));
    assert!(released.approved.is_empty());
}

/// `release` takes the batch, so a second release (or any further decide)
/// on the same turn finds nothing left to corrupt or double-release.
#[test]
fn release_drops_the_batch_and_a_repeat_release_is_none() {
    let q = WorkflowGateQueue::default();
    let id = ApprovalId::new("a");
    q.arm("turn-1", &id, &gate("wf", "node-a"));
    assert!(
        q.release("turn-1")
            .expect("no emergency gate installed")
            .is_some()
    );
    assert!(
        q.release("turn-1")
            .expect("no emergency gate installed")
            .is_none()
    );
    assert!(!q.is_armed("turn-1"));

    // A decide arriving after the release must not panic or resurrect it.
    q.decide("turn-1", &id, Verdict::Approve);
    assert!(!q.is_armed("turn-1"));
}

/// The first gate through sets the batch's representative effect; a
/// second sibling gate for the same run must not replace it, since every
/// gate of one run is documented to agree on everything but `node_id`.
#[test]
fn the_first_gates_effect_is_the_batchs_representative_effect() {
    let q = WorkflowGateQueue::default();
    let id_a = ApprovalId::new("a");
    let id_b = ApprovalId::new("b");
    let first = gate_effect(
        "wf",
        "node-a",
        &json!({"trigger": "one"}),
        "run-1",
        &[],
        &[],
        None,
    );
    let second = gate_effect(
        "wf",
        "node-b",
        &json!({"trigger": "different"}),
        "run-1",
        &[],
        &[],
        None,
    );
    q.arm("turn-1", &id_a, &first);
    q.arm("turn-1", &id_b, &second);
    q.decide("turn-1", &id_a, Verdict::Approve);
    q.decide("turn-1", &id_b, Verdict::Approve);
    let released = q
        .release("turn-1")
        .expect("no emergency gate installed")
        .expect("armed");
    assert_eq!(
        released.effect.payload, first.payload,
        "the representative effect must stay the first gate's, not the last"
    );
}

/// `rearm` is idempotent and *replaces* a turn rather than adding to it:
/// replaying the same journal snapshot twice must not accumulate
/// duplicate `undecided` entries for one gate.
#[test]
fn rearm_is_idempotent_and_does_not_accumulate() {
    let q = WorkflowGateQueue::default();
    let id = ApprovalId::new("a");
    let g = gate("wf", "node-a");
    q.rearm(vec![("turn-1".to_string(), id.clone(), &g)]);
    assert_eq!(q.undecided("turn-1"), 1);
    q.rearm(vec![("turn-1".to_string(), id.clone(), &g)]);
    assert_eq!(
        q.undecided("turn-1"),
        1,
        "replaying the same rehydrate twice must not double the undecided count"
    );
}

/// A verdict banked before a rearm is documented to be dropped along with
/// the rest of the turn — the rehydrated batch only knows what the
/// journal still shows parked, so a gate this process already decided
/// (but that a fresh rearm sees as still-parked) comes back undecided.
#[test]
fn rearm_drops_verdicts_banked_before_it() {
    let q = WorkflowGateQueue::default();
    let id_a = ApprovalId::new("a");
    let id_b = ApprovalId::new("b");
    let ga = gate("wf", "node-a");
    let gb = gate("wf", "node-b");
    q.arm("turn-1", &id_a, &ga);
    q.arm("turn-1", &id_b, &gb);
    q.decide("turn-1", &id_a, Verdict::Approve);
    assert_eq!(q.undecided("turn-1"), 1);

    // A journal replay that still sees both gates parked rebuilds the
    // batch from scratch — the in-process approval of `id_a` is gone.
    q.rearm(vec![
        ("turn-1".to_string(), id_a.clone(), &ga),
        ("turn-1".to_string(), id_b.clone(), &gb),
    ]);
    assert_eq!(
        q.undecided("turn-1"),
        2,
        "rearm must rebuild from the journal's view, not preserve this process's own verdicts"
    );
}

/// `undecided` on a turn nobody ever armed is `0`, not a panic — the
/// documented contract for an untracked turn.
#[test]
fn undecided_on_an_untracked_turn_is_zero() {
    let q = WorkflowGateQueue::default();
    assert_eq!(q.undecided("no-such-turn"), 0);
    assert!(!q.is_armed("no-such-turn"));
}

/// A poisoned lock (some other caller panicked while holding it) must
/// make every further call on this queue panic loudly rather than hand
/// back stale or partial batch state that a caller could mistake for a
/// clean read.
#[test]
fn a_poisoned_lock_panics_rather_than_silently_serving_stale_state() {
    let q = WorkflowGateQueue::default();
    let id = ApprovalId::new("a");
    q.arm("turn-1", &id, &gate("wf", "node-a"));

    let poison_q = q.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = poison_q.inner.lock().expect("workflow gate queue poisoned");
        panic!("simulated holder panic while the lock is held");
    }));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| q.undecided("turn-1")));
    assert!(
        result.is_err(),
        "a call against a poisoned lock must panic, not silently return a count"
    );
}
