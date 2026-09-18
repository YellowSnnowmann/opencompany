//! Canonical per-instance workspace layout under the data directory.
//!
//! `OPENCOMPANY_DATA_DIR` (the workspace root — `/data` in a hosted tenant
//! container, `$HOME/.opencompany` by default) holds everything a running
//! instance owns. [`DataLayout`] names the canonical subdirectories so stores,
//! agents, and tools resolve well-known locations instead of ad-hoc paths, and
//! owns their lifecycle: [`ensure`](DataLayout::ensure) creates them on boot and
//! — when asked (`[workspace].clear_tmp_on_startup`, on by default) — clears the
//! ephemeral `tmp/` scratch so none survives a restart.
//!
//! Per-company bundles live under [`companies_dir`](DataLayout::companies_dir)
//! (`companies/<slug>/`), each carrying its own `memory/`/`context/`. The
//! top-level [`memory_dir`](DataLayout::memory_dir) and friends are therefore
//! the *instance-shared* locations, distinct from per-company state, and are
//! created empty as the reserved home for shared artifacts.

use std::path::{Path, PathBuf};

use crate::Result;
use crate::error::OpenCompanyError;

/// The canonical directory layout under one instance's data root.
#[derive(Clone, Debug)]
pub struct DataLayout {
    root: PathBuf,
}

impl DataLayout {
    /// Roots a layout at `root` (the resolved `OPENCOMPANY_DATA_DIR`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The workspace root (the data directory itself).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Per-company bundle directories (`companies/<slug>/`). Owned by the fs
    /// store, which creates each company's bundle lazily; listed here so callers
    /// resolve it through the layout rather than a literal.
    pub fn companies_dir(&self) -> PathBuf {
        self.root.join("companies")
    }

    /// One company's repository mirror cache (`companies/<slug>/repos/`).
    ///
    /// **Owned by the repo cache, not by the fs bundle.** It shares the
    /// `companies/<slug>/` prefix with [`companies_dir`](Self::companies_dir)
    /// so a company's whole footprint stays in one subtree, but nothing in the
    /// fs store creates or reads it — and on a mongodb tenant the bundle
    /// directory has no other reason to exist at all, so the cache must create
    /// its own parents rather than assume a bundle put them there.
    ///
    /// It is deliberately **outside** any agent workspace
    /// (`harness/<company>/<agent>/workspace`): the mirrors are fetched
    /// host-side with a credential, and an agent that could write to them could
    /// rewrite what every later checkout sees.
    ///
    /// Nothing extra is needed to keep it inside the soft quota:
    /// [`usage_bytes`](Self::usage_bytes) already sums every regular file under
    /// the root, and this hangs off the root like everything else.
    pub fn company_repos_dir(&self, slug: &str) -> PathBuf {
        self.companies_dir().join(slug).join("repos")
    }

    /// One agent's shell audit sink (`companies/<slug>/audit/<agent_id>/`).
    ///
    /// **Host-owned, not part of the fs bundle**, exactly like
    /// [`company_repos_dir`](Self::company_repos_dir): it shares the
    /// `companies/<slug>/` prefix so a company's whole footprint stays in one
    /// subtree, but nothing in the fs store creates or reads it — and on a
    /// mongodb tenant the bundle directory has no other reason to exist, so
    /// whoever opens the sink must create its own parents rather than assume a
    /// bundle put them there.
    ///
    /// It is deliberately **outside** any agent workspace
    /// (`harness/<company>/<agent>/workspace`), which is also the
    /// `workspace_only` `SecurityPolicy` root the file tools enforce. While the
    /// sink lived inside that root, rewriting the record of an agent's own
    /// commands was a *policy-permitted* write through its ordinary file tools,
    /// not merely something `shell` could reach (issue #775). Moving it here
    /// turns those writes from permitted into refused.
    ///
    /// **One directory per agent, not one shared directory per company.**
    /// OpenHuman's `get_or_create_workspace_audit_logger` caches one logger per
    /// *directory* and the first caller's config wins, so a shared directory
    /// with per-agent file names would silently hand the second agent the first
    /// agent's log file.
    ///
    /// This is not tamper-evidence. Everything in the tenant is one uid and one
    /// process, so a deliberate shell command against this path still succeeds;
    /// see `docs/spec/security/agent-isolation.md`.
    ///
    /// Nothing extra is needed to keep it inside the soft quota:
    /// [`usage_bytes`](Self::usage_bytes) already sums every regular file under
    /// the root, and this hangs off the root like everything else.
    pub fn agent_audit_dir(&self, slug: &str, agent_id: &str) -> PathBuf {
        self.companies_dir().join(slug).join("audit").join(agent_id)
    }

    /// Instance-shared memory artifacts.
    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }

    /// Instance-shared durable-store artifacts.
    pub fn store_dir(&self) -> PathBuf {
        self.root.join("store")
    }

    /// Instance-shared file artifacts (exports, attachments).
    pub fn files_dir(&self) -> PathBuf {
        self.root.join("files")
    }

    /// Instance logs.
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Ephemeral scratch, cleared on startup.
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The canonical shared subdirectories, in creation order.
    fn shared_dirs(&self) -> [PathBuf; 5] {
        [
            self.memory_dir(),
            self.store_dir(),
            self.files_dir(),
            self.logs_dir(),
            self.tmp_dir(),
        ]
    }

    /// Materializes the layout: clears the ephemeral `tmp/` scratch (when
    /// `clear_tmp`) so nothing stale survives a restart, then creates every
    /// canonical shared subdirectory. Idempotent — existing directories are
    /// left in place.
    ///
    /// The per-company `companies/` tree is intentionally not pre-created: the
    /// fs store owns it and mints each bundle on demand.
    pub async fn ensure(&self, clear_tmp: bool) -> Result<()> {
        if clear_tmp {
            match tokio::fs::remove_dir_all(self.tmp_dir()).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(OpenCompanyError::Store(format!(
                        "clearing tmp {}: {e}",
                        self.tmp_dir().display()
                    )));
                }
            }
        }
        for dir in self.shared_dirs() {
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| OpenCompanyError::Store(format!("creating {}: {e}", dir.display())))?;
        }
        Ok(())
    }

    /// Total size in bytes of everything under the workspace root, for the
    /// soft-quota check. Used by `serve` to alert when a workspace exceeds its
    /// configured `[workspace].storage_quota_gb`.
    pub async fn usage_bytes(&self) -> Result<u64> {
        dir_bytes(self.root.clone()).await
    }

    /// Size in bytes of the ephemeral `tmp/` scratch directory.
    pub async fn tmp_bytes(&self) -> Result<u64> {
        dir_bytes(self.tmp_dir()).await
    }
}

/// Recursively sums the byte size of regular files under `dir`. A missing
/// directory is `0`, not an error. Symlinks are not followed (an iterative
/// stack walk, so no recursion depth limit and no symlink loops).
async fn dir_bytes(dir: PathBuf) -> Result<u64> {
    let read_err = |p: &Path, e: std::io::Error| {
        OpenCompanyError::Store(format!("measuring {}: {e}", p.display()))
    };
    let mut total = 0u64;
    let mut stack = vec![dir];
    while let Some(path) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&path).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(read_err(&path, e)),
        };
        while let Some(entry) = entries.next_entry().await.map_err(|e| read_err(&path, e))? {
            // `DirEntry::metadata` does not follow symlinks, so a symlink is
            // neither dir nor file here and is simply skipped.
            let meta = match entry.metadata().await {
                Ok(meta) => meta,
                // A file removed mid-walk (e.g. tmp churn) just isn't counted.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(read_err(&entry.path(), e)),
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
