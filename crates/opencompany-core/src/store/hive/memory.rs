//! [`MemoryHiveStore`]: the in-process [`HiveStore`], for tests and for a
//! coordinator adapter exercised without a backend.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::Result;
use crate::ports::hive::{HiveCommit, HiveMessageRow, HiveSnapshot, HiveStateDoc, HiveStore};
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
}

#[async_trait]
impl HiveStore for MemoryHiveStore {
    async fn load_hive(
        &self,
        company: &CompanyId,
        before: Option<u64>,
    ) -> Result<Option<HiveSnapshot>> {
        let _ = (company, before);
        todo!("load_hive")
    }

    async fn commit_hive(
        &self,
        company: &CompanyId,
        expected: Option<&str>,
        next: HiveStateDoc,
        appended: Vec<HiveMessageRow>,
    ) -> Result<HiveCommit> {
        let _ = (company, expected, next, appended);
        todo!("commit_hive")
    }

    async fn purge_hive(&self, company: &CompanyId) -> Result<()> {
        let _ = company;
        todo!("purge_hive")
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
