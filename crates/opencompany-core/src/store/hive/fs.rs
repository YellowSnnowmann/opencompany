//! [`FsHiveStore`]: the filesystem [`HiveStore`], under each company bundle's
//! `hive/` directory.
//!
//! - `hive/state.json` — the state document plus `messagesLen`, the byte length
//!   of the committed prefix of the log. Replaced by atomic, durable rename.
//! - `hive/messages.jsonl` — one [`HiveMessageRow`] per line.
//!
//! A commit truncates the log to the committed length, appends its rows in one
//! write, flushes them (`sync_data`), and only then renames the new state into
//! place. A crash anywhere before the rename leaves the old state naming the old
//! length, so whatever the interrupted commit wrote — whole lines or a torn one
//! — sits past `messagesLen`, is never read, and is cut off by the next commit.
//!
//! Commits and loads for one company serialize on the store's process-wide path
//! lock (`path_lock`), the same in-process guarantee every fs store gives.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::ports::hive::{HiveCommit, HiveMessageRow, HiveSnapshot, HiveStateDoc, HiveStore};
use crate::ports::types::CompanyId;
use crate::store::paths::Bundle;

/// The filesystem [`HiveStore`]: `<company bundle>/hive/{state.json,
/// messages.jsonl}` under one OpenCompany home.
#[derive(Clone, Debug)]
pub struct FsHiveStore {
    root: PathBuf,
}

/// `state.json` on disk: the port's document plus the committed log length.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnDisk {
    #[serde(flatten)]
    state: HiveStateDoc,
    messages_len: u64,
}

impl FsHiveStore {
    /// A store rooted at `root`, the OpenCompany home the bundles live under.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dir(&self, company: &CompanyId) -> PathBuf {
        Bundle::new(self.root.clone(), company).dir().join("hive")
    }
}

#[async_trait]
impl HiveStore for FsHiveStore {
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

#[allow(dead_code)]
fn unused(_: &Path) {}

#[cfg(test)]
#[path = "fs_tests.rs"]
mod tests;
