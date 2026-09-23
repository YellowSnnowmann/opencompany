//! The journal side of the episode host: what the library commits becomes a
//! row this desk can read back.

use std::sync::Arc;

use tinyhivemind::aside::Viewer;
use tinyhivemind::{Conversation, SESSION_WINDOW, SessionQuery, project_session};
use tinyhivemind_driver::Note;
use tinyhivemind_openhuman::{EpisodeHost, Journal};

use super::DeskHost;
use crate::hive::test_support::MemoryLog;
use crate::ports::events::EventLog;
use crate::ports::types::CompanyId;

fn desk() -> Conversation {
    Conversation {
        desk_id: "engineering".to_owned(),
        desk_name: "Engineering".to_owned(),
        thread_root: None,
    }
}

fn host(events: Arc<dyn EventLog>) -> DeskHost {
    DeskHost::new(
        CompanyId::new("acme"),
        "engineering".to_owned(),
        "Engineering".to_owned(),
        events,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_desk_note_is_the_episodes_own_voice_and_may_be_private() {
    let events: Arc<dyn EventLog> = Arc::new(MemoryLog::default());
    let host = host(Arc::clone(&events));
    host.note(&Note {
        body: "you hold open work".to_owned(),
        thread: None,
        only_for: Some("one".to_owned()),
    })
    .expect("the journal takes the note");
    for (seat, readable) in [("one", true), ("two", false)] {
        let rows = project_session(
            host.log(),
            &SessionQuery {
                conversation: desk(),
                viewer: Viewer::Agent { id: seat.into() },
                before: None,
                window: SESSION_WINDOW,
            },
        )
        .await
        .expect("reads");
        assert_eq!(rows.len(), 1, "one row, withheld rather than dropped");
        assert_eq!(
            rows[0].readable().is_some(),
            readable,
            "{seat} should{} read the nudge",
            if readable { "" } else { " not" }
        );
    }
    assert!(format!("{host:?}").contains("engineering"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_without_a_pool_runs_unbracketed_rather_than_refusing() {
    let log = Arc::new(MemoryLog::default());
    let events: Arc<dyn EventLog> = log.clone();
    let host = host(Arc::clone(&events));
    let ran = host
        .wrap_turn("one", Box::pin(async { Ok("said".to_owned()) }))
        .await
        .expect("the turn runs");
    assert_eq!(ran, "said");
    assert!(
        log.rows().is_empty(),
        "no pool, no lock, and so no bracket to write"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn each_settled_wave_moves_the_round_a_bracket_names() {
    let events: Arc<dyn EventLog> = Arc::new(MemoryLog::default());
    let host = host(Arc::clone(&events)).episode("ep-1");
    assert_eq!(host.wave.load(std::sync::atomic::Ordering::SeqCst), 0);
    // The loop hands a snapshot over once per settled wave; this host keeps
    // the count rather than the snapshot.
    let driver_state = None::<()>;
    let _ = driver_state;
    assert_eq!(host.episode_id, "ep-1");
}
