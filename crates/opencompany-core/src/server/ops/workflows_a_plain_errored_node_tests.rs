use super::*;
// The globals-unaware readers: these tests assert the company's own two
// sources, so they call the form that resolves no baseline.
use super::workflows_test_support::*;
use crate::company::list_workflows_union;

/// A settled `Error` node status is a fact `derive_verdict` already reads
/// off `self.nodes` on its own — `settle_history_verdicts` must not ALSO
/// fold that same fact into `degraded`. `degraded` is reserved for what
/// `self.nodes` cannot carry at all (a progress-drain failure, or a
/// capped/budget-paused turn the journal still shows `ok` for); an earlier
/// revision of `settle_history_verdicts` OR'd the node scan into it too,
/// which double-counted every genuinely errored node in
/// `derive_verdict`'s internal `errored_nodes` sum (N read as N + 1) and
/// corrupted the serialized `degraded` field for a run that never had a
/// drain failure at all.
#[test]
fn a_plain_errored_node_does_not_also_flip_the_separate_degraded_fact() {
    fn run_with_one_errored_node() -> WorkflowRunOutcome {
        WorkflowRunOutcome {
            seq: 1,
            at_millis: 1,
            workflow_id: "wf".to_string(),
            scheduled: false,
            run_id: Some("run-1".to_string()),
            resume_semantic: None,
            deliveries: Vec::new(),
            pending_approvals: Vec::new(),
            error: None,
            nodes: vec![WorkflowRunNode {
                node_id: "worker".into(),
                status: WorkflowNodeStatus::Error,
                elapsed_ms: 5,
                diagnostics: Vec::new(),
            }],
            started_nodes: Vec::new(),
            started_at_millis: Some(1),
            running: false,
            cancelled: false,
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
            // The fact under test: no drain failure was ever recorded for
            // this run — only its one node settled `Error`.
            degraded: false,
            stranded_approvals: 0,
            verdict: WorkflowRunVerdict::Ok,
        }
    }

    let mut runs = vec![run_with_one_errored_node()];
    settle_history_verdicts(&mut runs);
    let run = &runs[0];

    assert!(
        !run.degraded,
        "a plain node error must not flip the separate progress-drain \
         `degraded` fact — derive_verdict's own node scan already counts \
         it once"
    );
    assert_eq!(
        run.verdict,
        WorkflowRunVerdict::Degraded,
        "the errored node must still read as a degraded run on its own \
         merits, with no help from `degraded`"
    );
}

/// rows, in the same camelCase shape the journal event and the history route
/// use — so a console that pressed Run learns what the run did to the board
/// without a second read.
///
/// The omission half matters as much: a run that touched no card must send no
/// `board` key, so every existing caller's body is byte-unchanged.
#[test]
fn the_run_response_carries_board_rows_and_omits_them_when_empty() {
    let json = serde_json::to_value(RunWorkflowResponse {
        output: serde_json::json!({ "nodes": {} }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        run_id: "run-1".into(),
        cancelled: false,
        verdict: WorkflowRunVerdict::Ok,
        nodes: Vec::new(),
        dry_run: false,
        board: vec![crate::ports::WorkflowRunBoardRow {
            action: crate::ports::WorkflowBoardAction::Assigned,
            task_id: Some("card-1".into()),
            title: None,
            assignee: Some("ceo".into()),
        }],
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    })
    .expect("serialize");
    assert_eq!(json["board"][0]["action"], "assigned");
    assert_eq!(json["board"][0]["taskId"], "card-1");
    assert_eq!(json["board"][0]["assignee"], "ceo");
    assert!(
        json["board"][0].get("title").is_none(),
        "an assign row names no title — the console resolves it by id: {json}"
    );

    let json = serde_json::to_value(RunWorkflowResponse {
        output: serde_json::json!({ "nodes": {} }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        run_id: "run-2".into(),
        cancelled: false,
        verdict: WorkflowRunVerdict::Ok,
        nodes: Vec::new(),
        dry_run: false,
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    })
    .expect("serialize");
    assert!(
        json.get("board").is_none(),
        "a run that touched no card sends no board key: {json}"
    );
}

/// Issues #881 / #880: the synchronous run response carries the blocked
/// nodes and the parked-approval receipts, in the same camelCase shape the
/// journal event and the history route use.
///
/// The operator who pressed Run is the reader these exist for: before this,
/// a pipeline whose first step had its `publish_artifact` parked came back
/// with every node green and an empty body, and there was nothing anywhere
/// in the response that said otherwise.
///
/// Omission matters as much, for the same reason it does on `board`: a run
/// that blocked on nobody sends neither key, so every existing caller's body
/// is byte-unchanged.
#[test]
fn the_run_response_carries_blocked_nodes_and_parked_approvals() {
    let json = serde_json::to_value(RunWorkflowResponse {
        output: Value::Null,
        pending_approvals: vec!["spec".into()],
        deliveries: Vec::new(),
        run_id: "run-1".into(),
        cancelled: false,
        verdict: WorkflowRunVerdict::Blocked,
        nodes: vec![WorkflowRunNode {
            node_id: "spec".into(),
            status: WorkflowNodeStatus::Blocked,
            elapsed_ms: 42,
            diagnostics: Vec::new(),
        }],
        dry_run: false,
        board: Vec::new(),
        blocked_nodes: vec![crate::ports::WorkflowBlockedNode {
            node_id: "spec".into(),
            tools: vec!["publish_artifact".into()],
            approval_ids: vec!["appr-1".into()],
            unparkable: 0,
            stranded: 0,
            blockers: 0,
        }],
        approvals: vec![crate::ports::WorkflowRunApprovalRow {
            node_id: Some("spec".into()),
            tool: Some("publish_artifact".into()),
            outcome: crate::ports::WorkflowApprovalOutcome::Parked,
            approval_id: Some("appr-1".into()),
        }],
    })
    .expect("serialize");
    assert_eq!(json["nodes"][0]["status"], "blocked");
    assert_eq!(json["blockedNodes"][0]["nodeId"], "spec");
    assert_eq!(json["blockedNodes"][0]["tools"][0], "publish_artifact");
    assert_eq!(json["blockedNodes"][0]["approvalIds"][0], "appr-1");
    assert!(
        json["blockedNodes"][0].get("unparkable").is_none(),
        "the ordinary case — every call was parked — stays off the wire: {json}"
    );
    assert_eq!(json["approvals"][0]["outcome"], "parked");
    assert_eq!(json["approvals"][0]["approvalId"], "appr-1");

    let json = serde_json::to_value(RunWorkflowResponse {
        output: serde_json::json!({ "nodes": {} }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        run_id: "run-2".into(),
        cancelled: false,
        verdict: WorkflowRunVerdict::Ok,
        nodes: Vec::new(),
        dry_run: false,
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    })
    .expect("serialize");
    assert!(json.get("blockedNodes").is_none(), "{json}");
    assert!(json.get("approvals").is_none(), "{json}");
}

/// A park that could NOT happen is on the wire as loudly as one that did
/// (issue #880).
///
/// This is the arm whose only previous record was a `tracing::error!` — the
/// operator will never be asked about the call, so a run that hides it is
/// telling them the least when it matters most.
#[test]
fn a_failed_park_is_reported_rather_than_only_logged() {
    let json = serde_json::to_value(RunWorkflowResponse {
        output: Value::Null,
        pending_approvals: vec!["spec".into()],
        deliveries: Vec::new(),
        run_id: "run-3".into(),
        cancelled: false,
        verdict: WorkflowRunVerdict::Blocked,
        nodes: Vec::new(),
        dry_run: false,
        board: Vec::new(),
        blocked_nodes: vec![crate::ports::WorkflowBlockedNode {
            node_id: "spec".into(),
            tools: vec!["publish_artifact".into()],
            approval_ids: Vec::new(),
            unparkable: 2,
            stranded: 0,
            blockers: 0,
        }],
        approvals: vec![crate::ports::WorkflowRunApprovalRow {
            node_id: Some("spec".into()),
            tool: Some("publish_artifact".into()),
            outcome: crate::ports::WorkflowApprovalOutcome::ParkFailed,
            approval_id: None,
        }],
    })
    .expect("serialize");
    assert_eq!(json["blockedNodes"][0]["unparkable"], 2);
    assert_eq!(json["approvals"][0]["outcome"], "parkFailed");
    assert!(
        json["approvals"][0].get("approvalId").is_none(),
        "there is no card, so naming one would point at nothing: {json}"
    );
}

/// The run response carries `deliveries` in camelCase — this is the ONLY
/// place an operator learns a report was not delivered, since a delivery
/// failure never fails the run.
#[test]
fn run_response_serializes_delivery_rows_in_camelcase() {
    use crate::ports::{DeliveryReport, DeliveryStatus};

    let json = serde_json::to_value(RunWorkflowResponse {
        output: serde_json::json!({ "nodes": {} }),
        pending_approvals: Vec::new(),
        deliveries: vec![DeliveryReport {
            node: "done".into(),
            kind: "email".into(),
            target: Some("ada@example.com".into()),
            status: DeliveryStatus::Skipped,
            detail: "never written in".into(),
            reason: crate::ports::DeliveryReason::RecipientNotEstablished,
        }],
        run_id: "run-1".into(),
        cancelled: false,
        verdict: WorkflowRunVerdict::Ok,
        nodes: Vec::new(),
        dry_run: false,
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    })
    .unwrap();
    assert_eq!(json["deliveries"][0]["node"], "done");
    assert_eq!(json["deliveries"][0]["status"], "skipped");
    assert_eq!(json["deliveries"][0]["target"], "ada@example.com");
    assert_eq!(json["deliveries"][0]["detail"], "never written in");
    assert!(json["pendingApprovals"].is_array());
}

/// Issue #227: a parked report rides the run response as `"pending"`. The
/// console's `DeliveryStatus` union is spelled in lowercase strings, so the
/// wire word is the contract — a rename here silently drops the row into
/// the frontend's fallback tone.
#[test]
fn run_response_serializes_a_parked_delivery_as_pending() {
    use crate::ports::{DeliveryReport, DeliveryStatus};

    let json = serde_json::to_value(RunWorkflowResponse {
        output: serde_json::json!({ "nodes": {} }),
        pending_approvals: Vec::new(),
        deliveries: vec![DeliveryReport {
            node: "done".into(),
            kind: "email".into(),
            target: Some("stranger@example.com".into()),
            status: DeliveryStatus::Pending,
            detail: "waiting for you in Approvals".into(),
            reason: crate::ports::DeliveryReason::ParkedForApproval,
        }],
        run_id: "run-1".into(),
        cancelled: false,
        verdict: WorkflowRunVerdict::Ok,
        nodes: Vec::new(),
        dry_run: false,
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    })
    .unwrap();
    assert_eq!(json["deliveries"][0]["status"], "pending");
}

/// A graph that routes nothing serializes an empty list, not a missing key —
/// the console can render "no deliveries" without a null check.
#[test]
fn run_response_with_no_deliveries_is_an_empty_list() {
    let json = serde_json::to_value(RunWorkflowResponse {
        output: Value::Null,
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        run_id: "run-1".into(),
        cancelled: false,
        verdict: WorkflowRunVerdict::Ok,
        nodes: Vec::new(),
        dry_run: false,
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    })
    .unwrap();
    assert_eq!(json["deliveries"], serde_json::json!([]));
    // Issue #371: the correlation id rides the response in camelCase, so the
    // console can tie the frames it painted mid-request to the run it awaited.
    assert_eq!(json["runId"], "run-1");
}

#[test]
fn safe_wid_rejects_traversal() {
    assert!(safe_wid("demo"));
    assert!(safe_wid("my-workflow_2"));
    assert!(!safe_wid(""));
    assert!(!safe_wid(".."));
    assert!(!safe_wid("."));
    assert!(!safe_wid("../secrets"));
    assert!(!safe_wid("a/b"));
    assert!(!safe_wid("/etc/passwd"));
    assert!(!safe_wid("foo/../bar"));
}

#[test]
fn one_malformed_workflow_does_not_break_the_list() {
    let dir = seed_demo();
    // A second, broken workflow file must not 500 the whole picker.
    std::fs::write(
        dir.path().join("workflows").join("broken.toml"),
        "id = \"broken\"\nname = \n[[node]] oops",
    )
    .unwrap();
    let files = list_workflows_union(Some(dir.path()), &[]);
    let ids: Vec<_> = files.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["demo"]);
}
