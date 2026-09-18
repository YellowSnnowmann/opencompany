use super::*;
use crate::ports::tasks::TaskTitle;
use crate::ports::types::{EventSeq, StoredEvent};
use futures::stream::BoxStream;
use std::sync::Mutex;

/// An in-memory board, so the decorator is exercised against a real
/// `TaskStore` rather than a stub that cannot tell insert from update.
#[derive(Default)]
struct MemTasks {
    rows: Mutex<Vec<TaskRecord>>,
}

#[async_trait]
impl TaskStore for MemTasks {
    async fn list(&self, _company: &CompanyId) -> Result<Vec<TaskRecord>> {
        Ok(self.rows.lock().unwrap().clone())
    }
    async fn upsert(&self, _company: &CompanyId, task: &TaskRecord) -> Result<()> {
        let mut rows = self.rows.lock().unwrap();
        match rows.iter_mut().find(|t| t.id == task.id) {
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
    ) -> Result<bool> {
        let mut rows = self.rows.lock().unwrap();
        let Some(existing) = rows
            .iter_mut()
            .find(|existing| *existing == observed && existing.column == expected_column)
        else {
            return Ok(false);
        };
        *existing = task.clone();
        Ok(true)
    }
    async fn delete(&self, _company: &CompanyId, id: &str) -> Result<bool> {
        let mut rows = self.rows.lock().unwrap();
        let before = rows.len();
        rows.retain(|t| t.id != id);
        Ok(rows.len() != before)
    }
}

#[derive(Default)]
struct MemLog {
    appended: Mutex<Vec<CompanyEvent>>,
}

#[async_trait]
impl EventLog for MemLog {
    async fn append(&self, _id: &CompanyId, event: CompanyEvent) -> Result<EventSeq> {
        let mut got = self.appended.lock().unwrap();
        got.push(event);
        Ok(EventSeq::new(got.len() as u64))
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        Ok(Vec::new())
    }
    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(futures::stream::empty())
    }
}

/// An event log that refuses every append, so the "record-keeping never
/// fails the work it records" contract is tested rather than asserted.
struct BrokenLog;

#[async_trait]
impl EventLog for BrokenLog {
    async fn append(&self, _id: &CompanyId, _event: CompanyEvent) -> Result<EventSeq> {
        Err(crate::error::OpenCompanyError::Store(
            "log is down".to_string(),
        ))
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        Ok(Vec::new())
    }
    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(futures::stream::empty())
    }
}

fn card(id: &str, column: &str) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        title: TaskTitle::authored("Draft the launch note"),
        note: None,
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: String::new(),
        updated_at_millis: 1,
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: crate::ports::tasks::TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    }
}

fn wired() -> (Arc<BoardAnnouncer>, Arc<MemLog>, CompanyId) {
    let log = Arc::new(MemLog::default());
    let store = Arc::new(BoardAnnouncer::new(
        Arc::new(MemTasks::default()),
        log.clone(),
    ));
    (store, log, CompanyId::new("announcer-co"))
}

/// The headline of #464: a card written by *anything* announces itself, and
/// the first write of an id is an `opened` rather than an `updated`.
#[tokio::test]
async fn a_new_card_announces_that_it_opened() {
    let (store, log, co) = wired();
    store.upsert(&co, &card("t-1", "todo")).await.unwrap();

    let got = log.appended.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "one announcement for one new card: {got:?}");
    match &got[0] {
        CompanyEvent::TaskCardChanged {
            task_id,
            change,
            column,
        } => {
            assert_eq!(task_id, "t-1");
            assert_eq!(change, CHANGE_OPENED);
            assert_eq!(column.as_deref(), Some("todo"));
        }
        other => panic!("expected a board announcement, got {other:?}"),
    }
}

/// A second write of the same id is an update, and carries the column it
/// landed in — the frame a second console needs to follow a drag.
#[tokio::test]
async fn moving_a_card_announces_an_update_with_its_new_column() {
    let (store, log, co) = wired();
    store.upsert(&co, &card("t-1", "todo")).await.unwrap();
    store.upsert(&co, &card("t-1", "in_review")).await.unwrap();

    let got = log.appended.lock().unwrap().clone();
    assert_eq!(got.len(), 2, "open then move: {got:?}");
    match &got[1] {
        CompanyEvent::TaskCardChanged { change, column, .. } => {
            assert_eq!(change, CHANGE_UPDATED);
            assert_eq!(column.as_deref(), Some("in_review"));
        }
        other => panic!("expected a board announcement, got {other:?}"),
    }
}

/// Re-saving a card exactly as it already stands announces nothing. A
/// settle that re-persists an unchanged card is ordinary, and a frame for a
/// board that did not move is noise a console cannot tell from a real
/// change.
#[tokio::test]
async fn an_unchanged_re_save_is_silent() {
    let (store, log, co) = wired();
    store.upsert(&co, &card("t-1", "todo")).await.unwrap();
    store.upsert(&co, &card("t-1", "todo")).await.unwrap();

    assert_eq!(
        log.appended.lock().unwrap().len(),
        1,
        "the identical re-save must not announce"
    );
}

/// A removed card announces its removal without a column — a card that is
/// gone is not in one, and an empty string would read as a column whose id
/// is blank.
#[tokio::test]
async fn a_deleted_card_announces_removal_without_a_column() {
    let (store, log, co) = wired();
    store.upsert(&co, &card("t-1", "todo")).await.unwrap();
    assert!(store.delete(&co, "t-1").await.unwrap());

    let got = log.appended.lock().unwrap().clone();
    match got.last().expect("a removal announcement") {
        CompanyEvent::TaskCardChanged { change, column, .. } => {
            assert_eq!(change, CHANGE_REMOVED);
            assert!(column.is_none(), "a removed card is in no column");
        }
        other => panic!("expected a board announcement, got {other:?}"),
    }
}

/// Deleting an id the board never held changed nothing, so it announces
/// nothing.
#[tokio::test]
async fn deleting_an_absent_card_is_silent() {
    let (store, log, co) = wired();
    assert!(!store.delete(&co, "never-existed").await.unwrap());
    assert!(log.appended.lock().unwrap().is_empty());
}

/// Record-keeping never fails the work it records: a refusing event log
/// leaves the board write successful and the card on the board.
#[tokio::test]
async fn a_refusing_event_log_does_not_fail_the_board_write() {
    let store = BoardAnnouncer::new(Arc::new(MemTasks::default()), Arc::new(BrokenLog));
    let co = CompanyId::new("announcer-broken");

    store
        .upsert(&co, &card("t-1", "todo"))
        .await
        .expect("the board write succeeds even when the announcement cannot be filed");
    assert_eq!(store.list(&co).await.unwrap().len(), 1);
}
