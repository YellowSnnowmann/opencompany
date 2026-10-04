//! Backends for the [`HiveStore`](crate::ports::HiveStore) port, one file per
//! engine, plus the conformance suite all of them run.
//!
//! Kept in one directory rather than grown into `fs.rs`, `sqlite.rs` and
//! `mongodb.rs`: the port is self-contained, and each of those files is far past
//! the source-file cap already. The sqlite and mongodb halves are `impl` blocks
//! on the existing [`SqliteStore`](crate::store::SqliteStore) and
//! [`MongoStore`](crate::store::MongoStore), so one opened backend still serves
//! every port.

mod fs;
mod memory;

/// The backend-agnostic hive assertions every backend runs. Test-only.
#[cfg(test)]
pub mod conformance;

pub use fs::FsHiveStore;
pub use memory::MemoryHiveStore;
