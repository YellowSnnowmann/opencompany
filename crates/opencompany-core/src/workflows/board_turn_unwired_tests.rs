//! A workflow node's board write on a runtime with no task board is reported
//! as `boardUnwired`, not as a failed write the store refused.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::company::parse_workflow;
use crate::harness::HarnessPool;
use crate::ports::{WorkflowBoardAction, WorkflowRun, WorkflowRunContext};

use super::gated_tool_turn_tests::{Turn, record, spawn_script};

const AGENT_GRAPH: &str = r#"
id = "board"
name = "Board"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "work"
kind = "agent"
name = "Work"
summary = "Triage the inbox."
agent = "ceo"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "work"
[[edge]]
from = "work"
to = "done"
"#;

async fn run_without_board(turns: Vec<Turn>) -> WorkflowRun {
    let dir = tempfile::tempdir().unwrap();
    let base_url = spawn_script(turns).await;
    let (mut deps, _journal) = super::gated_tool_turn_tests::deps(base_url, dir.path());
    deps.tasks = None;
    let record = record();
    let pool = Arc::new(HarnessPool::new());
    pool.ensure(&record, &deps).await.expect("roster builds");
    let file = parse_workflow(AGENT_GRAPH).expect("graph parses");
    super::runner::run_workflow(
        pool,
        deps,
        &record,
        &file,
        Value::Null,
        &WorkflowRunContext::new(false),
    )
    .await
    .expect("a board write with no board must not fail the run")
}

#[tokio::test]
async fn a_spawn_with_no_board_wired_is_reported_as_board_unwired() {
    let run = run_without_board(vec![
        Turn::Call {
            tool: "spawn_task",
            args: json!({ "title": "Reply to the auditor" }),
        },
        Turn::Say("Opened a card for it."),
    ])
    .await;

    assert_eq!(run.board.len(), 1, "{:?}", run.board);
    assert_eq!(run.board[0].action, WorkflowBoardAction::BoardUnwired);
    assert!(run.board[0].action.failed());
    assert_eq!(run.board[0].task_id, None);
    assert!(
        run.notices.iter().any(|n| n.contains("has no task board")
            && n.contains("\"Reply to the auditor\" was not opened or changed")),
        "{:?}",
        run.notices
    );
}

#[tokio::test]
async fn an_assign_with_no_board_wired_is_reported_as_board_unwired() {
    let run = run_without_board(vec![
        Turn::Call {
            tool: "assign_task",
            args: json!({ "task_id": "card-1", "assignee": "ceo" }),
        },
        Turn::Say("Assigned it."),
    ])
    .await;

    assert_eq!(run.board.len(), 1, "{:?}", run.board);
    assert_eq!(run.board[0].action, WorkflowBoardAction::BoardUnwired);
    assert_eq!(run.board[0].task_id.as_deref(), Some("card-1"));
    assert!(
        run.notices
            .iter()
            .any(|n| n.contains("has no task board") && n.contains("\"card-1\"")),
        "{:?}",
        run.notices
    );
}

#[test]
fn board_unwired_serializes_as_camel_case() {
    assert_eq!(
        serde_json::to_value(WorkflowBoardAction::BoardUnwired).unwrap(),
        json!("boardUnwired")
    );
}
