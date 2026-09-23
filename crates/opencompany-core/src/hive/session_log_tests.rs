//! Tests for the company journal read as a `tinyhivemind` session log.

use std::sync::Arc;

use tinyhivemind::{
    SESSION_WINDOW, Sequence, SessionAuthor, SessionLog, SessionQuery, project_session,
};

use super::*;
use crate::hive::referral::HIVE_REFERRAL_AUTHOR;
use crate::hive::test_support::{MemoryLog, agent_reply, agent_reply_in, operator_message};
use crate::ports::events::EventLog;

fn adapter(log: &Arc<MemoryLog>) -> EventLogSessionLog {
    seated(log, Vec::new())
}

/// The adapter for a desk that seats `seats`, whose pair channels are part
/// of its transcript.
fn seated(log: &Arc<MemoryLog>, seats: Vec<String>) -> EventLogSessionLog {
    EventLogSessionLog::new(
        Arc::clone(log) as Arc<dyn EventLog>,
        MemoryLog::company(),
        "eng".into(),
        "Engineering".into(),
        seats,
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
        .append(
            &company,
            operator_message("ENGINEERING", "Decide the rollout.", None),
        )
        .await
        .unwrap();
    log.append(&company, agent_reply("eng", "planner", "Stage it."))
        .await
        .unwrap();
    log.append(
        &company,
        agent_reply(
            "eng",
            HIVE_REFERRAL_AUTHOR,
            "@writer on #Content answered: yes",
        ),
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

// ---------------------------------------------------------------------------
// A desk's private conversations are part of its transcript
// ---------------------------------------------------------------------------

/// A conversation two seats open is written to their own pair channel so the
/// room's timeline stays the room's. It is still this desk's transcript: the
/// asker cannot finish until it is answered, and the askee is turned inside
/// it, so the log has to admit it.
#[tokio::test]
async fn a_pair_channel_of_this_desks_seats_is_admitted() {
    let log = Arc::new(MemoryLog::default());
    let company = MemoryLog::company();
    log.append(
        &company,
        agent_reply_in(
            &crate::hive::referral::pair_conversation("ada", "grace"),
            "ada",
            "between us: does the rollout need a freeze?",
            vec!["grace".to_string()],
            None,
        ),
    )
    .await
    .unwrap();

    let page = seated(&log, vec!["ada".into(), "grace".into()])
        .read_before(None, SESSION_WINDOW)
        .await
        .expect("the log reads");

    assert_eq!(page.messages.len(), 1, "{:?}", page.messages);
    assert_eq!(
        page.messages[0].chat_id.as_deref(),
        Some("eng"),
        "reported under the desk id, like every other admitted row"
    );
}

/// The failure the widening could introduce. A pair channel is minted from
/// two roster ids and says nothing about where those two were talking, so
/// admitting one on the strength of its name alone would pull another desk's
/// private exchange into this transcript.
#[tokio::test]
async fn a_pair_channel_of_seats_elsewhere_is_not() {
    let log = Arc::new(MemoryLog::default());
    let company = MemoryLog::company();
    log.append(
        &company,
        agent_reply_in(
            &crate::hive::referral::pair_conversation("ada", "linus"),
            "ada",
            "a conversation on another desk",
            vec!["linus".to_string()],
            None,
        ),
    )
    .await
    .unwrap();

    // `ada` sits here; `linus` does not. One of the two is not enough.
    let page = seated(&log, vec!["ada".into(), "grace".into()])
        .read_before(None, SESSION_WINDOW)
        .await
        .expect("the log reads");

    assert!(page.messages.is_empty(), "{:?}", page.messages);
}

/// A desk with no seats declared admits no pair channel at all, which is what
/// every caller that does not seat a room passes.
#[tokio::test]
async fn a_desk_that_seats_nobody_admits_no_pair_channel() {
    let log = Arc::new(MemoryLog::default());
    let company = MemoryLog::company();
    log.append(
        &company,
        agent_reply_in(
            &crate::hive::referral::pair_conversation("ada", "grace"),
            "ada",
            "between us",
            vec!["grace".to_string()],
            None,
        ),
    )
    .await
    .unwrap();

    let page = adapter(&log)
        .read_before(None, SESSION_WINDOW)
        .await
        .expect("the log reads");

    assert!(page.messages.is_empty(), "{:?}", page.messages);
}

/// The channel key is not a conversation unless it parses as one. A desk
/// whose own id happens to start with the pair prefix must not be widened by
/// accident.
#[tokio::test]
async fn a_channel_that_is_not_a_pair_key_is_unaffected() {
    assert_eq!(
        crate::hive::referral::pair_seats("dm:ada+grace"),
        Some(("ada", "grace"))
    );
    assert_eq!(crate::hive::referral::pair_seats("eng"), None);
    assert_eq!(crate::hive::referral::pair_seats("dm:ada"), None);
}
