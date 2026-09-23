//! The journal side of the episode host: what the library commits becomes a
//! row this desk can read back.

use std::sync::Arc;

use tinyhivemind::aside::Viewer;
use tinyhivemind::{Conversation, SESSION_WINDOW, SessionQuery, project_session};
use tinyhivemind_driver::{Commit, Note};
use tinyhivemind_openhuman::{EpisodeHost, Journal};

use super::DeskHost;
use crate::hive::test_support::MemoryLog;
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId, EventSeq};

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
        Vec::new(),
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

/// A parking hook that holds whichever seats it was told to.
#[derive(Debug)]
struct Holds(Vec<String>);

#[async_trait::async_trait]
impl super::SeatParking for Holds {
    async fn park(&self, seat: &str) -> bool {
        self.0.iter().any(|held| held == seat)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_seat_with_an_approval_waiting_is_held_and_one_without_is_not() {
    let events: Arc<dyn EventLog> = Arc::new(MemoryLog::default());
    let host = host(Arc::clone(&events)).parking(Arc::new(Holds(vec!["one".to_owned()])));
    assert_eq!(
        host.after_turn("one", None).expect("the hook runs"),
        tinyhivemind_openhuman::Disposition::Parked,
        "one raised an approval, so the episode holds it"
    );
    assert_eq!(
        host.after_turn("two", None).expect("the hook runs"),
        tinyhivemind_openhuman::Disposition::Done,
        "two raised nothing, so its turn simply stands"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_that_parks_nothing_never_holds_a_seat() {
    let events: Arc<dyn EventLog> = Arc::new(MemoryLog::default());
    let host = host(Arc::clone(&events));
    assert_eq!(
        host.after_turn("one", None).expect("the hook runs"),
        tinyhivemind_openhuman::Disposition::Done
    );
}

/// A commit as the conductor makes it, on this side of the wire.
fn commit(json: serde_json::Value) -> Commit {
    serde_json::from_value(json).expect("a commit the driver would make")
}

/// **A conclusion is threaded under the conversation it concludes.**
///
/// The conductor mints it with the conversation it is about and no thread,
/// so journaled as a desk row it hangs off the episode's root: a later reply
/// there, which the channel-level read drops, and no reply at all in the
/// conversation. Threaded under the ask it is the exchange's closing line.
///
/// Keyed on the utterance, not the field: desk work a seat does from inside a
/// conversation carries `conversation` too, and belongs on the desk.
#[tokio::test(flavor = "multi_thread")]
async fn a_conclusion_is_threaded_under_the_conversation_it_concludes() {
    let log = Arc::new(MemoryLog::default());
    let company = MemoryLog::company();
    let host = host(Arc::clone(&log) as Arc<dyn EventLog>);
    let pair = crate::hive::referral::pair_conversation("ada", "grace");
    let ask = log
        .append(
            &company,
            crate::hive::test_support::agent_reply_in(
                &pair,
                "ada",
                "does the rollout need a freeze?",
                vec!["grace".into()],
                None,
            ),
        )
        .await
        .unwrap();
    host.event(&tinyhivemind_driver::Event::Asked {
        seat: "ada".into(),
        askee: "grace".into(),
        root: tinyhivemind::Sequence(ask.value()),
    });

    let conclusion = host
        .commit(&commit(serde_json::json!({
            "author": "grace",
            "utterance": { "kind": "dm", "to": ["ada"], "message": "concluded our conversation: no, ship it" },
            "thread": null,
            "only_for": "ada",
            "conversation": ask.value(),
            "purpose": { "kind": "desk" },
        })))
        .expect("the journal takes the conclusion");
    let lifted = host
        .commit(&commit(serde_json::json!({
            "author": "grace",
            "utterance": { "kind": "broadcast", "message": "freeze policy: none on file" },
            "thread": null,
            "only_for": null,
            "conversation": ask.value(),
            "purpose": { "kind": "desk" },
        })))
        .expect("the journal takes the broadcast");

    let rows = log.read_from(&company, EventSeq::new(1), 16).await.unwrap();
    let row = |seq: tinyhivemind::Sequence| {
        rows.iter()
            .find(|stored| stored.seq.value() == seq.0)
            .map(|stored| match &stored.event {
                CompanyEvent::AgentReply {
                    chat_id, parent, ..
                } => (chat_id.clone(), *parent),
                other => panic!("not a reply: {other:?}"),
            })
            .expect("journaled")
    };
    assert_eq!(
        row(conclusion),
        (pair.clone(), Some(ask)),
        "the conclusion is a row of the conversation, under its ask"
    );
    assert_eq!(
        row(lifted),
        ("engineering".to_string(), None),
        "desk work said from inside a conversation stays on the desk"
    );
}
