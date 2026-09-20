//! Tests for the company journal read as a `tinyhivemind` session log.

use std::sync::Arc;

use tinyhivemind::{SESSION_WINDOW, Sequence, SessionAuthor, SessionLog, SessionQuery, project_session};

use super::*;
use crate::hive::referral::HIVE_REFERRAL_AUTHOR;
use crate::hive::test_support::{MemoryLog, agent_reply, agent_reply_in, operator_message};
use crate::ports::events::EventLog;

fn adapter(log: &Arc<MemoryLog>) -> EventLogSessionLog {
    EventLogSessionLog::new(
        Arc::clone(log) as Arc<dyn EventLog>,
        MemoryLog::company(),
        "eng".into(),
        "Engineering".into(),
    )
}

#[tokio::test]
async fn the_log_adapter_attributes_and_pages_desk_rows() {
    let log = Arc::new(MemoryLog::default());
    let company = MemoryLog::company();
    log.append(&company, operator_message("sales", "not this desk", None))
        .await
        .unwrap();
    // Addressed by display name, in the wrong case: the adapter canonicalises
    // it, so the row still lands in the room it was meant for.
    let trigger = log
        .append(&company, operator_message("ENGINEERING", "Decide the rollout.", None))
        .await
        .unwrap();
    log.append(&company, agent_reply("eng", "planner", "Stage it."))
        .await
        .unwrap();
    log.append(
        &company,
        agent_reply("eng", HIVE_REFERRAL_AUTHOR, "@writer on #Content answered: yes"),
    )
    .await
    .unwrap();
    log.append(
        &company,
        agent_reply_in("eng", "planner", "quietly", vec!["reviewer".into()], None),
    )
    .await
    .unwrap();

    let adapter = adapter(&log);
    assert_eq!(adapter.desk_id(), "eng");
    assert_eq!(adapter.conversation(None).desk_name, "Engineering");
    let projected = project_session(
        &adapter,
        &SessionQuery {
            conversation: adapter.conversation(None),
            viewer: tinyhivemind::aside::Viewer::Operator,
            before: None,
            window: SESSION_WINDOW,
        },
    )
    .await
    .expect("the page contract holds");

    let authors: Vec<&SessionAuthor> = projected.iter().map(|m| &m.author).collect();
    assert_eq!(projected.len(), 4, "only this desk's chat: {projected:?}");
    assert!(matches!(authors[0], SessionAuthor::Operator));
    assert!(matches!(authors[1], SessionAuthor::Agent { id, .. } if id == "planner"));
    // An answer carried home is a system line, never a teammate.
    assert!(
        matches!(authors[2], SessionAuthor::System { kind, .. } if kind == HIVE_REFERRAL_AUTHOR)
    );
    assert_eq!(projected[0].sequence, Sequence(trigger.value()));
    assert!(matches!(
        projected[3].audience,
        Audience::Aside { ref members } if members == &["reviewer".to_string()]
    ));

    // The port's own paging contract: newest-first, `before` exclusive, and a
    // page no larger than asked for.
    let page = SessionLog::read_before(&adapter, None, 2)
        .await
        .expect("a bounded page");
    assert_eq!(page.messages.len(), 2);
    assert!(page.messages[0].sequence > page.messages[1].sequence);
    let cursor = page.next_before.expect("more rows remain");
    let older = SessionLog::read_before(&adapter, Some(cursor), 8)
        .await
        .expect("the older page");
    assert!(
        older.messages.iter().all(|m| m.sequence < cursor),
        "`before` is exclusive: {older:?}"
    );
    assert!(older.next_before.is_none(), "the journal ended: {older:?}");
}

#[tokio::test]
async fn a_desk_buried_behind_unrelated_rows_still_pages_through() {
    let log = Arc::new(MemoryLog::default());
    let company = MemoryLog::company();
    log.append(&company, operator_message("eng", "first", None))
        .await
        .unwrap();
    for _ in 0..600 {
        log.append(&company, operator_message("sales", "noise", None))
            .await
            .unwrap();
    }
    let adapter = adapter(&log);
    let page = SessionLog::read_before(&adapter, None, 4)
        .await
        .expect("keeps reading past a chat-less chunk");
    assert_eq!(page.messages.len(), 1);
    assert!(page.next_before.is_none());
}
