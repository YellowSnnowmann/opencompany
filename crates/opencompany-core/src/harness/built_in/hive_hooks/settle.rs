//! One coordinator turn, wrapped and settled (OC-2).
//!
//! [`SettleTurn::run`] is the body of `HiveHooks::wrap_turn`: it admits the
//! turn, brackets it on the journal, claims the per-turn queues, runs it inside
//! the agent's [`TurnEnvelope`], and then files everything the turn left — the
//! files it published, the cards it opened or moved, the approvals it asked
//! for — before handing the reply back to the adapter. What it learned about
//! how the turn ended (an iteration cap, a spend halt, a budget or wall-clock
//! pause) is said to the operator as system lines in the turn's chat; what the
//! reply row should carry (steps, outputs, the card) goes to the projector on
//! the [`TurnMetaBoard`](crate::hive::projector::TurnMetaBoard).
//!
//! Re-homed from the retired episode host's `seat_park` / `delivery` /
//! `seat_cards`: a coordinator turn is the old seat turn and the old operator
//! turn at once.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use tinyhivemind_core::embed::{ConversationKind, ConversationRef};
use tinyhivemind_hives::{Destination, TurnDisposition};
use tinyhivemind_openhuman::{HostedTurn, TurnScope};

use super::super::card_budget::{CardBudget, CardRefusal, normalize_title};
use super::super::progress_pump::ProgressPump;
use super::super::turn_envelope::{TurnEnvelope, price_usages, turn_findings};
use super::super::{AttemptOutcome, GRACEFUL_EMPTY_REPLY, steps};
use super::{HiveHooks, HiveSeat};
use crate::harness::built_in::brain::{
    BUDGET_PAUSED_PLACEHOLDER_REPLY, CEILING_PAUSED_PLACEHOLDER_REPLY, budget_pause_notice,
    ceiling_pause_notice, iteration_cap_pause_notice, spend_halt_notice,
};
use crate::harness::built_in::policy::{ApprovalScope, MAX_APPROVAL_REQUESTS_PER_TURN};
use crate::harness::built_in::publish::{self, PublishDestination};
use crate::hive::projector::TurnMeta;
use crate::hive::tools::HiveScope;
use crate::ports::types::{
    ApprovalOrigin, CompanyEvent, HiveTurnRef, OutboundMessage, TurnStep, TurnStepKind,
    TurnStepStatus,
};
use crate::runtime::delegation::{ChatTarget, DelegationRunner};
use crate::runtime::journal::{ApprovalConversation, TaskLink};

/// The cards one episode's agents may open between them, and the rule that a
/// title is opened once. An episode is several agents working one request; a
/// room that opened a card per agent per turn filled the board with copies.
const EPISODE_CARD_CAP: usize = 3;
/// Past this many tracked episodes the budgets are forgotten wholesale — a
/// settled episode opens no more cards, so nothing is lost but the memory.
const TRACKED_EPISODES: usize = 512;

/// One episode's [`CardBudget`].
#[derive(Default)]
struct EpisodeBudget {
    titles: Mutex<HashSet<String>>,
}

impl CardBudget for EpisodeBudget {
    fn reserve(&self, title: &str) -> Result<(), CardRefusal> {
        let mut titles = self.titles.lock().unwrap_or_else(PoisonError::into_inner);
        let key = normalize_title(title);
        if titles.contains(&key) {
            return Err(CardRefusal::Duplicate);
        }
        if titles.len() >= EPISODE_CARD_CAP {
            return Err(CardRefusal::Full {
                cap: EPISODE_CARD_CAP,
            });
        }
        titles.insert(key);
        Ok(())
    }

    fn release(&self, title: &str) {
        self.titles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&normalize_title(title));
    }
}

/// The card budgets of the episodes this hive is running.
#[derive(Default)]
pub(super) struct EpisodeCards {
    budgets: Mutex<HashMap<String, Arc<EpisodeBudget>>>,
}

impl EpisodeCards {
    /// The budget `episode_id`'s agents share.
    fn budget(&self, episode_id: &str) -> Arc<dyn CardBudget> {
        let mut budgets = self.budgets.lock().unwrap_or_else(PoisonError::into_inner);
        if budgets.len() >= TRACKED_EPISODES && !budgets.contains_key(episode_id) {
            budgets.clear();
        }
        let budget = budgets.entry(episode_id.to_string()).or_default();
        Arc::clone(budget) as Arc<dyn CardBudget>
    }
}

/// One coordinator turn being wrapped.
pub(super) struct SettleTurn<'a> {
    pub(super) hooks: &'a HiveHooks,
    pub(super) scope: &'a TurnScope,
    pub(super) seat: &'a HiveSeat,
    /// The console chat the turn answers in.
    pub(super) chat: &'a str,
}

/// What a wrapped turn hands back: the adapter's result, and how the
/// Coordinator should treat the agent afterwards.
type Settled = (
    tinyhivemind_openhuman::Result<openhuman_embed::TurnOutcome>,
    TurnDisposition,
);

impl SettleTurn<'_> {
    fn company(&self) -> &crate::ports::types::CompanyId {
        &self.hooks.company
    }

    fn episode_id(&self) -> Option<String> {
        self.scope
            .episode
            .as_ref()
            .map(|episode| episode.episode_id.clone())
    }

    fn hive_id(&self) -> Option<String> {
        match &self.scope.destination {
            Destination::Hive(hive) => Some(crate::hive::chat_for_hive(hive)),
            Destination::Agent(_) => None,
        }
    }

    fn turn_ref(&self) -> HiveTurnRef {
        HiveTurnRef {
            hive_id: self.hive_id(),
            episode_id: self.episode_id(),
        }
    }

    fn surface(&self) -> ConversationRef {
        let kind = match &self.scope.destination {
            Destination::Agent(_) => ConversationKind::Direct,
            Destination::Hive(hive) if hive == crate::hive::GENERAL_HIVE_ID => {
                ConversationKind::General
            }
            Destination::Hive(_) => ConversationKind::Desk,
        };
        ConversationRef {
            id: self.chat.to_string(),
            kind,
            thread_root: self.scope.thread.map(tinyhivemind_core::runtime::Sequence),
        }
    }

    async fn journal(&self, event: CompanyEvent) {
        let Some(events) = self.seat.deps.events.as_ref() else {
            return;
        };
        if let Err(error) = events.append(self.company(), event).await {
            tracing::warn!(
                company = %self.company(),
                agent = %self.seat.agent.agent_id,
                %error,
                "[hive] a coordinator turn's journal row could not be appended"
            );
        }
    }

    /// Says `text` in the turn's chat as `author`, outside the reply.
    async fn say(&self, author: Option<String>, text: String, task_id: Option<String>) {
        self.journal(CompanyEvent::AgentReply {
            chat_id: self.chat.to_string(),
            agent_id: author.unwrap_or_else(|| crate::ports::SYSTEM_AUTHOR.to_string()),
            text,
            steps: Vec::new(),
            outputs: Vec::new(),
            task_id,
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
            audience: Vec::new(),
            hive: None,
        })
        .await;
    }

    async fn bubble(&self, bubble: OutboundMessage) {
        self.journal(CompanyEvent::AgentReply {
            chat_id: self.chat.to_string(),
            agent_id: bubble
                .agent
                .unwrap_or_else(|| crate::ports::SYSTEM_AUTHOR.to_string()),
            text: bubble.text,
            steps: bubble.steps,
            outputs: bubble.outputs,
            task_id: bubble.task_id,
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
            audience: Vec::new(),
            hive: None,
        })
        .await;
    }

    /// Wraps and settles the turn. See the module docs.
    pub(super) async fn run(self, turn: HostedTurn<'_>, pump: Option<ProgressPump>) -> Settled {
        let agent = &self.seat.agent;
        let deps = &self.seat.deps;
        let manifest = agent.agent_id.clone();
        let pool = self.seat.pool.upgrade();
        let _admission = match &pool {
            Some(pool) => match pool.admit(self.company(), agent, deps).await {
                Ok(admission) => Some(admission),
                Err(refusal) => {
                    if let Some(pump) = pump {
                        let _ = pump.finish().await;
                    }
                    self.say(None, refusal.reply.clone(), None).await;
                    return (
                        Err(tinyhivemind_openhuman::Error::Harness(anyhow::anyhow!(
                            "the turn was refused before it ran: {}",
                            refusal.reply
                        ))),
                        TurnDisposition::Completed,
                    );
                }
            },
            None => None,
        };
        let turn_id = uuid::Uuid::new_v4().simple().to_string();
        self.journal(CompanyEvent::TurnStarted {
            turn_id: turn_id.clone(),
            chat_id: self.chat.to_string(),
            parent: None,
            by: None,
            agent_id: Some(manifest.clone()),
            hive: Some(self.turn_ref()),
        })
        .await;

        let episode_id = self.episode_id();
        let key = crate::runtime::hive_resume::turn_key(&manifest, episode_id.as_deref());
        let approvals = deps
            .approval_requests
            .claim(ApprovalScope::Seat(key.clone()));
        let destination = if deps.tasks.is_some() && deps.artifacts.is_some() {
            PublishDestination::Conversation
        } else {
            PublishDestination::Unclaimed
        };
        let publish = deps.pending_publishes.claim(destination);
        let outputs = deps.pending_publishes.output_collector().claim();
        let delegations = deps.delegations.claim_hive_turn(key.clone());
        let hooked = delegations.scoped(
            approvals.scoped(
                deps.approval_requests
                    .turn_scoped(publish.scoped(outputs.scoped(turn))),
            ),
        );
        let envelope = TurnEnvelope::new(agent, None);
        let hive = HiveScope {
            hive_id: self.hive_id(),
            episode_id: episode_id.clone(),
            thread: self.scope.thread,
        };
        let started = std::time::Instant::now();
        let result = match episode_id.as_deref() {
            Some(episode) => {
                let budget = self.hooks.cards.budget(episode);
                envelope
                    .run(
                        self.surface(),
                        Some(hive),
                        Box::pin(super::super::card_budget::scoped(budget, hooked)),
                    )
                    .await
            }
            None => {
                envelope
                    .run(self.surface(), Some(hive), Box::pin(hooked))
                    .await
            }
        };
        let elapsed = started.elapsed();
        let events = match pump {
            Some(pump) => pump.finish().await,
            None => Vec::new(),
        };
        let mut usages = vec![agent.tapped_usage()];
        price_usages(&manifest, &mut usages, &events);
        if let Err(error) = super::super::meter_turn_costs(
            &usages,
            &manifest,
            self.company(),
            deps,
            agent.chat_model().as_ref(),
            None,
        )
        .await
        {
            tracing::warn!(
                company = %self.company(),
                agent = %manifest,
                %error,
                "[hive] a coordinator turn's spend could not be metered"
            );
        }

        // The reply as the operator should read it: the provider's own words
        // restored to a failure, an empty completion said plainly, a pause
        // reduced to a placeholder whose explanation is the notice below.
        let mut budget_summary = None;
        let mut ceiling = None;
        let result = result.map(|mut outcome| {
            let reply = std::mem::take(&mut outcome.reply);
            outcome.reply = match agent.classify_turn(agent.unmask(Ok(reply)), elapsed) {
                AttemptOutcome::Reply(reply) => reply,
                AttemptOutcome::Empty => {
                    crate::harness::mcp_probe::scrub(GRACEFUL_EMPTY_REPLY, &[])
                }
                AttemptOutcome::BudgetPaused { summary } => {
                    budget_summary = Some(crate::harness::mcp_probe::redact(&summary, &[]));
                    BUDGET_PAUSED_PLACEHOLDER_REPLY.to_string()
                }
                AttemptOutcome::CeilingPaused { summary, elapsed } => {
                    ceiling = Some((crate::harness::mcp_probe::redact(&summary, &[]), elapsed));
                    CEILING_PAUSED_PLACEHOLDER_REPLY.to_string()
                }
                AttemptOutcome::Hard(error) => crate::harness::mcp_probe::scrub(
                    &format!("I could not finish this: {error}"),
                    &[],
                ),
            };
            outcome
        });
        let findings = turn_findings(
            &manifest,
            &events,
            envelope.halt(&usages),
            budget_summary,
            ceiling,
        );

        // Files the turn published, filed on a card for this conversation.
        let mut notices: Vec<String> = Vec::new();
        let mut card: Option<String> = None;
        let published = publish.drain();
        if !published.is_empty() && publish.is_claimed() {
            let count = published.len();
            let filing = publish::filing::PublishFiling {
                company: self.company(),
                deps,
            };
            match filing
                .record_conversation_publishes(
                    &manifest,
                    ChatTarget::channel(Some(self.chat)),
                    published,
                )
                .await
            {
                Ok(card_id) => card = Some(card_id),
                Err(error) => {
                    tracing::error!(
                        company = %self.company(),
                        agent = %manifest,
                        staged = count,
                        %error,
                        "[publish] a coordinator turn published files but they could not be recorded"
                    );
                    notices.push(publish::recording_failed_notice(count).trim().to_string());
                }
            }
        }
        drop(publish);

        // Cards the turn opened, assigned or reviewed.
        let mut bubbles: Vec<OutboundMessage> = Vec::new();
        if let Some(pool) = &pool
            && let Ok(Some(record)) = deps.store.load(self.company()).await
        {
            let run_turn =
                crate::harness::run_turn::HarnessRunTurn::new(Arc::clone(pool), Arc::clone(deps));
            let runner = DelegationRunner::new(
                &run_turn,
                &record,
                deps.tasks.as_ref(),
                self.company(),
                &deps.delegations,
                crate::harness::orchestrator::MAX_DELEGATIONS_PER_TURN,
            )
            .with_approvals(&deps.approval_requests)
            .with_workflow_refs(&deps.workflow_refs);
            match delegations
                .scoped(runner.drain_and_execute(Some(self.chat)))
                .await
            {
                Ok(drained) => {
                    card = card.or(drained.spawned_task);
                    bubbles = drained.bubbles;
                    for refused in drained.refused_cards {
                        notices.push(format!(
                            "(tried to {} card {:?}, but {})",
                            refused.tool, refused.card, refused.reason
                        ));
                    }
                }
                Err(error) => tracing::warn!(
                    company = %self.company(),
                    agent = %manifest,
                    %error,
                    "[hive] a coordinator turn's card writes could not be executed"
                ),
            }
        }
        drop(delegations);

        // Approvals the turn asked for: parked under its turn key, so the
        // decision releases this agent rather than starting a cycle.
        let drained = approvals.drain(MAX_APPROVAL_REQUESTS_PER_TURN);
        notices.extend(drained.overflow_notice());
        let mut parked = 0usize;
        for request in drained.requests {
            let Some(parker) = deps.approval_parker.as_ref() else {
                notices.push(format!(
                    "Your `{}` request could not be put in front of the operator (no approval \
                     queue is wired here), so nobody was asked.",
                    request.tool
                ));
                continue;
            };
            let site = crate::runtime::approval_park::ParkSite {
                task: TaskLink::Unlinked,
                conversation: ApprovalConversation {
                    thread: Some(self.chat.to_string()),
                    parent: None,
                },
                turn: Some(key.clone()),
                origin: Some(ApprovalOrigin::Hive {
                    agent_id: manifest.clone(),
                    episode_id: episode_id.clone(),
                }),
            };
            match parker.park(self.company(), request.effect, site).await {
                Ok(approval_id) => {
                    parked += 1;
                    tracing::info!(
                        company = %self.company(),
                        agent = %manifest,
                        %approval_id,
                        tool = %request.tool,
                        "[hive] a coordinator turn parked for operator approval"
                    );
                }
                Err(error) => {
                    tracing::error!(
                        company = %self.company(),
                        agent = %manifest,
                        tool = %request.tool,
                        %error,
                        "[hive] a coordinator turn's approval request could not be parked"
                    );
                    notices.push(format!(
                        "The `{}` request could not be saved, so no decision is pending for that \
                         work and it was not run. Ask {manifest} to request approval again.",
                        request.tool
                    ));
                }
            }
        }
        drop(approvals);

        let mut steps = steps::fold_steps(events);
        self.surface_mcp_failures(&mut steps).await;
        for bubble in bubbles {
            self.bubble(bubble).await;
        }
        if findings.hit_iteration_cap {
            notices.push(iteration_cap_pause_notice(&manifest));
        }
        if let Some(halt) = &findings.halted_for_spend {
            notices.push(spend_halt_notice(halt));
        }
        if let Some(pause) = &findings.budget_paused {
            notices.push(budget_pause_notice(pause));
        }
        if let Some(pause) = &findings.ceiling_paused {
            notices.push(ceiling_pause_notice(pause));
        }
        for notice in notices {
            self.say(None, notice, None).await;
        }
        self.hooks.meta.push(
            &self.scope.agent_id,
            TurnMeta {
                steps,
                outputs: outputs.drain(),
                task_id: card,
            },
        );
        match &result {
            Ok(_) => {
                self.journal(CompanyEvent::TurnSettled {
                    turn_id,
                    agent_id: Some(manifest.clone()),
                    chat_id: Some(self.chat.to_string()),
                    hive: Some(self.turn_ref()),
                    outcome: crate::ports::types::TurnOutcome::Committed,
                })
                .await;
            }
            Err(error) => {
                let timed_out = matches!(error, tinyhivemind_openhuman::Error::TimedOut);
                self.journal(CompanyEvent::TurnFailed {
                    turn_id,
                    error: error.to_string(),
                    agent_id: Some(manifest.clone()),
                    chat_id: Some(self.chat.to_string()),
                    hive: Some(self.turn_ref()),
                    outcome: timed_out.then_some(crate::ports::types::TurnOutcome::TimedOut),
                })
                .await;
            }
        }
        let disposition = if parked > 0 {
            TurnDisposition::Parked
        } else {
            TurnDisposition::Completed
        };
        (result, disposition)
    }

    /// Drains the MCP failure queue onto the turn's step timeline as error
    /// steps, and journals each as a scrubbed `McpCallFailed`. The queue is
    /// shared, so a failure another turn raised in the same window may land
    /// here; it is attributed to a turn either way rather than lost.
    async fn surface_mcp_failures(&self, steps: &mut Vec<TurnStep>) {
        for failure in self.seat.deps.mcp_failures.drain() {
            steps.push(TurnStep {
                kind: TurnStepKind::Note,
                status: TurnStepStatus::Error,
                label: format!("MCP: {} unavailable", failure.server),
                detail: Some(failure.scrubbed_message.clone()),
                elapsed_ms: None,
                ..TurnStep::default()
            });
            self.journal(CompanyEvent::McpCallFailed {
                task_id: None,
                server: failure.server,
                tool: failure.tool,
                status: failure.status,
                message: failure.scrubbed_message,
            })
            .await;
        }
    }
}

#[cfg(test)]
#[path = "settle_tests.rs"]
mod tests;
