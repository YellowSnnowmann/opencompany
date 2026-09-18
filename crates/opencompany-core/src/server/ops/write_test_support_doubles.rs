//! Test doubles shared by the `ops` write-plane split test files:
//! `FaultyArtifacts` (an `ArtifactStore` with one chosen fault) and
//! `RecordingReads` (a `WorkspaceStore` wrapper that counts bytes read).
//! Split out of `write_test_support.rs` to stay under the 750-line cap.

use super::*;

use crate::AppState;
use crate::ports::types::CompanyId;

pub(crate) struct FaultyArtifacts {
    pub(crate) listed: Vec<crate::ports::artifacts::ArtifactRecord>,
    pub(crate) list_fails: bool,
    pub(crate) upsert_fails: bool,
}

#[async_trait::async_trait]
impl crate::ports::artifacts::ArtifactStore for FaultyArtifacts {
    async fn list(
        &self,
        _: &CompanyId,
        _: Option<&str>,
    ) -> crate::Result<Vec<crate::ports::artifacts::ArtifactRecord>> {
        if self.list_fails {
            return Err(crate::error::OpenCompanyError::Store(
                "the artifact store is down".into(),
            ));
        }
        Ok(self.listed.clone())
    }
    async fn get(
        &self,
        _: &CompanyId,
        _: &str,
    ) -> crate::Result<Option<crate::ports::artifacts::ArtifactRecord>> {
        Ok(None)
    }
    async fn upsert(
        &self,
        _: &CompanyId,
        _: &crate::ports::artifacts::ArtifactRecord,
    ) -> crate::Result<()> {
        if self.upsert_fails {
            return Err(crate::error::OpenCompanyError::Store(
                "the disk is full".into(),
            ));
        }
        Ok(())
    }
    async fn delete(&self, _: &CompanyId, _: &str) -> crate::Result<bool> {
        Ok(false)
    }
}

pub(crate) async fn state_with_faulty_artifacts(
    home: &std::path::Path,
    artifacts: FaultyArtifacts,
) -> (AppState, CompanyId) {
    let state = state_with_company(home).await;
    let company = state.registry().list()[0].clone();
    let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest())
        .with_id(company.clone())
        .with_artifacts(std::sync::Arc::new(artifacts))
        .build()
        .await
        .expect("runtime");
    // `insert` replaces, so the routes now resolve through the faulty store
    // while the seeded admin on `state` carries over untouched.
    state
        .registry()
        .insert(company.clone(), std::sync::Arc::new(runtime));
    (state, company)
}

pub(crate) struct RecordingReads {
    inner: std::sync::Arc<dyn crate::ports::workspace::WorkspaceStore>,
    read_bytes: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl RecordingReads {
    pub(crate) fn new(inner: std::sync::Arc<dyn crate::ports::workspace::WorkspaceStore>) -> Self {
        Self {
            inner,
            read_bytes: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub(crate) fn bytes_read(&self, id: &str) -> u64 {
        self.read_bytes
            .lock()
            .unwrap()
            .get(id)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait::async_trait]
impl crate::ports::workspace::WorkspaceStore for RecordingReads {
    async fn admit_upload(&self, company: &CompanyId, name: &str, len: u64) -> crate::Result<()> {
        self.inner.admit_upload(company, name, len).await
    }

    async fn tree(
        &self,
        company: &CompanyId,
    ) -> crate::Result<Vec<crate::ports::workspace::WorkspaceNode>> {
        self.inner.tree(company).await
    }

    async fn read(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> crate::Result<Option<(crate::ports::workspace::WorkspaceNode, String)>> {
        let got = self.inner.read(company, id).await?;
        if let Some((_, body)) = &got {
            *self
                .read_bytes
                .lock()
                .unwrap()
                .entry(id.to_string())
                .or_default() += body.len() as u64;
        }
        Ok(got)
    }

    async fn read_capped(
        &self,
        company: &CompanyId,
        id: &str,
        max_bytes: u64,
    ) -> crate::Result<Option<(crate::ports::workspace::WorkspaceNode, String, u64)>> {
        self.inner.read_capped(company, id, max_bytes).await
    }

    async fn write_with_revision(
        &self,
        company: &CompanyId,
        id: &str,
        content: &str,
        author: crate::ports::workspace::WorkspaceOrigin,
        expected_updated_at: Option<u64>,
    ) -> crate::Result<crate::ports::workspace::WorkspaceNode> {
        self.inner
            .write_with_revision(company, id, content, author, expected_updated_at)
            .await
    }

    async fn create(
        &self,
        company: &CompanyId,
        node: &crate::ports::workspace::WorkspaceNode,
        content: Option<&str>,
    ) -> crate::Result<()> {
        self.inner.create(company, node, content).await
    }

    async fn adopt_or_create_folder(
        &self,
        company: &CompanyId,
        parent: Option<&str>,
        name: &str,
        origin: crate::ports::workspace::WorkspaceOrigin,
    ) -> crate::Result<crate::ports::workspace::FolderClaim> {
        self.inner
            .adopt_or_create_folder(company, parent, name, origin)
            .await
    }

    async fn create_binary(
        &self,
        company: &CompanyId,
        node: &crate::ports::workspace::WorkspaceNode,
        bytes: &[u8],
    ) -> crate::Result<crate::ports::workspace::WorkspaceNode> {
        self.inner.create_binary(company, node, bytes).await
    }

    async fn write_binary(
        &self,
        company: &CompanyId,
        id: &str,
        bytes: &[u8],
        mime: Option<&str>,
        author: crate::ports::workspace::WorkspaceOrigin,
    ) -> crate::Result<crate::ports::workspace::WorkspaceNode> {
        self.inner
            .write_binary(company, id, bytes, mime, author)
            .await
    }

    async fn read_bytes(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> crate::Result<
        Option<(
            crate::ports::workspace::WorkspaceNode,
            crate::ports::workspace::BlobStream,
        )>,
    > {
        self.inner.read_bytes(company, id).await
    }

    async fn rename_move(
        &self,
        company: &CompanyId,
        id: &str,
        name: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> crate::Result<crate::ports::workspace::WorkspaceNode> {
        self.inner.rename_move(company, id, name, parent).await
    }

    async fn swap_files(
        &self,
        company: &CompanyId,
        expected_id: Option<&str>,
        replacement_id: &str,
        name: &str,
    ) -> crate::Result<Option<crate::ports::workspace::WorkspaceNode>> {
        self.inner
            .swap_files(company, expected_id, replacement_id, name)
            .await
    }

    async fn delete(&self, company: &CompanyId, id: &str) -> crate::Result<bool> {
        self.inner.delete(company, id).await
    }

    async fn is_empty(&self, company: &CompanyId) -> crate::Result<bool> {
        self.inner.is_empty(company).await
    }
}
