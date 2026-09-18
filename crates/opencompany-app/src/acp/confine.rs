//! Keeping a harness inside the directory it was given.
//!
//! An ACP agent asks its *client* to read and write files — `fs/read_text_file`
//! and `fs/write_text_file` are client methods, served here. So the desktop is
//! the thing standing between a model's idea of a path and the operator's disk,
//! and this module is that boundary.
//!
//! ## Why it is in Rust and not in the webview
//!
//! The console renders the permission prompt, but it must never be the thing
//! that *enforces* the answer. A renderer decides what a person sees; a
//! compromised or merely buggy one would then decide what a model can read. The
//! check lives here, below the UI, so that a path escaping the session
//! directory is refused whether or not anything was rendered.
//!
//! ## Why canonicalisation is not optional
//!
//! `starts_with` on a raw path is the classic wrong answer. `/repo/../etc/passwd`
//! passes it. So does a symlink at `/repo/link` pointing anywhere. Both are
//! ordinary things to find in a working tree, and the second is not even
//! hostile — `node_modules` and build caches are full of links.
//!
//! So the root is canonicalised once, the target is canonicalised, and the
//! comparison happens between two resolved paths. A file that does not exist yet
//! — the common case for a write — has its *parent* resolved instead, because
//! there is nothing yet to resolve and the parent is what decides where the new
//! file lands.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfineError {
    #[error("the path is not absolute; ACP requires absolute paths")]
    NotAbsolute,
    #[error("the session root does not exist or cannot be resolved")]
    UnusableRoot,
    #[error("the parent directory does not exist")]
    NoParent,
    #[error("that path is outside the session's directory")]
    Escapes,
    #[error("that path is a directory, not a file")]
    IsDirectory,
}

/// A directory a harness session is allowed to touch, and nothing else.
#[derive(Clone, Debug)]
pub struct Confinement {
    /// Canonical, so every comparison is between resolved paths.
    root: PathBuf,
}

impl Confinement {
    /// Confines to `root`, resolving it once.
    ///
    /// Resolved at construction rather than per check: a root that is itself
    /// behind a symlink would otherwise compare unequal to every canonical
    /// target under it, and refuse everything — a failure that looks like the
    /// boundary working.
    pub fn new(root: &Path) -> Result<Self, ConfineError> {
        let root = root
            .canonicalize()
            .map_err(|_| ConfineError::UnusableRoot)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves `path` for reading, or refuses it.
    pub fn resolve_read(&self, path: &Path) -> Result<PathBuf, ConfineError> {
        self.require_absolute(path)?;
        let resolved = path.canonicalize().map_err(|_| ConfineError::Escapes)?;
        self.require_inside(&resolved)?;
        Ok(resolved)
    }

    /// Resolves `path` for writing, or refuses it.
    ///
    /// The file need not exist — that is the normal case for a write — so the
    /// **parent** is what gets resolved. A parent that is a symlink out of the
    /// tree is caught by exactly the same comparison, which is the point: a
    /// write to `<root>/link/escape.txt` where `link` leaves the root must be
    /// refused, and it is only visible after resolution.
    ///
    /// ## The dangling link
    ///
    /// Resolving the parent is not sufficient on its own, because a *dangling*
    /// symlink does not canonicalize. `<root>/link -> /outside/planted.txt`
    /// with no `planted.txt` yet fails `path.canonicalize()`, takes the
    /// does-not-exist-yet branch, and its parent is the root — so every check
    /// passes and the returned path is `<root>/link`. `fs::write` then follows
    /// the link and creates the file outside the root, which is the escape this
    /// module exists to refuse. The final component is therefore checked with
    /// `symlink_metadata`, which does not follow.
    ///
    /// What remains, and is not closeable here: a link planted between this
    /// resolution and the caller's `open`. Closing that needs `O_NOFOLLOW` on
    /// the write itself, which is the caller's syscall to make.
    pub fn resolve_write(&self, path: &Path) -> Result<PathBuf, ConfineError> {
        self.require_absolute(path)?;
        if let Ok(resolved) = path.canonicalize() {
            // Already exists: resolve it directly, so an existing symlink is
            // followed before the check rather than after.
            self.require_inside(&resolved)?;
            // A directory is never what a write means, and a path ending in
            // `..` resolves to one. Answering `Ok` would hand the caller a
            // target whose write fails at the OS with a much less obvious
            // message than this one.
            if resolved.is_dir() {
                return Err(ConfineError::IsDirectory);
            }
            return Ok(resolved);
        }

        let parent = path.parent().ok_or(ConfineError::NoParent)?;
        let name = path.file_name().ok_or(ConfineError::NoParent)?;
        let parent = parent.canonicalize().map_err(|_| ConfineError::NoParent)?;
        self.require_inside(&parent)?;

        // The final component reached here because it did not canonicalize.
        // That is usually "no such file", which is the ordinary case for a
        // write — but it is also what a dangling symlink looks like, and a
        // write through one lands wherever it points. `symlink_metadata` is the
        // one stat that does not follow, so it can tell the two apart.
        let target = parent.join(name);
        if std::fs::symlink_metadata(&target).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(ConfineError::Escapes);
        }
        Ok(target)
    }

    fn require_absolute(&self, path: &Path) -> Result<(), ConfineError> {
        // ACP mandates absolute paths. Accepting a relative one would silently
        // resolve it against this *process's* working directory, which has
        // nothing to do with the session.
        if path.is_absolute() {
            Ok(())
        } else {
            Err(ConfineError::NotAbsolute)
        }
    }

    fn require_inside(&self, resolved: &Path) -> Result<(), ConfineError> {
        // Component-wise, not string-prefix: `/repo-secrets` has `/repo` as a
        // string prefix but is a different directory, and `starts_with` on
        // `Path` compares whole components.
        if resolved.starts_with(&self.root) {
            Ok(())
        } else {
            Err(ConfineError::Escapes)
        }
    }
}

#[cfg(test)]
#[path = "confine_tests.rs"]
mod tests;
