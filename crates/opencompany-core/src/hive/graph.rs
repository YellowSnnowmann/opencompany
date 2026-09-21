//! One `OpenHumanHive` per desk, bound over the company's live agents.
//!
//! A hive is tinyhivemind's immutable view of one desk: the desk record, one
//! `RouteCandidate` per seat for the router, and one `AgentBinding` per seat
//! onto an already-built `openhuman_embed::Agent`. A shared agent — the CEO
//! on both the engineering and the content desk — is the **same** runtime
//! handle cloned into both hives; what keeps its turns from overlapping is
//! the per-agent `turn_lock` the harness pool holds, not the hive.
//!
//! Built from the same snapshot every other desk reader uses
//! (`delegation_tools::tinyhivemind_desks`), so a console-created desk, an
//! overlay member and an operator's reorder are all in the graph the router
//! sees. Rebuilt whole with the roster: a hive is cheap, and a stale one
//! would route to a seat that no longer sits there.

use std::collections::HashMap;
use std::sync::Arc;

use tinyhivemind_embed::RouteCandidate;
use tinyhivemind_openhuman::{AgentBinding, HiveGraph, OpenHumanHive};

use crate::ports::types::CompanyRecord;

/// One desk's hive, with the identity the driver keys on.
#[derive(Debug)]
pub struct DeskHive {
    /// The desk id.
    pub desk_id: String,
    /// The desk's display name, for the session log and the prompt.
    pub desk_name: String,
    /// The validated graph and bindings.
    pub hive: OpenHumanHive,
    /// The candidate snapshot version a Jev evaluation must echo.
    pub roster_version: u64,
}

impl DeskHive {
    /// The canonical members, in desk order.
    pub fn members(&self) -> Vec<String> {
        self.hive.members().map(str::to_string).collect()
    }

    /// The desk lead — the first member — the deterministic fallback every
    /// route on this desk has.
    #[must_use]
    pub fn lead(&self) -> Option<String> {
        self.hive.members().next().map(str::to_string)
    }
}

/// Why a desk got no hive.
#[derive(Debug, thiserror::Error)]
pub enum HiveBuildError {
    /// tinyhivemind refused the graph.
    #[error("desk `{desk_id}`: {source}")]
    Invalid {
        /// The desk.
        desk_id: String,
        /// The refusal.
        #[source]
        source: tinyhivemind_openhuman::Error,
    },
}

/// Builds one hive per desk of two or more bound seats.
///
/// `bind` resolves a manifest agent id to its runtime handle; a member with
/// no handle (an overlay teammate the harness did not build, a retired one)
/// is left out of the graph rather than failing the desk, and a desk left
/// with fewer than two bound seats gets no hive at all — it answers through
/// one seat, as a desk of one always has. A desk tinyhivemind refuses is
/// reported and skipped, so one malformed desk cannot take the company's
/// other rooms down with it.
pub fn desk_hives(
    record: &CompanyRecord,
    roster_version: u64,
    bind: &dyn Fn(&str) -> Option<openhuman_embed::Agent>,
) -> (HashMap<String, Arc<DeskHive>>, Vec<HiveBuildError>) {
    let snapshots = crate::runtime::delegation_tools::tinyhivemind_desks(record);
    let desks = snapshots.set();
    let agents = record.effective_agents();
    let mut hives = HashMap::new();
    let mut errors = Vec::new();
    for desk in desks.iter() {
        if crate::server::chat_history::is_general_chat(Some(&desk.id)) {
            continue;
        }
        let Ok(members) = desks.members(&desk.id) else {
            continue;
        };
        let mut bindings = Vec::new();
        let mut candidates = Vec::new();
        let mut bound_members = Vec::new();
        for member in members {
            if !record.is_roster_agent(member) {
                continue;
            }
            let Some(agent) = bind(member) else {
                continue;
            };
            let profile = agents.iter().find(|agent| agent.id == member);
            candidates.push(RouteCandidate {
                id: member.to_string(),
                label: profile
                    .and_then(|agent| agent.name.clone())
                    .unwrap_or_else(|| member.to_string()),
                role: profile.map(|agent| agent.role.clone()),
                description: profile.and_then(|agent| agent.description.clone()),
                capabilities: Vec::new(),
                learned_topics: Vec::new(),
                available: true,
            });
            bindings.push(AgentBinding::new(member, agent));
            bound_members.push(member.to_string());
        }
        if bound_members.len() < 2 {
            continue;
        }
        let graph = HiveGraph::new(
            tinyhivemind::desk::Desk {
                id: desk.id.clone(),
                name: desk.name.clone(),
                description: desk.description.clone(),
                members: bound_members,
                responder_mode: desk.responder_mode.clone(),
            },
            candidates,
        );
        match OpenHumanHive::new(graph, bindings) {
            Ok(hive) => {
                hives.insert(
                    desk.id.clone(),
                    Arc::new(DeskHive {
                        desk_id: desk.id.clone(),
                        desk_name: desk.name.clone(),
                        hive,
                        roster_version,
                    }),
                );
            }
            Err(source) => errors.push(HiveBuildError::Invalid {
                desk_id: desk.id.clone(),
                source,
            }),
        }
    }
    (hives, errors)
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
