//! Hive desks: every `[[group_chat]]` is one `tinyhivemind_openhuman::OpenHumanHive`
//! whose seats are the company's `openhuman_embed` agents, driven through
//! completion episodes (`docs/spec/runtime/hive.md`).
//!
//! This file is an index. Each submodule owns one seam and documents it in
//! its own `//!` header; `README.md` beside this file lists them.

/// Jev routing over the TinyHumans System One proxy: the host-owned
/// `SystemOneTransport` and the `jev_router` constructor. Gated with the
/// harness because it is the harness's `reqwest` it posts with.
#[cfg(feature = "openhuman")]
pub mod jev;
