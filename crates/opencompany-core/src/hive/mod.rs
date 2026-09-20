//! Hive desks: tinyhivemind's completion-driven episodes hosted over one
//! process-wide OpenHuman runtime (plan `hive-desks`).
//!
//! This file is an index. Each concern lives in its own module and is listed,
//! file by file, in this directory's `README.md`. The MCP server and the turn
//! registry under it are gated with the harness (`openhuman`): OpenCompany's
//! own tools are served to the agents over MCP because `openhuman_embed::Agent`
//! has no seam for an in-process host tool, and this crate's `openhuman-embed`
//! dependency enables `AgentSpec::mcp` unconditionally, so every harness build
//! — the `rust-gated` lane included — can attach the server. (The plan named
//! the `mcp` feature; that feature adds only OpenHuman's own MCP client
//! surface, which the server does not need, and gating on it would leave the
//! harness lane's turns without their tools.)

/// Jev routing over the TinyHumans System One proxy: the host-owned
/// `SystemOneTransport` and the `jev_router` constructor (plan Phase 7).
/// Gated with the harness whose credential seam it reads.
#[cfg(feature = "openhuman")]
pub mod jev;
/// The JSON-RPC Streamable-HTTP MCP server the company agents call their
/// speech and OpenCompany tools on (plan Phase 3).
#[cfg(feature = "openhuman")]
pub mod mcp_server;
/// The `[group_chat.routing]` block, its resolved `RoutingPolicy`, and the
/// desk-routing wire shapes (plan Phase 4).
pub mod routing;
/// The in-flight turn registry, the speech fold and the tool adapter the
/// server dispatches through (plan Phase 3).
#[cfg(feature = "openhuman")]
pub mod tools;
