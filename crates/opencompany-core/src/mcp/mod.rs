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
//!
//! `decl` and `policy` are ungated so their tests run in the default lane.
//! See `src/mcp/README.md` for the file-by-file map.

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
