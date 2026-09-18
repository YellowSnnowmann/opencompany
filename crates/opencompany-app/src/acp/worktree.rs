//! Giving each dispatched task its own checkout.
//!
//! ## Why not one shared directory
//!
//! block/buzz points every local agent at one `REPOS` directory and relies on
//! there being effectively one live workspace at a time. That does not survive
//! this product's headline requirement: N connected hosts means N companies can
//! dispatch work to this machine at once, and two agents editing one tree is a
//! data race with no attribution — whoever wrote last wins, and neither task's
//! diff means anything afterwards.
//!
//! It also makes `parallelism` a lie. Configuring three concurrent workers is
//! pointless if the filesystem caps you at one.
//!
//! A worktree per task fixes all three: concurrent tasks cannot collide, each
//! has a branch and therefore a reviewable diff, and cancelling one is a
//! discard rather than an unpick.
//!
//! ## What is deliberately not automatic
//!
//! **Nothing with uncommitted changes is ever removed.** A task that failed
//! half way has left the only copy of whatever it did, and reclaiming disk is
//! never worth destroying that. `release` refuses, says so, and leaves the
//! directory for a person.
//!
//! A target that is not a git repository is **not** an error either. Plenty of
//! real work happens outside version control, and refusing it would make the
//! feature unavailable exactly where an operator is most likely to be
//! experimenting. Such a task gets a plain directory and no isolation
//! guarantee, which is reported rather than hidden.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git failed: {0}")]
    Git(String),
    #[error("could not prepare {path}: {reason}")]
    Io { path: PathBuf, reason: String },
    #[error("{path} has uncommitted changes and was left alone")]
    Dirty { path: PathBuf },
}

/// How a task's directory was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Isolation {
    /// A real git worktree on its own branch. Concurrent tasks cannot collide.
    Worktree,
    /// A plain directory, because the target is not a git repository. Usable,
    /// but two tasks pointed at it would interfere — reported so the console
    /// can say so rather than implying a guarantee that is not there.
    PlainDirectory,
}

/// A directory a task works in.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWorkspace {
    pub path: PathBuf,
    pub isolation: Isolation,
    /// The branch created for this task, when it is a worktree.
    pub branch: Option<String>,
}

/// Prepares a workspace for `task_id` from `repo`.
///
/// `root` is where worktrees are kept — one directory per (connection, company,
/// task), so two hosts dispatching a task of the same name cannot land in one
/// place.
pub async fn acquire(
    root: &Path,
    repo: &Path,
    task_id: &str,
) -> Result<TaskWorkspace, WorktreeError> {
    let path = root.join(task_id);
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|e| WorktreeError::Io {
            path: root.to_path_buf(),
            reason: e.to_string(),
        })?;

    if !is_git_repo(repo).await {
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| WorktreeError::Io {
                path: path.clone(),
                reason: e.to_string(),
            })?;
        return Ok(TaskWorkspace {
            path,
            isolation: Isolation::PlainDirectory,
            branch: None,
        });
    }

    let branch = format!("oc/{task_id}");
    // `-b` creates the branch, so two tasks never share one. A worktree that
    // already exists (a retried task) is reused rather than failing the run.
    if path.exists() {
        return Ok(TaskWorkspace {
            path,
            isolation: Isolation::Worktree,
            branch: Some(branch),
        });
    }
    git(
        repo,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &path.display().to_string(),
        ],
    )
    .await?;

    Ok(TaskWorkspace {
        path,
        isolation: Isolation::Worktree,
        branch: Some(branch),
    })
}

/// Removes a task's workspace, **unless it holds uncommitted work**.
///
/// The refusal is the point. Reclaiming disk is never worth destroying the only
/// copy of what a failed task produced.
pub async fn release(repo: &Path, workspace: &TaskWorkspace) -> Result<(), WorktreeError> {
    if workspace.isolation == Isolation::PlainDirectory {
        // Never removed: nothing here tracked what was in it, so nothing here
        // can know whether it mattered.
        return Ok(());
    }
    if is_dirty(&workspace.path).await {
        return Err(WorktreeError::Dirty {
            path: workspace.path.clone(),
        });
    }
    git(
        repo,
        &["worktree", "remove", &workspace.path.display().to_string()],
    )
    .await?;
    Ok(())
}

async fn is_git_repo(path: &Path) -> bool {
    matches!(
        git(path, &["rev-parse", "--is-inside-work-tree"]).await,
        Ok(out) if out.trim() == "true"
    )
}

/// Whether a worktree holds changes nothing has recorded.
///
/// `--porcelain` covers untracked files too, which matters: a task whose whole
/// output is a new file has produced exactly one untracked entry and nothing
/// else, and treating that as clean would delete its only result.
async fn is_dirty(path: &Path) -> bool {
    match git(path, &["status", "--porcelain"]).await {
        Ok(out) => !out.trim().is_empty(),
        // Unreadable: assume dirty. The safe direction — refusing to remove
        // something we cannot inspect beats removing it.
        Err(_) => true,
    }
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| WorktreeError::Git(e.to_string()))?;
    if !output.status.success() {
        return Err(WorktreeError::Git(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
