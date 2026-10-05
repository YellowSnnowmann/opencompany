//! The company hive's turn hooks: what this host does around every
//! coordinator turn (OC-2).
//!
//! The Coordinator decides who runs; the TinyHiveMind OpenHuman adapter runs
//! the agent's turn on its own `openhuman_embed::Agent` handle and calls these
//! hooks around it, in order:
//!
//! 1. [`prepare`](TurnHooks::prepare) — finds the turn's [`HiveSeat`] (the
//!    pooled [`CompanyAgent`] behind that coordinator id, and the pool and deps
//!    it was built by) and roots the turn in the agent's workspace;
//! 2. [`progress`](TurnHooks::progress) — starts the progress pump that streams
//!    the turn live to the desk or DM it answers and keeps its steps;
//! 3. [`wrap_turn`](TurnHooks::wrap_turn) — the envelope: admission gates, the
//!    turn bracket on the journal, the per-turn claims (approvals, publishes,
//!    outputs, delegations, the episode's card budget), the agent's turn lock
//!    and tool executor, and afterwards everything the turn left to file
//!    ([`settle`]);
//! 4. [`after_turn`](TurnHooks::after_turn) — `Parked` when the turn put an
//!    approval in front of the operator, so the Coordinator holds the agent
//!    until the decision releases it.
//!
//! One coordinator turn per agent at a time (the Coordinator's own rule), so
//! the per-turn state lives in a slot keyed by coordinator agent id.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock, Weak};

use tinyhivemind_hives::{Destination, TurnDisposition};
use tinyhivemind_openhuman::{HostedTurn, TurnHooks, TurnOptions, TurnProgressSink, TurnScope};

use super::progress_pump::ProgressPump;
use super::{CompanyAgent, HarnessDeps, HarnessPool};
use crate::hive::projector::{HiveRoster, TurnMetaBoard};
use crate::ports::types::CompanyId;

mod settle;

/// One registered agent of the company hive, as the hooks need it.
#[derive(Clone)]
pub struct HiveSeat {
    /// The pooled agent whose handle the Coordinator runs.
    pub agent: Arc<CompanyAgent>,
    /// The pool that built it, for its admission gates and isolated turns.
    pub pool: Weak<HarnessPool>,
    /// The deps it was built with: queues, stores, the approval parker.
    pub deps: Arc<HarnessDeps>,
}

/// Every registered agent of one company hive, by coordinator agent id.
///
/// The hooks look a turn's agent up here synchronously. The map holds the
/// only host-side clones of the agents' `openhuman_embed::Agent` handles
/// besides the pools' own, which is why a roster rebuild removes an entry
/// before `OpenHumanHost::replace_agent` rebuilds the handle: OpenHuman keeps
/// an id reserved while any clone is alive.
#[derive(Default)]
pub struct HiveAgents {
    seats: RwLock<HashMap<String, HiveSeat>>,
}

impl HiveAgents {
    /// Adds or replaces the seat registered under `coordinator_id`.
    pub fn insert(&self, coordinator_id: String, seat: HiveSeat) {
        self.seats
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(coordinator_id, seat);
    }

    /// Drops the seat registered under `coordinator_id`, and with it this
    /// map's clone of the agent handle.
    pub fn remove(&self, coordinator_id: &str) -> Option<HiveSeat> {
        self.seats
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(coordinator_id)
    }

    /// The seat registered under `coordinator_id`.
    #[must_use]
    pub fn get(&self, coordinator_id: &str) -> Option<HiveSeat> {
        self.seats
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(coordinator_id)
            .cloned()
    }

    /// The coordinator ids of every seat.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.seats
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }
}

/// One coordinator turn in progress.
struct TurnSlot {
    seat: HiveSeat,
    /// The console chat the turn answers in.
    chat: String,
    pump: Option<ProgressPump>,
    disposition: Option<TurnDisposition>,
}

/// The [`TurnHooks`] the company hive registers its agents under.
pub struct HiveHooks {
    company: CompanyId,
    agents: Arc<HiveAgents>,
    roster: Arc<HiveRoster>,
    meta: Arc<TurnMetaBoard>,
    slots: std::sync::Mutex<HashMap<String, TurnSlot>>,
    cards: settle::EpisodeCards,
}

impl std::fmt::Debug for HiveHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HiveHooks")
            .field("company", &self.company)
            .finish_non_exhaustive()
    }
}

impl HiveHooks {
    /// The hooks of `company`'s hive over its registered `agents`.
    #[must_use]
    pub fn new(
        company: CompanyId,
        agents: Arc<HiveAgents>,
        roster: Arc<HiveRoster>,
        meta: Arc<TurnMetaBoard>,
    ) -> Self {
        Self {
            company,
            agents,
            roster,
            meta,
            slots: std::sync::Mutex::new(HashMap::new()),
            cards: settle::EpisodeCards::default(),
        }
    }

    fn slots(&self) -> std::sync::MutexGuard<'_, HashMap<String, TurnSlot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The console chat a turn answers in: its hive for a hive turn, else the
    /// agent's own direct line.
    fn chat_of(&self, scope: &TurnScope) -> String {
        match &scope.destination {
            Destination::Hive(hive) => crate::hive::chat_for_hive(hive),
            Destination::Agent(_) => format!(
                "{}{}",
                crate::runtime::assignee::DM_PREFIX,
                self.roster.manifest_id(&self.company, &scope.agent_id)
            ),
        }
    }
}

impl TurnHooks for HiveHooks {
    fn prepare(&self, scope: &TurnScope) -> TurnOptions {
        let Some(seat) = self.agents.get(&scope.agent_id) else {
            tracing::warn!(
                company = %self.company,
                agent = %scope.agent_id,
                "[hive] a coordinator turn for an agent this host has no seat for; running it bare"
            );
            return TurnOptions::default();
        };
        let cwd = seat
            .agent
            .workspace()
            .is_dir()
            .then(|| seat.agent.workspace().to_path_buf());
        let chat = self.chat_of(scope);
        self.slots().insert(
            scope.agent_id.clone(),
            TurnSlot {
                seat,
                chat,
                pump: None,
                disposition: None,
            },
        );
        TurnOptions { cwd }
    }

    fn progress(&self, scope: &TurnScope) -> Option<TurnProgressSink> {
        let mut slots = self.slots();
        let slot = slots.get_mut(&scope.agent_id)?;
        let stream = crate::turn_stream::TurnStreamCtx {
            company: self.company.clone(),
            agent_id: slot.seat.agent.agent_id.clone(),
            route: crate::turn_stream::LiveRoute::Chat {
                chat_id: slot.chat.clone(),
            },
            message_seq: None,
        };
        let pump = ProgressPump::start(slot.seat.agent.step_labels.clone(), Some(stream), None);
        let sender = pump.sender();
        slot.pump = Some(pump);
        Some(sender)
    }

    fn wrap_turn<'a>(&'a self, scope: &'a TurnScope, turn: HostedTurn<'a>) -> HostedTurn<'a> {
        Box::pin(self.wrapped(scope, turn))
    }

    fn after_turn(
        &self,
        scope: &TurnScope,
        _usage: Option<&openhuman_core::agent::tinyagents::host::LastTurnUsage>,
    ) -> tinyhivemind_openhuman::Result<TurnDisposition> {
        // Metered once, in `wrap_turn`, from the bridge tap the pool meters
        // from too — so a turn is never billed twice and the adapter's usage
        // is not read.
        let disposition = self
            .slots()
            .remove(&scope.agent_id)
            .and_then(|slot| slot.disposition)
            .unwrap_or(TurnDisposition::Completed);
        Ok(disposition)
    }
}

impl HiveHooks {
    // The error type is the adapter's own (`TurnHooks::wrap_turn` returns it),
    // so its size is TinyHiveMind's to change, not this crate's.
    #[allow(clippy::result_large_err)]
    async fn wrapped<'a>(
        &'a self,
        scope: &'a TurnScope,
        turn: HostedTurn<'a>,
    ) -> tinyhivemind_openhuman::Result<openhuman_embed::TurnOutcome> {
        let taken = {
            let mut slots = self.slots();
            slots
                .get_mut(&scope.agent_id)
                .map(|slot| (slot.seat.clone(), slot.chat.clone(), slot.pump.take()))
        };
        let Some((seat, chat, pump)) = taken else {
            return turn.await;
        };
        let settle = settle::SettleTurn {
            hooks: self,
            scope,
            seat: &seat,
            chat: &chat,
        };
        let (result, disposition) = settle.run(turn, pump).await;
        if let Some(slot) = self.slots().get_mut(&scope.agent_id) {
            slot.disposition = Some(disposition);
        }
        result
    }
}

#[cfg(test)]
#[path = "hive_hooks_tests.rs"]
mod tests;
