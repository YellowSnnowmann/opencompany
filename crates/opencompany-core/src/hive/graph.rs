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

use tinyhivemind_driver::{AgentBinding, BoundHive, HiveGraph};
use tinyhivemind_embed::RouteCandidate;
use tinyhivemind_openhuman::EmbedSeat;

use crate::ports::types::CompanyRecord;

/// One desk's hive, with the identity the driver keys on.
#[derive(Debug)]
pub struct DeskHive {
    /// The desk id.
    pub desk_id: String,
    /// The desk's display name, for the session log and the prompt.
    pub desk_name: String,
    /// The validated graph and bindings.
    pub hive: BoundHive<EmbedSeat>,
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
        source: tinyhivemind_driver::Error,
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
        if crate::ports::general_channel::is_general_spelling(&desk.id) {
            continue;
        }
        let Ok(members) = desks.members(&desk.id) else {
            continue;
        };
        let seats = bind_seats(record, &agents, members.iter().copied(), bind);
        if seats.members.len() < 2 {
            continue;
        }
        let BoundSeats {
            members: bound_members,
            candidates,
            bindings,
        } = seats;
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
        match BoundHive::new(graph, bindings) {
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

/// Whether operator DMs run as episodes. **On unless switched off.**
///
/// A DM is the surface this whole line of work is for: an agent there could
/// not reach its teammates, and a live run had a PM asked for two engineers'
/// input spend fifteen tool calls on it and then escalate to a human. As an
/// episode it has `ask`, the asking turn ends, and the answers arrive in a
/// later brief.
///
/// # What had to land first
///
/// It waited behind a flag for three things, all now on `main`: a seat's
/// approvals reached nobody (#2467, #2471 — a seat now parks and the episode
/// resumes on the decision); a seat could not hand over a deliverable
/// (#2472, #2480); and two episodes on one desk read each other's in-flight
/// rows while a seat was amnesiac about its own operator line (#2483).
///
/// `OPENCOMPANY_DM_EPISODES=0` restores the pooled turn.
#[must_use]
pub fn dm_episodes_enabled(env: &dyn crate::app::config::EnvSource) -> bool {
    env.get("OPENCOMPANY_DM_EPISODES")
        .is_none_or(|value| !matches!(value.trim(), "0" | "false" | "no" | "off"))
}

/// Whether a desk-assigned card's work runs as an episode rather than as one
/// pooled turn on the desk's lead.
///
/// **Default off**, which is the opposite of
/// [`dm_episodes_enabled`](self::dm_episodes_enabled) and deliberate: a convened
/// card's episode is awaited *inside* its dispatch cycle, and that cycle holds the
/// company-wide `serial` lock. So while a room works a card, nothing else in that
/// company runs — no chat turn, no other dispatch, no scheduler tick. For a
/// twelve-round episode that is minutes of a single-threaded company.
///
/// That is a real regression for anyone who turns this on, and it is why the flag
/// exists rather than the behaviour simply shipping. Taking a narrower lock is the
/// fix, and it is deliberately not attempted here: the lock has to be chosen
/// before the cycle starts while whether a card convenes is decided inside it, so
/// the two can disagree — and a narrow lock on a turn that then runs pooled would
/// let a hand-off reassign the card under it. Sizing that properly wants the
/// measurement this flag exists to collect: how long a room actually holds, and
/// what it costs.
///
/// `OPENCOMPANY_CARD_EPISODES=1` turns it on.
#[must_use]
pub fn card_episodes_enabled(env: &dyn crate::app::config::EnvSource) -> bool {
    env.get("OPENCOMPANY_CARD_EPISODES")
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
}

/// The room a **card** convenes, keyed on the card and made of its desk.
///
/// # Why the card id and not the desk id
///
/// So two cards on one desk do not share a conversation. The key is what every
/// row an episode journals carries as its `chat_id`, and a card id matches no
/// desk — which `journal_task_outcome` already relies on when it files a run's
/// reply under the card: `chat_history::owns` routes it nowhere, so the row stays
/// timeline material reachable through `task_id` rather than appearing in a
/// channel. Keying here the same way means an episode's rows land exactly where
/// the card's own already do, with no new channel and no projection.
///
/// It also settles approvals for free: `EpisodeSeatParking` stamps the hive's key
/// as the park's conversation, so a convened card's continuation answers on the
/// card timeline — which is where a dispatched card's narration belongs and
/// deliberately not the origin channel (issue #1092).
///
/// # Only a desk-assigned card convenes one
///
/// `None` for every other assignee, and that is the operator's choice rather than
/// an inference: a card worked by a room is **owned by the desk**, so assigning it
/// to one is how you ask for a room. A card on a single teammate, an unassigned
/// card, a desk with no roster members, or a desk whose seats will not bind all
/// answer `None` and take the ordinary pooled dispatch — the same rule
/// [`desk_hives`] applies when it skips a desk with nobody to deliberate with.
///
/// Every other property is the desk's own: its name, description, responder mode
/// and seat order. Only the key changes.
#[must_use]
pub fn card_hive(
    record: &CompanyRecord,
    card_id: &str,
    desk_id: &str,
    roster_version: u64,
    bind: &dyn Fn(&str) -> Option<openhuman_embed::Agent>,
) -> Option<DeskHive> {
    let snapshots = crate::runtime::delegation_tools::tinyhivemind_desks(record);
    let desks = snapshots.set();
    let desk = desks.iter().find(|desk| desk.id == desk_id)?;
    if crate::ports::general_channel::is_general_spelling(&desk.id) {
        return None;
    }
    let members = desks.members(&desk.id).ok()?;
    let agents = record.effective_agents();
    let seats = bind_seats(record, &agents, members.iter().copied(), bind);
    // The same floor `desk_hives` holds: a room of one cannot deliberate, and
    // `ask` would have no legal target in it.
    if seats.members.len() < 2 {
        return None;
    }
    let graph = HiveGraph::new(
        tinyhivemind::desk::Desk {
            id: card_id.to_owned(),
            name: desk.name.clone(),
            description: desk.description.clone(),
            members: seats.members,
            responder_mode: desk.responder_mode.clone(),
        },
        seats.candidates,
    );
    BoundHive::new(graph, seats.bindings)
        .inspect_err(|source| {
            tracing::warn!(
                company = %record.id,
                card = %card_id,
                desk = %desk_id,
                %source,
                "[hive] a card's room would not validate; it falls back to a pooled dispatch"
            );
        })
        .ok()
        .map(|hive| DeskHive {
            desk_id: card_id.to_owned(),
            desk_name: desk.name.clone(),
            hive,
            roster_version,
        })
}

/// One desk's membership, bound onto live agents and ready for a graph.
struct BoundSeats {
    /// The canonical ids that bound, in the order given.
    members: Vec<String>,
    /// One router candidate per bound seat.
    candidates: Vec<RouteCandidate>,
    /// One binding per bound seat.
    bindings: Vec<AgentBinding<EmbedSeat>>,
}

/// Binds `member_ids` onto live agents, dropping any that is not a roster
/// teammate or that `bind` cannot resolve.
///
/// Extracted from [`desk_hives`] so a room can be built from a desk's membership
/// while being **keyed on something else** — see [`card_hive`], which re-keys it
/// onto a card id. The split is at the membership rather than at the graph
/// because the graph is the only thing the two callers disagree about; sharing
/// the loop is what keeps "who may sit in a room" one rule.
fn bind_seats<'a>(
    record: &CompanyRecord,
    agents: &[crate::company::Agent],
    member_ids: impl Iterator<Item = &'a str>,
    bind: &dyn Fn(&str) -> Option<openhuman_embed::Agent>,
) -> BoundSeats {
    let mut seats = BoundSeats {
        members: Vec::new(),
        candidates: Vec::new(),
        bindings: Vec::new(),
    };
    for member in member_ids {
        if !record.is_roster_agent(member) {
            continue;
        }
        let Some(agent) = bind(member) else {
            continue;
        };
        let profile = agents.iter().find(|agent| agent.id == member);
        seats.candidates.push(RouteCandidate {
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
        seats.bindings.push(AgentBinding::new(member, EmbedSeat(agent)));
        seats.members.push(member.to_string());
    }
    seats
}

/// One hive per operator DM: the teammate it belongs to, and everyone it may
/// consult.
///
/// # Why a DM is not a one-member room
///
/// Because a one-member room cannot consult anyone. `ask` resolves its target
/// through `BoundHive::resolve_dm`, which refuses a recipient that is not a
/// member -- and asking yourself is refused separately. A teammate alone in
/// its own DM would carry `ask` on its belt with no legal target for it.
///
/// So the hive holds the roster, and the *round* holds one seat. Membership
/// says who may be turned to; routing says who is. The operator's message
/// goes to the teammate whose DM it is and wakes nobody else; the others sit
/// idle until that teammate asks one of them something.
///
/// # What this decides
///
/// That consulting is unscoped. `delegates_to` narrows *delegation*, and it
/// names desks rather than teammates, so it cannot express "these colleagues
/// may be asked". Binding the roster means any teammate can be asked a
/// question in a DM -- which is a different act from putting work on them,
/// and the narrower rule is still the board's to enforce.
pub fn dm_hives(
    record: &CompanyRecord,
    roster_version: u64,
    bind: &dyn Fn(&str) -> Option<openhuman_embed::Agent>,
) -> (HashMap<String, Arc<DeskHive>>, Vec<HiveBuildError>) {
    let agents = record.effective_agents();
    let mut hives = HashMap::new();
    let mut errors = Vec::new();
    // Bound once: every DM's membership is the same roster in a different
    // order, and binding is a clone of a handle the pool already holds.
    let bound: Vec<(String, openhuman_embed::Agent)> = agents
        .iter()
        .filter(|agent| record.is_roster_agent(&agent.id))
        .filter_map(|agent| bind(&agent.id).map(|handle| (agent.id.clone(), handle)))
        .collect();
    for (owner, _) in &bound {
        // The owner first, because `DeskHive::lead` is the first member and a
        // DM's responder is never in doubt: it is whose DM it is.
        let ordered: Vec<&(String, openhuman_embed::Agent)> = bound
            .iter()
            .filter(|(member, _)| member == owner)
            .chain(bound.iter().filter(|(member, _)| member != owner))
            .collect();
        let profile = |id: &str| agents.iter().find(|agent| agent.id == id);
        let candidates: Vec<RouteCandidate> = ordered
            .iter()
            .map(|(member, _)| RouteCandidate {
                id: member.clone(),
                label: profile(member)
                    .and_then(|agent| agent.name.clone())
                    .unwrap_or_else(|| member.clone()),
                role: profile(member).map(|agent| agent.role.clone()),
                description: profile(member).and_then(|agent| agent.description.clone()),
                capabilities: Vec::new(),
                learned_topics: Vec::new(),
                available: true,
            })
            .collect();
        let bindings: Vec<AgentBinding<EmbedSeat>> = ordered
            .iter()
            .map(|(member, handle)| AgentBinding::new(member.clone(), EmbedSeat(handle.clone())))
            .collect();
        let members: Vec<String> = ordered.iter().map(|(member, _)| member.clone()).collect();
        let chat = format!("{}{owner}", crate::runtime::assignee::DM_PREFIX);
        let name = profile(owner)
            .and_then(|agent| agent.name.clone())
            .unwrap_or_else(|| owner.clone());
        let graph = HiveGraph::new(
            tinyhivemind::desk::Desk {
                id: chat.clone(),
                name: name.clone(),
                description: profile(owner).and_then(|agent| agent.description.clone()),
                members,
                // The lead answers, always. A DM has one correct responder and
                // it is not a question a router should be asked.
                responder_mode: tinyhivemind::desk::ResponderMode::Lead,
            },
            candidates,
        );
        match BoundHive::new(graph, bindings) {
            Ok(hive) => {
                hives.insert(
                    chat.clone(),
                    Arc::new(DeskHive {
                        desk_id: chat,
                        desk_name: name,
                        hive,
                        roster_version,
                    }),
                );
            }
            Err(source) => errors.push(HiveBuildError::Invalid {
                desk_id: chat,
                source,
            }),
        }
    }
    (hives, errors)
}
