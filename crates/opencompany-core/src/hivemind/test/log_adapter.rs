//! Tests for the company journal read as a `tinyhivemind` session log.

use std::sync::Arc;

use tinyhivemind_hive::{SESSION_WINDOW, Sequence, SessionAuthor, SessionQuery, project_session};

use super::super::*;
use super::fixtures::*;
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, EventSeq};

pub(crate) async fn seed_desk(log: &MemoryLog) -> EventSeq {
    let company = MemoryLog::company();
    // Rows the desk must not see, interleaved so the adapter has to filter
    // rather than merely truncate.
    log.append(
        &company,
        CompanyEvent::OperatorMessage {
            text: "not this desk".into(),
            by: None,
            chat: Some("sales".into()),
            parent: None,
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        },
    )
    .await
    .unwrap();
    log.append(
        &company,
        CompanyEvent::LifecycleChanged {
            from: "running".into(),
            to: "paused".into(),
            by: crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::Operator,
                id: "operator".into(),
            },
        },
    )
    .await
    .unwrap();
    log.append(
        &company,
        CompanyEvent::OperatorMessage {
            text: "Decide the rollout.".into(),
            by: None,
            // Addressed by display name, in the wrong case: the adapter
            // canonicalises it, so the room still opens on the desk.
            chat: Some("ENGINEERING".into()),
            parent: None,
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        },
    )
    .await
    .unwrap()
}

fn session_log(log: &Arc<MemoryLog>) -> EventLogSessionLog {
    EventLogSessionLog::new(
        Arc::clone(log) as Arc<dyn EventLog>,
        MemoryLog::company(),
        "eng".into(),
        "Engineering".into(),
    )
}

fn conversation() -> tinyhivemind_hive::Conversation {
    tinyhivemind_hive::Conversation {
        desk_id: "eng".into(),
        desk_name: "Engineering".into(),
        thread_root: None,
    }
}

#[tokio::test]
async fn the_log_adapter_attributes_and_pages_desk_rows() {
    let log = Arc::new(MemoryLog::default());
    let trigger = seed_desk(&log).await;
    let company = MemoryLog::company();
    log.append(
        &company,
        CompanyEvent::AgentReply {
            audience: Vec::new(),
            chat_id: "eng".into(),
            agent_id: "planner".into(),
            text: "!propose #stage Stage the rollout.".into(),
            steps: Vec::new(),
            task_id: None,
            outputs: Vec::new(),
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
        },
    )
    .await
    .unwrap();
    log.append(
        &company,
        CompanyEvent::AgentReply {
            audience: Vec::new(),
            chat_id: "eng".into(),
            agent_id: HIVE_REPORT_AUTHOR.into(),
            text: "An earlier episode ended.".into(),
            steps: Vec::new(),
            task_id: None,
            outputs: Vec::new(),
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
        },
    )
    .await
    .unwrap();

    let adapter = session_log(&log);
    let projected = project_session(
        &adapter,
        &SessionQuery {
            conversation: conversation(),
            viewer: tinyhivemind_hive::aside::Viewer::Operator,
            before: None,
            window: SESSION_WINDOW,
        },
    )
    .await
    .expect("the page contract holds");

    let authors: Vec<&SessionAuthor> = projected.iter().map(|m| &m.author).collect();
    assert_eq!(projected.len(), 3, "only this desk's chat: {projected:?}");
    assert!(matches!(authors[0], SessionAuthor::Operator));
    assert!(
        matches!(authors[1], SessionAuthor::Agent { id, .. } if id == "planner"),
        "{authors:?}"
    );
    // The room's own outcome row is a system line, so it can never be counted
    // as a supporter and is never hidden by a blind round.
    assert!(
        matches!(authors[2], SessionAuthor::System { kind, .. } if kind == HIVE_REPORT_AUTHOR),
        "{authors:?}"
    );
    assert_eq!(projected[0].sequence, Sequence(trigger.value()));

    // The port's own paging contract: newest-first, `before` exclusive, and a
    // page no larger than asked for.
    let page = tinyhivemind_hive::SessionLog::read_before(&adapter, None, 2)
        .await
        .expect("a bounded page");
    assert_eq!(page.messages.len(), 2);
    assert!(page.messages[0].sequence > page.messages[1].sequence);
    let cursor = page.next_before.expect("more rows remain");
    let older = tinyhivemind_hive::SessionLog::read_before(&adapter, Some(cursor), 8)
        .await
        .expect("the older page");
    assert!(
        older.messages.iter().all(|m| m.sequence < cursor),
        "`before` is exclusive: {older:?}"
    );
}
