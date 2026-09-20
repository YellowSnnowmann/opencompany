//! The episode host: one desk message in, one completion-driven episode out.
//!
//! `tinyhivemind_openhuman::CompletionDriver` proposes rounds and folds
//! committed utterances; it never runs a turn, appends a row, or persists
//! anything. This module is the host around it: it routes the opening message
//! (Jev when a router is configured, the desk lead otherwise), opens or
//! resumes the episode on the journal, runs each proposed round through a
//! [`SeatRunner`] (concurrently — one `tokio` task per seat, joined), appends
//! every utterance as an `AgentReply` whose journal sequence **is** the
//! committed sequence, folds the round, journals every step the console
//! draws, checkpoints the driver state, and carries a referral out to another
//! desk and its answer back.
//!
//! The seat runner is a trait so the whole loop is testable without a model:
//! `driver_tests.rs` scripts one. The production runner lives beside the
//! harness pool, where the per-agent turn lock and the MCP outbox are.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tinyhivemind::speech::Utterance;
use tinyhivemind::{Sequence, SharingState};
use tinyhivemind_embed::{Router, RoutingPlan};
use tinyhivemind_hive::{CompletionEpisodeState, CompletionStep, apply_assignment, completion_status};
use tinyhivemind_openhuman::{BroadcastRouting, CompletionDriver, DriverState, HostAction};

use crate::error::{OpenCompanyError, Result};
use crate::hive::episode_store::{self, PersistedEpisode};
use crate::hive::graph::DeskHive;
use crate::hive::referral::{DeskReferral, ReturnAddress};
use crate::hive::routing::{EffectiveRouting, RoutingPlanDto, router_of};
use crate::hive::round::{self, RoundOutcome, SeatAssignment};
use crate::hive::tools::HiveTurn;
use crate::ports::events::EventLog;
use crate::ports::types::{
    ChatOutput, CompanyEvent, CompanyId, CompanyRecord, EpisodeReason, EventSeq, Mention,
    TurnStep,
};

/// One seat turn as the driver asks for it.
#[derive(Clone, Debug)]
pub struct SeatTurn {
    /// The company.
    pub company: CompanyId,
    /// The seat.
    pub agent_id: String,
    /// The rendered prompt (`hive::prompt`), sentinel first.
    pub message: String,
    /// The desk the turn answers on.
    pub chat_id: String,
    /// The thread root the episode runs in.
    pub thread_root: Option<EventSeq>,
    /// The message the turn answers, for the live turn stream.
    pub message_seq: Option<EventSeq>,
    /// The episode coordinates the MCP server attributes the seat's speech to.
    pub hive: HiveTurn,
    /// How long the turn may run once it holds its lock.
    pub timeout: Duration,
}

/// What one seat turn produced.
#[derive(Clone, Debug, Default)]
pub struct SeatOutcome {
    /// The reply text, used only to salvage a turn that called no tool.
    pub reply: String,
    /// The speech the seat made through the MCP server — at most one.
    pub utterances: Vec<Utterance>,
    /// The scrubbed step timeline, carried onto the reply row.
    pub steps: Vec<TurnStep>,
    /// Outputs the turn produced, carried onto the reply row.
    pub outputs: Vec<ChatOutput>,
}

/// Why a seat turn produced nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeatFailure {
    /// The turn ran past its timeout, counted from lock acquisition.
    TimedOut,
    /// The turn errored.
    Failed(String),
}

/// Runs one seat turn. The production implementation binds to the harness
/// pool; tests script one.
#[async_trait]
pub trait SeatRunner: Send + Sync {
    /// Runs the turn and hands back what the seat said.
    async fn run_seat(&self, seat: SeatTurn) -> std::result::Result<SeatOutcome, SeatFailure>;
}

/// The message that opens, joins or reopens an episode.
#[derive(Clone, Debug)]
pub struct Trigger {
    /// The row's sequence — the episode watermark on an opening.
    pub seq: EventSeq,
    /// The text, verbatim.
    pub text: String,
    /// The thread the message was in, when in one.
    pub parent: Option<EventSeq>,
    /// The mentions the host resolved on the message.
    pub mentions: Vec<Mention>,
    /// The referral hop the episode runs at; zero for an operator's message.
    pub hop: u32,
    /// Where a referral's answer goes home to.
    pub origin: Option<ReturnAddress>,
    /// The asking desk's name when this is a referred question, for the
    /// assignment text.
    pub referred_from: Option<String>,
}

/// How an episode ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpisodeReport {
    /// The episode.
    pub episode_id: String,
    /// The desk.
    pub desk_id: String,
    /// Rounds run in this drive.
    pub rounds: u32,
    /// Why it closed.
    pub reason: EpisodeReason,
    /// The seat whose completion closed it.
    pub completed_by: Option<String>,
    /// The text of the closing utterance, when a seat completed.
    pub summary: Option<String>,
}

/// Everything one company's episodes run over.
pub struct HiveDispatcher {
    /// The company snapshot the roster, desks and policy are read from.
    pub record: Arc<CompanyRecord>,
    /// The journal.
    pub events: Arc<dyn EventLog>,
    /// One hive per desk of two or more bound seats.
    pub hives: HashMap<String, Arc<DeskHive>>,
    /// The semantic router, when a TinyHumans key resolved one.
    pub router: Option<Arc<dyn Router>>,
    /// Runs the seat turns.
    pub seats: Arc<dyn SeatRunner>,
}

/// The state of one episode while it is being driven.
pub(crate) struct EpisodeRun {
    pub(crate) episode_id: String,
    pub(crate) desk: Arc<DeskHive>,
    pub(crate) thread_root: EventSeq,
    pub(crate) state: DriverState,
    pub(crate) sharing: BTreeMap<String, SharingState>,
    pub(crate) assignments: HashMap<String, SeatAssignment>,
    pub(crate) hop: u32,
    pub(crate) origin: Option<ReturnAddress>,
    pub(crate) rounds: u32,
    pub(crate) forced: Option<EpisodeReason>,
    pub(crate) last_completion: Option<(String, String)>,
    /// Revisions committed before the last reassignment restarted the
    /// driver state, so journaled round numbers keep climbing.
    pub(crate) revision_base: u64,
}

impl HiveDispatcher {
    /// The hive for a desk, when the desk runs one.
    #[must_use]
    pub fn hive(&self, desk_id: &str) -> Option<Arc<DeskHive>> {
        self.hives.get(desk_id).cloned()
    }

    /// Whether a desk message opens an episode here: the desk has a hive of
    /// two or more seats.
    #[must_use]
    pub fn runs_episodes(&self, desk_id: &str) -> bool {
        self.hives.contains_key(desk_id)
    }

    /// Drives a desk message to the end of its episode: opens or joins the
    /// episode on `(desk, thread)`, runs rounds until every assigned seat
    /// completed or the host closes it, and carries any answer home.
    pub async fn run_desk_message(&self, desk_id: &str, trigger: Trigger) -> Result<EpisodeReport> {
        let desk = self.hive(desk_id).ok_or_else(|| {
            OpenCompanyError::InvalidRequest(format!("desk `{desk_id}` runs no hive"))
        })?;
        let thread_root = trigger.parent.unwrap_or(trigger.seq);
        let mut run = match episode_store::open_episode_for(
            self.events.as_ref(),
            &self.record.id,
            desk_id,
            Some(thread_root),
        )
        .await?
        {
            Some(open) => self.resume(&desk, open.episode_id, thread_root, &trigger).await?,
            None => self.open(&desk, thread_root, &trigger).await?,
        };
        let report = self.drive(&mut run).await?;
        if let Some(origin) = run.origin.clone() {
            self.deliver_answer(&run, &origin, &report).await?;
        }
        Ok(report)
    }

    /// Opens a new episode: routes the message, journals the opening, starts
    /// the driver.
    async fn open(&self, desk: &Arc<DeskHive>, thread_root: EventSeq, trigger: &Trigger) -> Result<EpisodeRun> {
        let routing = crate::hive::routing::desk_routing(&self.record, &desk.desk_id);
        let policy = routing.policy();
        let explicit = explicit_seat(desk, &trigger.mentions);
        let lead = desk.lead().ok_or_else(|| {
            OpenCompanyError::Harness(format!("desk `{}` has no seats", desk.desk_id))
        })?;
        let request = desk.hive.desk_request(
            trigger.text.clone(),
            Vec::new(),
            Some(Sequence(thread_root.value())),
            desk.roster_version,
            policy,
        );
        let plan = desk
            .hive
            .route_desk(
                self.router.as_deref(),
                None,
                &request,
                explicit.as_deref(),
                &lead,
            )
            .await
            .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
        let plan_dto = RoutingPlanDto::from(&plan);
        let mut participants = plan_dto.agent_ids();
        if participants.is_empty() {
            // A clarification is a routing answer the room cannot act on:
            // the lead answers, and asks if it must.
            participants.push(lead.clone());
        }
        let episode_id = uuid::Uuid::new_v4().simple().to_string();
        self.events
            .append(
                &self.record.id,
                CompanyEvent::EpisodeOpened {
                    chat_id: desk.desk_id.clone(),
                    episode_id: episode_id.clone(),
                    opened_by_seq: trigger.seq.value(),
                    parent: Some(thread_root),
                    participants: participants.clone(),
                    plan: plan_dto,
                    hop: trigger.hop,
                },
            )
            .await?;
        tracing::info!(
            desk = %desk.desk_id,
            episode = %episode_id,
            router = ?router_of(&plan),
            participants = ?participants,
            "[hive] episode opened"
        );
        let episode = CompletionEpisodeState::opened(
            tinyhivemind::Conversation {
                desk_id: desk.desk_id.clone(),
                desk_name: desk.desk_name.clone(),
                thread_root: Some(Sequence(thread_root.value())),
            },
            // Exclusive: every committed row is newer than the trigger.
            Sequence(trigger.seq.value()),
            participants.iter().map(String::as_str),
        )
        .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
        let state = CompletionDriver::new(&desk.hive, routing.round_width)
            .and_then(|driver| driver.start(episode))
            .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
        let assignment = SeatAssignment::trigger(trigger);
        Ok(EpisodeRun {
            episode_id,
            desk: Arc::clone(desk),
            thread_root,
            state,
            sharing: BTreeMap::new(),
            assignments: participants
                .iter()
                .map(|id| (id.clone(), assignment.clone()))
                .collect(),
            hop: trigger.hop,
            origin: trigger.origin.clone(),
            rounds: 0,
            forced: None,
            last_completion: None,
            revision_base: 0,
        })
    }

    /// Joins a message to the episode already open on its thread: reloads
    /// the checkpoint, replays rows committed since, and assigns the message
    /// to the seat it named or the desk lead.
    async fn resume(
        &self,
        desk: &Arc<DeskHive>,
        episode_id: String,
        thread_root: EventSeq,
        trigger: &Trigger,
    ) -> Result<EpisodeRun> {
        let routing = crate::hive::routing::desk_routing(&self.record, &desk.desk_id);
        let persisted = episode_store::latest_state(self.events.as_ref(), &self.record.id, &episode_id)
            .await?
            .ok_or_else(|| {
                OpenCompanyError::Harness(format!(
                    "episode `{episode_id}` is open but has no checkpoint"
                ))
            })?;
        let mut run = self.resume_from(desk, persisted, &routing).await?;
        let seat = explicit_seat(desk, &trigger.mentions)
            .or_else(|| {
                run.state
                    .episode()
                    .participants
                    .first()
                    .map(|participant| participant.agent_id.clone())
            })
            .or_else(|| desk.lead())
            .ok_or_else(|| OpenCompanyError::Harness("no seat to assign".into()))?;
        if !run
            .state
            .episode()
            .participants
            .iter()
            .any(|participant| participant.agent_id == seat)
        {
            return Err(OpenCompanyError::Harness(format!(
                "`{seat}` is not a participant of episode `{episode_id}`"
            )));
        }
        run.reassign(&seat, trigger.seq, &routing)?;
        run.assignments
            .insert(seat, SeatAssignment::trigger(trigger));
        let _ = thread_root;
        Ok(run)
    }

    /// Rebuilds a run from its checkpoint and replays the rows a later
    /// commit journaled before the host stopped.
    pub(crate) async fn resume_from(
        &self,
        desk: &Arc<DeskHive>,
        persisted: PersistedEpisode,
        routing: &EffectiveRouting,
    ) -> Result<EpisodeRun> {
        let state: DriverState = serde_json::from_value(persisted.state.clone())?;
        let driver = CompletionDriver::new(&desk.hive, routing.round_width)
            .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
        let mut state = driver
            .resume(state)
            .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
        let later = episode_store::replies_after(
            self.events.as_ref(),
            &self.record.id,
            &persisted.episode_id,
            persisted.revision,
        )
        .await?;
        for reply in later {
            let utterance = round::utterance_of(&reply.episode, reply.text);
            let event = tinyhivemind_openhuman::CommittedUtterance {
                author_id: reply.agent_id.clone(),
                sequence: Sequence(reply.seq.value()),
                utterance,
            };
            match driver.apply_committed(&state, event, None).await {
                Ok(transition) => state = transition.state,
                // A broadcast needs routing inputs to fold and a stale row is
                // already folded: neither is worth failing a resume over.
                Err(error) => tracing::debug!(
                    episode = %persisted.episode_id,
                    %error,
                    "[hive] a journaled row was not replayed on resume"
                ),
            }
        }
        let state_revision = state.revision();
        Ok(EpisodeRun {
            episode_id: persisted.episode_id,
            desk: Arc::clone(desk),
            thread_root: persisted.thread_root.unwrap_or(EventSeq::new(0)),
            state,
            sharing: persisted.sharing,
            assignments: HashMap::new(),
            hop: persisted.hop,
            origin: persisted.origin,
            rounds: 0,
            forced: None,
            last_completion: None,
            revision_base: persisted.revision.saturating_sub(state_revision),
        })
    }

    /// Runs rounds until the episode completes or the host closes it.
    pub(crate) async fn drive(&self, run: &mut EpisodeRun) -> Result<EpisodeReport> {
        let routing = crate::hive::routing::desk_routing(&self.record, &run.desk.desk_id);
        let driver = CompletionDriver::new(&run.desk.hive, routing.round_width)
            .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
        let policy = routing.policy();
        loop {
            if matches!(
                completion_status(run.state.episode()),
                CompletionStep::Complete { .. }
            ) {
                let reason = run.forced.unwrap_or(EpisodeReason::CompleteEpisode);
                return self.complete(run, reason).await;
            }
            if run.rounds >= routing.max_rounds {
                return self.complete(run, EpisodeReason::RoundCap).await;
            }
            let pending = driver
                .pending_round(&run.state)
                .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
            if pending.is_empty() {
                return self.complete(run, EpisodeReason::Failed).await;
            }
            let outcome: RoundOutcome = round::run_round(self, run, &pending, &routing).await?;
            let thread_context: Vec<String> = Vec::new();
            let transition = driver
                .apply_committed_round(
                    &run.state,
                    &pending,
                    outcome.committed.clone(),
                    Some(BroadcastRouting {
                        primary: self.router.as_deref(),
                        reasoning: None,
                        policy: &policy,
                        roster_version: run.desk.roster_version,
                        thread_context: &thread_context,
                    }),
                )
                .await
                .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
            drop(pending);
            let revision_before = run.revision();
            run.state = transition.state;
            run.rounds += 1;
            if let Some(reason) = outcome.forced {
                run.forced = Some(run.forced.map_or(reason, |held| held.min_severity(reason)));
            }
            if let Some(completion) = outcome.last_completion {
                run.last_completion = Some(completion);
            }
            let actions: Vec<serde_json::Value> = transition
                .actions
                .iter()
                .map(|action| match action {
                    HostAction::RunAgents { agent_ids, plan } => serde_json::json!({
                        "kind": "run_agents",
                        "agentIds": agent_ids,
                        "plan": RoutingPlanDto::from(plan),
                    }),
                    HostAction::DeliverDm { route, message } => serde_json::json!({
                        "kind": "deliver_dm",
                        "route": route,
                        "messageChars": message.chars().count(),
                    }),
                })
                .collect();
            self.events
                .append(
                    &self.record.id,
                    CompanyEvent::RoundCommitted {
                        chat_id: run.desk.desk_id.clone(),
                        episode_id: run.episode_id.clone(),
                        revision: revision_before,
                        utterances: outcome.records.clone(),
                        actions,
                    },
                )
                .await?;
            self.checkpoint(run).await?;
            self.journal_actions(run, &transition.actions, &outcome, revision_before)
                .await?;
            self.refer(run, &outcome, &routing).await?;
        }
    }

    /// Journals the typed frames behind the fold's host actions and records
    /// the assignments they made.
    async fn journal_actions(
        &self,
        run: &mut EpisodeRun,
        actions: &[HostAction],
        outcome: &RoundOutcome,
        revision: u64,
    ) -> Result<()> {
        // Actions come back in commit order, one per broadcast or dm, so the
        // n-th action pairs with the n-th routed utterance of the round.
        let mut routed = outcome
            .records
            .iter()
            .filter(|record| {
                matches!(
                    record.kind,
                    crate::ports::types::UtteranceKind::Broadcast
                        | crate::ports::types::UtteranceKind::Dm
                )
            });
        for action in actions {
            let Some(record) = routed.next() else { break };
            let message_seq = record.message_seq.unwrap_or(record.sequence);
            match action {
                HostAction::RunAgents { agent_ids, plan } => {
                    let plan_dto = RoutingPlanDto::from(plan);
                    let router = router_of(plan);
                    let text = outcome
                        .texts
                        .get(&record.agent_id)
                        .cloned()
                        .unwrap_or_default();
                    for id in agent_ids {
                        run.assignments.insert(
                            id.clone(),
                            SeatAssignment::broadcast(&record.agent_id, message_seq, &text),
                        );
                    }
                    self.events
                        .append(
                            &self.record.id,
                            CompanyEvent::BroadcastRouted {
                                chat_id: run.desk.desk_id.clone(),
                                episode_id: run.episode_id.clone(),
                                revision,
                                agent_id: record.agent_id.clone(),
                                message_seq,
                                plan: plan_dto,
                                probabilities: None,
                                router,
                            },
                        )
                        .await?;
                }
                HostAction::DeliverDm { .. } => {
                    self.events
                        .append(
                            &self.record.id,
                            CompanyEvent::DmDelivered {
                                chat_id: run.desk.desk_id.clone(),
                                episode_id: run.episode_id.clone(),
                                from: record.agent_id.clone(),
                                to: record.to.clone(),
                                message_seq,
                            },
                        )
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Writes the driver checkpoint.
    pub(crate) async fn checkpoint(&self, run: &EpisodeRun) -> Result<()> {
        episode_store::save_state(
            self.events.as_ref(),
            &self.record.id,
            &PersistedEpisode {
                episode_id: run.episode_id.clone(),
                desk: run.desk.desk_id.clone(),
                thread_root: Some(run.thread_root),
                revision: run.revision(),
                state: serde_json::to_value(&run.state)?,
                sharing: run.sharing.clone(),
                hop: run.hop,
                origin: run.origin.clone(),
            },
        )
        .await?;
        Ok(())
    }

    /// Closes the episode on the journal.
    async fn complete(&self, run: &EpisodeRun, reason: EpisodeReason) -> Result<EpisodeReport> {
        let (completed_by, summary, summary_seq) = match &run.last_completion {
            Some((agent, text)) => (
                Some(agent.clone()),
                Some(text.clone()),
                run.state
                    .episode()
                    .participants
                    .iter()
                    .find(|participant| &participant.agent_id == agent)
                    .and_then(|participant| participant.completed_at)
                    .map(|sequence| sequence.0),
            ),
            None => (None, None, None),
        };
        self.events
            .append(
                &self.record.id,
                CompanyEvent::EpisodeCompleted {
                    chat_id: run.desk.desk_id.clone(),
                    episode_id: run.episode_id.clone(),
                    revision: run.revision(),
                    completed_by: completed_by.clone(),
                    rounds: run.rounds,
                    reason,
                    summary_seq,
                },
            )
            .await?;
        tracing::info!(
            desk = %run.desk.desk_id,
            episode = %run.episode_id,
            rounds = run.rounds,
            ?reason,
            "[hive] episode completed"
        );
        Ok(EpisodeReport {
            episode_id: run.episode_id.clone(),
            desk_id: run.desk.desk_id.clone(),
            rounds: run.rounds,
            reason,
            completed_by,
            summary,
        })
    }

    /// Decides and dispatches the crossings a round's utterances raise.
    async fn refer(&self, run: &EpisodeRun, outcome: &RoundOutcome, routing: &EffectiveRouting) -> Result<()> {
        let policy = routing.referral_policy();
        if !policy.enabled {
            return Ok(());
        }
        let members = crate::runtime::delegation_tools::tinyhivemind_roster(&self.record);
        let snapshots = crate::runtime::delegation_tools::tinyhivemind_desks(&self.record);
        let desks = snapshots.set();
        let roster = tinyhivemind_core::roster::Roster::new(&members, &[], &[]);
        let queue = crate::hive::referral::JournalReferralQueue::new(self.events.as_ref(), &self.record.id);
        for record in &outcome.records {
            let Some(text) = outcome.texts.get(&record.agent_id) else {
                continue;
            };
            let mentions = tinyhivemind_core::mention::resolve(
                text,
                None,
                &tinyhivemind_core::mention::MentionAuthor::Agent {
                    id: record.agent_id.clone(),
                },
                &roster,
                &desks,
            );
            if mentions.is_empty() {
                continue;
            }
            let input = tinyhivemind::referral::ReferralInput {
                key: tinyhivemind::dispatch::DispatchKey {
                    trigger_sequence: record.sequence,
                },
                conversation: tinyhivemind::dispatch::DispatchConversation {
                    desk_id: run.desk.desk_id.clone(),
                    thread_root: Some(run.thread_root.value()),
                },
                author_id: record.agent_id.clone(),
                content: text.clone(),
                mentions,
                hop: run.hop,
                origin: None,
            };
            if let Err(error) =
                tinyhivemind::referral::dispatch_referral(&queue, policy, &input, &roster, &desks)
                    .await
            {
                tracing::warn!(episode = %run.episode_id, %error, "[hive] referral decision failed");
            }
        }
        for referral in queue.drain() {
            let crossing = DeskReferral::from_referral(
                &referral,
                &self.record,
                &run.episode_id,
                Some(run.thread_root),
                policy.returns,
            );
            self.dispatch_referral(crossing).await?;
        }
        Ok(())
    }

    /// Opens the far desk's episode on a crossing: seeds the question as a
    /// row under [`HIVE_REFERRAL_AUTHOR`](crate::hive::referral::HIVE_REFERRAL_AUTHOR),
    /// journals the forward marker, and drives it as its own task so the
    /// asking desk's rounds are not held on it.
    async fn dispatch_referral(&self, crossing: DeskReferral) -> Result<()> {
        if !self.runs_episodes(&crossing.to_desk) {
            tracing::info!(
                to_desk = %crossing.to_desk,
                "[hive] referral target runs no hive; the question stays on its desk"
            );
            return Ok(());
        }
        let seed = self
            .events
            .append(
                &self.record.id,
                CompanyEvent::AgentReply {
                    chat_id: crossing.to_desk.clone(),
                    agent_id: crate::hive::referral::HIVE_REFERRAL_AUTHOR.to_string(),
                    text: crossing.seed_text(),
                    steps: Vec::new(),
                    outputs: Vec::new(),
                    task_id: None,
                    parent: None,
                    mentions: Vec::new(),
                    mention_depth: 0,
                    audience: Vec::new(),
                    episode: None,
                },
            )
            .await?;
        let forward = self
            .events
            .append(&self.record.id, crossing.forward_event(None))
            .await?;
        let trigger = Trigger {
            seq: seed,
            text: crossing.content.clone(),
            parent: None,
            mentions: Vec::new(),
            hop: crossing.hop,
            origin: crossing.return_address(forward),
            referred_from: Some(crossing.from_desk_name.clone()),
        };
        let dispatcher = self.clone_for_task();
        let to_desk = crossing.to_desk.clone();
        tokio::spawn(async move {
            if let Err(error) = dispatcher.run_desk_message(&to_desk, trigger).await {
                tracing::warn!(desk = %to_desk, %error, "[hive] a referred episode failed");
            }
        });
        Ok(())
    }

    /// Carries a completed referral's answer home and reopens the seat that
    /// asked.
    async fn deliver_answer(&self, run: &EpisodeRun, origin: &ReturnAddress, report: &EpisodeReport) -> Result<()> {
        let answered_by = report
            .completed_by
            .clone()
            .unwrap_or_else(|| run.desk.desk_id.clone());
        let answer = report
            .summary
            .clone()
            .unwrap_or_else(|| format!("(the desk closed without an answer: {:?})", report.reason));
        let text = crate::hive::referral::returned_note(&answered_by, &run.desk.desk_name, &answer);
        let answer_seq = self
            .events
            .append(
                &self.record.id,
                CompanyEvent::AgentReply {
                    chat_id: origin.desk.clone(),
                    agent_id: crate::hive::referral::HIVE_REFERRAL_AUTHOR.to_string(),
                    text,
                    steps: Vec::new(),
                    outputs: Vec::new(),
                    task_id: None,
                    parent: origin.thread_root,
                    mentions: Vec::new(),
                    mention_depth: 0,
                    audience: Vec::new(),
                    episode: None,
                },
            )
            .await?;
        self.events
            .append(
                &self.record.id,
                crate::hive::referral::return_event(
                    &self.record,
                    origin,
                    &run.desk.desk_id,
                    &answered_by,
                    answer_seq,
                    &run.episode_id,
                ),
            )
            .await?;
        let Some(desk) = self.hive(&origin.desk) else {
            return Ok(());
        };
        let routing = crate::hive::routing::desk_routing(&self.record, &desk.desk_id);
        let Some(persisted) =
            episode_store::latest_state(self.events.as_ref(), &self.record.id, &origin.episode_id)
                .await?
        else {
            return Ok(());
        };
        let mut home = self.resume_from(&desk, persisted, &routing).await?;
        if home
            .state
            .episode()
            .participants
            .iter()
            .all(|participant| participant.agent_id != origin.asker)
        {
            return Ok(());
        }
        home.reassign(&origin.asker, answer_seq, &routing)?;
        home.assignments.insert(
            origin.asker.clone(),
            SeatAssignment::answer(&run.desk.desk_name, answer_seq),
        );
        let dispatcher = self.clone_for_task();
        tokio::spawn(async move {
            match dispatcher.drive(&mut home).await {
                Ok(report) => {
                    if let Some(origin) = home.origin.clone()
                        && let Err(error) = dispatcher.deliver_answer(&home, &origin, &report).await
                    {
                        tracing::warn!(%error, "[hive] an answer could not be carried home");
                    }
                }
                Err(error) => tracing::warn!(%error, "[hive] a reopened episode failed"),
            }
        });
        Ok(())
    }

    /// A handle for a spawned task: the same journal, hives, router and
    /// runner.
    fn clone_for_task(&self) -> Arc<Self> {
        Arc::new(Self {
            record: Arc::clone(&self.record),
            events: Arc::clone(&self.events),
            hives: self.hives.clone(),
            router: self.router.clone(),
            seats: Arc::clone(&self.seats),
        })
    }
}

impl EpisodeRun {
    /// The journaled revision: the driver's, plus what earlier driver states
    /// of this episode committed before a reassignment restarted it.
    pub(crate) fn revision(&self) -> u64 {
        self.revision_base + self.state.revision()
    }

    /// Reopens a seat with new work at `at`.
    ///
    /// The driver state is opaque and the library reopens a seat only
    /// through a routed broadcast, so a host-side reassignment — a follow-up
    /// message in the thread, an answer coming home — is a fresh start from
    /// the reopened episode. The receipts it drops are rows older than `at`,
    /// which could never be replayed into it anyway; the revision carries on
    /// through [`revision_base`](Self::revision_base).
    fn reassign(&mut self, seat: &str, at: EventSeq, routing: &EffectiveRouting) -> Result<()> {
        let episode = apply_assignment(self.state.episode(), [seat], Sequence(at.value()))
            .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
        self.revision_base = self.revision();
        self.state = CompletionDriver::new(&self.desk.hive, routing.round_width)
            .and_then(|driver| driver.start(episode))
            .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
        Ok(())
    }
}

impl EpisodeReason {
    /// The more serious of two host-forced reasons, when a round both timed
    /// out a seat and failed another.
    fn min_severity(self, other: Self) -> Self {
        match (self, other) {
            (Self::Failed, _) | (_, Self::Failed) => Self::Failed,
            (Self::Timeout, _) | (_, Self::Timeout) => Self::Timeout,
            (held, _) => held,
        }
    }
}

/// The plan's seats, used by the resume path and the tests.
#[must_use]
pub fn plan_seats(plan: &RoutingPlan) -> Vec<String> {
    RoutingPlanDto::from(plan).agent_ids()
}

/// The first mentioned teammate that sits on this desk, in reading order —
/// the explicit responder a route bypasses the router for.
fn explicit_seat(desk: &DeskHive, mentions: &[Mention]) -> Option<String> {
    let mut ordered: Vec<&Mention> = mentions.iter().collect();
    ordered.sort_by_key(|mention| mention.offset);
    ordered
        .into_iter()
        .filter_map(|mention| mention.target.agent_id())
        .find(|id| desk.hive.binding(id).is_some())
        .map(str::to_string)
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
