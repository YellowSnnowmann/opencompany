use super::*;

#[tokio::test]
async fn spawn_task_tool_enqueues_a_task() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    // An empty store loads no record, so assignee grounding fails open and
    // the string is queued exactly as typed — isolating this test to the
    // plain enqueue path. Grounding itself is covered separately below.
    let tool = SpawnTaskTool::new(
        queue.clone(),
        CompanyId::new("acme"),
        Arc::new(MemStore::default()),
    );
    tool.execute(json!({ "title": "Ship it", "note": "soon", "assignee": "eng" }))
        .await
        .expect("execute");
    let drained = queue.drain(MAX_DELEGATIONS_PER_TURN);
    assert_eq!(
        drained,
        vec![Delegation::SpawnTask {
            title: "Ship it".to_string(),
            note: Some("soon".to_string()),
            assignee: Some("eng".to_string()),
        }]
    );
}

#[tokio::test]
async fn spawn_task_tool_requires_a_title() {
    let queue = DelegationQueue::default();
    let tool = SpawnTaskTool::new(
        queue.clone(),
        CompanyId::new("acme"),
        Arc::new(MemStore::default()),
    );
    assert!(tool.execute(json!({ "note": "no title" })).await.is_err());
    assert_eq!(queue.queued(), 0);
}

#[tokio::test]
async fn assign_task_tool_enqueues_an_assignment() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = AssignTaskTool::new(queue.clone());
    tool.execute(json!({ "task_id": "t1", "assignee": "eng", "note": "closer to it" }))
        .await
        .expect("execute");
    assert_eq!(
        queue.drain(MAX_DELEGATIONS_PER_TURN),
        vec![Delegation::AssignTask {
            task_id: "t1".to_string(),
            assignee: "eng".to_string(),
            note: Some("closer to it".to_string()),
        }]
    );
}

#[tokio::test]
async fn assign_task_tool_requires_a_card_and_an_assignee() {
    let queue = DelegationQueue::default();
    let tool = AssignTaskTool::new(queue.clone());
    assert!(tool.execute(json!({ "assignee": "eng" })).await.is_err());
    assert!(tool.execute(json!({ "task_id": "t1" })).await.is_err());
    // A blank string is not an assignee.
    assert!(
        tool.execute(json!({ "task_id": "t1", "assignee": "  " }))
            .await
            .is_err()
    );
    assert_eq!(queue.queued(), 0);
}

#[tokio::test]
async fn review_task_tool_enqueues_both_verdicts() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = ReviewTaskTool::new(queue.clone());
    let approved = tool
        .execute(json!({ "task_id": "t1", "decision": "approve", "note": "good" }))
        .await
        .expect("approve");
    let revised = tool
        .execute(json!({ "task_id": "t2", "decision": "revise" }))
        .await
        .expect("revise");

    // Issue #453: staged truth, not the past tense. The card has not moved
    // when this sentence is written — the drain the claim promises is what
    // moves it — and saying otherwise is what made an undrained turn a lie
    // told through the agent.
    assert!(!approved.is_error);
    let text = approved.text();
    assert!(text.contains("Recorded your approval of card t1"), "{text}");
    assert!(text.contains("as this turn completes"), "{text}");
    assert!(!text.contains("has moved"), "nothing has moved yet: {text}");
    let text = revised.text();
    assert!(text.contains("card t2 returns to To-do"), "{text}");
    assert!(text.contains("as this turn completes"), "{text}");

    assert_eq!(
        queue.drain(MAX_DELEGATIONS_PER_TURN),
        vec![
            Delegation::ReviewTask {
                task_id: "t1".to_string(),
                decision: ReviewDecision::Approve,
                note: Some("good".to_string()),
            },
            Delegation::ReviewTask {
                task_id: "t2".to_string(),
                decision: ReviewDecision::Revise,
                note: None,
            },
        ]
    );
}

/// An unrecognised verdict is an error, never a silent approval — a card
/// must not pass review because the model typed something unexpected.
#[tokio::test]
async fn review_task_tool_rejects_an_unknown_verdict_rather_than_approving() {
    let queue = DelegationQueue::default();
    let tool = ReviewTaskTool::new(queue.clone());
    assert!(
        tool.execute(json!({ "task_id": "t1", "decision": "maybe" }))
            .await
            .is_err()
    );
    assert!(tool.execute(json!({ "task_id": "t1" })).await.is_err());
    assert_eq!(queue.queued(), 0, "nothing may be queued on a bad verdict");
}

/// Both lifecycle tools are internal delegation work, so the approval
/// policy must classify them as such — never as an external effect to park.
#[test]
fn the_lifecycle_tools_are_internal_delegation_tools() {
    assert!(is_delegation_tool(ASSIGN_TASK_TOOL));
    assert!(is_delegation_tool(REVIEW_TASK_TOOL));
}

