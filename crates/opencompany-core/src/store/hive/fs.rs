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
use crate::error::OpenCompanyError;
use crate::ports::hive::{
    CommitCheck, HiveCommit, HiveMessageRow, HiveSnapshot, HiveStateDoc, HiveStore, check_commit,
    load_bound,
};
use crate::ports::types::CompanyId;
use crate::store::fs::{
    create_dir_all_durable, io_err, path_lock, read_optional, sync_parent_dir, write_atomic,
};
use crate::store::paths::Bundle;

const STATE_JSON: &str = "state.json";
const MESSAGES_JSONL: &str = "messages.jsonl";

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
        let dir = self.dir(company);
        let state_path = dir.join(STATE_JSON);
        let lock = path_lock(&state_path);
        let _guard = lock.lock().await;
        let Some(on_disk) = read_state(&state_path).await? else {
            return Ok(None);
        };
        let bound = load_bound(on_disk.state.next_sequence, before);
        let messages = read_committed(&dir.join(MESSAGES_JSONL), on_disk.messages_len)
            .await?
            .into_iter()
            .filter(|row| row.sequence < bound)
            .collect();
        Ok(Some(HiveSnapshot {
            state: on_disk.state,
            messages,
        }))
    }

    async fn commit_hive(
        &self,
        company: &CompanyId,
        expected: Option<&str>,
        next: HiveStateDoc,
        appended: Vec<HiveMessageRow>,
    ) -> Result<HiveCommit> {
        let dir = self.dir(company);
        let state_path = dir.join(STATE_JSON);
        let lock = path_lock(&state_path);
        let _guard = lock.lock().await;
        let current = read_state(&state_path).await?;
        let current_key = current
            .as_ref()
            .map(|on_disk| (on_disk.state.revision.as_str(), on_disk.state.next_sequence));
        if let CommitCheck::Conflict(current) = check_commit(current_key, expected, &next, &appended)? {
            return Ok(HiveCommit::Conflict { current });
        }
        let committed_len = current.as_ref().map_or(0, |on_disk| on_disk.messages_len);

        create_dir_all_durable(&dir).await?;
        let messages_len = if appended.is_empty() {
            committed_len
        } else {
            let mut lines = String::new();
            for row in &appended {
                lines.push_str(&serde_json::to_string(row)?);
                lines.push('\n');
            }
            append_after(dir.join(MESSAGES_JSONL), committed_len, lines).await?
        };
        let on_disk = OnDisk {
            state: next,
            messages_len,
        };
        write_atomic(&state_path, &serde_json::to_string(&on_disk)?).await?;
        Ok(HiveCommit::Committed)
    }

    /// State first, so the company reads as absent the moment the purge
    /// starts. A crash before the log goes leaves a log no state names, which
    /// the next first commit truncates to zero.
    async fn purge_hive(&self, company: &CompanyId) -> Result<()> {
        let dir = self.dir(company);
        let state_path = dir.join(STATE_JSON);
        let lock = path_lock(&state_path);
        let _guard = lock.lock().await;
        for path in [state_path.clone(), dir.join(MESSAGES_JSONL)] {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_err(&path, error)),
            }
        }
        Ok(())
    }
}

/// Reads `state.json`, `None` when the company has none.
async fn read_state(path: &Path) -> Result<Option<OnDisk>> {
    let contents = read_optional(path).await?;
    if contents.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&contents)?))
}

/// Parses the first `len` bytes of the log — the committed prefix, and only
/// that. Strict: a line inside the committed prefix that does not parse is
/// damage, and skipping it would silently drop a committed message.
async fn read_committed(path: &Path, len: u64) -> Result<Vec<HiveMessageRow>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(io_err(path, error)),
    };
    let len = usize::try_from(len).unwrap_or(usize::MAX);
    if bytes.len() < len {
        return Err(OpenCompanyError::Store(format!(
            "hive message log {} is shorter ({} bytes) than its committed length ({len})",
            path.display(),
            bytes.len()
        )));
    }
    bytes[..len]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(Into::into))
        .collect()
}

/// Cuts the log back to `committed_len` — dropping whatever an interrupted
/// commit left past it — writes `lines` in one `write_all`, and flushes them
/// before returning the new committed length.
///
/// The flush comes before the caller's state rename: the rename is what makes
/// these bytes committed, so they must be on stable storage first.
async fn append_after(path: PathBuf, committed_len: u64, lines: String) -> Result<u64> {
    tokio::task::spawn_blocking(move || {
        use std::io::{Seek, SeekFrom, Write};

        let creating = !path.exists();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| io_err(&path, error))?;
        let on_disk = file.metadata().map_err(|error| io_err(&path, error))?.len();
        if on_disk < committed_len {
            return Err(OpenCompanyError::Store(format!(
                "hive message log {} is shorter ({on_disk} bytes) than its committed length \
                 ({committed_len})",
                path.display()
            )));
        }
        file.set_len(committed_len)
            .map_err(|error| io_err(&path, error))?;
        file.seek(SeekFrom::Start(committed_len))
            .map_err(|error| io_err(&path, error))?;
        file.write_all(lines.as_bytes())
            .map_err(|error| io_err(&path, error))?;
        file.sync_data().map_err(|error| io_err(&path, error))?;
        if creating {
            sync_parent_dir(&path)?;
        }
        Ok(committed_len + lines.len() as u64)
    })
    .await
    .map_err(|error| OpenCompanyError::Store(format!("spawn_blocking failed: {error}")))?
}

#[cfg(test)]
#[path = "fs_tests.rs"]
mod tests;
