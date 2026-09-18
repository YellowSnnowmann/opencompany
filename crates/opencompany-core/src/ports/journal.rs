//! The [`JournalStore`] port: the durable sink under the runtime journal
//! (issue #726).
//!
//! [`RuntimeJournal`](crate::runtime::journal::RuntimeJournal) holds the
//! at-most-once effect set, the parked-approval queue, single-use and standing
//! grants, and the cycle brackets. Until this port existed it wrote all of that
//! to a `journal.jsonl` inside the company bundle **unconditionally** — outside
//! the port surface [`StorageHandles`](crate::store::StorageHandles) swaps for
//! every other durable store. On a hosted tenant with
//! `OPENCOMPANY_STORAGE=mongodb` the container's `/data` is documented ephemeral
//! scratch (see `docs/spec/runtime/storage.md`), so container replacement — a
//! deploy, a reschedule, a node drain, an OOM kill — discarded the file: every
//! previously executed effect became eligible to fire again, and every parked
//! approval and grant silently vanished.
//!
//! ## Two byte-level operations, and no semantics
//!
//! This trait is deliberately *not* a mirror of `RuntimeJournal`'s ~25 methods.
//! The journal's entire persistence contract is: append one opaque line, and
//! read every line back in the order they were appended. Everything semantic —
//! the record enum with its `#[serde(default)]` archaeology, the corrupt-line
//! skip, the merged-line recovery, the in-memory replay state — stays in
//! `RuntimeJournal` and is therefore backend-agnostic by construction. A
//! backend stores strings; it never learns what a `JournalRecord` is, and a new
//! record variant needs no backend change.
//!
//! ## Why not ride [`EventLog`](crate::ports::EventLog)
//!
//! [`CompanyEvent`](crate::ports::CompanyEvent) is a closed, binding enum with
//! no marker variants, which is the reason the journal exists as a separate log
//! in the first place. And the event log is *pruned* under a retention policy —
//! rotating away an `EffectExecuted` key would silently un-commit it and let an
//! at-most-once effect fire a second time. The journal is append-only with no
//! retention, and those two contracts cannot share one log.
//!
//! ## Migration is a one-time, receipt-gated, verbatim import
//!
//! The fs implementation *is* today's file at today's path, so the default
//! backend migrates nothing. For sqlite and mongodb the builder copies an
//! existing `journal.jsonl` in **file order, verbatim** (raw strings — a corrupt
//! or merged line migrates byte-for-byte, so `RuntimeJournal`'s own recovery
//! still applies to it), then writes a receipt.
//!
//! The receipt is what makes a crash mid-import safe:
//! [`complete_import`](JournalStore::complete_import) clears whatever a previous
//! attempt wrote before re-copying, and only then records the receipt — so an
//! interrupted import re-runs the whole wipe-and-copy instead of replaying a
//! truncated prefix. A truncated prefix is precisely the bug this port exists to
//! fix: it would drop at-most-once keys.
//!
//! The source file is left in place. The receipt makes a second import
//! impossible, and a rollback to an older binary still finds the history it
//! knows how to read — where renaming the file would hand that binary an
//! *empty* at-most-once set.

use async_trait::async_trait;

use crate::Result;
use crate::ports::types::CompanyId;

/// The failure a journal record is written to outlast (issue #392).
///
/// The journal's own per-record decision — which record kind needs which level,
/// and why — lives on `JournalRecord::durability` in
/// [`crate::runtime::journal`]. This is only the vocabulary the decision is
/// expressed in, and it lives here because the sink is what has to honour it.
///
/// A backend MUST NOT flatten the two into one level. Flattening upward taxes
/// the journal's highest-volume records with a flush they do not need;
/// flattening downward silently drops the guarantee that keeps an already-fired
/// effect from firing again after a power loss, which is the one thing the split
/// was bought for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Durability {
    /// Durable against the process dying, not against the machine dying: in the
    /// kernel's page cache (or the server's memory) when the append returns.
    Process,
    /// On stable storage when the append returns, at the cost of one flush.
    Host,
}

/// The durable byte sink under one company's runtime journal.
#[async_trait]
pub trait JournalStore: Send + Sync {
    /// Appends one opaque record line, durable to `durability` before this
    /// returns.
    ///
    /// The at-most-once guarantee is that an effect's key reaches durable
    /// storage *before* the side effect runs, so a backend that acknowledges a
    /// write it has not yet made durable to the requested level breaks the
    /// contract this port carries. What each shipped backend does:
    ///
    /// | | [`Durability::Process`] | [`Durability::Host`] |
    /// |---|---|---|
    /// | fs | one `O_APPEND` `write_all`, awaited | the same, plus `sync_data`, and the parent chain created with each new directory's parent flushed |
    /// | sqlite | commit under `synchronous=NORMAL` (WAL) | commit under `synchronous=FULL` |
    /// | mongodb | acknowledged insert | insert with `j:true` write concern |
    ///
    /// The fs backend's directory handling is not incidental: a record flushed
    /// under ancestors that were never written down is lost with them, so a
    /// [`Durability::Host`] append into a fresh data directory must flush the
    /// chain it creates (issue #392).
    ///
    /// `line` never contains a newline — the caller serialises one JSON record
    /// per call — and a backend must preserve it byte-for-byte.
    ///
    /// Errors are fail-closed: an `Err` reaches the caller *before* the side
    /// effect, so the effect does not run. The residual ambiguity (a timeout on
    /// a write the server did commit) leaves a committed key with no effect,
    /// which is the at-most-once contract's documented safe direction.
    async fn append_journal(
        &self,
        id: &CompanyId,
        line: &str,
        durability: Durability,
    ) -> Result<()>;

    /// Every line ever appended for `id`, in append order.
    ///
    /// Order is load-bearing rather than cosmetic: replay folds records in
    /// sequence, so a park read back after the resolution that drains it would
    /// resurrect a resolved approval.
    ///
    /// Damaged lines are returned as they are stored, not filtered — deciding
    /// what a line means is the journal's job, and a backend that silently
    /// dropped one would be un-committing an effect key on the caller's behalf.
    async fn read_journal(&self, id: &CompanyId) -> Result<Vec<String>>;

    /// Whether this backend has already taken (or does not need) a one-time
    /// import of a pre-existing filesystem journal.
    ///
    /// `true` closes the gate forever: the builder never imports again, so a
    /// `journal.jsonl` reappearing later — a rollback, a stray copy into the
    /// data dir — cannot wipe and replace the backend's own history.
    ///
    /// The fs backend answers `true` unconditionally: its store *is* the file,
    /// so there is nothing to import and [`complete_import`](Self::complete_import)
    /// is unreachable for it.
    async fn journal_imported(&self, id: &CompanyId) -> Result<bool>;

    /// Replaces this company's journal with `lines` and records the import
    /// receipt.
    ///
    /// Clear-then-copy-then-receipt, in that order, and the order is the whole
    /// safety argument — see the module docs. Implementations do it atomically
    /// where the backend allows (sqlite: one transaction); where it does not
    /// (mongodb), the receipt landing last still makes an interrupted attempt
    /// re-run from the top rather than leave a truncated journal behind a
    /// closed gate.
    ///
    /// An empty `lines` is a legitimate call, not a no-op to optimise away: it
    /// is how a company with no prior filesystem journal closes its gate.
    async fn complete_import(&self, id: &CompanyId, lines: Vec<String>) -> Result<()>;
}

/// An in-memory [`JournalStore`], for tests that need a durable sink which is
/// **not** the filesystem.
///
/// It stands in for a database backend in the one place that matters: a test can
/// delete a company's bundle directory — the simulated container replacement
/// this port exists to survive — and this store still holds every record. Doing
/// that against the real sqlite or mongodb backend would tie the proof to a
/// cargo feature (and, for mongodb, to a live server), so the invariant would go
/// unasserted in the default build.
///
/// Deliberately holds the same *strings* the real backends hold, so a record
/// crossing it is exercised through the identical serialise/parse path.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryJournalStore {
    inner: std::sync::Mutex<MemoryJournalState>,
}

#[cfg(test)]
#[derive(Default)]
struct MemoryJournalState {
    lines: std::collections::HashMap<CompanyId, Vec<String>>,
    imported: std::collections::HashSet<CompanyId>,
}

#[cfg(test)]
impl MemoryJournalStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryJournalState> {
        self.inner.lock().expect("memory journal poisoned")
    }

    /// Drops the import receipt for `id`, so the next boot imports again.
    ///
    /// The shape of a crash *between* the copy and the receipt: a partial
    /// journal sitting behind an open gate. Tests use it to prove the retry
    /// wipes rather than appends.
    pub fn forget_receipt(&self, id: &CompanyId) {
        self.lock().imported.remove(id);
    }
}

#[cfg(test)]
#[async_trait]
impl JournalStore for MemoryJournalStore {
    async fn append_journal(
        &self,
        id: &CompanyId,
        line: &str,
        _durability: Durability,
    ) -> Result<()> {
        self.lock()
            .lines
            .entry(id.clone())
            .or_default()
            .push(line.to_string());
        Ok(())
    }

    async fn read_journal(&self, id: &CompanyId) -> Result<Vec<String>> {
        Ok(self.lock().lines.get(id).cloned().unwrap_or_default())
    }

    async fn journal_imported(&self, id: &CompanyId) -> Result<bool> {
        Ok(self.lock().imported.contains(id))
    }

    async fn complete_import(&self, id: &CompanyId, lines: Vec<String>) -> Result<()> {
        let mut state = self.lock();
        state.lines.insert(id.clone(), lines);
        state.imported.insert(id.clone());
        Ok(())
    }
}
