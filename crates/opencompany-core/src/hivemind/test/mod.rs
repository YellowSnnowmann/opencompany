//! Unit tests for the hive-mind desk seam, split by topic:
//!
//! - [`fixtures`] — the in-memory journal, scripted turn runner, and small
//!   manifest/desk builders every other topic file (and several sibling test
//!   files elsewhere in `hivemind/`) is built from.
//! - `manifest` — the [`HiveConfig`] manifest knob.
//! - `log_adapter` — the company journal read as a `tinyhivemind` session log.
//! - `episode_driver` — the [`EpisodeDriver`] host loop.
//! - `watermark` — the episode watermark divider rendered into a transcript.
//!
//! Re-exported here at the same visibility the flat file used to grant, so
//! `crate::hivemind::test::MemoryLog` and the sibling test files' `super::test::{..}`
//! imports keep resolving unchanged.

mod episode_driver;
mod fixtures;
mod log_adapter;
mod manifest;
mod watermark;

pub(crate) use fixtures::MemoryLog;
pub(super) use fixtures::{ScriptedRunner, desk_of, record};
pub(super) use log_adapter::seed_desk;
