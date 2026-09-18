use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Notify;

static GATES: LazyLock<Mutex<HashMap<PathBuf, Arc<Notify>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn key(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Arms a one-shot "the send just succeeded" signal for `path`. Returns
/// the `Notify` a test awaits.
pub(crate) fn arm(path: &Path) -> Arc<Notify> {
    let notify = Arc::new(Notify::new());
    GATES
        .lock()
        .expect("send-probe poisoned")
        .insert(key(path), notify.clone());
    notify
}

/// Called right after a successful `tx.send` in `stage_atomic_bytes`'s
/// detached task. No-op unless `path` was armed.
pub(crate) fn notify_sent(path: &Path) {
    if let Some(notify) = GATES
        .lock()
        .expect("send-probe poisoned")
        .remove(&key(path))
    {
        notify.notify_one();
    }
}
