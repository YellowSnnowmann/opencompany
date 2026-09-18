use crate::ports::{WorkflowBlockedNode, WorkflowRun};
use crate::workflows::caps::ParkedCalls;
use serde_json::{Value, json};

/// The block trigger, pinned exhaustively over the shapes a drain can hand
/// back. Emptiness is the whole decision — a node blocks on the bare
/// presence of a gated call, never on a guess about which one "mattered".
#[test]
fn only_a_drain_that_gated_nothing_leaves_the_node_alone() {
    let cases = [
        (ParkedCalls::default(), true),
        (
            ParkedCalls {
                tools: vec!["publish_artifact".to_string()],
                approval_ids: vec!["appr-1".to_string()],
                unparkable: 0,
                blockers: 0,
            },
            false,
        ),
        // A park that FAILED still blocks, and is strictly worse: there is
        // no card, so nobody will be asked.
        (
            ParkedCalls {
                tools: vec!["publish_artifact".to_string()],
                approval_ids: Vec::new(),
                unparkable: 1,
                blockers: 0,
            },
            false,
        ),
        // Excess discarded past the per-turn cap: the drain dropped the
        // entries, so there is no tool name — and the node still blocks.
        (
            ParkedCalls {
                tools: Vec::new(),
                approval_ids: Vec::new(),
                unparkable: 3,
                blockers: 0,
            },
            false,
        ),
    ];
    for (parked, expected) in cases {
        assert_eq!(parked.is_empty(), expected, "{parked:?}");
    }
}

/// The wording rule, and the main review item on this change: the operator
/// is told what the run **parked**, never what it is "waiting on". A receipt
/// cannot go stale; a settle-time outstanding count becomes a fresh lie the
/// moment somebody approves one of the cards.
#[test]
fn the_operator_sentence_is_a_receipt_not_a_snapshot() {
    let notice = super::runner::blocked_notice(&WorkflowBlockedNode {
        node_id: "spec".to_string(),
        tools: vec!["publish_artifact".to_string()],
        approval_ids: vec!["appr-1".to_string(), "appr-2".to_string()],
        unparkable: 0,
        stranded: 0,
        blockers: 0,
    });
    assert!(notice.contains("parked 2 approvals"), "{notice}");
    assert!(
        !notice.to_lowercase().contains("waiting on"),
        "a stale-able snapshot phrasing must not appear: {notice}"
    );
    assert!(
        notice.contains("the steps after it did not run"),
        "the consequence is the part the operator actually needs: {notice}"
    );
}

/// A park that could not happen reads differently from one that did, and
/// says so out loud: there is no card to decide, so pointing the operator at
/// Approvals would send them to an empty page.
#[test]
fn an_unparkable_call_is_not_worded_as_something_to_approve() {
    let notice = super::runner::blocked_notice(&WorkflowBlockedNode {
        node_id: "spec".to_string(),
        tools: vec!["publish_artifact".to_string()],
        approval_ids: Vec::new(),
        unparkable: 1,
        stranded: 0,
        blockers: 0,
    });
    assert!(
        notice.contains("could not be queued for approval"),
        "{notice}"
    );
    assert!(!notice.contains("parked"), "{notice}");
}

/// **Issue #2028.** A node held open by a question must not promise the
/// automatic continue only a gated call earns: a blocker is answered with
/// one of four verdicts and two of them never run the step again. The
/// notice names the four from the shared fragment, so it cannot drift from
/// what the host actually accepts or from the diagnosis the run logs.
#[test]
fn a_blocker_node_offers_the_four_verdicts_instead_of_an_automatic_continue() {
    let notice = super::runner::blocked_notice(&WorkflowBlockedNode {
        node_id: "draft".to_string(),
        tools: Vec::new(),
        approval_ids: vec!["appr-1".to_string()],
        unparkable: 0,
        stranded: 0,
        blockers: 1,
    });
    assert!(
        !notice.contains("continues on its own"),
        "answering a question does not continue the run by itself: {notice}"
    );
    assert!(
        notice.contains(crate::ports::blockers::BLOCKER_VERDICT_CHOICES),
        "all four verdicts are reachable, so all four have to be named: {notice}"
    );

    let mixed = super::runner::blocked_notice(&WorkflowBlockedNode {
        node_id: "draft".to_string(),
        tools: vec!["publish_artifact".to_string()],
        approval_ids: vec!["appr-1".to_string(), "appr-2".to_string()],
        unparkable: 0,
        stranded: 0,
        blockers: 1,
    });
    assert!(
        mixed.contains("continues this run on its own")
            && mixed.contains(crate::ports::blockers::BLOCKER_VERDICT_CHOICES),
        "a mixed node has to describe both, since neither sentence is true of all of it: \
         {mixed}"
    );

    let gated = super::runner::blocked_notice(&WorkflowBlockedNode {
        node_id: "draft".to_string(),
        tools: vec!["publish_artifact".to_string()],
        approval_ids: vec!["appr-1".to_string()],
        unparkable: 0,
        stranded: 0,
        blockers: 0,
    });
    assert!(
        gated.contains("continues on its own")
            && !gated.contains(crate::ports::blockers::BLOCKER_VERDICT_CHOICES),
        "a gated call really does resume on approval, and has no verdicts to choose: {gated}"
    );
}

/// A `WorkflowRun` written before either field existed still loads — the
/// `#[serde(default)]` contract, which for `WorkflowRunFinished` is the
/// difference between replaying a company's history at boot and losing it.
#[test]
fn a_pre_881_run_payload_still_deserializes() {
    let legacy = json!({
        "output": Value::Null,
        "pending_approvals": [],
        "deliveries": [],
        "cancelled": false,
        "nodes": [],
        "notices": [],
        "board": []
    });
    let run: WorkflowRun = serde_json::from_value(legacy).expect("a pre-#881 payload loads");
    assert!(run.blocked_nodes.is_empty());
    assert!(run.approvals.is_empty());
}
