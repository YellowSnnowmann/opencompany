//! Tests for the own-folder lifecycle tools (issue #671).

use std::sync::Arc;

use super::*;
use crate::company::workspace_scaffold::{ensure_agent_folder, ensure_workspace_scaffold};
use crate::harness::workspace_tools::tests::{TEST_AGENT, agent_origin, file, folder, ws};
use crate::ports::artifacts::{ArtifactKind, ArtifactRecord, ArtifactStore};
use crate::ports::types::CompanyId;
use crate::ports::workspace::{WorkspaceNode, WorkspaceStore};
use crate::store::FsOps;

/// The revision [`file`] stamps, and therefore the CAS token every note in
/// these tests answers to.
pub(crate) const NOTE_REV: u64 = 2_000;
/// The revision [`folder`] stamps.
pub(crate) const FOLDER_REV: u64 = 1_000;

/// A live workspace shaped like the one the lifecycle tools were written for:
/// the boot scaffold, this agent's own home holding a note and an empty folder,
/// a teammate's home holding a note, plus `standards/` and a root `README.md`
/// to have something outside the agent's reach.
///
/// Carries an **artifact store** alongside the workspace store, because
/// `build_agent` always wires one and `workspace_delete` reads it to decide
/// whether a removal is recoverable. A fixture without one would exercise only
/// the [`History::Unknown`] branch and quietly stop checking the sentence the
/// agent actually reads.
pub(crate) struct Home {
    _dir: tempfile::TempDir,
    pub(crate) store: Arc<dyn WorkspaceStore>,
    pub(crate) artifacts: Arc<dyn ArtifactStore>,
    pub(crate) company: CompanyId,
}

pub(crate) async fn own_home(company: &str) -> Home {
    let dir = tempfile::tempdir().expect("tempdir");
    let ops = Arc::new(FsOps::new(dir.path()));
    let store: Arc<dyn WorkspaceStore> = ops.clone();
    let artifacts: Arc<dyn ArtifactStore> = ops;
    let id = CompanyId::new(company);

    store
        .create(&id, &folder("f-standards", "standards", None), None)
        .await
        .expect("folder");
    store
        .create(
            &id,
            &file("n-eng", "Engineering standards.md", Some("f-standards")),
            Some("# Engineering\nReview every PR."),
        )
        .await
        .expect("note");
    store
        .create(&id, &file("n-readme", "readme.md", None), Some("# Root"))
        .await
        .expect("readme");
    ensure_workspace_scaffold(store.as_ref(), &id)
        .await
        .unwrap();

    let mine = ensure_agent_folder(store.as_ref(), &id, TEST_AGENT)
        .await
        .unwrap();
    // Agent-authored on purpose: a rename must be provably origin-preserving,
    // and an operator-stamped fixture could not tell a preserved stamp from a
    // restamped one.
    store
        .create(
            &id,
            &WorkspaceNode {
                created_by: agent_origin(),
                updated_by: agent_origin(),
                ..file("n-draft", "Draft.md", Some(&mine))
            },
            Some("# Draft"),
        )
        .await
        .unwrap();
    store
        .create(&id, &folder("f-archive", "archive", Some(&mine)), None)
        .await
        .unwrap();

    let theirs = ensure_agent_folder(store.as_ref(), &id, "cmo")
        .await
        .unwrap();
    store
        .create(
            &id,
            &file("n-mate", "Plan.md", Some(&theirs)),
            Some("# Plan"),
        )
        .await
        .unwrap();

    Home {
        _dir: dir,
        store,
        artifacts,
        company: id,
    }
}

impl Home {
    /// The delete tool as the builder wires it: this agent, this company, the
    /// artifact store attached.
    pub(crate) fn deleter(&self) -> WorkspaceDeleteTool {
        WorkspaceDeleteTool::new(
            ws(self.store.clone(), self.company.clone())
                .with_artifacts(Some(self.artifacts.clone())),
        )
    }

    pub(crate) fn renamer(&self) -> WorkspaceRenameTool {
        WorkspaceRenameTool::new(ws(self.store.clone(), self.company.clone()))
    }

    pub(crate) async fn tree(&self) -> Vec<WorkspaceNode> {
        self.store.tree(&self.company).await.unwrap()
    }

    /// Whether a node id is still in the tree.
    pub(crate) async fn has(&self, id: &str) -> bool {
        self.tree().await.iter().any(|node| node.id == id)
    }

    pub(crate) async fn read(&self, id: &str) -> (WorkspaceNode, String) {
        self.store
            .read(&self.company, id)
            .await
            .unwrap()
            .expect("the node is still there")
    }

    pub(crate) async fn node(&self, id: &str) -> WorkspaceNode {
        self.read(id).await.0
    }

    /// This agent's home folder id, read live.
    pub(crate) async fn home_id(&self) -> String {
        self.tree()
            .await
            .iter()
            .find(|n| n.name == TEST_AGENT)
            .expect("the home folder")
            .id
            .clone()
    }

    /// Add a note directly inside this agent's own folder.
    pub(crate) async fn add_own(&self, node: WorkspaceNode, content: &str) {
        let parent = self.home_id().await;
        self.store
            .create(
                &self.company,
                &WorkspaceNode {
                    parent_id: Some(parent),
                    ..node
                },
                Some(content),
            )
            .await
            .unwrap();
    }

    /// Add a binary node directly inside this agent's own folder.
    pub(crate) async fn add_own_binary(&self, id: &str, name: &str, bytes: &[u8]) {
        let parent = self.home_id().await;
        let node = WorkspaceNode {
            mime: Some("image/png".to_string()),
            ..file(id, name, Some(&parent))
        };
        self.store
            .create_binary(&self.company, &node, bytes)
            .await
            .unwrap();
    }

    /// Record `node_id` as a published deliverable, so deleting it is the
    /// recoverable case.
    pub(crate) async fn publish(&self, artifact_id: &str, node_id: &str, body: &str) {
        let mut record = ArtifactRecord::new(
            artifact_id,
            "t-1",
            "Launch spec",
            ArtifactKind::Markdown,
            body,
            TEST_AGENT,
            1,
        );
        record.stamp_workspace_node(node_id);
        self.artifacts.upsert(&self.company, &record).await.unwrap();
    }
}

/// An artifact store whose `list` always errors, so `history_of`'s
/// `published_record_for_node` call fails on every delete.
pub(crate) struct FailingArtifacts;

#[async_trait::async_trait]
impl ArtifactStore for FailingArtifacts {
    async fn list(
        &self,
        _company: &CompanyId,
        _task_id: Option<&str>,
    ) -> crate::Result<Vec<crate::ports::artifacts::ArtifactRecord>> {
        Err(crate::error::OpenCompanyError::Store("boom".into()))
    }
    async fn get(
        &self,
        _company: &CompanyId,
        _id: &str,
    ) -> crate::Result<Option<crate::ports::artifacts::ArtifactRecord>> {
        unreachable!("history_of only lists")
    }
    async fn upsert(
        &self,
        _company: &CompanyId,
        _artifact: &crate::ports::artifacts::ArtifactRecord,
    ) -> crate::Result<()> {
        unreachable!("history_of only lists")
    }
    async fn delete(&self, _company: &CompanyId, _id: &str) -> crate::Result<bool> {
        unreachable!("history_of only lists")
    }
}
