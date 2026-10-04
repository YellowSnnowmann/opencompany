//! Filesystem-backed persistence for the runtime's durable ports.
//!
//! Each company owns a [`Bundle`] directory (see [`paths`]) holding its
//! manifest, event log, ledger, cycle traces, and secrets. The [`fs`]
//! module implements [`CompanyStore`](crate::ports::CompanyStore),
//! [`EventLog`](crate::ports::EventLog),
//! [`TraceStore`](crate::ports::TraceStore), and
//! [`SecretStore`](crate::ports::SecretStore) over that layout. Company memory
//! is not stored here: it is OpenHuman's (`crate::memory`).

/// Store-agnostic bundle export and import: read everything through the four
/// durable ports and write the canonical fs [`Bundle`](paths::Bundle) layout
/// (and the inverse). The dep-free core operates on an unpacked bundle
/// directory; a single-file `.tar` wrapper is gated behind the `export` feature.
pub mod export;
pub mod fs;
/// Filesystem backends for the WS3 console ports (tasks, usage,
/// skill-state, workspace tree) over the same [`Bundle`](paths::Bundle) layout.
pub mod fs_ops;
/// Backends for the [`HiveStore`](crate::ports::HiveStore) port — the
/// per-company compare-and-swap state document and append-only message log a
/// hive coordinator persists through — and their conformance suite.
pub mod hive;
pub mod layout;
/// The canonical per-instance directory layout under `OPENCOMPANY_DATA_DIR`
/// (`companies/`, `memory/`, `store/`, `files/`, `logs/`, `tmp/`) and the
/// startup lifecycle that creates them and, by default, clears `tmp/`.
pub mod lock;
/// The one-shot boot migration off the legacy doubled home layout
/// (`companies/companies/<slug>`), run by `serve`, `export`, and `import`
/// against the resolved home before anything reads it.
pub mod migrate;
pub mod paths;

/// Config-driven backend selection: maps `OPENCOMPANY_STORAGE` (fs | sqlite |
/// mongodb) onto opened port implementations, injected once per process into
/// every company's `RuntimeBuilder`.
pub mod select;

/// Char-boundary-safe slicing, so a byte offset landing mid-codepoint widens
/// to the boundary instead of panicking the slice.
pub(crate) mod text;

#[cfg(feature = "sqlite")]
pub mod sqlite;

/// MongoDB-backed implementations of all five storage ports over the official
/// async driver — the multi-tenant platform backend: every document is keyed
/// on `company_id`, the hosting layer points each tenant at its own database
/// on a shared cluster, and an `owners` collection makes the company → tenant
/// map durable for shared-database platform mode. Only links under `mongodb`.
#[cfg(feature = "mongodb")]
pub mod mongodb;

/// A backend-agnostic port-conformance suite: async assertions parameterized
/// over any [`CompanyStore`](crate::ports::CompanyStore) /
/// [`EventLog`](crate::ports::EventLog) /
/// [`TraceStore`](crate::ports::TraceStore) implementation. Both the fs and
/// sqlite backends run the identical suite, so a new store proves it upholds the
/// port contract (per-company isolation, append-only logs, monotonic seqs,
/// export totality) rather than re-testing each backend by hand. Test-only.
#[cfg(test)]
pub mod conformance;

pub use fs::{
    FsCompanyStore, FsEventLog, FsInboxStore, FsJournalStore, FsSecretStore, FsTraceStore,
};
pub use fs_ops::FsOps;
pub use hive::{FsHiveStore, MemoryHiveStore};
pub use layout::DataLayout;
// Only the boot entry point is re-exported here. The migration is a one-shot
// step the binary runs before it reads anything, and its silent core and result
// types are its own business — reachable at `store::migrate::*` for anyone
// reading the rules, not part of the store's own surface.
pub use migrate::migrate_legacy_nest_announced;
pub use paths::{Bundle, DATA_DIR_ENV, home_divergence_warning, resolve_home};
pub use select::{
    StorageHandles, StorageKind, StorageSettings, open_storage, plaintext_secret_refusal,
    refuse_bundle_env,
};

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;

#[cfg(feature = "mongodb")]
pub use mongodb::MongoStore;
