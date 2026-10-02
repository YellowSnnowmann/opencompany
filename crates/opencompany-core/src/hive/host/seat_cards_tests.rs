use std::sync::Arc;

use super::*;
use crate::ports::tasks::{TaskDeliverable, TaskTitle};
use crate::store::FsOps;

const DESK: &str = "studio";
const EPISODE: &str = "ep-1";

fn spawn(title: &str) -> Delegation {
    Delegation::SpawnTask {
        title: title.to_owned(),
        note: Some("brief".to_owned()),
        assignee: None,
    }
}

fn desk<'a>(tasks: &'a dyn TaskStore, company: &'a CompanyId) -> CardDesk<'a> {
    CardDesk {
        tasks,
        company,
        record: None,
        desk_id: DESK,
        thread_root: Some(EventSeq::new(5)),
        episode_id: EPISODE,
        opened_at: Some(EventSeq::new(5)),
    }
}

fn handler(column: &str, assignee: &str) -> TaskRecord {
    TaskRecord {
        id: "handler".to_owned(),
        title: TaskTitle::authored("Plan the launch"),
        note: None,
        column: column.to_owned(),
        priority: "medium".to_owned(),
        assignee: assignee.to_owned(),
        updated_at_millis: 1,
        origin: TaskOrigin::new(Some(DESK.to_owned()), Some(EventSeq::new(5))),
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_message_seq: Some(EventSeq::new(5)),
        origin_workflow_id: None,
        bounced: None,
        opened_by: None,
    }
}

fn store() -> (tempfile::TempDir, Arc<dyn TaskStore>) {
    let dir = tempfile::tempdir().unwrap();
    let tasks: Arc<dyn TaskStore> = Arc::new(FsOps::new(dir.path()));
    (dir, tasks)
}

#[test]
fn the_budget_holds_three_cards_and_one_per_title() {
    let cards = EpisodeCards::default();
    assert_eq!(cards.reserve("Draft the post"), Ok(()));
    assert_eq!(
        cards.reserve("draft the post!"),
        Err(CardRefusal::Duplicate)
    );
    assert_eq!(cards.reserve("Book the venue"), Ok(()));
    assert_eq!(cards.reserve("Order lunch"), Ok(()));
    assert_eq!(
        cards.reserve("Hire a band"),
        Err(CardRefusal::Full {
            cap: EPISODE_CARD_CAP
        })
    );
    cards.release("Order lunch");
    assert_eq!(cards.reserve("Hire a band"), Ok(()));
}

#[tokio::test]
async fn the_first_card_takes_over_the_message_card_and_the_next_is_minted() {
    let (_dir, tasks) = store();
    let company = CompanyId::new("acme");
    tasks
        .upsert(&company, &handler(COLUMN_TODO, DESK))
        .await
        .unwrap();
    let cards = EpisodeCards::default();
    cards.reserve("Draft the post").unwrap();
    cards.reserve("Book the venue").unwrap();
    let opened = cards
        .open(
            &desk(tasks.as_ref(), &company),
            "writer",
            vec![spawn("Draft the post"), spawn("Book the venue")],
        )
        .await;
    assert!(opened.problems.is_empty(), "{:?}", opened.problems);
    assert_eq!(opened.cards[0], "handler");
    let board = tasks.list(&company).await.unwrap();
    assert_eq!(board.len(), 2);
    let adopted = board.iter().find(|card| card.id == "handler").unwrap();
    assert_eq!(adopted.assignee, "writer");
    assert!(adopted.note.as_deref().unwrap().contains("Draft the post"));
    assert_eq!(
        adopted.opened_by.as_ref().map(|by| by.agent_id.as_str()),
        Some("writer")
    );
    let minted = board.iter().find(|card| card.id != "handler").unwrap();
    assert_eq!(minted.title, "Book the venue");
    assert_eq!(minted.column, COLUMN_TODO);
    assert_eq!(minted.origin_chat_id(), Some(DESK));
    assert_eq!(minted.origin_parent(), Some(EventSeq::new(5)));
    assert_eq!(
        minted.opened_by,
        Some(TaskOpener {
            agent_id: "writer".to_owned(),
            episode_id: Some(EPISODE.to_owned()),
        })
    );
    assert_eq!(cards.publish_target("writer"), Some(minted.id.clone()));
    assert_eq!(cards.publish_target("analyst"), Some("handler".to_owned()));
}

#[tokio::test]
async fn a_card_somebody_already_owns_is_not_taken_over() {
    let (_dir, tasks) = store();
    let company = CompanyId::new("acme");
    tasks
        .upsert(&company, &handler(COLUMN_TODO, "analyst"))
        .await
        .unwrap();
    let cards = EpisodeCards::default();
    cards.reserve("Draft the post").unwrap();
    let opened = cards
        .open(
            &desk(tasks.as_ref(), &company),
            "writer",
            vec![spawn("Draft the post")],
        )
        .await;
    assert_ne!(opened.cards, vec!["handler".to_owned()]);
    assert_eq!(tasks.list(&company).await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_resumed_episode_recalls_its_cards_once_and_its_message_card() {
    let (_dir, tasks) = store();
    let company = CompanyId::new("acme");
    let first = EpisodeCards::default();
    first.reserve("Draft the post").unwrap();
    first
        .open(
            &desk(tasks.as_ref(), &company),
            "writer",
            vec![spawn("Draft the post")],
        )
        .await;
    let mut message = handler("in_review", "writer");
    message.id = "message".to_owned();
    tasks.upsert(&company, &message).await.unwrap();

    let resumed = EpisodeCards::default();
    let board = tasks.list(&company).await.unwrap();
    resumed.recall(&board, DESK, EPISODE, Some(EventSeq::new(5)));
    resumed.recall(&board, DESK, EPISODE, Some(EventSeq::new(5)));
    assert_eq!(
        resumed.reserve("draft the post"),
        Err(CardRefusal::Duplicate)
    );
    assert_eq!(resumed.reserve("Two"), Ok(()));
    assert_eq!(resumed.reserve("Three"), Ok(()));
    assert!(matches!(
        resumed.reserve("Four"),
        Err(CardRefusal::Full { .. })
    ));
    assert_eq!(
        resumed.publish_target("analyst"),
        Some("message".to_owned())
    );
}

struct Refusing;

#[async_trait::async_trait]
impl TaskStore for Refusing {
    async fn list(&self, _: &CompanyId) -> crate::Result<Vec<TaskRecord>> {
        Ok(Vec::new())
    }
    async fn upsert(&self, _: &CompanyId, _: &TaskRecord) -> crate::Result<()> {
        Err(crate::OpenCompanyError::Harness("disk full".to_owned()))
    }
    async fn update_if_column(
        &self,
        _: &CompanyId,
        _: &TaskRecord,
        _: &TaskRecord,
        _: &str,
    ) -> crate::Result<bool> {
        Ok(false)
    }
    async fn delete(&self, _: &CompanyId, _: &str) -> crate::Result<bool> {
        Ok(false)
    }
}

#[tokio::test]
async fn a_card_the_board_refuses_is_named_to_the_seat_and_its_hold_released() {
    let company = CompanyId::new("acme");
    let cards = EpisodeCards::default();
    cards.reserve("Draft the post").unwrap();
    let opened = cards
        .open(
            &desk(&Refusing, &company),
            "writer",
            vec![
                spawn("Draft the post"),
                Delegation::AssignTask {
                    task_id: "t".to_owned(),
                    assignee: "x".to_owned(),
                    note: None,
                },
            ],
        )
        .await;
    assert!(opened.cards.is_empty());
    assert_eq!(opened.problems.len(), 1);
    assert!(
        opened.problems[0].contains("\"Draft the post\""),
        "{:?}",
        opened.problems
    );
    assert_eq!(
        cards.reserve("Draft the post"),
        Ok(()),
        "the title is free again"
    );
}

#[tokio::test]
async fn a_minted_publish_card_is_marked_as_the_message_card() {
    let (_dir, tasks) = store();
    let company = CompanyId::new("acme");
    let mut minted = handler("in_review", "writer");
    minted.id = "minted".to_owned();
    minted.origin_message_seq = None;
    tasks.upsert(&company, &minted).await.unwrap();
    stamp_message_card(&desk(tasks.as_ref(), &company), "minted")
        .await
        .unwrap();
    let board = tasks.list(&company).await.unwrap();
    assert_eq!(board[0].origin_message_seq, Some(EventSeq::new(5)));
}

#[test]
fn the_seat_note_names_the_card_verb_and_no_withheld_one() {
    assert!(SEAT_CARDS_NOTE.contains("`spawn_task`"));
    assert!(SEAT_CARDS_NOTE.contains(&format!("`{}ask`", crate::hive::host::TOOL_PREFIX)));
    for withheld in crate::harness::built_in::EPISODE_WITHHELD_TOOLS {
        assert!(!SEAT_CARDS_NOTE.contains(withheld), "{withheld}");
    }
}
