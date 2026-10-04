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
