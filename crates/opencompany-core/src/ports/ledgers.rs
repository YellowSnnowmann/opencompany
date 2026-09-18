//! The [`LedgerStore`] port: a company's declared ledgers and their event logs.
//!
//! Two things live behind it and they have very different lifetimes. A
//! **declaration** ([`LedgerSpec`]) is small, rewritten in place, and there are
//! at most [`MAX_DECLARED`](crate::ledger::MAX_DECLARED) of them. A ledger's
//! **events** are append-only and unbounded, and folding them is what produces
//! the rows anybody reads.
//!
//! # Why the log, and not the rows
//!
//! Because the rows are derivable and the log is not. A store that kept current
//! state would have to be told *how* a row changed in order to keep the history,
//! and every backend would have to agree about it; a store that keeps events
//! only has to keep them in order. That is also what makes the one deletion
//! rule enforceable — see [`AuthorKind::may_delete`].
//!
//! Company A's ledgers MUST be invisible to company B, like every other port.

use async_trait::async_trait;

use crate::Result;
use crate::ledger::{LedgerEvent, LedgerSpec};
use crate::ports::types::CompanyId;

/// Durable per-company ledger declarations and event logs.
#[async_trait]
pub trait LedgerStore: Send + Sync {
    /// Every ledger this company has **declared**.
    ///
    /// Built-ins are not stored: they ship with the runtime, and persisting a
    /// copy would let a company's stored version drift from the code every
    /// prompt and route is written against. The registry adds them back on
    /// read.
    async fn list_specs(&self, company: &CompanyId) -> Result<Vec<LedgerSpec>>;

    /// Inserts or replaces a declaration by slug.
    async fn put_spec(&self, company: &CompanyId, spec: &LedgerSpec) -> Result<()>;

    /// Removes a declaration by slug; returns whether one was removed.
    ///
    /// **The entries are left alone.** A ledger nobody reads is worth retiring;
    /// the work recorded in it is not, and deleting a log to tidy a registry is
    /// exactly the loss the append-only shape exists to prevent. Deleting the
    /// rows too is [`purge_ledger`](Self::purge_ledger), which is a separate,
    /// deliberate act.
    async fn delete_spec(&self, company: &CompanyId, slug: &str) -> Result<bool>;

    /// Appends one event.
    ///
    /// The store's only ordering obligation: [`events`](Self::events) returns
    /// what this appended, in the order it was appended. Everything the fold
    /// promises rests on that and on nothing else — in particular not on the
    /// event's timestamp, which is written by whichever replica is running.
    async fn append(&self, company: &CompanyId, event: &LedgerEvent) -> Result<()>;

    /// Every event for one ledger, oldest first.
    async fn events(&self, company: &CompanyId, ledger: &str) -> Result<Vec<LedgerEvent>>;

    /// Removes every event naming `entry`; returns whether any were removed.
    ///
    /// **A person's call, never an agent's** — see
    /// [`AuthorKind::may_delete`](crate::ledger::AuthorKind::may_delete). The
    /// port cannot enforce that (it has no principal), so every caller must,
    /// and there is exactly one caller.
    async fn purge_entry(&self, company: &CompanyId, ledger: &str, entry: &str) -> Result<bool>;

    /// Removes every event on one ledger; returns whether any were removed.
    ///
    /// The other half of retiring a ledger *and* its record. A person's call,
    /// for the same reason.
    async fn purge_ledger(&self, company: &CompanyId, ledger: &str) -> Result<bool>;
}
