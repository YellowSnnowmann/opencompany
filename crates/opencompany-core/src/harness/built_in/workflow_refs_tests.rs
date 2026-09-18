use super::*;

fn wf(id: &str, run: Option<&str>, action: TaskOutputAction) -> TaskOutputWorkflow {
    TaskOutputWorkflow {
        workflow_id: id.to_string(),
        run_id: run.map(str::to_string),
        action,
    }
}

/// The shared-handle property the whole design rests on: the tool pushes
/// through one clone, the brain drains through another, and the drain
/// empties the queue so a second dispatch starts clean.
#[test]
fn a_clone_sees_the_same_queue_and_a_drain_empties_it() {
    let queue = WorkflowRefQueue::default();
    let tool_side = queue.clone();
    tool_side.push(wf("digest", Some("wf-1"), TaskOutputAction::Ran));
    assert_eq!(queue.queued(), 1);

    let drained = queue.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].workflow_id, "digest");
    assert_eq!(queue.queued(), 0, "a drain must not leak into the next run");
    assert!(queue.drain().is_empty());
}

/// A redirect abandons the turn that staged, so the queue is cleared per
/// turn — a workflow an abandoned re-run started must not be credited to
/// the card that eventually settles.
#[test]
fn clear_discards_an_abandoned_turns_refs() {
    let queue = WorkflowRefQueue::default();
    queue.push(wf("stale", None, TaskOutputAction::Created));
    queue.clear();
    assert!(queue.drain().is_empty());
}

/// The common shape: author a graph, then run it. One workflow, one link,
/// and the link is the one that can show what actually executed.
#[test]
fn creating_then_running_one_workflow_collapses_to_the_run() {
    let queue = WorkflowRefQueue::default();
    queue.push(wf("digest", None, TaskOutputAction::Created));
    queue.push(wf("digest", Some("wf-1"), TaskOutputAction::Ran));

    let drained = queue.drain();
    assert_eq!(drained.len(), 1, "got {drained:?}");
    assert_eq!(drained[0].action, TaskOutputAction::Ran);
    assert_eq!(drained[0].run_id.as_deref(), Some("wf-1"));
}

/// Run twice: the later run wins, because the card's link should open the
/// run that most recently happened rather than an earlier one.
#[test]
fn a_later_run_supersedes_an_earlier_one() {
    let queue = WorkflowRefQueue::default();
    queue.push(wf("digest", Some("wf-1"), TaskOutputAction::Ran));
    queue.push(wf("digest", Some("wf-2"), TaskOutputAction::Ran));
    let drained = queue.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].run_id.as_deref(), Some("wf-2"));
}

/// The reverse order must NOT downgrade: authoring a second graph after
/// running the first is two workflows, and re-saving one already run keeps
/// the run link.
#[test]
fn a_later_create_does_not_downgrade_a_run() {
    let queue = WorkflowRefQueue::default();
    queue.push(wf("digest", Some("wf-1"), TaskOutputAction::Ran));
    queue.push(wf("digest", None, TaskOutputAction::Created));
    queue.push(wf("weekly", None, TaskOutputAction::Created));

    let drained = queue.drain();
    assert_eq!(drained.len(), 2, "got {drained:?}");
    assert_eq!(drained[0].workflow_id, "digest");
    assert_eq!(
        drained[0].run_id.as_deref(),
        Some("wf-1"),
        "a re-save must not erase the link to the run"
    );
    // First-appearance order, so the list reads as the turn happened.
    assert_eq!(drained[1].workflow_id, "weekly");
}

/// The bound, so a pathological turn cannot turn a board poll into a
/// payload. Distinct workflows past the cap are dropped, not merged.
#[test]
fn the_staged_list_is_capped() {
    let queue = WorkflowRefQueue::default();
    for i in 0..MAX_WORKFLOW_REFS + 5 {
        queue.push(wf(&format!("wf-{i}"), None, TaskOutputAction::Created));
    }
    assert_eq!(queue.drain().len(), MAX_WORKFLOW_REFS);
}
