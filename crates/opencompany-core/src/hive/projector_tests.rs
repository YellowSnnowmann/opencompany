//! The projector over a real Coordinator with scripted runners: a DM reply
//! lands in the DM, a desk reply threads under the operator's line, a peer
//! message becomes a `HiveMessage`, a completed episode settles once, and a
//! reconciled projector appends nothing twice.

use std::sync::{Arc, OnceLock};

use tinyhivemind_hives::{
    AgentRegistration, AgentRunner, CoordinatorOptions, EpisodeAction, HiveInfo, MemoryStorage,
    SendMessage, TurnDisposition, TurnFuture, TurnOutcome, TurnRequest,
};

use super::*;
use crate::store::FsEventLog;

/// What a scripted agent does on its turn.
#[derive(Clone)]
enum Script {
    /// Reply with this text.
    Reply(&'static str),
    /// Message `to` directly, then reply.
    Message {
        to: &'static str,
        body: &'static str,
    },
    /// Complete the episode it is in with this text.
    Complete(&'static str),
}

struct Scripted {
    script: Script,
    coordinator: Arc<OnceLock<Coordinator>>,
}

impl AgentRunner for Scripted {
    fn run(&self, request: TurnRequest) -> TurnFuture {
        let script = self.script.clone();
        let coordinator = Arc::clone(&self.coordinator);
        Box::pin(async move {
            let coordinator = coordinator.get().expect("coordinator set").clone();
            let reply = match script {
                Script::Reply(text) => Some(text.to_string()),
                Script::Message { to, body } => {
                    coordinator
                        .send(SendMessage {
                            message_id: format!("{}:dm", request.agent_id),
                            sender: request.agent_id.clone(),
                            destination: Destination::Agent(to.to_string()),
                            body: body.to_string(),
                            thread: None,
                            only_for: Vec::new(),
                            starters: Vec::new(),
                        })
                        .await?;
                    None
                }
                Script::Complete(text) => {
                    if let Some(episode) = &request.episode {
                        coordinator
                            .submit_action(
                                &request.agent_id,
                                &episode.episode_id,
                                EpisodeAction::Complete {
                                    body: text.to_string(),
                                },
                            )
                            .await?;
                    }
                    None
                }
            };
            Ok(TurnOutcome {
                session_id: format!("{}:session", request.agent_id),
                reply,
                disposition: TurnDisposition::Completed,
            })
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    events: Arc<dyn EventLog>,
    coordinator: Coordinator,
    projector: Projector,
    meta: Arc<TurnMetaBoard>,
    roster: Arc<HiveRoster>,
}

fn company() -> CompanyId {
    CompanyId::new("acme")
}

async fn fixture(agents: &[(&str, Script)]) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let coordinator = Coordinator::new(
        "rt".into(),
        Arc::new(MemoryStorage::new()),
        CoordinatorOptions {
            round_width: 4,
            ..CoordinatorOptions::default()
        },
    )
    .await
    .expect("coordinator");
    let cell = Arc::new(OnceLock::new());
    let _ = cell.set(coordinator.clone());
    let roster = Arc::new(HiveRoster::default());
    let mut map = HashMap::new();
    for (id, script) in agents {
        let coordinator_id = format!("acme--{id}");
        map.insert(coordinator_id.clone(), (*id).to_string());
        coordinator
            .register_agent(AgentRegistration {
                agent_id: coordinator_id,
                runtime_id: "rt".into(),
                runner: Arc::new(Scripted {
                    script: script.clone(),
                    coordinator: Arc::clone(&cell),
                }),
            })
            .await
            .expect("register");
    }
    roster.install(map);
    let meta = Arc::new(TurnMetaBoard::default());
    let projector = Projector::new(
        company(),
        Arc::clone(&events),
        coordinator.clone(),
        Arc::clone(&roster),
        Arc::clone(&meta),
    );
    projector.reconcile().await.expect("reconcile");
    Fixture {
        _dir: dir,
        events,
        coordinator,
        projector,
        meta,
        roster,
    }
}

fn host(id: &str, destination: Destination, body: &str, starters: &[&str]) -> SendMessage {
    SendMessage {
        message_id: id.to_string(),
        sender: String::new(),
        destination,
        body: body.to_string(),
        thread: None,
        only_for: Vec::new(),
        starters: starters.iter().map(|s| (*s).to_string()).collect(),
    }
}

async fn journal(events: &Arc<dyn EventLog>) -> Vec<(EventSeq, CompanyEvent)> {
    events
        .read_from(&company(), EventSeq::new(0), usize::MAX)
        .await
        .expect("read")
        .into_iter()
        .map(|row| (row.seq, row.event))
        .collect()
}

#[tokio::test]
async fn a_dm_reply_lands_in_the_operators_dm_with_its_turn_meta() {
    let f = fixture(&[("writer", Script::Reply("drafted"))]).await;
    let operator = f
        .events
        .append(
            &company(),
            CompanyEvent::OperatorMessage {
                text: "draft it".into(),
                by: None,
                chat: Some("dm:writer".into()),
                parent: None,
                deliverable: None,
                mentions: Vec::new(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("operator line");
    let receipt = f
        .coordinator
        .send_as_host(host(
            &format!("op:{}", operator.value()),
            Destination::Agent("acme--writer".into()),
            "draft it",
            &[],
        ))
        .await
        .expect("send");
    f.projector
        .note_host_line(
            receipt.sequence,
            "dm:writer",
            Some(operator),
            Some("acme--writer"),
        )
        .await;
    f.meta.push(
        "acme--writer",
        TurnMeta {
            task_id: Some("card-1".into()),
            ..TurnMeta::default()
        },
    );
    f.coordinator.run_until_idle().await.expect("run");
    assert_eq!(f.projector.project().await.expect("project"), 1);
    let rows = journal(&f.events).await;
    let (_, reply) = rows.last().expect("a reply");
    match reply {
        CompanyEvent::AgentReply {
            chat_id,
            agent_id,
            text,
            task_id,
            hive,
            ..
        } => {
            assert_eq!(chat_id, "dm:writer");
            assert_eq!(agent_id, "writer");
            assert_eq!(text, "drafted");
            assert_eq!(task_id.as_deref(), Some("card-1"));
            assert!(hive.is_some());
        }
        other => panic!("expected a reply, got {other:?}"),
    }
    assert_eq!(
        f.projector.project().await.expect("again"),
        0,
        "nothing twice"
    );
}

#[tokio::test]
async fn a_hive_thread_sequence_resolves_to_its_host_journal_sequence() {
    let f = fixture(&[]).await;
    let host_seq = EventSeq::new(41);
    f.projector
        .note_host_line(7, "content", Some(host_seq), None)
        .await;

    assert_eq!(f.projector.host_sequence_of(7).await, Some(host_seq));
    assert_eq!(f.projector.host_sequence_of(8).await, None);
}

#[tokio::test]
async fn a_desk_reply_threads_under_the_operators_line_and_the_episode_settles_once() {
    let f = fixture(&[
        ("writer", Script::Complete("here is the post")),
        ("editor", Script::Reply("looks good")),
    ])
    .await;
    f.coordinator
        .create_hive(HiveInfo {
            hive_id: "content".into(),
            name: "Content".into(),
            description: None,
            members: vec!["acme--writer".into(), "acme--editor".into()],
        })
        .await
        .expect("hive");
    let operator = EventSeq::new(41);
    let receipt = f
        .coordinator
        .send_as_host(host(
            "op:41",
            Destination::Hive("content".into()),
            "write a post",
            &["acme--writer"],
        ))
        .await
        .expect("send");
    f.projector
        .note_host_line(receipt.sequence, "content", Some(operator), None)
        .await;
    f.coordinator.run_until_idle().await.expect("run");
    f.projector.project().await.expect("project");
    let rows = journal(&f.events).await;
    let replies: Vec<&CompanyEvent> = rows
        .iter()
        .map(|(_, event)| event)
        .filter(|event| matches!(event, CompanyEvent::AgentReply { .. }))
        .collect();
    assert!(
        !replies.is_empty(),
        "the starter's completion is on the desk: {rows:?}"
    );
    for reply in &replies {
        let CompanyEvent::AgentReply {
            chat_id,
            parent,
            hive,
            ..
        } = reply
        else {
            unreachable!()
        };
        assert_eq!(chat_id, "content");
        assert_eq!(
            *parent,
            Some(operator),
            "threaded under the operator's line"
        );
        assert!(hive.as_ref().is_some_and(|hive| hive.episode_id.is_some()));
    }
    let settled = rows
        .iter()
        .filter(|(_, event)| matches!(event, CompanyEvent::HiveEpisodeSettled { .. }))
        .count();
    assert_eq!(settled, 1);
    f.projector.project().await.expect("again");
    let again = journal(&f.events).await;
    assert_eq!(again.len(), rows.len(), "a second pass appends nothing");

    // A fresh projector over the same journal resumes where this one stopped.
    let fresh = Projector::new(
        company(),
        Arc::clone(&f.events),
        f.coordinator.clone(),
        Arc::clone(&f.roster),
        Arc::new(TurnMetaBoard::default()),
    );
    fresh.reconcile().await.expect("reconcile");
    assert_eq!(fresh.project().await.expect("project"), 0);
}

#[tokio::test]
async fn a_peer_message_is_a_hive_message_not_a_reply() {
    let f = fixture(&[
        (
            "writer",
            Script::Message {
                to: "acme--editor",
                body: "can you check this?",
            },
        ),
        ("editor", Script::Reply("checked")),
    ])
    .await;
    f.coordinator
        .send_as_host(host(
            "op:3",
            Destination::Agent("acme--writer".into()),
            "ask the editor",
            &[],
        ))
        .await
        .expect("send");
    f.coordinator.run_until_idle().await.expect("run");
    f.projector.project().await.expect("project");
    let rows = journal(&f.events).await;
    let peer = rows.iter().find_map(|(_, event)| match event {
        CompanyEvent::HiveMessage {
            sender,
            destination,
            text,
            ..
        } => Some((sender.clone(), destination.clone(), text.clone())),
        _ => None,
    });
    assert_eq!(
        peer,
        Some((
            "writer".to_string(),
            HiveDestination::Agent("editor".to_string()),
            "can you check this?".to_string()
        ))
    );
}

#[test]
fn the_roster_maps_ids_both_ways_and_strips_a_retired_prefix() {
    let roster = HiveRoster::default();
    roster.install(HashMap::from([(
        "acme--ceo".to_string(),
        "ceo".to_string(),
    )]));
    assert_eq!(roster.manifest_id(&company(), "acme--ceo"), "ceo");
    assert_eq!(roster.manifest_id(&company(), "acme--gone"), "gone");
    assert_eq!(roster.coordinator_id("ceo").as_deref(), Some("acme--ceo"));
}
