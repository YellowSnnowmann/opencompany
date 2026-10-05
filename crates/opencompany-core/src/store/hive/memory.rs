//! [`MemoryHiveStore`]: the in-process [`HiveStore`], for tests and for a
//! coordinator adapter exercised without a backend.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::Result;
use crate::ports::hive::{
    CommitCheck, HiveCommit, HiveMessageRow, HiveSnapshot, HiveStateDoc, HiveStore, check_commit,
    load_bound,
};
use crate::ports::types::CompanyId;

/// An in-memory [`HiveStore`]: one state document and one ordered row map per
/// company, behind a single mutex so a commit is trivially all-or-nothing.
///
/// Holds the same opaque JSON the durable backends hold, so a caller tested
/// against it crosses the identical contract — it is held to the same
/// conformance suite.
#[derive(Default)]
pub struct MemoryHiveStore {
    companies: Mutex<HashMap<CompanyId, Company>>,
}

#[derive(Default)]
struct Company {
    state: Option<HiveStateDoc>,
    messages: BTreeMap<u64, HiveMessageRow>,
}

impl MemoryHiveStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<CompanyId, Company>> {
        self.companies.lock().expect("memory hive store poisoned")
    }
}

#[async_trait]
impl HiveStore for MemoryHiveStore {
    async fn load_hive(
        &self,
        company: &CompanyId,
        before: Option<u64>,
    ) -> Result<Option<HiveSnapshot>> {
        let companies = self.lock();
        let Some(entry) = companies.get(company) else {
            return Ok(None);
        };
        let Some(state) = entry.state.clone() else {
            return Ok(None);
        };
        let bound = load_bound(state.next_sequence, before);
        let messages = entry
            .messages
            .range(..bound)
            .map(|(_, row)| row.clone())
            .collect();
        Ok(Some(HiveSnapshot { state, messages }))
    }

    async fn commit_hive(
        &self,
        company: &CompanyId,
        expected: Option<&str>,
        next: HiveStateDoc,
        appended: Vec<HiveMessageRow>,
    ) -> Result<HiveCommit> {
        let mut companies = self.lock();
        let entry = companies.entry(company.clone()).or_default();
        let current = entry
            .state
            .as_ref()
            .map(|state| (state.revision.as_str(), state.next_sequence));
        if let CommitCheck::Conflict(current) = check_commit(current, expected, &next, &appended)? {
            return Ok(HiveCommit::Conflict { current });
        }
        for row in appended {
            entry.messages.insert(row.sequence, row);
        }
        entry.state = Some(next);
        Ok(HiveCommit::Committed)
    }

    async fn purge_hive(&self, company: &CompanyId) -> Result<()> {
        self.lock().remove(company);
        Ok(())
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
