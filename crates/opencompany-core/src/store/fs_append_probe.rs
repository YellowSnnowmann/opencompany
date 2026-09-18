use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

/// `(plain, durable)` append counts, keyed like [`super::path_lock`] on the
/// absolutised path so a test and the code under test always meet.
static COUNTS: LazyLock<Mutex<HashMap<PathBuf, (usize, usize)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn key(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn record(path: &Path, synced: bool) {
    let mut counts = COUNTS.lock().expect("append-probe poisoned");
    let entry = counts.entry(key(path)).or_insert((0, 0));
    if synced {
        entry.1 += 1;
    } else {
        entry.0 += 1;
    }
}

/// The `(plain, durable)` appends observed for `path`. Tests use their own
/// temp paths, so no two of them share a tally.
pub(crate) fn counts(path: &Path) -> (usize, usize) {
    COUNTS
        .lock()
        .expect("append-probe poisoned")
        .get(&key(path))
        .copied()
        .unwrap_or((0, 0))
}

/// How many times [`super::sync_parent_dir`] has flushed each directory.
///
/// The same argument as the append tally: a flushed directory and an
/// unflushed one are identical on disk, so the honest check is to count the
/// request where it is made. A count rather than a set, because *how often*
/// is the question for the directory flush — it is meant to be paid by the
/// append that creates a file and by no other. Never cleared: tests own
/// unique temp paths and ask about their own.
static DIR_SYNCS: LazyLock<Mutex<HashMap<PathBuf, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn record_dir_sync(path: &Path) {
    *DIR_SYNCS
        .lock()
        .expect("append-probe poisoned")
        .entry(key(path))
        .or_insert(0) += 1;
}

/// How many times `path`'s directory entry block was flushed.
pub(crate) fn dir_syncs(path: &Path) -> usize {
    DIR_SYNCS
        .lock()
        .expect("append-probe poisoned")
        .get(&key(path))
        .copied()
        .unwrap_or(0)
}

/// How many times [`super::write_atomic_bytes`] flushed a temp file's data
/// before publishing it, keyed on the **final** path rather than the temp
/// one — the temp name carries a fresh id per call, so a test could never
/// ask about it.
///
/// Counted here for the reason [`dir_syncs`] is: a flushed file and an
/// unflushed one are identical on disk, so the honest check is to count the
/// request at the point it is made. This proves the call happens; it does
/// not — and cannot — prove what a power cut would leave behind.
static ATOMIC_SYNCS: LazyLock<Mutex<HashMap<PathBuf, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn record_atomic_sync(path: &Path) {
    *ATOMIC_SYNCS
        .lock()
        .expect("append-probe poisoned")
        .entry(key(path))
        .or_insert(0) += 1;
}

/// How many times `path` was flushed before being published by a rename.
pub(crate) fn atomic_syncs(path: &Path) -> usize {
    ATOMIC_SYNCS
        .lock()
        .expect("append-probe poisoned")
        .get(&key(path))
        .copied()
        .unwrap_or(0)
}

/// The order in which [`super::write_atomic_bytes`] publish renames have
/// landed, globally, since the process started.
///
/// A multi-file save (`FsCompanyStore::save_gated` writes `company.toml`
/// then `meta.json`) has a crash-ordering property neither
/// [`counts`] nor [`atomic_syncs`] can answer: *which file's publish is
/// observable first* if the process dies between the two. Each is
/// individually atomic+durable (that is what those two probes prove), but
/// nothing about a single path's own counters says anything about a
/// **different** path's write landing before or after it. This log does:
/// it is one global, append-only sequence of every publish, in the order
/// `write_atomic_bytes` actually completed them.
static WRITE_ORDER: LazyLock<Mutex<Vec<PathBuf>>> = LazyLock::new(|| Mutex::new(Vec::new()));

pub(crate) fn record_write_order(path: &Path) {
    WRITE_ORDER
        .lock()
        .expect("append-probe poisoned")
        .push(key(path));
}

/// The subsequence of the global publish order restricted to `paths`,
/// in the order they actually landed. Tests use their own unique temp
/// paths, so restricting to the paths under test is enough to make this
/// deterministic even though the log itself is never cleared.
pub(crate) fn write_order_for(paths: &[&Path]) -> Vec<PathBuf> {
    let keys: Vec<PathBuf> = paths.iter().map(|p| key(p)).collect();
    WRITE_ORDER
        .lock()
        .expect("append-probe poisoned")
        .iter()
        .filter(|p| keys.contains(p))
        .cloned()
        .collect()
}
