//! `tinyhivemind_hives::Storage` over this crate's [`HiveStore`] port.
//!
//! The Coordinator persists one bounded *state row* (memberships, delivery
//! states, session ids, conductor checkpoints) plus an append-only transcript,
//! and commits both behind a revision compare-and-swap. [`HiveStore`] is the
//! same shape with opaque JSON and a string revision, implemented by every
//! storage backend this crate selects (filesystem, SQLite, MongoDB — the last
//! with shared-single-DB tenant namespacing), so a hosted tenant's hive lives
//! in its own database exactly as its journal does.
//!
//! The mapping is mechanical:
//!
//! * the Coordinator's `u64` revision is the store's decimal string revision,
//!   with revision `0` — nothing committed — meaning "no document";
//! * the state row is `StoredState` as serde writes it (its transcript fields
//!   are `#[serde(skip)]`), next to its `next_sequence`;
//! * one `TranscriptRow` is one [`HiveMessageRow`], keyed by its message's
//!   sequence;
//! * a store `Conflict` is the Coordinator's `RevisionConflict`, which its
//!   writer absorbs by reloading and retrying.
//!
//! Loading ignores any message row at or past the state's `next_sequence`: a
//! row the store wrote ahead of a state commit that never landed is not part
//! of the committed transcript, and replaying it would duplicate the sequence
//! the next commit assigns.

use std::sync::Arc;

use tinyhivemind_hives::{
    Commit, Error as HiveError, Storage, StorageFuture, StoredState, TranscriptRow,
};

use crate::ports::hive::{HiveCommit, HiveMessageRow, HiveStateDoc, HiveStore};
use crate::ports::types::CompanyId;

/// One company's Coordinator state, kept in a [`HiveStore`].
#[derive(Clone)]
pub struct PortStorage {
    company: CompanyId,
    store: Arc<dyn HiveStore>,
}

impl std::fmt::Debug for PortStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortStorage")
            .field("company", &self.company)
            .finish_non_exhaustive()
    }
}

impl PortStorage {
    /// The storage of `company`'s hive in `store`.
    #[must_use]
    pub fn new(company: CompanyId, store: Arc<dyn HiveStore>) -> Self {
        Self { company, store }
    }
}

/// A store failure as the Coordinator's error. The library has no variant for
/// a host's own I/O, so it travels as an invalid-state error naming the cause;
/// the Coordinator treats every storage error as "nothing committed".
fn host_error(error: impl std::fmt::Display) -> HiveError {
    HiveError::InvalidState(format!("hive store: {error}"))
}

/// The store's revision string for a Coordinator revision; `None` for `0`.
#[must_use]
pub fn revision_key(revision: u64) -> Option<String> {
    (revision != 0).then(|| revision.to_string())
}

/// The Coordinator revision a store revision string names. A string this
/// adapter did not write reads as `0`, which can only conflict.
#[must_use]
pub fn revision_of(key: Option<&str>) -> u64 {
    key.and_then(|key| key.parse().ok()).unwrap_or(0)
}

impl Storage for PortStorage {
    fn load(&self) -> StorageFuture<'_, StoredState> {
        Box::pin(async move {
            let Some(snapshot) = self
                .store
                .load_hive(&self.company, None)
                .await
                .map_err(host_error)?
            else {
                return Ok(StoredState::default());
            };
            let mut state: StoredState = serde_json::from_value(snapshot.state.body)?;
            let stored = revision_of(Some(&snapshot.state.revision));
            if state.revision != stored {
                return Err(HiveError::InvalidState(format!(
                    "hive store: state body is at revision {} but the document at {stored}",
                    state.revision
                )));
            }
            let mut rows: Vec<HiveMessageRow> = snapshot
                .messages
                .into_iter()
                .filter(|row| row.sequence < state.next_sequence)
                .collect();
            rows.sort_by_key(|row| row.sequence);
            for row in rows {
                let row: TranscriptRow = serde_json::from_value(row.body)?;
                state.append(row);
            }
            Ok(state)
        })
    }

    fn commit<'a>(&'a self, commit: Commit<'a>) -> StorageFuture<'a, ()> {
        Box::pin(async move {
            let next = HiveStateDoc {
                revision: commit.state.revision.to_string(),
                next_sequence: commit.state.next_sequence,
                body: serde_json::to_value(commit.state)?,
            };
            let appended = commit
                .appended
                .iter()
                .map(|row| {
                    Ok(HiveMessageRow {
                        sequence: row.message.sequence,
                        body: serde_json::to_value(row)?,
                    })
                })
                .collect::<Result<Vec<_>, serde_json::Error>>()?;
            let expected = revision_key(commit.expected_revision);
            match self
                .store
                .commit_hive(&self.company, expected.as_deref(), next, appended)
                .await
                .map_err(host_error)?
            {
                HiveCommit::Committed => Ok(()),
                HiveCommit::Conflict { current } => Err(HiveError::RevisionConflict {
                    expected: commit.expected_revision,
                    actual: revision_of(current.as_deref()),
                }),
            }
        })
    }
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
