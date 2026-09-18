use super::*;
use crate::ports::types::{Actor, ActorKind, ApprovalId, Verdict};

fn resolved(id: &str) -> CompanyEvent {
    CompanyEvent::ApprovalResolved {
        approval_id: ApprovalId::new(id),
        verdict: Verdict::Approve,
        by: Actor {
            kind: ActorKind::Operator,
            id: "operator".into(),
        },
    }
}

fn ids(batch: &[CompanyEvent]) -> Vec<String> {
    batch
        .iter()
        .map(|e| match e {
            CompanyEvent::ApprovalResolved { approval_id, .. } => approval_id.to_string(),
            _ => unreachable!(),
        })
        .collect()
}

/// The keystone: four approvals from one turn owe **one** continuation, and
/// it carries all four decisions.
#[test]
fn a_turn_continues_once_after_its_last_decision() {
    let q = ContinuationQueue::default();
    for _ in 0..4 {
        q.arm("cycle-1");
    }

    assert!(q.decide("cycle-1", Some(resolved("a"))).is_none());
    assert!(q.decide("cycle-1", Some(resolved("b"))).is_none());
    assert!(q.decide("cycle-1", Some(resolved("c"))).is_none());
    let batch = q
        .decide("cycle-1", Some(resolved("d")))
        .expect("the last decision unblocks the turn");

    assert_eq!(ids(&batch), vec!["a", "b", "c", "d"]);
    assert_eq!(q.waiting(), 0, "the turn's state is dropped once it runs");
}

/// Two turns blocked at once do not unblock each other — the whole reason
/// the key is the parking cycle and not the thread they share.
#[test]
fn turns_are_independent() {
    let q = ContinuationQueue::default();
    q.arm("cycle-1");
    q.arm("cycle-1");
    q.arm("cycle-2");

    assert!(q.decide("cycle-1", Some(resolved("a"))).is_none());
    let second = q
        .decide("cycle-2", Some(resolved("z")))
        .expect("cycle-2 was blocked on one decision only");
    assert_eq!(ids(&second), vec!["z"]);
    assert_eq!(q.outstanding("cycle-1"), 1, "cycle-1 still waits");

    let first = q.decide("cycle-1", Some(resolved("b"))).expect("unblocked");
    assert_eq!(ids(&first), vec!["a", "b"]);
}

/// A turn that parked exactly one approval continues on that decision, so
/// the ordinary single-approval path is byte-for-byte what it was.
#[test]
fn a_single_approval_turn_continues_immediately() {
    let q = ContinuationQueue::default();
    q.arm("cycle-1");
    let batch = q.decide("cycle-1", Some(resolved("a"))).expect("unblocked");
    assert_eq!(ids(&batch), vec!["a"]);
}

/// An approval with no turn key — a pre-#469 journal line — is never gated.
#[test]
fn an_unarmed_turn_continues_alone() {
    let q = ContinuationQueue::default();
    let batch = q
        .decide("never-armed", Some(resolved("a")))
        .expect("an unknown turn is not a turn to wait on");
    assert_eq!(ids(&batch), vec!["a"]);
}

/// An expiry carries no event of its own — the sweep appends that itself —
/// but it is still a decision, so it still unblocks the turn.
#[test]
fn an_expiry_counts_as_a_decision() {
    let q = ContinuationQueue::default();
    q.arm("cycle-1");
    q.arm("cycle-1");

    assert!(q.decide("cycle-1", Some(resolved("a"))).is_none());
    let batch = q
        .decide("cycle-1", None)
        .expect("the expiry was the last thing the turn waited on");
    assert_eq!(
        ids(&batch),
        vec!["a"],
        "the expiry contributes no event, only the unblocking"
    );
}

/// A turn whose every approval expired unblocks with nothing to say. The
/// caller must be able to tell that from "one decision arrived", because an
/// empty batch owes no cycle at all.
#[test]
fn a_wholly_expired_turn_yields_an_empty_batch() {
    let q = ContinuationQueue::default();
    q.arm("cycle-1");
    assert_eq!(
        q.decide("cycle-1", None).expect("unblocked"),
        Vec::new(),
        "nothing to tell the brain"
    );
}

/// Recovery rebuilds the counters, so a restart mid-turn comes back still
/// knowing that turn is blocked rather than continuing it early.
#[test]
fn recovery_rearms_the_outstanding_counts() {
    let q = ContinuationQueue::default();
    q.rearm(vec![
        "cycle-1".to_string(),
        "cycle-1".to_string(),
        "cycle-2".to_string(),
    ]);
    assert_eq!(q.outstanding("cycle-1"), 2);
    assert_eq!(q.outstanding("cycle-2"), 1);

    assert!(q.decide("cycle-1", Some(resolved("a"))).is_none());
    assert!(q.decide("cycle-1", Some(resolved("b"))).is_some());
}

/// The decrement and the "was that the last one" test are one critical
/// section, so concurrent resolves cannot both be handed a batch. Exactly
/// one of eight threads may run the continuation.
#[test]
fn concurrent_decisions_hand_the_batch_to_exactly_one_caller() {
    let q = ContinuationQueue::default();
    for _ in 0..8 {
        q.arm("cycle-1");
    }

    let batches = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for i in 0..8 {
        let q = q.clone();
        let batches = Arc::clone(&batches);
        handles.push(std::thread::spawn(move || {
            if let Some(batch) = q.decide("cycle-1", Some(resolved(&format!("a{i}")))) {
                batches.lock().unwrap().push(batch);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let batches = batches.lock().unwrap();
    assert_eq!(batches.len(), 1, "two continuations for one turn");
    assert_eq!(
        batches[0].len(),
        8,
        "the single continuation carries every decision"
    );
}

/// Issue #561: the number the console words its confirmation from.
///
/// `outstanding` counts the decisions a turn is still blocked on INCLUDING
/// the one being resolved, because the resolve reads it before the
/// follow-up cycle decrements it. `CompanyRuntime::decisions_still_awaited`
/// subtracts that one; this pins the arithmetic it subtracts from, since a
/// count that is off by one here is a sentence that is wrong on screen.
#[test]
fn outstanding_counts_the_decision_being_made() {
    let q = ContinuationQueue::default();
    for _ in 0..3 {
        q.arm("cycle-1");
    }

    // Before any decision: three parked, so approving one leaves two.
    assert_eq!(q.outstanding("cycle-1"), 3);

    assert!(q.decide("cycle-1", Some(resolved("a1"))).is_none());
    assert_eq!(q.outstanding("cycle-1"), 2, "one decided, two still owed");

    assert!(q.decide("cycle-1", Some(resolved("a2"))).is_none());
    assert_eq!(q.outstanding("cycle-1"), 1);

    // The last decision releases the turn and drops its state, so nothing
    // is outstanding — which is what the console renders as "picking it up".
    assert!(q.decide("cycle-1", Some(resolved("a3"))).is_some());
    assert_eq!(q.outstanding("cycle-1"), 0);
}

/// A turn this queue never armed reports nothing outstanding, and continues
/// on its own. The console must read that as "picking it up", not as a wait
/// with no end.
#[test]
fn an_ungated_turn_is_awaiting_nothing() {
    let q = ContinuationQueue::default();
    assert_eq!(q.outstanding("never-armed"), 0);
    assert!(q.decide("never-armed", Some(resolved("a1"))).is_some());
}
