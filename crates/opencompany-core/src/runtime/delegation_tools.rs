//! Brain-agnostic roster, desk and board-tool primitives (issue #176).
//!
//! The pieces both brains share one definition of:
//!
//! - the `spawn_task` tool name, schema and argument parser ([`SPAWN_TASK_TOOL`],
//!   [`spawn_task_schema`], [`SpawnTaskArgs`]) — the harness `orchestrator`
//!   module re-exports the name, and [`delegation_manifest_entries`] is the
//!   catalog the hosted brain registers with Medulla;
//! - the desk and roster resolvers — [`desk_lead`], [`desk_default_responder`],
//!   [`chat_responder`], [`desk_ids`], [`roster_agent_ids`];
//! - the **reach** rule — [`teammate_targets`] and [`reach_is_unrestricted`] —
//!   which the company hive's send policy (`hive::policy`) decides
//!   `hivemind_send_agent` under, and the team brief renders.
//!
//! The desk and teammate hand-off tools (`delegate_to_desk`,
//! `delegate_to_teammate`) and their target checks are gone with the hive
//! cutover (OC-2): agents talk to each other through the permanent
//! `hivemind_*` tools.
//!
//! Compiled in every build (no feature gate): the hosted brain is in the
//! default build, and the harness path re-exports from here.

use serde_json::{Value, json};

use crate::brain::medulla::wire::ToolManifestEntry;
use crate::ports::types::CompanyRecord;

/// TinyHiveMind's active roster snapshot for one routing/dispatch decision.
pub fn tinyhivemind_roster(record: &CompanyRecord) -> Vec<tinyhivemind_core::roster::RosterMember> {
    record
        .effective_agents()
        .into_iter()
        .filter(|agent| !record.is_retired(&agent.id))
        .map(|agent| tinyhivemind_core::roster::RosterMember {
            id: agent.id,
            name: agent.name,
        })
        .collect()
}

pub struct TinyHiveDeskSnapshots {
    declared: Vec<tinyhivemind_core::desk::Desk>,
    added: Vec<tinyhivemind_core::desk::Desk>,
    member_additions: Vec<tinyhivemind_core::desk::DeskMember>,
    orders: Vec<tinyhivemind_core::desk::DeskOrder>,
    retired: Vec<String>,
}

impl TinyHiveDeskSnapshots {
    pub fn set(&self) -> tinyhivemind_core::desk::DeskSet<'_> {
        tinyhivemind_core::desk::DeskSet::new(
            &self.declared,
            &self.added,
            &self.member_additions,
            &self.orders,
            &self.retired,
        )
    }
}

pub fn tinyhivemind_desks(record: &CompanyRecord) -> TinyHiveDeskSnapshots {
    use tinyhivemind_core::desk::{Desk, DeskMember, DeskOrder, ResponderMode};
    TinyHiveDeskSnapshots {
        declared: record
            .manifest
            .group_chats
            .iter()
            .map(|chat| Desk {
                id: chat.id.clone(),
                name: chat.name.clone(),
                description: chat.description.clone(),
                members: chat.members.clone(),
                responder_mode: ResponderMode::Lead,
            })
            .collect(),
        added: record
            .overlay_desks
            .iter()
            .map(|desk| Desk {
                id: desk.id.clone(),
                name: desk.name.clone(),
                description: desk.description.clone(),
                members: desk.members.clone(),
                responder_mode: match desk.responder {
                    crate::ports::types::ResponderMode::Lead => ResponderMode::Lead,
                    crate::ports::types::ResponderMode::Auto => ResponderMode::Auto,
                },
            })
            .collect(),
        member_additions: record
            .overlay_desk_members
            .iter()
            .map(|member| DeskMember {
                desk_id: member.desk_id.clone(),
                agent_id: member.agent_id.clone(),
            })
            .collect(),
        orders: record
            .overlay_desk_order
            .iter()
            .map(|order| DeskOrder {
                desk_id: order.desk_id.clone(),
                ordered: order.ordered.clone(),
            })
            .collect(),
        retired: record.overlay_retired_agents.clone(),
    }
}

/// The `spawn_task` tool name — open a tracked task card on the board.
pub const SPAWN_TASK_TOOL: &str = "spawn_task";

/// The `spawn_task` argument schema, shared by the harness tool's
/// `parameters_schema` and the hosted manifest entry so the two never drift.
pub fn spawn_task_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string", "description": "The task title." },
            "note": { "type": "string", "description": "An optional longer brief." },
            "assignee": { "type": "string", "description": "An optional desk/teammate id to own it." }
        },
        "required": ["title"],
        "additionalProperties": false
    })
}

/// The board tool advertised to Medulla on the hosted path: `spawn_task`, with
/// the same name + schema the harness exposes. Registered on top of the
/// manifest's own `tools.allow` catalog; the device services the resulting
/// tool-call frame in [`CycleHostImpl`](crate::runtime::cycle) by opening a
/// board card.
pub fn delegation_manifest_entries() -> Vec<ToolManifestEntry> {
    vec![
        ToolManifestEntry {
            name: SPAWN_TASK_TOOL.to_string(),
            description: Some(
                "Open a tracked task card on the company's board for work that should be \
followed up. Provide a `title`, an optional `note` brief, and an optional `assignee` (a desk \
or teammate id)."
                    .to_string(),
            ),
            input_schema: Some(spawn_task_schema()),
        },
    ]
}

/// The lead member of a desk: the first effective member (manifest ∪ overlay)
/// that is a real roster teammate. `None` when no desk matches or none of its
/// members are on the roster.
///
/// Resolves the desk key (id or case-insensitive name) against both manifest
/// and operator-created overlay desks, then reads the same **effective**
/// membership the REST `list_desks` handler uses
/// ([`CompanyRecord::effective_desk_members`]), so routing and the console
/// cannot drift. An overlay-added lead is reachable on a desk the manifest left
/// empty.
///
/// Lifted here from the harness-only delegation runtime so the hosted path can
/// resolve a desk lead without the `openhuman` feature.
///
/// An [`Auto`](crate::ports::types::ResponderMode::Auto) channel (issue #1835)
/// answers `None` **by definition, not by accident**: no lead exists there, so
/// every lead-derived surface — the org chart's crown, the members-pane badge
/// — stays honest without knowing the mode. The
/// deterministic "who answers here" question is [`desk_default_responder`],
/// which ignores the mode on purpose.
pub fn desk_lead(record: &CompanyRecord, desk: &str) -> Option<String> {
    let desk_id = record.resolve_desk_id(desk)?;
    if !record.desk_responder_mode(&desk_id).is_lead() {
        return None;
    }
    record
        .effective_desk_members(&desk_id)
        .into_iter()
        .find(|m| record.is_roster_agent(m))
}

/// The desk's first effective roster member, **whatever its responder mode**
/// (issue #1835).
///
/// For a [`Lead`](crate::ports::types::ResponderMode::Lead) desk this is the
/// lead, byte-for-byte what [`desk_lead`] answers. For an `Auto` channel it is
/// the deterministic fallback: the answer wherever per-message selection
/// cannot run — the default build (the selector compiles only under the
/// harness feature), the cycle's small-talk fast path, or a selection failure.
/// One function, so "the fallback" cannot drift from "the pre-selector
/// behaviour".
pub fn desk_default_responder(record: &CompanyRecord, desk: &str) -> Option<String> {
    let desk_id = record.resolve_desk_id(desk)?;
    record
        .effective_desk_members(&desk_id)
        .into_iter()
        .find(|m| record.is_roster_agent(m))
}

/// Which roster teammate answers a message addressed to `chat` — or `None`
/// when the key names neither a desk nor a teammate.
///
/// The four arms, in order, are the ones the harness brain's `responder_for`
/// has always tried: a desk key resolves to its
/// [`desk_lead`], a bare roster agent id answers as itself, and the console's
/// `dm:<teammate-id>` thread key is unwrapped and tried both ways (issue #982,
/// step 3) — last, so it can only claim a key that resolves to nothing today.
///
/// **Returns `None` rather than falling back to the orchestrator**, because the
/// two callers want different things from a miss: the harness brain logs a
/// warning and answers as the orchestrator it resolved when it was built, while
/// the cycle's small-talk fast path (issue #1725) resolves the orchestrator off
/// the record in hand. Folding the fallback in here would make one of those
/// wrong.
///
/// Lifted out of the harness brain so the fast path — which is brain-agnostic
/// and runs before any brain — attributes its reply to the same teammate the
/// turn it replaced would have been answered by.
///
/// #general answers to nobody here, so both callers resolve their own
/// orchestrator for it.
pub fn chat_responder(record: &CompanyRecord, chat: &str) -> Option<String> {
    // The desk arm asks [`desk_default_responder`], not [`desk_lead`]: for a
    // lead desk the two are identical, and for an `Auto` channel (issue #1835)
    // — where `desk_lead` is `None` by definition — this stays the
    // deterministic answer. The per-message selection that may override it is
    // the harness brain's own rung, deliberately not folded in here: this seam
    // is sync, record-only, and shared with the small-talk fast path, which
    // must not pay an inference call to attribute a greeting.
    let direct = |key: &str| {
        desk_default_responder(record, key).or_else(|| record.resolve_roster_agent_id(key))
    };
    if let Some(responder) = desk_default_responder(record, chat) {
        return Some(responder);
    }
    if chat == crate::ports::general_channel::GENERAL_CHANNEL_ID {
        return None;
    }
    if let Some(agent) = record.resolve_roster_agent_id(chat) {
        return Some(agent);
    }
    // A `dm:` key names a **teammate**, so the roster is asked first when the
    // prefix was used. Unwrapping straight into the desk-first `direct`
    // resolver let a desk capture it: a blueprint may declare both a teammate
    // and a desk with id `main` — manifest validation does not forbid the
    // collision — and the prefixed address exists precisely to reach that
    // teammate, so handing it to the desk's lead answers the wrong party in
    // the one case the prefix was added for (issue #1743). The desk arm stays
    // as the fallback, so `dm:<desk>` still resolves a desk that no teammate
    // shares an id with, exactly as before.
    crate::runtime::assignee::dm_key(chat)
        .and_then(|key| record.resolve_roster_agent_id(key).or_else(|| direct(key)))
}

/// Every desk id the company actually has: the manifest `[[group_chat]]` desks
/// in declaration order, then any operator-created overlay desks, deduplicated.
///
/// Every desk is a hive of the company hive (OC-2), so this is also the set of
/// hives it syncs. Reads the same two sources [`CompanyRecord::resolve_desk_id`]
/// searches, so "what ids exist" and "does this id resolve" cannot disagree.
pub fn desk_ids(record: &CompanyRecord) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for chat in &record.manifest.group_chats {
        if !ids.contains(&chat.id) {
            ids.push(chat.id.clone());
        }
    }
    for desk in &record.overlay_desks {
        if !ids.contains(&desk.id) && !crate::ports::general_channel::is_general_spelling(&desk.id)
        {
            ids.push(desk.id.clone());
        }
    }
    ids
}


/// Every roster teammate id the company has: manifest agents in declaration
/// order, then operator-added overlay teammates, deduplicated.
///
/// The teammate-side counterpart of [`desk_ids`], and read for the same reason —
/// "who exists" and "does this key resolve" must come from one source.
pub fn roster_agent_ids(record: &CompanyRecord) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for id in record
        .manifest
        .agents
        .iter()
        .map(|a| &a.id)
        .chain(record.overlay_agents.iter().map(|a| &a.id))
    {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ids
}

/// Everybody on a desk with `caller`, in desk-membership order, deduplicated,
/// never the caller — the desk-peer arm of [`teammate_targets`] on its own.
pub fn desk_peers(record: &CompanyRecord, caller: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for desk in desks_of_member(record, caller) {
        for member in record.effective_desk_members(&desk) {
            if member != caller
                && record.is_roster_agent(&member)
                && !ids.iter().any(|held| held == &member)
            {
                ids.push(member);
            }
        }
    }
    ids
}

/// Whether a [`delegates_to`](crate::company::Agent::delegates_to) list places
/// no bound on where its holder may hand work: **empty**, or carrying the
/// [`DELEGATES_TO_WILDCARD`](crate::company::DELEGATES_TO_WILDCARD).
///
/// Empty is unrestricted on the same convention as an omitted `tools` grant or
/// an omitted `ledgers` list — the manifest says nothing, so nothing is
/// narrowed. Only a list that names desks narrows. This is what makes a
/// teammate able to reach the rest of its company without an operator
/// enumerating the company in every agent's manifest entry.
pub fn reach_is_unrestricted(allowed: &[String]) -> bool {
    allowed.is_empty()
        || allowed
            .iter()
            .any(|entry| entry.trim() == crate::company::DELEGATES_TO_WILDCARD)
}

/// The teammates `caller` may message directly (`hivemind_send_agent`, decided
/// by the company hive's send policy) — issue #884's reach rule. Never the
/// caller itself.
///
/// With an unrestricted `allowed` (see [`reach_is_unrestricted`]) — the
/// ordinary case, an agent whose manifest entry says nothing — that is
/// **everybody on the roster**: desk-mates first, then everybody else in
/// roster order. A teammate is not locked out of the company because nobody
/// wrote it a list, and the orchestrator and a desk-less specialist are
/// reachable like anyone else.
///
/// With a list that names desks, the reach is everybody on a desk with the
/// caller, plus everybody on a desk the list permits. The desk-peer arm is the
/// one #884 added and the one that closes D1: a desk's lead can reach the
/// specialist sitting beside it without going back through the orchestrator.
/// The allowlist arm is a re-reading of the #176 desk permission at teammate
/// granularity — a member allowed to hand work to a desk is allowed to hand it
/// to somebody on that desk.
///
/// Order is desk-peers first, then the rest, each in the desk's own membership
/// order, deduplicated — so a refusal (and the team brief) lists the nearest
/// options first.
pub fn teammate_targets(record: &CompanyRecord, caller: &str, allowed: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = desk_peers(record, caller);
    let push = |ids: &mut Vec<String>, id: &str| {
        if id != caller && record.is_roster_agent(id) && !ids.iter().any(|held| held == id) {
            ids.push(id.to_string());
        }
    };
    if reach_is_unrestricted(allowed) {
        for id in roster_agent_ids(record) {
            push(&mut ids, &id);
        }
        return ids;
    }
    for desk in allowed
        .iter()
        .filter_map(|entry| record.resolve_desk_id(entry.trim()))
    {
        for member in record.effective_desk_members(&desk) {
            push(&mut ids, &member);
        }
    }
    ids
}

/// Every desk `member` sits on, in [`desk_ids`] order.
pub(crate) fn desks_of_member(record: &CompanyRecord, member: &str) -> Vec<String> {
    desk_ids(record)
        .into_iter()
        .filter(|id| {
            record
                .effective_desk_members(id)
                .iter()
                .any(|m| m == member)
        })
        .collect()
}

/// The desk `member` sits on, but only when unambiguous — `member` belongs to
/// exactly one desk. `None` both when `member` is on no desk and when it is on
/// two or more.
///
/// `pub(crate)`: the issue #1862 prerequisite's default-owner lever —
/// `apply_workflow_proposal` (`server/ops/tasks.rs`) fills a proposal's
/// omitted `owner_desk` from the assignee's desk. That default gets written to
/// disk, so picking
/// the first desk in manifest declaration order for a teammate who sits on
/// several would silently misrepresent ownership (issue #1882 review) —
/// leaving `owner_desk` unset is the same permissive "best-effort" stance the
/// caller already takes toward an assignee with no desk at all.
pub(crate) fn sole_desk_of_member(record: &CompanyRecord, member: &str) -> Option<String> {
    let mut desks = desks_of_member(record, member).into_iter();
    let first = desks.next()?;
    if desks.next().is_some() {
        return None;
    }
    Some(first)
}

/// Parsed `spawn_task` arguments: a required title, an optional brief note, and
/// an optional assignee. Blank strings are treated as absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnTaskArgs {
    /// The task title (required, non-blank).
    pub title: String,
    /// An optional longer brief.
    pub note: Option<String>,
    /// An optional desk/teammate id to own the card.
    pub assignee: Option<String>,
}

impl SpawnTaskArgs {
    /// Parses `spawn_task` args, returning `None` when `title` is missing or
    /// blank (the one hard requirement).
    pub fn parse(args: &Value) -> Option<Self> {
        let title = trimmed_str(args, "title")?;
        Some(Self {
            title,
            note: trimmed_str(args, "note"),
            assignee: trimmed_str(args, "assignee"),
        })
    }
}

/// Reads `key` as a string, trims it, and returns it only when non-empty.
fn trimmed_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
#[path = "delegation_tools_tests_core.rs"]
mod tests_core;
#[cfg(test)]
#[path = "delegation_tools_tests_part2.rs"]
mod tests_part2;
