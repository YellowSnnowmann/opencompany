//! Hive desks: tinyhivemind's completion-driven episodes hosted over one
//! process-wide OpenHuman runtime (plan `hive-desks`).
//!
//! This file is an index. Each concern lives in its own module and is listed,
//! file by file, in this directory's `README.md`. Everything that reaches the
//! MCP wire is gated on the `mcp` feature: OpenCompany's own tools are served
//! to the agents over MCP because `openhuman_embed::Agent` has no seam for an
//! in-process host tool, and a build without `mcp` has no `McpServer` to
//! attach.

/// Jev routing over the TinyHumans System One proxy: the host-owned
/// `SystemOneTransport` and the `jev_router` constructor (plan Phase 7).
/// Gated with the harness whose credential seam it reads.
#[cfg(feature = "openhuman")]
pub mod jev;
/// The JSON-RPC Streamable-HTTP MCP server the company agents call their
/// speech and OpenCompany tools on (plan Phase 3).
#[cfg(feature = "mcp")]
pub mod mcp_server;
/// The `[group_chat.routing]` block, its resolved `RoutingPolicy`, and the
/// desk-routing wire shapes (plan Phase 4).
pub mod routing;
/// The in-flight turn registry, the speech fold and the tool adapter the
/// server dispatches through (plan Phase 3).
#[cfg(feature = "mcp")]
pub mod tools;
