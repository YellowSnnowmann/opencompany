//! Tests for the episode host, driven by a scripted seat runner over agents
//! on the process-wide ephemeral runtime — no model, no MCP server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use openhuman_embed::AgentSpec;
use tinyhivemind::speech::Utterance;

use super::*;
use crate::harness::openhuman_runtime::{RuntimeBoot, global};
use crate::hive::episode_store::{EpisodeStatus, list_episodes};
use crate::hive::graph::desk_hives;
use crate::hive::prompt::sentinel;
use crate::hive::test_support::{MemoryLog, TWO_DESKS, operator_message, record};
use crate::ports::types::UtteranceKind;

/// One scripted answer: what the seat says on its n-th turn.
#[derive(Clone)]
enum Say {
    Speak(Utterance),
    Bare(&'static str),
    Fail,
    Slow(Duration, Utterance),
}

/// Answers from a per-agent script, in order; records every prompt it saw.
struct Script {
    lines: Mutex<HashMap<String, Vec<Say>>>,
    seen: Mutex<Vec<SeatTurn>>,
    /// The seats running at once, and the most seen together.
    live: Mutex<(usize, usize)>,
}

impl Script {
    fn new(lines: &[(&str, Vec<Say>)]) -> Arc<Self> {
        Arc::new(Self {
            lines: Mutex::new(
                lines
                    .iter()
                    .map(|(id, says)| ((*id).to_string(), says.clone()))
                    .collect(),
            ),
            seen: Mutex::new(Vec::new()),
            live: Mutex::new((0, 0)),
        })
    }

    fn prompts_for(&self, agent: &str) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|seat| seat.agent_id == agent)
            .map(|seat| seat.message.clone())
            .collect()
    }

    fn peak_concurrency(&self) -> usize {
        self.live.lock().unwrap().1
    }
}

#[async_trait]
impl SeatRunner for Script {
    async fn run_seat(&self, seat: SeatTurn) -> std::result::Result<SeatOutcome, SeatFailure> {
        {
            let mut live = self.live.lock().unwrap();
            live.0 += 1;
            live.1 = live.1.max(live.0);
        }
        self.seen.lock().unwrap().push(seat.clone());
        let next = self
            .lines
            .lock()
            .unwrap()
            .get_mut(&seat.agent_id)
            .and_then(|says| (!says.is_empty()).then(|| says.remove(0)));
        // Let siblings start so the overlap is observable.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let outcome = match next.unwrap_or(Say::Speak(Utterance::CompleteEpisode {
            message: "done".into(),
        })) {
            Say::Speak(utterance) => Ok(SeatOutcome {
                reply: String::new(),
                utterances: vec![utterance],
                ..Default::default()
            }),
            Say::Bare(text) => Ok(SeatOutcome {
                reply: text.to_string(),
                ..Default::default()
            }),
            Say::Fail => Err(SeatFailure::Failed("boom".into())),
            Say::Slow(wait, utterance) => {
                if wait > seat.timeout {
                    Err(SeatFailure::TimedOut)
                } else {
                    tokio::time::sleep(wait).await;
                    Ok(SeatOutcome {
                        reply: String::new(),
                        utterances: vec![utterance],
                        ..Default::default()
                    })
                }
            }
        };
        self.live.lock().unwrap().0 -= 1;
        outcome
    }
}

fn post(text: &str) -> Say {
    Say::Speak(Utterance::Post {
        message: text.into(),
    })
}

fn complete(text: &str) -> Say {
    Say::Speak(Utterance::CompleteEpisode {
        message: text.into(),
    })
}

fn dm(to: &[&str], text: &str) -> Say {
    Say::Speak(Utterance::Dm {
        to: to.iter().map(|id| (*id).to_string()).collect(),
        message: text.into(),
    })
}

fn broadcast(text: &str) -> Say {
    Say::Speak(Utterance::Broadcast {
        message: text.into(),
    })
}

/// A dispatcher over `TWO_DESKS` with agents on the ephemeral runtime.
async fn dispatcher(script: Arc<Script>) -> (HiveDispatcher, Arc<MemoryLog>) {
    let runtime = global(RuntimeBoot::ephemeral()).await.expect("runtime");
    let record = Arc::new(record(TWO_DESKS));
    let salt = uuid::Uuid::new_v4().simple().to_string();
    let agents: HashMap<String, openhuman_embed::Agent> = ["ceo", "engineer", "writer"]
        .into_iter()
        .map(|id| {
            let agent = runtime
                .agent(AgentSpec::new(format!("hive-driver-{id}-{}", &salt[..8])))
                .expect("agent");
            (id.to_string(), agent)
        })
        .collect();
    let (hives, errors) = desk_hives(&record, 1, &|id| agents.get(id).cloned());
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(hives.len(), 2, "two desks of two");
    let log = Arc::new(MemoryLog::default());
    (
        HiveDispatcher {
            record,
            events: log.clone(),
            hives,
            router: None,
            seats: script,
        },
        log,
    )
}

fn trigger(seq: EventSeq, text: &str) -> Trigger {
    Trigger {
        seq,
        text: text.into(),
        parent: None,
        mentions: Vec::new(),
        hop: 0,
        origin: None,
        referred_from: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_two_seat_desk_completes_in_two_rounds_with_both_seats_running_at_once() {
    let script = Script::new(&[
        (
            "engineer",
            vec![post("Plan: two sprints."), complete("Plan stands.")],
        ),
        ("ceo", vec![post("Budget is fine."), complete("Approved.")]),
    ]);
    let (host, log) = dispatcher(script.clone()).await;
    let seq = log
        .append(
            &MemoryLog::company(),
            operator_message("engineering", "Plan the login page.", None),
        )
        .await
        .unwrap();

    let report = host
        .run_desk_message("engineering", trigger(seq, "Plan the login page."))
        .await
        .expect("the episode runs");
    assert_eq!(report.reason, EpisodeReason::CompleteEpisode);
    assert_eq!(report.rounds, 2);
    // The primary seat's completion is the room's answer.
    assert_eq!(report.completed_by.as_deref(), Some("engineer"));
    assert_eq!(report.summary.as_deref(), Some("Plan stands."));
    assert!(script.peak_concurrency() >= 2, "seats ran together");

    // Without a router the opening round is the desk in order, bounded by
    // the round width: both seats.
    let kinds = log.kinds();
    assert_eq!(kinds[1], "EpisodeOpened");
    assert!(kinds.contains(&"RoundStarted"));
    assert!(kinds.contains(&"RoundCommitted"));
    assert!(kinds.contains(&"EpisodeStateSaved"));
    assert_eq!(*kinds.last().unwrap(), "EpisodeCompleted");
    let replies = log.replies("engineering");
    assert_eq!(replies.len(), 4, "{replies:?}");

    let episodes = list_episodes(log.as_ref(), &MemoryLog::company(), None, None, 10)
        .await
        .unwrap();
    assert_eq!(episodes.len(), 1);
    assert_eq!(episodes[0].status, EpisodeStatus::Completed);
    assert_eq!(episodes[0].participants, vec!["engineer", "ceo"]);
    assert_eq!(episodes[0].revision, 4);

    // Every seat prompt opens with the sentinel and carries the fence.
    let prompts = script.prompts_for("ceo");
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].starts_with(&sentinel("engineering", &report.episode_id, 0)));
    assert!(prompts[1].starts_with(&sentinel("engineering", &report.episode_id, 2)));
    assert!(prompts[0].contains("The operator asked (^1):\nPlan the login page."));
    assert!(prompts[0].contains("`mcp_call_tool` on server `opencompany`"));
    // The second round's delta shows the first round's posts, not the
    // trigger again.
    assert!(prompts[1].contains("@engineer (^"));
    assert!(prompts[1].contains("Plan: two sprints."));
    // The assignment stands for the whole episode.
    assert!(prompts[1].contains("The operator asked (^1)"));

    // The reply rows carry the episode metadata, the brackets carry the seat.
    let rows = log.rows();
    let seat_rows: Vec<_> = rows
        .iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::AgentReply {
                agent_id,
                episode: Some(episode),
                parent,
                ..
            } => Some((agent_id.clone(), episode.clone(), *parent)),
            _ => None,
        })
        .collect();
    assert_eq!(seat_rows.len(), 4);
    assert_eq!(seat_rows[0].1.kind, UtteranceKind::Post);
    assert_eq!(seat_rows[0].1.revision, 0);
    assert_eq!(seat_rows[0].2, Some(seq));
    assert_eq!(seat_rows[3].1.kind, UtteranceKind::CompleteEpisode);
    assert_eq!(seat_rows[3].1.revision, 2);
    let brackets: Vec<_> = rows
        .iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::TurnStarted {
                agent_id: Some(agent),
                episode_id: Some(_),
                round_revision: Some(revision),
                ..
            } => Some(("started", agent.clone(), *revision)),
            CompanyEvent::TurnSettled {
                agent_id: Some(agent),
                round_revision: Some(revision),
                outcome,
                ..
            } => Some((outcome.as_str(), agent.clone(), *revision)),
            _ => None,
        })
        .collect();
    assert_eq!(brackets.len(), 8, "{brackets:?}");
    assert!(brackets.iter().all(|(word, _, _)| *word != "failed"));
    let committed: Vec<_> = rows
        .iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::RoundCommitted {
                revision,
                utterances,
                ..
            } => Some((*revision, utterances.len())),
            _ => None,
        })
        .collect();
    assert_eq!(committed, vec![(0, 2), (2, 2)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dm_narrows_its_row_and_a_broadcast_without_jev_falls_back_to_the_other_seat() {
    let script = Script::new(&[
        (
            "engineer",
            vec![
                dm(&["ceo"], "quietly: can we afford it?"),
                complete("Shipped."),
            ],
        ),
        (
            "ceo",
            vec![broadcast("Someone take the copy."), complete("Fine.")],
        ),
    ]);
    let (host, log) = dispatcher(script).await;
    let seq = log
        .append(
            &MemoryLog::company(),
            operator_message("engineering", "Go.", None),
        )
        .await
        .unwrap();
    let report = host
        .run_desk_message("engineering", trigger(seq, "Go."))
        .await
        .expect("the episode runs");
    assert_eq!(report.reason, EpisodeReason::CompleteEpisode);
    let rows = log.rows();
    let dm_row = rows
        .iter()
        .find_map(|stored| match &stored.event {
            CompanyEvent::AgentReply {
                audience,
                episode: Some(episode),
                ..
            } if episode.kind == UtteranceKind::Dm => Some((audience.clone(), episode.to.clone())),
            _ => None,
        })
        .expect("the dm row");
    assert_eq!(dm_row, (vec!["ceo".to_string()], vec!["ceo".to_string()]));
    assert!(
        rows.iter()
            .any(|stored| matches!(&stored.event, CompanyEvent::DmDelivered { from, to, .. } if from == "engineer" && to == &["ceo".to_string()]))
    );
    let routed = rows
        .iter()
        .find_map(|stored| match &stored.event {
            CompanyEvent::BroadcastRouted {
                agent_id,
                plan,
                router,
                ..
            } => Some((agent_id.clone(), plan.agent_ids(), *router)),
            _ => None,
        })
        .expect("the broadcast was routed");
    assert_eq!(routed.0, "ceo");
    assert_eq!(routed.1, vec!["engineer"]);
    assert_eq!(routed.2, crate::hive::routing::Router::Fallback);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seat_that_never_speaks_is_retried_then_completed_on_its_behalf() {
    let script = Script::new(&[
        (
            "engineer",
            vec![
                Say::Bare("I think so"),
                Say::Bare("still thinking"),
                Say::Bare("<<<POST done anyway POST>>>"),
                Say::Bare("<<<POST done anyway POST>>>"),
            ],
        ),
        ("ceo", vec![complete("ok")]),
    ]);
    let (host, log) = dispatcher(script.clone()).await;
    let seq = log
        .append(
            &MemoryLog::company(),
            operator_message("engineering", "Go.", None),
        )
        .await
        .unwrap();
    let report = host
        .run_desk_message("engineering", trigger(seq, "Go."))
        .await
        .expect("the episode runs");
    assert_eq!(report.reason, EpisodeReason::Failed);
    let prompts = script.prompts_for("engineer");
    assert_eq!(prompts.len(), 4, "four attempts");
    assert!(prompts[1].contains("## Reminder"));
    assert!(prompts[3].starts_with("Hive turn: desk engineering"));
    let salvaged = log
        .replies("engineering")
        .into_iter()
        .find(|(agent, _)| agent == "engineer")
        .expect("the engineer's salvaged row");
    assert_eq!(salvaged.1, "done anyway");
    let settled: Vec<&'static str> = log
        .rows()
        .iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::TurnSettled {
                agent_id: Some(agent),
                outcome,
                ..
            } if agent == "engineer" => Some(outcome.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        settled,
        vec![
            "no_utterance",
            "no_utterance",
            "no_utterance",
            "no_utterance"
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_or_timed_out_seat_is_settled_as_such_and_the_room_still_closes() {
    let script = Script::new(&[
        ("engineer", vec![Say::Fail]),
        (
            "ceo",
            vec![Say::Slow(
                Duration::from_secs(3600),
                Utterance::Post {
                    message: "late".into(),
                },
            )],
        ),
    ]);
    let (host, log) = dispatcher(script).await;
    let seq = log
        .append(
            &MemoryLog::company(),
            operator_message("engineering", "Go.", None),
        )
        .await
        .unwrap();
    let report = host
        .run_desk_message("engineering", trigger(seq, "Go."))
        .await
        .expect("the episode runs");
    assert_eq!(report.reason, EpisodeReason::Failed);
    let outcomes: Vec<(String, &'static str)> = log
        .rows()
        .iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::TurnFailed {
                agent_id: Some(agent),
                outcome: Some(outcome),
                ..
            } => Some((agent.clone(), outcome.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        outcomes,
        vec![
            ("engineer".to_string(), "failed"),
            ("ceo".to_string(), "timed_out")
        ]
    );
    assert!(log.rows().iter().any(|stored| matches!(
        &stored.event,
        CompanyEvent::EpisodeCompleted {
            reason: EpisodeReason::Failed,
            ..
        }
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_follow_up_in_the_thread_joins_the_open_episode_and_a_resume_replays_as_a_no_op() {
    let script = Script::new(&[
        ("engineer", vec![post("first"), complete("done")]),
        ("ceo", vec![complete("fine"), complete("still fine")]),
    ]);
    let (host, log) = dispatcher(script.clone()).await;
    let company = MemoryLog::company();
    let seq = log
        .append(&company, operator_message("engineering", "Go.", None))
        .await
        .unwrap();
    let first = host
        .run_desk_message("engineering", trigger(seq, "Go."))
        .await
        .expect("runs");
    assert_eq!(first.reason, EpisodeReason::CompleteEpisode);
    // The episode on this thread is complete, so a follow-up in the thread
    // opens a new one on the same thread rather than joining a closed room.
    let follow = log
        .append(
            &company,
            operator_message("engineering", "And the tests?", Some(seq.value())),
        )
        .await
        .unwrap();
    let mut second = trigger(follow, "And the tests?");
    second.parent = Some(seq);
    let report = host
        .run_desk_message("engineering", second)
        .await
        .expect("runs");
    assert_ne!(report.episode_id, first.episode_id);
    let episodes = list_episodes(log.as_ref(), &company, None, None, 10)
        .await
        .unwrap();
    assert_eq!(episodes.len(), 2);
    assert!(
        episodes
            .iter()
            .all(|e| e.parent_id.as_deref() == Some(&seq.value().to_string()))
    );

    // Resume from the checkpoint: every committed row replays as a no-op
    // and the driver state comes back at the same revision.
    let desk = host.hive("engineering").unwrap();
    let routing = crate::hive::routing::desk_routing(&host.record, "engineering");
    let persisted =
        crate::hive::episode_store::latest_state(log.as_ref(), &company, &first.episode_id)
            .await
            .unwrap()
            .expect("checkpoint");
    let resumed = host
        .resume_from(&desk, persisted.clone(), &routing)
        .await
        .unwrap();
    assert_eq!(
        resumed.state.revision(),
        persisted.state["revision"].as_u64().unwrap()
    );
    assert!(matches!(
        tinyhivemind_hive::completion_status(resumed.state.episode()),
        tinyhivemind_hive::CompletionStep::Complete { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desk_mention_refers_the_question_and_the_answer_comes_home() {
    let script = Script::new(&[
        (
            "engineer",
            vec![
                post("@#content can you draft the copy?"),
                complete("waiting on copy"),
                complete("copy is in, done"),
            ],
        ),
        ("ceo", vec![complete("fine"), complete("fine again")]),
        ("writer", vec![complete("Here is the copy: Sign in.")]),
    ]);
    let (host, log) = dispatcher(script.clone()).await;
    let company = MemoryLog::company();
    let seq = log
        .append(
            &company,
            operator_message("engineering", "Build login.", None),
        )
        .await
        .unwrap();
    let report = host
        .run_desk_message("engineering", trigger(seq, "Build login."))
        .await
        .expect("runs");
    assert_eq!(report.reason, EpisodeReason::CompleteEpisode);
    // The far desk's episode and the answer run on their own tasks.
    for _ in 0..200 {
        let done = log.rows().iter().any(|stored| {
            matches!(
                &stored.event,
                CompanyEvent::ReferralEnqueued {
                    returning: true,
                    ..
                }
            )
        });
        if done {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let rows = log.rows();
    let forward = rows
        .iter()
        .find_map(|stored| match &stored.event {
            CompanyEvent::ReferralEnqueued {
                returning: false,
                from_desk,
                to_desk,
                asker,
                episode_id,
                hop,
                ..
            } => Some((
                from_desk.clone(),
                to_desk.clone(),
                asker.clone(),
                episode_id.clone(),
                *hop,
            )),
            _ => None,
        })
        .expect("the forward marker");
    assert_eq!(forward.0, "engineering");
    assert_eq!(forward.1, "content");
    assert_eq!(forward.2, "engineer");
    assert_eq!(forward.3.as_deref(), Some(report.episode_id.as_str()));
    assert_eq!(forward.4, 1);
    let content = log.replies("content");
    assert_eq!(content[0].0, crate::hive::referral::HIVE_REFERRAL_AUTHOR);
    assert!(
        content[0]
            .1
            .contains("asks: @#content can you draft the copy?")
    );
    assert!(
        content
            .iter()
            .any(|(agent, text)| agent == "writer" && text.contains("Sign in."))
    );
    let home = log
        .replies("engineering")
        .into_iter()
        .find(|(agent, _)| agent == crate::hive::referral::HIVE_REFERRAL_AUTHOR)
        .expect("the answer came home");
    assert!(
        home.1
            .contains("@writer on #Content desk answered: Here is the copy: Sign in.")
    );
    let episodes = list_episodes(log.as_ref(), &company, None, None, 10)
        .await
        .unwrap();
    assert_eq!(episodes.len(), 2, "{episodes:?}");
    assert!(episodes.iter().any(|e| e.chat_id == "content"));
    // The writer's prompt named the asking desk.
    let writer_prompts = script.prompts_for("writer");
    assert_eq!(writer_prompts.len(), 1);
    assert!(writer_prompts[0].contains("#Engineering desk put this question to this desk"));
    // The engineer was reopened once the answer landed and completed again.
    for _ in 0..200 {
        if script.prompts_for("engineer").len() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let engineer_prompts = script.prompts_for("engineer");
    assert_eq!(engineer_prompts.len(), 3, "reopened by the answer");
    assert!(engineer_prompts[2].contains("answered the question you put to it"));
}
