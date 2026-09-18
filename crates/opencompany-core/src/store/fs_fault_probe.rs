use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

static FAIL_NEXT: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn key(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Arms a one-shot failure for the next [`write_atomic_bytes`] call
/// targeting `path`.
pub(crate) fn fail_next_write(path: &Path) {
    FAIL_NEXT
        .lock()
        .expect("fault-probe poisoned")
        .insert(key(path));
}

/// Consumes the armed failure for `path`, if any. One-shot so a retry
/// after the injected failure exercises the real write.
pub(crate) fn should_fail(path: &Path) -> bool {
    FAIL_NEXT
        .lock()
        .expect("fault-probe poisoned")
        .remove(&key(path))
}

static FAIL_MID_WRITE: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Arms a one-shot failure for the next [`stage_atomic_bytes`]'s
/// `File::create` to *succeed* and the write that follows it to fail
/// (issue #1828 review, seventh round). Unlike [`fail_next_write`],
/// which fails before any filesystem call, this simulates the failure
/// mode that actually leaves a temp file behind: the create succeeded,
/// so a `.tmp-*` file already exists, and only the subsequent
/// `write_all`/`sync_data` fails.
pub(crate) fn fail_next_mid_write(path: &Path) {
    FAIL_MID_WRITE
        .lock()
        .expect("fault-probe poisoned")
        .insert(key(path));
}

/// Consumes the armed mid-write failure for `path`, if any.
pub(crate) fn should_fail_mid_write(path: &Path) -> bool {
    FAIL_MID_WRITE
        .lock()
        .expect("fault-probe poisoned")
        .remove(&key(path))
}

static FAIL_NEXT_DIR_SYNC: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Arms a one-shot failure for the parent-directory fsync that follows a
/// successful rename in [`commit_staged`] (issue #1828 review, tenth
/// round). Unlike [`fail_next_commit`], which fails before the rename,
/// this reaches the state where the destination is *already replaced* on
/// disk and only the durability step failed.
pub(crate) fn fail_next_dir_sync(path: &Path) {
    FAIL_NEXT_DIR_SYNC
        .lock()
        .expect("fault-probe poisoned")
        .insert(key(path));
}

/// Consumes the armed post-rename dir-sync failure for `path`, if any.
pub(crate) fn should_fail_dir_sync(path: &Path) -> bool {
    FAIL_NEXT_DIR_SYNC
        .lock()
        .expect("fault-probe poisoned")
        .remove(&key(path))
}

static FAIL_NEXT_COMMIT: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Arms a one-shot failure for the next [`commit_staged`] of `path`,
/// i.e. the rename/`sync_parent_dir` step rather than the staging write
/// (issue #1828 review, ninth round). Lets a test drive the case where
/// the *first* file of a two-file save is already published and the
/// second commit then fails.
pub(crate) fn fail_next_commit(path: &Path) {
    FAIL_NEXT_COMMIT
        .lock()
        .expect("fault-probe poisoned")
        .insert(key(path));
}

/// Consumes the armed commit failure for `path`, if any.
pub(crate) fn should_fail_commit(path: &Path) -> bool {
    FAIL_NEXT_COMMIT
        .lock()
        .expect("fault-probe poisoned")
        .remove(&key(path))
}

static FAIL_NEXT_EXISTS_CHECK: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Arms a one-shot failure for the next `try_exists` probe of `path`
/// (issue #1828 review, third round: an existence check can fail for
/// reasons other than "not found" — a transient I/O error or an ACL
/// denial on the bundle directory — and that failure must not be
/// silently read as "does not exist").
pub(crate) fn fail_next_exists_check(path: &Path) {
    FAIL_NEXT_EXISTS_CHECK
        .lock()
        .expect("fault-probe poisoned")
        .insert(key(path));
}

/// Consumes the armed existence-check failure for `path`, if any.
pub(crate) fn should_fail_exists_check(path: &Path) -> bool {
    FAIL_NEXT_EXISTS_CHECK
        .lock()
        .expect("fault-probe poisoned")
        .remove(&key(path))
}
