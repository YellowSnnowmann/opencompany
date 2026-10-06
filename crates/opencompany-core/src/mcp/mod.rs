//! Everything OpenCompany knows about MCP tool servers, in one place.
//!
//! - [`decl`] — the declaration data model: what a company declares, how the
//!   three layers merge, where credentials and health live, the bundle's
//!   `mcp.json`, and what a server says about itself.
//! - [`policy`] — per-tool approval policy: tiers, stored overrides, and the
//!   per-agent narrowing.
//!
//! - [`probe`] — dialling a server once, classifying the failure, and
//!   recording its health, tool inventory and identity (`openhuman`).
//! - [`runtime`] — the company-scoped registry store: directory search,
//!   installs, connections and calls (`openhuman`).
//! - [`agent`] — what one agent reaches: the servers attached to its spec, the
//!   registry tools on its belt, and the briefs that describe them
//!   (`openhuman`).
//!
//! `decl` and `policy` are ungated so their tests run in the default lane.
//! See `src/mcp/README.md` for the file-by-file map.

/// What one agent reaches over MCP, assembled from the company's declarations
/// and that agent's grants.
#[cfg(feature = "openhuman")]
pub mod agent;
pub mod decl;
/// Per-tool approval policy for MCP servers: the tier vocabulary, the
/// operator's stored overrides, and the ladder that resolves one from the
/// other. Ungated — the console route that edits a policy ships without the
/// harness, and the gate that enforces one ships with it.
pub mod policy;
/// Probing a declared server and classifying its failures into the health
/// record the console renders.
#[cfg(feature = "openhuman")]
pub mod probe;
/// The company-scoped MCP registry store behind the console's registry routes
/// and the agents' `mcp_registry_*` tools.
#[cfg(feature = "openhuman")]
pub mod runtime;
