//! The company hive: TinyHiveMind's durable `Coordinator` and its OpenHuman
//! adapter, bound to one company (OC-2).
//!
//! This file is an index. Each concern lives in its own module and is listed,
//! file by file, in this directory's `README.md`. The Coordinator itself, its
//! storage adapter over this crate's `HiveStore` port, the projector that
//! journals its transcript, the send policy and the starter route are pure
//! enough to compile on the default build; the runtime that registers live
//! `openhuman_embed::Agent` handles, the Jev transport, and the MCP server
//! with its in-flight registry are gated with the harness (`openhuman`).

/// Jev routing over the TinyHumans System One proxy: the host-owned
/// `SystemOneTransport` and the `jev_router` constructor (plan Phase 7).
/// Gated with the harness whose credential seam it reads.
#[cfg(feature = "openhuman")]
pub mod jev;
/// The JSON-RPC Streamable-HTTP MCP server the company agents may reach
/// OpenCompany's tools on, and the in-flight registry it shares with the turn
/// envelope (plan Phase 3).
#[cfg(feature = "openhuman")]
pub mod mcp_server;
/// Coordination metrics folded from the journal — concurrency, contacts,
/// completion — behind `opencompany measure` (Phase 8).
pub mod measure;
/// Who may send to whom: the reach rules a teammate's `hivemind_send_*` /
/// `ask` / `broadcast` calls are decided under (the `SendAuthorizer`).
pub mod policy;
/// The Coordinator transcript journaled as company events: replies, private
/// rows, settled episodes and interrupted turns.
pub mod projector;
/// Which hive members start an operator message: mention, then Jev, then the
/// desk's default responder.
pub mod route;
/// The `[group_chat.routing]` block, its resolved policy, the company
/// `CoordinatorOptions` folded from it, and the desk-routing wire shapes.
pub mod routing;
/// One company's hive: the Coordinator, the OpenHuman host its agents are
/// registered on, the projector and the run task. Needs live agent handles,
/// so it is gated with the harness.
#[cfg(feature = "openhuman")]
pub mod runtime;
/// `tinyhivemind_hives::Storage` over this crate's `HiveStore` port.
pub mod storage;
/// The in-flight turn registry and the MCP tool adapter.
#[cfg(feature = "openhuman")]
pub mod tools;

/// The hive the company's `#general` line runs in.
///
/// TinyHiveMind core reserves `general` and `main` (any case) as desk
/// identities except for its own default desk, whose id and name are both
/// `General`; a hive created as `general` is accepted by the Coordinator but
/// fails every episode it opens with `ReservedDeskIdentity`. So the console's
/// `general` chat runs in that default desk, and every boundary between a
/// console chat and a hive translates with [`hive_id_for_chat`] /
/// [`chat_for_hive`]. A desk id is never `general` (the console reserves it),
/// so the mapping is one-to-one. Any other desk whose id or name core would
/// reserve is renamed on the way in ([`RESERVED_DESK_PREFIX`], [`hive_name`]).
pub const GENERAL_HIVE_ID: &str = "General";

/// The prefix a desk id core reserves (`main`, say) runs under in the hive.
pub const RESERVED_DESK_PREFIX: &str = "desk-";

/// Whether core reserves `identity` as a desk id or name.
fn reserved(identity: &str) -> bool {
    identity.eq_ignore_ascii_case("general") || identity.eq_ignore_ascii_case("main")
}

/// The hive id behind console chat `chat` (a desk id, or `general`).
#[must_use]
pub fn hive_id_for_chat(chat: &str) -> String {
    if chat == crate::ports::general_channel::GENERAL_CHANNEL_ID {
        GENERAL_HIVE_ID.to_string()
    } else if reserved(chat) {
        format!("{RESERVED_DESK_PREFIX}{chat}")
    } else {
        chat.to_string()
    }
}

/// The console chat behind hive `hive_id`; the inverse of
/// [`hive_id_for_chat`].
#[must_use]
pub fn chat_for_hive(hive_id: &str) -> String {
    if hive_id == GENERAL_HIVE_ID {
        return crate::ports::general_channel::GENERAL_CHANNEL_ID.to_string();
    }
    match hive_id.strip_prefix(RESERVED_DESK_PREFIX) {
        Some(desk) if reserved(desk) => desk.to_string(),
        _ => hive_id.to_string(),
    }
}

/// The hive name a desk called `name` runs under: its own, unless core
/// reserves it, in which case the desk id is appended so it is not.
#[must_use]
pub fn hive_name(hive_id: &str, name: &str) -> String {
    if hive_id != GENERAL_HIVE_ID && reserved(name.trim()) {
        format!("{} ({hive_id})", name.trim())
    } else {
        name.to_string()
    }
}

/// The permanent tools the TinyHiveMind OpenHuman adapter attaches to every
/// registered agent (`docs/opencompany-migration.md` in `vendor/tinyhivemind`):
/// how an agent reads hives, messages a teammate, posts, asks, broadcasts and
/// completes its part of an episode. Named here so an agent's tool scope can
/// admit them, and so no belt tool of this crate's can shadow one
/// (`attach_tools` refuses a name collision).
pub const HIVEMIND_TOOLS: &[&str] = &[
    "hivemind_list_hives",
    "hivemind_list_agents",
    "hivemind_read",
    "hivemind_send_hive",
    "hivemind_send_agent",
    "hivemind_post",
    "hivemind_ask",
    "hivemind_broadcast",
    "hivemind_complete",
];

#[cfg(test)]
#[path = "hive_tests.rs"]
mod tests;
