use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Notify;

static GATES: LazyLock<Mutex<HashMap<PathBuf, Receiver<()>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static BLOCKED: LazyLock<Notify> = LazyLock::new(Notify::new);

fn key(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Arms a one-shot stall for the next [`stage_atomic_bytes`] write
/// targeting `path`. Returns the sender a test uses to release it.
pub(crate) fn arm(path: &Path) -> Sender<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    GATES
        .lock()
        .expect("stall-probe poisoned")
        .insert(key(path), rx);
    tx
}

/// Called from inside the blocking write closure. No-op unless `path`
/// was armed. Notifies [`wait_blocked`], then parks this blocking-pool
/// thread until the test's sender releases it.
pub(crate) fn maybe_block(path: &Path) {
    let gate = GATES
        .lock()
        .expect("stall-probe poisoned")
        .remove(&key(path));
    if let Some(gate) = gate {
        BLOCKED.notify_one();
        let _ = gate.recv();
    }
}

/// Waits until an armed write has reached its stall point. `notify_one`
/// stores its permit if called before this is polled, so there is no
/// race between arming, spawning the write, and awaiting this.
pub(crate) async fn wait_blocked() {
    BLOCKED.notified().await;
}

type CommitStall = (Receiver<()>, Arc<Notify>);

static COMMIT_GATES: LazyLock<Mutex<HashMap<PathBuf, CommitStall>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// One armed commit stall and the notification that belongs only to it.
pub(crate) struct CommitGate {
    release: Sender<()>,
    blocked: Arc<Notify>,
}

impl CommitGate {
    /// Releases the parked commit closure.
    pub(crate) fn send(self, value: ()) -> Result<(), std::sync::mpsc::SendError<()>> {
        self.release.send(value)
    }

    /// Waits until this exact armed commit reaches its stall point.
    pub(crate) async fn wait_blocked(&self) {
        self.blocked.notified().await;
    }
}

/// Same idea as [`arm`]/[`maybe_block`]/[`wait_blocked`] above, but for
/// [`commit_staged`]'s blocking closure instead of [`stage_atomic_bytes`]'s.
/// The returned gate owns its notification, so parallel tests cannot consume
/// each other's wakeup merely because they are both stalling commits.
pub(crate) fn arm_commit(path: &Path) -> CommitGate {
    let (tx, rx) = std::sync::mpsc::channel();
    let blocked = Arc::new(Notify::new());
    COMMIT_GATES
        .lock()
        .expect("stall-probe poisoned")
        .insert(key(path), (rx, blocked.clone()));
    CommitGate {
        release: tx,
        blocked,
    }
}

/// Called from inside `commit_staged`'s blocking closure, before the
/// rename. No-op unless `path` was armed.
pub(crate) fn maybe_block_commit(path: &Path) {
    let gate = COMMIT_GATES
        .lock()
        .expect("stall-probe poisoned")
        .remove(&key(path));
    if let Some((release, blocked)) = gate {
        blocked.notify_one();
        let _ = release.recv();
    }
}
