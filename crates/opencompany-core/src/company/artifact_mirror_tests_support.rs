//! Shared fixtures for the `artifact_mirror` test split: the in-memory
//! stores, fault-injecting stores, and the small tree/path helpers every
//! publish/concurrency/folder/fault test builds on (split out of
//! `artifact_mirror_tests.rs`).

use std::sync::Arc;

use super::*;
pub(super) use crate::company::workspace_scaffold::ARTIFACTS_ROOT;
pub(super) use crate::ports::artifacts::ArtifactKind;
use crate::store::FsOps;

/// One `FsOps` backing both ports, so a test exercises the real stores
/// rather than a stub that cannot tell a create from an overwrite.
pub(super) fn stores() -> (tempfile::TempDir, Arc<FsOps>, CompanyId) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ops = Arc::new(FsOps::new(dir.path()));
    (dir, ops, CompanyId::new("mirror-co"))
}

/// A store that delegates everything to a real backend but **refuses every
/// create**, which is the shape a quota refusal takes here.
///
/// A wrapper rather than a configured quota because the assertion is about
/// what `materialize` does when the create fails, not about which limit
/// produced the failure — and a real limit would tie the test to whichever
/// cap happens to be tunable.
pub(super) struct RefusingCreate(pub(super) Arc<FsOps>);

#[async_trait::async_trait]
impl WorkspaceStore for RefusingCreate {
    async fn tree(&self, company: &CompanyId) -> Result<Vec<WorkspaceNode>> {
        WorkspaceStore::tree(&*self.0, company).await
    }
    async fn read(&self, company: &CompanyId, id: &str) -> Result<Option<(WorkspaceNode, String)>> {
        WorkspaceStore::read(&*self.0, company, id).await
    }
    async fn read_capped(
        &self,
        company: &CompanyId,
        id: &str,
        max_bytes: u64,
    ) -> Result<Option<(WorkspaceNode, String, u64)>> {
        WorkspaceStore::read_capped(&*self.0, company, id, max_bytes).await
    }
    async fn write_with_revision(
        &self,
        company: &CompanyId,
        id: &str,
        content: &str,
        author: WorkspaceOrigin,
        expected_updated_at: Option<u64>,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::write_with_revision(
            &*self.0,
            company,
            id,
            content,
            author,
            expected_updated_at,
        )
        .await
    }
    async fn create(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        content: Option<&str>,
    ) -> Result<()> {
        // Folders still have to be creatable: the scaffold mints the agent
        // and task folders on the way in, and refusing those would fail the
        // publish before it ever reaches the replacement.
        if node.kind == NodeKind::Folder {
            return WorkspaceStore::create(&*self.0, company, node, content).await;
        }
        Err(OpenCompanyError::InvalidRequest("over quota".to_string()))
    }
    /// Folders are claimed for real, for the same reason `create` lets them
    /// through: the scaffold walks `agents/<id>/<task>/` on the way in, and
    /// refusing that would fail the publish before it ever reaches the file
    /// this double exists to refuse.
    async fn adopt_or_create_folder(
        &self,
        company: &CompanyId,
        parent: Option<&str>,
        name: &str,
        origin: WorkspaceOrigin,
    ) -> Result<crate::ports::workspace::FolderClaim> {
        WorkspaceStore::adopt_or_create_folder(&*self.0, company, parent, name, origin).await
    }
    async fn create_binary(
        &self,
        _company: &CompanyId,
        _node: &WorkspaceNode,
        _bytes: &[u8],
    ) -> Result<WorkspaceNode> {
        Err(OpenCompanyError::InvalidRequest("over quota".to_string()))
    }
    async fn write_binary(
        &self,
        company: &CompanyId,
        id: &str,
        bytes: &[u8],
        mime: Option<&str>,
        author: WorkspaceOrigin,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::write_binary(&*self.0, company, id, bytes, mime, author).await
    }
    async fn read_bytes(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> Result<Option<(WorkspaceNode, crate::ports::workspace::BlobStream)>> {
        WorkspaceStore::read_bytes(&*self.0, company, id).await
    }
    async fn rename_move(
        &self,
        company: &CompanyId,
        id: &str,
        name: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::rename_move(&*self.0, company, id, name, parent).await
    }
    async fn swap_files(
        &self,
        company: &CompanyId,
        expected_id: Option<&str>,
        replacement_id: &str,
        name: &str,
    ) -> Result<Option<WorkspaceNode>> {
        WorkspaceStore::swap_files(&*self.0, company, expected_id, replacement_id, name).await
    }
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        WorkspaceStore::delete(&*self.0, company, id).await
    }
    async fn is_empty(&self, company: &CompanyId) -> Result<bool> {
        WorkspaceStore::is_empty(&*self.0, company).await
    }
}

/// [`RefusingCreate`] with a twist: on the note create, a rival publisher
/// **adopts the just-minted parent folder** before the create is refused
/// (issue #1839) — the exact mid-write race the adoption lease exists for.
///
/// This is what `RefusingCreate` alone cannot model: there, the folders this
/// publish minted are swept because nobody else laid a claim on them. Here a
/// second writer has, so `materialize`'s rollback must find the lease and
/// leave the folder standing rather than delete the one the rival is about to
/// write into.
pub(super) struct AdoptParentThenRefuse(pub(super) Arc<FsOps>);

#[async_trait::async_trait]
impl WorkspaceStore for AdoptParentThenRefuse {
    async fn tree(&self, company: &CompanyId) -> Result<Vec<WorkspaceNode>> {
        WorkspaceStore::tree(&*self.0, company).await
    }
    async fn read(&self, company: &CompanyId, id: &str) -> Result<Option<(WorkspaceNode, String)>> {
        WorkspaceStore::read(&*self.0, company, id).await
    }
    async fn read_capped(
        &self,
        company: &CompanyId,
        id: &str,
        max_bytes: u64,
    ) -> Result<Option<(WorkspaceNode, String, u64)>> {
        WorkspaceStore::read_capped(&*self.0, company, id, max_bytes).await
    }
    async fn write_with_revision(
        &self,
        company: &CompanyId,
        id: &str,
        content: &str,
        author: WorkspaceOrigin,
        expected_updated_at: Option<u64>,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::write_with_revision(
            &*self.0,
            company,
            id,
            content,
            author,
            expected_updated_at,
        )
        .await
    }
    async fn create(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        content: Option<&str>,
    ) -> Result<()> {
        if node.kind == NodeKind::Folder {
            return WorkspaceStore::create(&*self.0, company, node, content).await;
        }
        // The note create is about to be refused — but first a rival adopts
        // the folder this publish just minted for it, taking the lease. The
        // rival is a real `adopt_or_create_folder` against the inner store,
        // so the flag lands exactly as it would in production.
        if let Some(parent_id) = node.parent_id.as_deref() {
            let nodes = WorkspaceStore::tree(&*self.0, company).await?;
            if let Some(parent) = nodes.iter().find(|n| n.id == parent_id) {
                WorkspaceStore::adopt_or_create_folder(
                    &*self.0,
                    company,
                    parent.parent_id.as_deref(),
                    &parent.name,
                    WorkspaceOrigin::Operator,
                )
                .await?;
            }
        }
        Err(OpenCompanyError::InvalidRequest("over quota".to_string()))
    }
    async fn adopt_or_create_folder(
        &self,
        company: &CompanyId,
        parent: Option<&str>,
        name: &str,
        origin: WorkspaceOrigin,
    ) -> Result<crate::ports::workspace::FolderClaim> {
        WorkspaceStore::adopt_or_create_folder(&*self.0, company, parent, name, origin).await
    }
    async fn create_binary(
        &self,
        _company: &CompanyId,
        _node: &WorkspaceNode,
        _bytes: &[u8],
    ) -> Result<WorkspaceNode> {
        Err(OpenCompanyError::InvalidRequest("over quota".to_string()))
    }
    async fn write_binary(
        &self,
        company: &CompanyId,
        id: &str,
        bytes: &[u8],
        mime: Option<&str>,
        author: WorkspaceOrigin,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::write_binary(&*self.0, company, id, bytes, mime, author).await
    }
    async fn read_bytes(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> Result<Option<(WorkspaceNode, crate::ports::workspace::BlobStream)>> {
        WorkspaceStore::read_bytes(&*self.0, company, id).await
    }
    async fn rename_move(
        &self,
        company: &CompanyId,
        id: &str,
        name: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::rename_move(&*self.0, company, id, name, parent).await
    }
    async fn swap_files(
        &self,
        company: &CompanyId,
        expected_id: Option<&str>,
        replacement_id: &str,
        name: &str,
    ) -> Result<Option<WorkspaceNode>> {
        WorkspaceStore::swap_files(&*self.0, company, expected_id, replacement_id, name).await
    }
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        WorkspaceStore::delete(&*self.0, company, id).await
    }
    async fn delete_if_empty(&self, company: &CompanyId, id: &str) -> Result<bool> {
        // Forward to the inner backend's override, per the port contract, so
        // the rollback exercises the real fs guard rather than the decorator
        // default.
        WorkspaceStore::delete_if_empty(&*self.0, company, id).await
    }
    async fn is_empty(&self, company: &CompanyId) -> Result<bool> {
        WorkspaceStore::is_empty(&*self.0, company).await
    }
}

/// Every node id in the workspace, sorted — the set, which is what `tree`
/// actually promises.
pub(super) async fn sorted_ids(ws: &dyn WorkspaceStore, company: &CompanyId) -> Vec<String> {
    let mut ids: Vec<String> = ws
        .tree(company)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    ids.sort();
    ids
}

/// A node's rendered path, so an assertion reads as a path rather than a
/// ULID.
pub(super) async fn path_of(ws: &dyn WorkspaceStore, company: &CompanyId, id: &str) -> String {
    let nodes = ws.tree(company).await.unwrap();
    let mut parts = Vec::new();
    let mut cursor = Some(id.to_string());
    while let Some(current) = cursor {
        let Some(node) = nodes.iter().find(|n| n.id == current) else {
            break;
        };
        parts.push(node.name.clone());
        cursor = node.parent_id.clone();
    }
    parts.reverse();
    parts.join("/")
}

pub(super) fn target<'a>(source: &'a str, body: &'a str) -> PublishTarget<'a> {
    PublishTarget {
        agent_id: "cmo",
        task_id: "t-1",
        task_title: None,
        source,
        payload: MirrorPayload::Text(body),
        existing_node_id: None,
    }
}

/// A real store whose swap boundary pauses until two publishers have both
/// staged their payloads. This makes the race deterministic without
/// replacing the compare-and-swap implementation under test.
pub(super) struct PausedSwap(pub(super) Arc<FsOps>, pub(super) Arc<tokio::sync::Barrier>);

#[async_trait::async_trait]
impl WorkspaceStore for PausedSwap {
    async fn tree(&self, company: &CompanyId) -> Result<Vec<WorkspaceNode>> {
        WorkspaceStore::tree(&*self.0, company).await
    }
    async fn read(&self, company: &CompanyId, id: &str) -> Result<Option<(WorkspaceNode, String)>> {
        WorkspaceStore::read(&*self.0, company, id).await
    }
    async fn read_capped(
        &self,
        company: &CompanyId,
        id: &str,
        max_bytes: u64,
    ) -> Result<Option<(WorkspaceNode, String, u64)>> {
        WorkspaceStore::read_capped(&*self.0, company, id, max_bytes).await
    }
    async fn write_with_revision(
        &self,
        company: &CompanyId,
        id: &str,
        content: &str,
        author: WorkspaceOrigin,
        expected_updated_at: Option<u64>,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::write_with_revision(
            &*self.0,
            company,
            id,
            content,
            author,
            expected_updated_at,
        )
        .await
    }
    async fn create(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        content: Option<&str>,
    ) -> Result<()> {
        WorkspaceStore::create(&*self.0, company, node, content).await
    }
    async fn create_binary(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        bytes: &[u8],
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::create_binary(&*self.0, company, node, bytes).await
    }
    async fn write_binary(
        &self,
        company: &CompanyId,
        id: &str,
        bytes: &[u8],
        mime: Option<&str>,
        author: WorkspaceOrigin,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::write_binary(&*self.0, company, id, bytes, mime, author).await
    }
    async fn read_bytes(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> Result<Option<(WorkspaceNode, crate::ports::workspace::BlobStream)>> {
        WorkspaceStore::read_bytes(&*self.0, company, id).await
    }
    async fn rename_move(
        &self,
        company: &CompanyId,
        id: &str,
        name: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::rename_move(&*self.0, company, id, name, parent).await
    }
    async fn adopt_or_create_folder(
        &self,
        company: &CompanyId,
        parent: Option<&str>,
        name: &str,
        origin: WorkspaceOrigin,
    ) -> Result<crate::ports::workspace::FolderClaim> {
        WorkspaceStore::adopt_or_create_folder(&*self.0, company, parent, name, origin).await
    }
    async fn swap_files(
        &self,
        company: &CompanyId,
        expected_id: Option<&str>,
        replacement_id: &str,
        name: &str,
    ) -> Result<Option<WorkspaceNode>> {
        self.1.wait().await;
        WorkspaceStore::swap_files(&*self.0, company, expected_id, replacement_id, name).await
    }
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        WorkspaceStore::delete(&*self.0, company, id).await
    }
    async fn is_empty(&self, company: &CompanyId) -> Result<bool> {
        WorkspaceStore::is_empty(&*self.0, company).await
    }
}

/// A real store that holds the first `arrivals` `tree()` reads at a
/// two-party barrier, so two publishers provably act on the *same* snapshot.
///
/// # Why the tree read and not the folder write
///
/// This is the race's own precondition: both publishers read "that folder is
/// not there", and before issue #759 both then created. Pausing where the
/// snapshot is taken forces exactly that interleaving without replacing any
/// of the code under test — the folder claim, whichever backend decides it,
/// runs for real afterwards.
///
/// It is also the only pause point that exists **on both sides of the fix**,
/// which is what lets these two tests be run against the base commit to
/// watch them fail. A barrier inside the new primitive could only ever
/// observe the fixed code.
///
/// `arrivals` is a budget rather than a switch: after that many `tree()`
/// calls the barrier is bypassed, so a publisher that fails early (which is
/// exactly what the *unfixed* code does) cannot strand its partner waiting
/// for a rendezvous that will never come.
pub(super) struct PausedTreeRead {
    inner: Arc<FsOps>,
    barrier: Arc<tokio::sync::Barrier>,
    arrivals: std::sync::atomic::AtomicUsize,
}

impl PausedTreeRead {
    pub(super) fn new(inner: Arc<FsOps>, arrivals: usize) -> Self {
        Self {
            inner,
            barrier: Arc::new(tokio::sync::Barrier::new(2)),
            arrivals: std::sync::atomic::AtomicUsize::new(arrivals),
        }
    }
}

#[async_trait::async_trait]
impl WorkspaceStore for PausedTreeRead {
    async fn tree(&self, company: &CompanyId) -> Result<Vec<WorkspaceNode>> {
        let budget = self
            .arrivals
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok();
        if budget {
            self.barrier.wait().await;
        }
        WorkspaceStore::tree(&*self.inner, company).await
    }
    async fn read(&self, company: &CompanyId, id: &str) -> Result<Option<(WorkspaceNode, String)>> {
        WorkspaceStore::read(&*self.inner, company, id).await
    }
    async fn read_capped(
        &self,
        company: &CompanyId,
        id: &str,
        max_bytes: u64,
    ) -> Result<Option<(WorkspaceNode, String, u64)>> {
        WorkspaceStore::read_capped(&*self.inner, company, id, max_bytes).await
    }
    async fn write_with_revision(
        &self,
        company: &CompanyId,
        id: &str,
        content: &str,
        author: WorkspaceOrigin,
        expected_updated_at: Option<u64>,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::write_with_revision(
            &*self.inner,
            company,
            id,
            content,
            author,
            expected_updated_at,
        )
        .await
    }
    async fn create(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        content: Option<&str>,
    ) -> Result<()> {
        WorkspaceStore::create(&*self.inner, company, node, content).await
    }
    async fn adopt_or_create_folder(
        &self,
        company: &CompanyId,
        parent: Option<&str>,
        name: &str,
        origin: WorkspaceOrigin,
    ) -> Result<crate::ports::workspace::FolderClaim> {
        WorkspaceStore::adopt_or_create_folder(&*self.inner, company, parent, name, origin).await
    }
    async fn create_binary(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        bytes: &[u8],
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::create_binary(&*self.inner, company, node, bytes).await
    }
    async fn write_binary(
        &self,
        company: &CompanyId,
        id: &str,
        bytes: &[u8],
        mime: Option<&str>,
        author: WorkspaceOrigin,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::write_binary(&*self.inner, company, id, bytes, mime, author).await
    }
    async fn read_bytes(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> Result<Option<(WorkspaceNode, crate::ports::workspace::BlobStream)>> {
        WorkspaceStore::read_bytes(&*self.inner, company, id).await
    }
    async fn rename_move(
        &self,
        company: &CompanyId,
        id: &str,
        name: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> Result<WorkspaceNode> {
        WorkspaceStore::rename_move(&*self.inner, company, id, name, parent).await
    }
    async fn swap_files(
        &self,
        company: &CompanyId,
        expected_id: Option<&str>,
        replacement_id: &str,
        name: &str,
    ) -> Result<Option<WorkspaceNode>> {
        WorkspaceStore::swap_files(&*self.inner, company, expected_id, replacement_id, name).await
    }
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        WorkspaceStore::delete(&*self.inner, company, id).await
    }
    async fn is_empty(&self, company: &CompanyId) -> Result<bool> {
        WorkspaceStore::is_empty(&*self.inner, company).await
    }
}

/// Every node under `parent` carrying `name`, by id.
pub(super) fn named_children(nodes: &[WorkspaceNode], parent: &str, name: &str) -> Vec<String> {
    nodes
        .iter()
        .filter(|n| n.parent_id.as_deref() == Some(parent) && n.name == name)
        .map(|n| n.id.clone())
        .collect()
}

// -- the two store faults, told apart --------------------------------

/// An artifact store with one chosen fault, so a test can ask for exactly
/// the failure it means: unreadable (`list`) or unwritable (`upsert`).
pub(super) struct FaultyArtifacts {
    pub(super) listed: Vec<ArtifactRecord>,
    pub(super) list_fails: bool,
    pub(super) upsert_fails: bool,
}

#[async_trait::async_trait]
impl ArtifactStore for FaultyArtifacts {
    async fn list(&self, _: &CompanyId, _: Option<&str>) -> Result<Vec<ArtifactRecord>> {
        if self.list_fails {
            return Err(OpenCompanyError::Store("the artifact store is down".into()));
        }
        Ok(self.listed.clone())
    }
    async fn get(&self, _: &CompanyId, _: &str) -> Result<Option<ArtifactRecord>> {
        Ok(None)
    }
    async fn upsert(&self, _: &CompanyId, _: &ArtifactRecord) -> Result<()> {
        if self.upsert_fails {
            return Err(OpenCompanyError::Store("the disk is full".into()));
        }
        Ok(())
    }
    async fn delete(&self, _: &CompanyId, _: &str) -> Result<bool> {
        Ok(false)
    }
}

pub(super) fn published_as(node_id: &str) -> ArtifactRecord {
    let mut record = ArtifactRecord::new(
        "a-1",
        "t-1",
        "Launch",
        ArtifactKind::Markdown,
        "agent draft",
        "cmo",
        1,
    );
    record.stamp_workspace_node(node_id);
    record
}
