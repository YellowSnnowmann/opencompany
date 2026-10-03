//! Tests for `start_task` — the verb that fires the dispatch edge.

use super::super::*;
use crate::harness::built_in::board_start::{BoardStarter, BoardStarterHandle};
use crate::ports::tasks::{COLUMN_IN_PROGRESS, COLUMN_TODO, TaskDeliverable, TaskTitle};
use crate::ports::types::CompanyId;
use crate::ports::{TaskRecord, TaskStore};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// The board the tool reads, and nothing more of a store than it touches.
#[derive(Default)]
struct Board {
    rows: Mutex<Vec<TaskRecord>>,
}

#[async_trait]
impl TaskStore for Board {
    async fn list(&self, _company: &CompanyId) -> crate::Result<Vec<TaskRecord>> {
        Ok(self.rows.lock().expect("board").clone())
    }
    async fn upsert(&self, _company: &CompanyId, task: &TaskRecord) -> crate::Result<()> {
        let mut rows = self.rows.lock().expect("board");
        match rows.iter_mut().find(|row| row.id == task.id) {
            Some(slot) => *slot = task.clone(),
            None => rows.push(task.clone()),
        }
        Ok(())
    }
    async fn update_if_column(
        &self,
        _company: &CompanyId,
        task: &TaskRecord,
        observed: &TaskRecord,
        expected_column: &str,
    ) -> crate::Result<bool> {
        let mut rows = self.rows.lock().expect("board");
        let Some(current) = rows.iter_mut().find(|row| row.id == observed.id) else {
            return Ok(false);
        };
        if current != observed || current.column != expected_column {
            return Ok(false);
        }
        *current = task.clone();
        Ok(true)
    }
    async fn delete(&self, _company: &CompanyId, _id: &str) -> crate::Result<bool> {
        unreachable!("start_task deletes nothing")
    }
}

/// What the capability was asked to start.
#[derive(Default)]
struct Started {
    cards: Mutex<Vec<TaskRecord>>,
}

#[async_trait]
impl BoardStarter for Started {
    async fn start(&self, _observed: &TaskRecord, card: &TaskRecord) -> crate::Result<bool> {
        self.cards.lock().expect("started").push(card.clone());
        Ok(true)
    }
}

struct ChangedBeforeStart;

#[async_trait]
impl BoardStarter for ChangedBeforeStart {
    async fn start(&self, _observed: &TaskRecord, _card: &TaskRecord) -> crate::Result<bool> {
        Ok(false)
    }
}

fn card(id: &str, column: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        title: TaskTitle::system("Ship the thing"),
        note: None,
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: "engineer".to_string(),
        updated_at_millis: 1,
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_message_seq: None,
        origin_workflow_id: None,
        bounced: None,
        opened_by: None,
    }
}

fn tool(board: &Arc<Board>, handle: BoardStarterHandle) -> StartTaskTool {
    StartTaskTool::new(
        CompanyId::new("acme"),
        Arc::clone(board) as Arc<dyn TaskStore>,
        handle,
    )
}

/// The happy path: a To-do card goes to Working **through the capability**, not
/// through the plain port — which is the whole reason this tool exists, since the
/// plain port cannot fire the dispatch edge.
#[tokio::test]
async fn starting_a_todo_card_hands_it_to_the_capability_in_working() {
    let board = Arc::new(Board::default());
    board
        .upsert(&CompanyId::new("acme"), &card("c1", COLUMN_TODO))
        .await
        .unwrap();
    let started = Arc::new(Started::default());
    let handle = BoardStarterHandle::default();
    handle.set(&(Arc::clone(&started) as Arc<dyn BoardStarter>));

    let res = tool(&board, handle)
        .execute(serde_json::json!({ "task_id": "c1" }))
        .await
        .unwrap();
    assert!(!res.is_error, "{}", res.text());

    let handed = started.cards.lock().expect("started").clone();
    assert_eq!(handed.len(), 1);
    assert_eq!(handed[0].id, "c1");
    assert_eq!(
        handed[0].column, COLUMN_IN_PROGRESS,
        "the card must already carry Working, or the edge does not fire"
    );
}

#[tokio::test]
async fn start_task_reports_a_card_changed_during_the_dispatch_race() {
    let board = Arc::new(Board::default());
    board
        .upsert(&CompanyId::new("acme"), &card("c1", COLUMN_TODO))
        .await
        .unwrap();
    let handle = BoardStarterHandle::default();
    let racing = Arc::new(ChangedBeforeStart);
    handle.set(&(Arc::clone(&racing) as Arc<dyn BoardStarter>));

    let result = tool(&board, handle)
        .execute(serde_json::json!({ "task_id": "c1" }))
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(result.text().contains("changed while it was being started"));
}

/// With nothing wired the tool refuses in its own turn rather than reporting a
/// start. A receipt for work that will not happen is the failure this codebase
/// keeps paying for — the seat that told an operator to fix a board that was
/// never broken.
#[tokio::test]
async fn an_unwired_board_refuses_instead_of_claiming_a_start() {
    let board = Arc::new(Board::default());
    board
        .upsert(&CompanyId::new("acme"), &card("c1", COLUMN_TODO))
        .await
        .unwrap();

    let res = tool(&board, BoardStarterHandle::default())
        .execute(serde_json::json!({ "task_id": "c1" }))
        .await
        .unwrap();
    assert!(res.is_error);
    let said = res.text();
    assert!(said.contains("no board dispatch wired"), "{said}");
}

/// A card already past To-do is left alone. The edge is a transition and not a
/// state, so re-writing the column would start no second attempt — and saying it
/// did would be the same lie.
#[tokio::test]
async fn a_card_already_working_is_left_alone() {
    let board = Arc::new(Board::default());
    board
        .upsert(&CompanyId::new("acme"), &card("c1", COLUMN_IN_PROGRESS))
        .await
        .unwrap();
    let started = Arc::new(Started::default());
    let handle = BoardStarterHandle::default();
    handle.set(&(Arc::clone(&started) as Arc<dyn BoardStarter>));

    let res = tool(&board, handle)
        .execute(serde_json::json!({ "task_id": "c1" }))
        .await
        .unwrap();
    assert!(res.is_error, "{}", res.text());
    assert!(
        started.cards.lock().expect("started").is_empty(),
        "nothing may be handed to the capability"
    );
}

/// An id that names no card is a clean refusal naming the read that finds one.
#[tokio::test]
async fn an_unknown_card_is_a_clean_refusal() {
    let board = Arc::new(Board::default());
    let started = Arc::new(Started::default());
    let handle = BoardStarterHandle::default();
    handle.set(&(Arc::clone(&started) as Arc<dyn BoardStarter>));

    let res = tool(&board, handle)
        .execute(serde_json::json!({ "task_id": "nope" }))
        .await
        .unwrap();
    assert!(res.is_error, "{}", res.text());
    assert!(started.cards.lock().expect("started").is_empty());
}
