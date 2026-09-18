//! Workspace scaffold: rollback of a partially-minted scaffold, including
//! races where a child folder appears mid-rollback.

use std::sync::Arc;

use super::*;
use crate::store::FsOps;

async fn store() -> (tempfile::TempDir, Arc<dyn WorkspaceStore>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ops: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    (dir, ops)
}

/// A node's rendered `parent/child` path, for readable assertions.
fn path_of(nodes: &[WorkspaceNode], node: &WorkspaceNode) -> String {
    match &node.parent_id {
        None => node.name.clone(),
        Some(parent) => match nodes.iter().find(|n| &n.id == parent) {
            Some(p) => format!("{}/{}", path_of(nodes, p), node.name),
            None => node.name.clone(),
        },
    }
}

fn paths(nodes: &[WorkspaceNode]) -> Vec<String> {
    let mut out: Vec<String> = nodes.iter().map(|n| path_of(nodes, n)).collect();
    out.sort();
    out
}

async fn tree_paths(ws: &Arc<dyn WorkspaceStore>, company: &CompanyId) -> Vec<String> {
    paths(&ws.tree(company).await.unwrap())
}

/// A minted folder that never received the write it was made for is swept
/// when the caller rolls back — leaving no empty `agents/<id>/` for the
/// Repair button. The reserved root it hangs off is scaffolding and stays.
#[tokio::test]
async fn rollback_removes_a_minted_folder_that_stayed_empty() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    let (home, created) = ensure_agent_folder_tracked(ws.as_ref(), &company, "ceo")
        .await
        .unwrap();
    assert!(created);
    assert!(
        tree_paths(&ws, &company)
            .await
            .contains(&"agents/ceo".to_string())
    );

    rollback_empty_minted_folders(ws.as_ref(), &company, &[home]).await;

    let paths = tree_paths(&ws, &company).await;
    assert!(
        !paths.contains(&"agents/ceo".to_string()),
        "an empty minted folder must be swept when its write never landed: {paths:?}"
    );
    assert!(
        paths.contains(&"agents".to_string()),
        "the scaffolded root must survive the rollback: {paths:?}"
    );
}

/// The over-deletion guard: a folder that gained a child in the window — a
/// concurrent create, or the very write the caller thought had failed — is
/// left exactly as it stands, and its child is never deleted out from under
/// it by a recursive sweep.
#[tokio::test]
async fn rollback_keeps_a_minted_folder_that_gained_a_child() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();
    let (home, _) = ensure_agent_folder_tracked(ws.as_ref(), &company, "ceo")
        .await
        .unwrap();

    ws.create(
        &company,
        &WorkspaceNode {
            id: "kept-note".to_string(),
            name: "brief.md".to_string(),
            kind: NodeKind::File,
            parent_id: Some(home.clone()),
            updated_at_millis: 1,
            created_by: WorkspaceOrigin::Operator,
            updated_by: WorkspaceOrigin::Operator,
            mime: None,
            size: None,
            sha256: None,
            adopted: false,
        },
        Some("# keep me"),
    )
    .await
    .unwrap();

    rollback_empty_minted_folders(ws.as_ref(), &company, &[home]).await;

    let paths = tree_paths(&ws, &company).await;
    assert!(
        paths.contains(&"agents/ceo".to_string()),
        "a folder that gained a child must survive: {paths:?}"
    );
    assert!(
        paths.contains(&"agents/ceo/brief.md".to_string()),
        "and its child must not be deleted out from under it: {paths:?}"
    );
}

/// A store double that, the first time `tree` is called, hands back a
/// snapshot exactly like the real one below it — and then, *after*
/// capturing that snapshot but before returning it, writes a child into
/// `inject_child_under` directly against the wrapped store. This puts the
/// wrapped store one write ahead of whatever the caller does with the
/// snapshot it receives — precisely the shape of the race review found: a
/// concurrent adopter's write landing in the window between a `tree()`
/// read and a later `delete()` built from it.
///
/// Every other method forwards straight through; only `tree`'s first call
/// carries the injected write, so a second `tree()` call (e.g. inside
/// `delete_if_empty`'s own fresh check) sees it, but the snapshot handed
/// to the *caller* of the first call never does.
struct InjectChildAfterFirstTree {
    inner: Arc<dyn WorkspaceStore>,
    inject_child_under: String,
    injected: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl WorkspaceStore for InjectChildAfterFirstTree {
    async fn tree(&self, company: &CompanyId) -> Result<Vec<WorkspaceNode>> {
        let snapshot = self.inner.tree(company).await?;
        if !self
            .injected
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.inner
                .create(
                    company,
                    &WorkspaceNode {
                        id: "raced-in-note".to_string(),
                        name: "raced-in.md".to_string(),
                        kind: NodeKind::File,
                        parent_id: Some(self.inject_child_under.clone()),
                        updated_at_millis: 1,
                        created_by: WorkspaceOrigin::Operator,
                        updated_by: WorkspaceOrigin::Operator,
                        mime: None,
                        size: None,
                        sha256: None,
                        adopted: false,
                    },
                    Some("landed mid-rollback"),
                )
                .await
                .expect("inject concurrent child");
        }
        Ok(snapshot)
    }

    async fn read(&self, company: &CompanyId, id: &str) -> Result<Option<(WorkspaceNode, String)>> {
        self.inner.read(company, id).await
    }
    async fn read_capped(
        &self,
        company: &CompanyId,
        id: &str,
        max_bytes: u64,
    ) -> Result<Option<(WorkspaceNode, String, u64)>> {
        self.inner.read_capped(company, id, max_bytes).await
    }

    async fn write_with_revision(
        &self,
        company: &CompanyId,
        id: &str,
        content: &str,
        author: WorkspaceOrigin,
        expected_updated_at: Option<u64>,
    ) -> Result<WorkspaceNode> {
        self.inner
            .write_with_revision(company, id, content, author, expected_updated_at)
            .await
    }

    async fn create(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        content: Option<&str>,
    ) -> Result<()> {
        self.inner.create(company, node, content).await
    }

    async fn adopt_or_create_folder(
        &self,
        company: &CompanyId,
        parent: Option<&str>,
        name: &str,
        origin: WorkspaceOrigin,
    ) -> Result<crate::ports::workspace::FolderClaim> {
        self.inner
            .adopt_or_create_folder(company, parent, name, origin)
            .await
    }

    async fn create_binary(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        bytes: &[u8],
    ) -> Result<WorkspaceNode> {
        self.inner.create_binary(company, node, bytes).await
    }

    async fn write_binary(
        &self,
        company: &CompanyId,
        id: &str,
        bytes: &[u8],
        mime: Option<&str>,
        author: WorkspaceOrigin,
    ) -> Result<WorkspaceNode> {
        self.inner
            .write_binary(company, id, bytes, mime, author)
            .await
    }

    async fn read_bytes(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> Result<Option<(WorkspaceNode, crate::ports::workspace::BlobStream)>> {
        self.inner.read_bytes(company, id).await
    }

    async fn rename_move(
        &self,
        company: &CompanyId,
        id: &str,
        name: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> Result<WorkspaceNode> {
        self.inner.rename_move(company, id, name, parent).await
    }

    async fn swap_files(
        &self,
        company: &CompanyId,
        expected_id: Option<&str>,
        replacement_id: &str,
        name: &str,
    ) -> Result<Option<WorkspaceNode>> {
        self.inner
            .swap_files(company, expected_id, replacement_id, name)
            .await
    }

    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        self.inner.delete(company, id).await
    }

    // Forwarded explicitly, exactly like the production decorators
    // (`WorkspaceAnnouncer`, `QuotaEnforcedWorkspace`, `DerivedGuardWorkspace`)
    // — so this test exercises the wrapped store's own `delete_if_empty`
    // (here, `FsOps`'s single-lock override) rather than the default trait
    // method re-deriving the check at this wrapper's level.
    async fn delete_if_empty(&self, company: &CompanyId, id: &str) -> Result<bool> {
        self.inner.delete_if_empty(company, id).await
    }

    async fn is_empty(&self, company: &CompanyId) -> Result<bool> {
        self.inner.is_empty(company).await
    }
}

/// The race itself: a child lands under a minted-but-empty folder in the
/// window between `rollback_empty_minted_folders`'s own `tree()` read and
/// the `delete` it would have issued from that stale read. Before the
/// fix, `rollback_empty_minted_folders` decided emptiness from that same
/// stale snapshot and called the unconditional `delete`, which recursed
/// through the folder and erased the concurrently-landed child with it —
/// this test fails on that code, asserting the child survives. After the
/// fix, the decision is `delete_if_empty`'s own fresh re-check, which sees
/// the child and refuses to remove the folder.
#[tokio::test]
async fn rollback_does_not_erase_a_child_that_lands_mid_rollback() {
    let (_dir, real) = store().await;
    let company = CompanyId::new("acme");
    ensure_workspace_scaffold(real.as_ref(), &company)
        .await
        .unwrap();
    let (home, _) = ensure_agent_folder_tracked(real.as_ref(), &company, "ceo")
        .await
        .unwrap();

    let racy: Arc<dyn WorkspaceStore> = Arc::new(InjectChildAfterFirstTree {
        inner: real.clone(),
        inject_child_under: home.clone(),
        injected: std::sync::atomic::AtomicBool::new(false),
    });

    rollback_empty_minted_folders(racy.as_ref(), &company, &[home]).await;

    let paths = tree_paths(&real, &company).await;
    assert!(
        paths.contains(&"agents/ceo".to_string()),
        "a folder a concurrent write landed a child into mid-rollback must survive: {paths:?}"
    );
    assert!(
        paths.contains(&"agents/ceo/raced-in.md".to_string()),
        "the concurrently-landed child must not be erased by the rollback: {paths:?}"
    );
}

/// A reserved root is never a rollback target even when handed in: an empty
/// `agents/` is ordinary boot scaffolding, and the next boot re-lays it.
#[tokio::test]
async fn rollback_never_removes_a_reserved_root() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    let root = ws
        .adopt_or_create_folder(&company, None, AGENTS_ROOT, WorkspaceOrigin::Seed)
        .await
        .unwrap()
        .into_node()
        .id;

    rollback_empty_minted_folders(ws.as_ref(), &company, &[root]).await;

    assert!(
        tree_paths(&ws, &company)
            .await
            .contains(&"agents".to_string()),
        "an empty reserved root must never be swept"
    );
}
