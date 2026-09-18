use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use tokio::sync::Notify;

static GATES: LazyLock<Mutex<HashMap<PathBuf, ()>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static BLOCKED: LazyLock<Notify> = LazyLock::new(Notify::new);

fn key(dir: &Path) -> PathBuf {
    std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// Arms a one-shot stall for the next `remove_staged` of a temp file
/// living directly in `dir`.
pub(crate) fn arm(dir: &Path) {
    GATES
        .lock()
        .expect("cleanup-probe poisoned")
        .insert(key(dir), ());
}

/// No-op unless this temp's directory was armed. Notifies
/// [`wait_blocked`], then parks forever — the test aborts the task
/// rather than releasing it, which is the scenario under test.
pub(crate) async fn maybe_block(tmp: &Path) {
    let armed = tmp
        .parent()
        .map(|dir| {
            GATES
                .lock()
                .expect("cleanup-probe poisoned")
                .remove(&key(dir))
                .is_some()
        })
        .unwrap_or(false);
    if armed {
        BLOCKED.notify_one();
        std::future::pending::<()>().await;
    }
}

/// Waits until an armed cleanup has reached its stall point.
pub(crate) async fn wait_blocked() {
    BLOCKED.notified().await;
}
