use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use tinyflows::graph::{
    Checkpoint, CheckpointConfig, CheckpointMetadata, CheckpointTuple, Checkpointer,
    FileCheckpointer, PendingWrite,
};

/// Durable workflow checkpoints scoped to one company bundle.
#[derive(Clone)]
pub struct WorkflowCheckpointStore {
    inner: FileCheckpointer<Value>,
}

/// Whether `thread_id` is safe to use as a single filesystem path component.
///
/// Every `thread_id` this store actually sees is minted by
/// [`crate::ports::generate_id`] — `{millis-hex}-{counter-hex}` — via a
/// run's own id (`WorkflowRunContext::new`) or, on a checkpoint resume, an
/// earlier run's id carried forward through `PAYLOAD_THREAD_ID` / a blocked
/// node's stash (see `crate::workflows::runner::run_workflow_inner` and
/// `crate::runtime::workflow_resume`). No producer in this codebase threads a
/// workflow name, node id, or other operator/agent-controlled text into it.
/// `tinyflows`' own `FileCheckpointer` also percent-encodes every byte outside
/// `[a-z0-9._-]` before it ever reaches a path, so `/` and `\` cannot survive
/// into a filename there either.
///
/// This check is the same property enforced a layer earlier, in the code this
/// crate owns rather than the vendored one, so a malformed thread id is
/// refused before it reaches the checkpoint backend at all rather than relying
/// solely on that backend's own escaping.
fn validate_thread_id(thread_id: &str) -> tinyflows::graph::Result<()> {
    let safe = !thread_id.is_empty()
        && thread_id != "."
        && thread_id != ".."
        && !thread_id.contains("..")
        && thread_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if safe {
        Ok(())
    } else {
        Err(tinyflows::graph::GraphError::Checkpoint(format!(
            "refusing to use `{thread_id:?}` as a checkpoint thread id — it is not a plain \
             alphanumeric/`.`/`_`/`-` filesystem component"
        )))
    }
}

impl WorkflowCheckpointStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            inner: FileCheckpointer::new(base_dir),
        }
    }

    pub fn base_dir(&self) -> &Path {
        self.inner.base_dir()
    }

    pub async fn has_resume_point(&self, thread_id: &str) -> tinyflows::graph::Result<bool> {
        Ok(self
            .get_scoped(thread_id, None, &[])
            .await?
            .is_some_and(|checkpoint| {
                !checkpoint.next_nodes.is_empty() || !checkpoint.interrupts.is_empty()
            }))
    }

    pub async fn prune_settled(&self, thread_id: &str) -> tinyflows::graph::Result<()> {
        self.delete_thread(thread_id).await
    }
}

#[async_trait]
impl Checkpointer<Value> for WorkflowCheckpointStore {
    async fn put(
        &self,
        checkpoint: Checkpoint<Value>,
    ) -> tinyflows::graph::Result<tinyflows::graph::ids::CheckpointId> {
        validate_thread_id(&checkpoint.thread_id)?;
        self.inner.put(checkpoint).await
    }

    async fn get(
        &self,
        thread_id: &str,
        checkpoint_id: Option<&str>,
    ) -> tinyflows::graph::Result<Option<Checkpoint<Value>>> {
        validate_thread_id(thread_id)?;
        self.inner.get(thread_id, checkpoint_id).await
    }

    async fn get_scoped(
        &self,
        thread_id: &str,
        checkpoint_id: Option<&str>,
        namespace: &[String],
    ) -> tinyflows::graph::Result<Option<Checkpoint<Value>>> {
        validate_thread_id(thread_id)?;
        Ok(self
            .inner
            .get_thread(thread_id)
            .await?
            .into_iter()
            .rev()
            .find(|checkpoint| {
                checkpoint.namespace == namespace
                    && checkpoint_id.is_none_or(|id| checkpoint.checkpoint_id.as_str() == id)
            }))
    }

    async fn list(&self, thread_id: &str) -> tinyflows::graph::Result<Vec<CheckpointMetadata>> {
        validate_thread_id(thread_id)?;
        self.inner.list(thread_id).await
    }

    async fn put_writes(
        &self,
        config: &CheckpointConfig,
        writes: &[PendingWrite],
    ) -> tinyflows::graph::Result<()> {
        validate_thread_id(&config.thread_id)?;
        self.inner.put_writes(config, writes).await
    }

    async fn get_writes(
        &self,
        config: &CheckpointConfig,
    ) -> tinyflows::graph::Result<Vec<PendingWrite>> {
        validate_thread_id(&config.thread_id)?;
        self.inner.get_writes(config).await
    }

    async fn get_thread(
        &self,
        thread_id: &str,
    ) -> tinyflows::graph::Result<Vec<Checkpoint<Value>>> {
        validate_thread_id(thread_id)?;
        self.inner.get_thread(thread_id).await
    }

    async fn state_history(
        &self,
        thread_id: &str,
        namespace: &[String],
        limit: Option<usize>,
    ) -> tinyflows::graph::Result<Vec<CheckpointTuple<Value>>> {
        validate_thread_id(thread_id)?;
        self.inner.state_history(thread_id, namespace, limit).await
    }

    async fn list_threads(&self) -> tinyflows::graph::Result<Vec<String>> {
        self.inner.list_threads().await
    }

    async fn delete_thread(&self, thread_id: &str) -> tinyflows::graph::Result<()> {
        validate_thread_id(thread_id)?;
        self.inner.delete_thread(thread_id).await
    }

    async fn delete_checkpoints(
        &self,
        thread_id: &str,
        ids: &[String],
    ) -> tinyflows::graph::Result<usize> {
        validate_thread_id(thread_id)?;
        self.inner.delete_checkpoints(thread_id, ids).await
    }
}

#[cfg(test)]
#[path = "checkpoint_store_tests.rs"]
mod tests;
