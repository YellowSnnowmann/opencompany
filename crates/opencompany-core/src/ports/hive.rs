//! The [`HiveStore`] port: the durable state under a company's hive coordinator.
//!
//! Hive coordination is moving onto TinyHiveMind's coordinator, whose own
//! persistence port the host implements. That adapter lands separately; this
//! port is the backend-neutral store it sits on, and it deliberately knows
//! nothing about TinyHiveMind — no type from those crates appears here, so the
//! store does not move when the library's version does.
//!
//! ## Two things, and no semantics
//!
//! Per company the store holds:
//!
//! - **one state document** ([`HiveStateDoc`]), replaced whole by a
//!   compare-and-swap on its opaque `revision`, and
//! - **an append-only message log** ([`HiveMessageRow`]), one row per
//!   `sequence`.
//!
//! Both bodies are opaque JSON. Deciding what a body means is the caller's job,
//! so a new coordinator field needs no backend change — the same split
//! [`JournalStore`](crate::ports::journal::JournalStore) makes for the runtime
//! journal.
//!
//! ## One commit is the only write
//!
//! [`HiveStore::commit_hive`] appends message rows and swaps the state document
//! in one operation. The state document records `next_sequence`, the first
//! sequence **not** yet committed; that field is what makes the pair crash-safe
//! on a backend that cannot write both at once. Rows are written first and the
//! state document last, so a crash between the two leaves rows at or above the
//! recorded `next_sequence`: [`HiveStore::load_hive`] never returns them, and
//! the next commit overwrites them. A reader therefore sees either the whole
//! commit or none of it.
//!
//! ## Conflicts are answers, not errors
//!
//! A commit whose `expected` revision is not the stored one returns
//! [`HiveCommit::Conflict`] carrying the revision that is stored, and writes
//! nothing. A malformed commit — a row outside the range the commit claims, a
//! `next_sequence` that goes backwards, a revision that does not change — is an
//! [`OpenCompanyError::InvalidRequest`], because no reload makes it valid.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::error::OpenCompanyError;
use crate::ports::types::CompanyId;

/// A company's hive state document: the coordinator's whole durable state
/// other than its message log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HiveStateDoc {
    /// Opaque revision token, minted by the committer. A commit must change it,
    /// so a stale writer's compare-and-swap can never match by accident.
    pub revision: String,
    /// The first message sequence not yet committed. Rows at or above it are
    /// uncommitted leftovers of an interrupted write and are never loaded.
    pub next_sequence: u64,
    /// The coordinator's state, opaque to the store.
    pub body: serde_json::Value,
}

/// One row of a company's hive message log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HiveMessageRow {
    /// The row's position in the log; unique per company.
    pub sequence: u64,
    /// The message, opaque to the store.
    pub body: serde_json::Value,
}

/// What [`HiveStore::load_hive`] reads: the state document and the committed
/// message rows below the requested bound, in sequence order.
#[derive(Clone, Debug, PartialEq)]
pub struct HiveSnapshot {
    /// The current state document.
    pub state: HiveStateDoc,
    /// Committed rows, ascending by sequence.
    pub messages: Vec<HiveMessageRow>,
}

/// The outcome of [`HiveStore::commit_hive`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HiveCommit {
    /// The rows and the new state document are durable.
    Committed,
    /// The stored revision was not the expected one; nothing was written.
    Conflict {
        /// The revision actually stored (`None`: the company has no state).
        current: Option<String>,
    },
}

/// Durable per-company hive coordination state: a compare-and-swap state
/// document plus an append-only message log.
#[async_trait]
pub trait HiveStore: Send + Sync {
    /// The company's state document and every committed message row with
    /// `sequence < before` (`None`: all of them), ascending by sequence.
    ///
    /// `None` when the company has no state document. Rows at or above the
    /// document's `next_sequence` are never returned, whatever the backend
    /// holds.
    async fn load_hive(
        &self,
        company: &CompanyId,
        before: Option<u64>,
    ) -> Result<Option<HiveSnapshot>>;

    /// Appends `appended` and swaps the state document from revision
    /// `expected` (`None`: no document yet) to `next`, all or nothing.
    ///
    /// Every appended row must lie in `[current next_sequence,
    /// next.next_sequence)` and the rows must ascend strictly; `next` must not
    /// lower `next_sequence` and must carry a new revision. Rewriting a row
    /// left above the stored `next_sequence` by an interrupted commit is
    /// expected, not an error.
    async fn commit_hive(
        &self,
        company: &CompanyId,
        expected: Option<&str>,
        next: HiveStateDoc,
        appended: Vec<HiveMessageRow>,
    ) -> Result<HiveCommit>;

    /// Removes the company's state document and every message row. Removing
    /// a company with no hive state is a no-op.
    async fn purge_hive(&self, company: &CompanyId) -> Result<()>;
}

/// Whether a commit may go ahead, decided against the stored document's
/// `(revision, next_sequence)` before anything is written.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommitCheck {
    /// The compare-and-swap matches and the commit is well formed.
    Proceed,
    /// The stored revision is not the expected one.
    Conflict(Option<String>),
}

/// The one compare-and-swap and validation rule every backend applies, so the
/// backends cannot drift on what a well-formed commit is.
///
/// The swap is checked first: a stale writer learns it is stale (and reloads)
/// before it is told its rows no longer fit.
pub(crate) fn check_commit(
    current: Option<(&str, u64)>,
    expected: Option<&str>,
    next: &HiveStateDoc,
    appended: &[HiveMessageRow],
) -> Result<CommitCheck> {
    let stored = current.map(|(revision, _)| revision);
    if stored != expected {
        return Ok(CommitCheck::Conflict(stored.map(str::to_owned)));
    }
    if next.revision.is_empty() {
        return Err(invalid("the new revision is empty".into()));
    }
    if stored == Some(next.revision.as_str()) {
        return Err(invalid(format!(
            "the new revision `{}` is the stored one; a commit must change it",
            next.revision
        )));
    }
    let floor = current.map_or(0, |(_, next_sequence)| next_sequence);
    if next.next_sequence < floor {
        return Err(invalid(format!(
            "next_sequence {} is below the stored {floor}",
            next.next_sequence
        )));
    }
    let mut previous: Option<u64> = None;
    for row in appended {
        if row.sequence < floor || row.sequence >= next.next_sequence {
            return Err(invalid(format!(
                "row sequence {} is outside [{floor}, {})",
                row.sequence, next.next_sequence
            )));
        }
        if previous.is_some_and(|previous| row.sequence <= previous) {
            return Err(invalid(format!(
                "row sequence {} does not ascend strictly",
                row.sequence
            )));
        }
        previous = Some(row.sequence);
    }
    Ok(CommitCheck::Proceed)
}

/// The `$lt` bound a load reads rows under: the caller's `before`, clipped to
/// the committed range.
pub(crate) fn load_bound(next_sequence: u64, before: Option<u64>) -> u64 {
    before.map_or(next_sequence, |before| before.min(next_sequence))
}

fn invalid(message: String) -> OpenCompanyError {
    OpenCompanyError::InvalidRequest(format!("hive commit: {message}"))
}

#[cfg(test)]
#[path = "hive_tests.rs"]
mod tests;
