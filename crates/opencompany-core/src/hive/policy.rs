//! Who may send to whom inside the company hive.
//!
//! The permanent `hivemind_*` tools let any registered agent message any other
//! and post to any hive it belongs to; the Coordinator enforces hive
//! membership on its own. What it cannot know is OpenCompany's *reach* rule,
//! which the retired `delegate_to_teammate` tool enforced: a teammate may hand
//! work to the people it sits on a desk with, plus the members of any desk its
//! manifest `delegates_to` allowlist names — or anybody, when that list is
//! empty or `"*"`. That rule moves here, behind TinyHiveMind's
//! `SendAuthorizer` seam, so a refused `hivemind_send_agent` reaches the model
//! as the tool's error text and its turn continues.
//!
//! [`ReachPolicy`] is the pure decision over a [`CompanyRecord`] and builds
//! everywhere; the `SendAuthorizer` impl that consults it is gated with the
//! OpenHuman adapter it plugs into. The record is swapped in on every
//! [`ReachPolicy::install`], so a desk or roster edit is in force on the next
//! send without rebuilding the host.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use crate::ports::types::CompanyRecord;
use crate::runtime::delegation_tools::{reach_is_unrestricted, teammate_targets};

/// The live reach rule for one company's hive.
#[derive(Default)]
pub struct ReachPolicy {
    state: RwLock<ReachState>,
}

#[derive(Default)]
struct ReachState {
    record: Option<Arc<CompanyRecord>>,
    /// Coordinator agent id (`{company}--{agent}`) → manifest agent id.
    agents: HashMap<String, String>,
}

impl std::fmt::Debug for ReachPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReachPolicy").finish_non_exhaustive()
    }
}

impl ReachPolicy {
    /// An empty policy: until [`install`](Self::install) runs, every direct
    /// send is refused, because no record says who may reach whom.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Swaps in the company as it is now, with the map from each roster
    /// agent's Coordinator id to its manifest id.
    pub fn install(&self, record: Arc<CompanyRecord>, agents: HashMap<String, String>) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.record = Some(record);
        state.agents = agents;
    }

    /// The manifest id behind a Coordinator agent id, when it is a roster
    /// agent this policy knows.
    #[must_use]
    pub fn manifest_id(&self, coordinator_id: &str) -> Option<String> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .agents
            .get(coordinator_id)
            .cloned()
    }

    /// Whether `actor` may send a direct message to `target`, both by
    /// Coordinator id. `Err` carries the refusal the model reads.
    pub fn may_message_agent(&self, actor: &str, target: &str) -> Result<(), String> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let Some(record) = state.record.as_deref() else {
            return Err("refused: the company's roster is not loaded yet; try again".to_string());
        };
        let caller = state.agents.get(actor).map_or(actor, String::as_str);
        let Some(target_id) = state.agents.get(target) else {
            return Err(format!(
                "refused: there is no teammate \"{target}\" in this company. Call \
                 `hivemind_list_agents` for the ids you can message."
            ));
        };
        decide_direct(record, caller, target_id)
    }
}

/// The reach rule itself, over manifest ids.
///
/// A teammate may message the teammates it shares a desk with, plus the
/// members of every desk its `delegates_to` allowlist names — anybody, when
/// the list is empty or `"*"`. Never itself: a message to oneself would run
/// the turn it is already in.
pub fn decide_direct(record: &CompanyRecord, caller: &str, target: &str) -> Result<(), String> {
    if caller == target {
        return Err(format!(
            "refused: you are \"{caller}\" — messaging yourself would re-enter the turn you are \
             already running. Do it in this turn instead."
        ));
    }
    let allowed = delegates_to(record, caller);
    if reach_is_unrestricted(&allowed) {
        return Ok(());
    }
    let reachable = teammate_targets(record, caller, &allowed);
    if reachable.iter().any(|id| id == target) {
        return Ok(());
    }
    Err(if reachable.is_empty() {
        format!(
            "refused: you may not message \"{target}\", and there is nobody else you can message \
             directly either. Post in a hive you belong to, or do the work yourself."
        )
    } else {
        format!(
            "refused: you may not message \"{target}\": they are not on a desk with you, and no \
             desk you may reach has them on it. The teammates you can message are: {}.",
            reachable.join(", ")
        )
    })
}

/// The caller's manifest `delegates_to` allowlist; an overlay teammate has
/// none, which is unrestricted.
fn delegates_to(record: &CompanyRecord, caller: &str) -> Vec<String> {
    record
        .manifest
        .agents
        .iter()
        .find(|agent| agent.id == caller)
        .map(|agent| agent.delegates_to.clone())
        .unwrap_or_default()
}

#[cfg(feature = "openhuman")]
mod authorizer {
    use tinyhivemind_openhuman::{Error, Result, SendAuthorizer, SendRequest};

    use super::ReachPolicy;

    impl SendAuthorizer for ReachPolicy {
        fn authorize(&self, actor: &str, request: &SendRequest) -> Result<()> {
            match request {
                SendRequest::Agent { agent_id, .. } => self
                    .may_message_agent(actor, agent_id)
                    .map_err(Error::SendDenied),
                // Hive posts, asks and broadcasts are bounded by membership,
                // which the Coordinator enforces itself.
                SendRequest::Hive { .. } | SendRequest::Ask { .. } | SendRequest::Broadcast { .. } => {
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
