//! One round: the seats the driver proposed, run at once, each brought to
//! exactly one utterance.
//!
//! A round is the unit of concurrency. Every proposed seat gets its own
//! `tokio` task (its own stack — the OpenHuman turn chain is deep) and the
//! round joins them all; a seat that shares its agent with another desk's
//! round waits on that agent's turn lock inside its task, never holding this
//! round's siblings up. What comes back per seat is folded to **exactly one**
//! utterance: a turn that spoke once is committed as is; a turn that spoke
//! not at all is retried with a stricter reminder (up to [`MAX_ATTEMPTS`]),
//! then its reply is salvaged through `fence::extract_post` and committed as
//! a `complete_episode` so the room never hangs on a seat that will not use
//! its tools; a turn that timed out or errored is settled as such and
//! completed synthetically for the same reason.
//!
//! Every seat turn is bracketed on the journal — `TurnStarted` before, and
//! `TurnSettled` or `TurnFailed` after, with the seat, the episode and the
//! round on each — which is the per-seat lane the console draws.

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::join_all;
use tinyhivemind::aside::Viewer;
use tinyhivemind::speech::{Utterance, fence};
use tinyhivemind::Sequence;
use tinyhivemind_openhuman::{CommittedUtterance, PendingRound};

use crate::error::Result;
use crate::hive::driver::{EpisodeRun, HiveDispatcher, SeatFailure, SeatOutcome, SeatTurn, Trigger};
use crate::hive::prompt::{self, SeatPrompt};
use crate::hive::session_log::EventLogSessionLog;
use crate::hive::tools::HiveTurn;
use crate::ports::types::{
    CompanyEvent, EpisodeReason, EventSeq, ReplyEpisode, RoundUtteranceRecord, TurnOutcome,
    UtteranceKind,
};

/// Attempts a seat gets to end its turn with a speech tool before the host
/// completes it on its behalf.
pub const MAX_ATTEMPTS: u32 = 4;

/// What a seat is asked to do this round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatAssignment {
    /// The assignment text.
    pub text: String,
}

impl SeatAssignment {
    /// The opening assignment: answer the message that opened the episode.
    #[must_use]
    pub fn trigger(trigger: &Trigger) -> Self {
        let text = match &trigger.referred_from {
            Some(desk) => format!(
                "#{desk} put this question to this desk (^{}):\n{}\n\nAnswer it from what this \
                 desk knows, and say plainly if it does not know — a wrong answer crossing desks \
                 is worse than no answer.",
                trigger.seq.value(),
                trigger.text.trim()
            ),
            None => format!(
                "The operator asked (^{}):\n{}",
                trigger.seq.value(),
                trigger.text.trim()
            ),
        };
        Self { text }
    }

    /// A seat reopened by a routed broadcast.
    #[must_use]
    pub fn broadcast(author: &str, seq: u64, message: &str) -> Self {
        Self {
            text: format!("@{author} handed you this by broadcast (^{seq}):\n{}", message.trim()),
        }
    }

    /// A seat reopened because the desk it asked answered.
    #[must_use]
    pub fn answer(desk_name: &str, seq: EventSeq) -> Self {
        Self {
            text: format!(
                "#{desk_name} answered the question you put to it (^{}). Take the answer up, \
                 and complete your assignment when nothing is left open.",
                seq.value()
            ),
        }
    }

    /// The standing instruction for a seat with nothing new.
    #[must_use]
    pub fn carry_on() -> Self {
        Self {
            text: "Carry your assignment on. If nothing is left open for you, call \
                   `complete_episode`."
                .into(),
        }
    }
}

/// What one round committed.
#[derive(Debug, Default)]
pub struct RoundOutcome {
    /// The utterances, in commit order, as the driver folds them.
    pub committed: Vec<CommittedUtterance>,
    /// The same, as the journal records them.
    pub records: Vec<RoundUtteranceRecord>,
    /// Each seat's utterance text, for referral decisions and assignments.
    pub texts: HashMap<String, String>,
    /// A host-forced reason, when a seat had to be completed synthetically.
    pub forced: Option<EpisodeReason>,
    /// The last seat that completed this round, and what it said.
    pub last_completion: Option<(String, String)>,
}

/// The kind a journaled reply carried, back as the utterance the driver
/// replays.
#[must_use]
pub fn utterance_of(episode: &ReplyEpisode, text: String) -> Utterance {
    match episode.kind {
        UtteranceKind::Post => Utterance::Post { message: text },
        UtteranceKind::Broadcast => Utterance::Broadcast { message: text },
        UtteranceKind::Dm => Utterance::Dm {
            to: episode.to.clone(),
            message: text,
        },
        UtteranceKind::CompleteEpisode => Utterance::CompleteEpisode { message: text },
    }
}

/// One seat's settled turn, before it is journaled.
struct Settled {
    agent_id: String,
    utterance: Utterance,
    steps: Vec<crate::ports::types::TurnStep>,
    outputs: Vec<crate::ports::types::ChatOutput>,
    forced: Option<EpisodeReason>,
}

/// Runs one proposed round to a committed utterance per seat.
pub(crate) async fn run_round(
    host: &HiveDispatcher,
    run: &mut EpisodeRun,
    pending: &PendingRound<'_>,
    routing: &crate::hive::routing::EffectiveRouting,
) -> Result<RoundOutcome> {
    let revision = run.revision();
    let seats: Vec<String> = pending
        .agents()
        .iter()
        .map(|seat| seat.hive_agent_id.to_string())
        .collect();
    let round_started = host
        .events
        .append(
            &host.record.id,
            CompanyEvent::RoundStarted {
                chat_id: run.desk.desk_id.clone(),
                episode_id: run.episode_id.clone(),
                revision,
                agent_ids: seats.clone(),
            },
        )
        .await?;
    let members = run.desk.members();
    let allowed: &[&str] = if members.len() >= 2 {
        prompt::DESK_KINDS
    } else {
        prompt::SOLO_KINDS
    };
    let log = EventLogSessionLog::new(
        Arc::clone(&host.events),
        host.record.id.clone(),
        run.desk.desk_id.clone(),
        run.desk.desk_name.clone(),
    );
    let conversation = log.conversation(Some(Sequence(run.thread_root.value())));

    let mut settled: Vec<Settled> = Vec::new();
    let mut remaining: Vec<(String, u32)> = seats.iter().map(|id| (id.clone(), 1)).collect();
    while !remaining.is_empty() {
        let mut turns = Vec::new();
        for (agent_id, attempt) in &remaining {
            let viewer = Viewer::Agent {
                id: agent_id.clone(),
            };
            let (delta, next_state) = prompt::delta_for(
                &log,
                &conversation,
                &viewer,
                run.sharing.get(agent_id),
                Sequence(round_started.value()),
            )
            .await
            .map_err(|error| crate::error::OpenCompanyError::Harness(error.to_string()))?;
            run.sharing.insert(agent_id.clone(), next_state);
            let assignment = run
                .assignments
                .get(agent_id)
                .cloned()
                .unwrap_or_else(SeatAssignment::carry_on);
            let note = (*attempt > 1).then(|| prompt::retry_note(*attempt));
            let message = SeatPrompt {
                desk_id: &run.desk.desk_id,
                desk_name: &run.desk.desk_name,
                episode_id: &run.episode_id,
                revision,
                agent_id,
                assignment: &assignment.text,
                delta: &delta,
                allowed,
                retry_note: note.as_deref(),
            }
            .render();
            let turn_id = uuid::Uuid::new_v4().simple().to_string();
            host.events
                .append(
                    &host.record.id,
                    CompanyEvent::TurnStarted {
                        turn_id: turn_id.clone(),
                        chat_id: run.desk.desk_id.clone(),
                        parent: Some(run.thread_root),
                        by: None,
                        agent_id: Some(agent_id.clone()),
                        episode_id: Some(run.episode_id.clone()),
                        round_revision: Some(revision),
                    },
                )
                .await?;
            let seat = SeatTurn {
                company: host.record.id.clone(),
                agent_id: agent_id.clone(),
                message,
                chat_id: run.desk.desk_id.clone(),
                thread_root: Some(run.thread_root),
                message_seq: Some(run.thread_root),
                hive: HiveTurn {
                    desk_id: run.desk.desk_id.clone(),
                    episode_id: run.episode_id.clone(),
                    revision,
                    turn_id: turn_id.clone(),
                    members: members.clone(),
                },
                timeout: routing.turn_timeout(),
            };
            let runner = Arc::clone(&host.seats);
            let attempt = *attempt;
            turns.push(async move {
                let handle = tokio::spawn(async move { runner.run_seat(seat).await });
                let outcome = match handle.await {
                    Ok(outcome) => outcome,
                    Err(join) => Err(SeatFailure::Failed(format!("seat task aborted: {join}"))),
                };
                (agent_id.clone(), turn_id, attempt, outcome)
            });
        }
        let outcomes = join_all(turns).await;
        remaining.clear();
        for (agent_id, turn_id, attempt, outcome) in outcomes {
            match fold_seat(&agent_id, attempt, outcome, allowed) {
                Fold::Retry => {
                    settle(host, run, &turn_id, &agent_id, revision, TurnOutcome::NoUtterance).await?;
                    remaining.push((agent_id, attempt + 1));
                }
                Fold::Done(done, outcome) => {
                    settle(host, run, &turn_id, &agent_id, revision, outcome).await?;
                    settled.push(done);
                }
                Fold::Failed(done, failure) => {
                    fail(host, run, &turn_id, &agent_id, revision, &failure).await?;
                    settled.push(done);
                }
            }
        }
    }

    // Append in stable desk order, so a replay commits the same sequence
    // order the driver saw.
    settled.sort_by_key(|done| seats.iter().position(|id| *id == done.agent_id));
    let mut outcome = RoundOutcome::default();
    for done in settled {
        let kind = UtteranceKind::of(&done.utterance);
        let to = match &done.utterance {
            Utterance::Dm { to, .. } => to.clone(),
            _ => Vec::new(),
        };
        let text = done.utterance.message().to_string();
        let seq = host
            .events
            .append(
                &host.record.id,
                CompanyEvent::AgentReply {
                    chat_id: run.desk.desk_id.clone(),
                    agent_id: done.agent_id.clone(),
                    text: text.clone(),
                    steps: done.steps,
                    outputs: done.outputs,
                    task_id: None,
                    parent: Some(run.thread_root),
                    mentions: Vec::new(),
                    mention_depth: 0,
                    audience: to.clone(),
                    episode: Some(ReplyEpisode {
                        id: run.episode_id.clone(),
                        revision,
                        kind,
                        to: to.clone(),
                        routed_by: None,
                    }),
                },
            )
            .await?;
        if matches!(done.utterance, Utterance::CompleteEpisode { .. }) {
            outcome.last_completion = Some((done.agent_id.clone(), text.clone()));
        }
        if let Some(reason) = done.forced {
            outcome.forced = Some(match outcome.forced {
                Some(EpisodeReason::Failed) => EpisodeReason::Failed,
                _ => reason,
            });
        }
        outcome.texts.insert(done.agent_id.clone(), text);
        outcome.records.push(RoundUtteranceRecord {
            agent_id: done.agent_id.clone(),
            sequence: seq.value(),
            kind,
            message_seq: Some(seq.value()),
            to,
        });
        outcome.committed.push(CommittedUtterance {
            author_id: done.agent_id,
            sequence: Sequence(seq.value()),
            utterance: done.utterance,
        });
    }
    Ok(outcome)
}

enum Fold {
    Retry,
    Done(Settled, TurnOutcome),
    Failed(Settled, SeatFailure),
}

/// Folds a seat's turn to exactly one utterance, or asks for another go.
fn fold_seat(
    agent_id: &str,
    attempt: u32,
    outcome: std::result::Result<SeatOutcome, SeatFailure>,
    allowed: &[&str],
) -> Fold {
    match outcome {
        Ok(mut turn) => {
            if !turn.utterances.is_empty() {
                // One action per turn: the MCP server refuses a second
                // speech call, so a longer outbox is a host bug — the first
                // is the one that was recorded.
                let utterance = narrow(turn.utterances.remove(0), allowed);
                return Fold::Done(
                    Settled {
                        agent_id: agent_id.to_string(),
                        utterance,
                        steps: turn.steps,
                        outputs: turn.outputs,
                        forced: None,
                    },
                    TurnOutcome::Committed,
                );
            }
            let salvaged = fence::extract_post(&turn.reply);
            if allowed == prompt::SOLO_KINDS && !salvaged.is_empty() {
                // Nobody else in the room: a bare reply is the answer.
                return Fold::Done(
                    Settled {
                        agent_id: agent_id.to_string(),
                        utterance: Utterance::CompleteEpisode { message: salvaged },
                        steps: turn.steps,
                        outputs: turn.outputs,
                        forced: None,
                    },
                    TurnOutcome::Committed,
                );
            }
            if attempt < MAX_ATTEMPTS {
                return Fold::Retry;
            }
            let message = if salvaged.is_empty() {
                "(no action)".to_string()
            } else {
                salvaged
            };
            Fold::Done(
                Settled {
                    agent_id: agent_id.to_string(),
                    utterance: Utterance::CompleteEpisode { message },
                    steps: turn.steps,
                    outputs: turn.outputs,
                    forced: Some(EpisodeReason::Failed),
                },
                TurnOutcome::NoUtterance,
            )
        }
        Err(failure) => {
            let (message, reason) = match &failure {
                SeatFailure::TimedOut => ("(no action: the turn timed out)", EpisodeReason::Timeout),
                SeatFailure::Failed(_) => ("(no action: the turn failed)", EpisodeReason::Failed),
            };
            Fold::Failed(
                Settled {
                    agent_id: agent_id.to_string(),
                    utterance: Utterance::CompleteEpisode {
                        message: message.to_string(),
                    },
                    steps: Vec::new(),
                    outputs: Vec::new(),
                    forced: Some(reason),
                },
                failure,
            )
        }
    }
}

/// A seat with nobody to broadcast to or dm posts instead; the MCP server
/// already refuses those calls on a desk of one, so this is belt and braces.
fn narrow(utterance: Utterance, allowed: &[&str]) -> Utterance {
    if allowed.contains(&"broadcast") {
        return utterance;
    }
    match utterance {
        Utterance::Broadcast { message } | Utterance::Dm { message, .. } => {
            Utterance::Post { message }
        }
        other => other,
    }
}

async fn settle(
    host: &HiveDispatcher,
    run: &EpisodeRun,
    turn_id: &str,
    agent_id: &str,
    revision: u64,
    outcome: TurnOutcome,
) -> Result<()> {
    host.events
        .append(
            &host.record.id,
            CompanyEvent::TurnSettled {
                turn_id: turn_id.to_string(),
                agent_id: Some(agent_id.to_string()),
                chat_id: Some(run.desk.desk_id.clone()),
                episode_id: Some(run.episode_id.clone()),
                round_revision: Some(revision),
                outcome,
            },
        )
        .await?;
    Ok(())
}

async fn fail(
    host: &HiveDispatcher,
    run: &EpisodeRun,
    turn_id: &str,
    agent_id: &str,
    revision: u64,
    failure: &SeatFailure,
) -> Result<()> {
    let (error, outcome) = match failure {
        SeatFailure::TimedOut => ("the seat turn ran past its timeout".to_string(), TurnOutcome::TimedOut),
        SeatFailure::Failed(error) => (error.clone(), TurnOutcome::Failed),
    };
    tracing::warn!(
        desk = %run.desk.desk_id,
        episode = %run.episode_id,
        agent = agent_id,
        %error,
        "[hive] a seat turn did not answer; completed on its behalf"
    );
    host.events
        .append(
            &host.record.id,
            CompanyEvent::TurnFailed {
                turn_id: turn_id.to_string(),
                error,
                agent_id: Some(agent_id.to_string()),
                chat_id: Some(run.desk.desk_id.clone()),
                episode_id: Some(run.episode_id.clone()),
                round_revision: Some(revision),
                outcome: Some(outcome),
            },
        )
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "round_tests.rs"]
mod tests;
