use super::*;
use crate::store::FsOps;

#[derive(Clone, Copy)]
enum ManifestFault {
    Err,
    Vanished,
}

struct ManifestFaultStore {
    inner: Arc<dyn WorkspaceStore>,
    fault: ManifestFault,
}

impl ManifestFaultStore {
    async fn is_manifest(&self, company: &CompanyId, id: &str) -> bool {
        self.inner
            .tree(company)
            .await
            .map(|nodes| {
                nodes
                    .iter()
                    .any(|n| n.id == id && n.name.eq_ignore_ascii_case(MANIFEST_NAME))
            })
            .unwrap_or(false)
    }
}

#[async_trait]
impl WorkspaceStore for ManifestFaultStore {
    async fn tree(&self, company: &CompanyId) -> crate::Result<Vec<WorkspaceNode>> {
        self.inner.tree(company).await
    }
    async fn read(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> crate::Result<Option<(WorkspaceNode, String)>> {
        if self.is_manifest(company, id).await {
            return match self.fault {
                ManifestFault::Err => Err(crate::error::OpenCompanyError::Conflict(
                    "store busy".to_string(),
                )),
                ManifestFault::Vanished => Ok(None),
            };
        }
        self.inner.read(company, id).await
    }
    async fn read_capped(
        &self,
        company: &CompanyId,
        id: &str,
        max_bytes: u64,
    ) -> crate::Result<Option<(WorkspaceNode, String, u64)>> {
        self.inner.read_capped(company, id, max_bytes).await
    }
    async fn write_with_revision(
        &self,
        company: &CompanyId,
        id: &str,
        content: &str,
        author: WorkspaceOrigin,
        expected_updated_at: Option<u64>,
    ) -> crate::Result<WorkspaceNode> {
        self.inner
            .write_with_revision(company, id, content, author, expected_updated_at)
            .await
    }
    async fn create(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        content: Option<&str>,
    ) -> crate::Result<()> {
        self.inner.create(company, node, content).await
    }
    async fn adopt_or_create_folder(
        &self,
        company: &CompanyId,
        parent: Option<&str>,
        name: &str,
        origin: WorkspaceOrigin,
    ) -> crate::Result<FolderClaim> {
        self.inner
            .adopt_or_create_folder(company, parent, name, origin)
            .await
    }
    async fn create_binary(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        bytes: &[u8],
    ) -> crate::Result<WorkspaceNode> {
        self.inner.create_binary(company, node, bytes).await
    }
    async fn write_binary(
        &self,
        company: &CompanyId,
        id: &str,
        bytes: &[u8],
        mime: Option<&str>,
        author: WorkspaceOrigin,
    ) -> crate::Result<WorkspaceNode> {
        self.inner
            .write_binary(company, id, bytes, mime, author)
            .await
    }
    async fn read_bytes(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> crate::Result<Option<(WorkspaceNode, crate::ports::workspace::BlobStream)>> {
        self.inner.read_bytes(company, id).await
    }
    async fn rename_move(
        &self,
        company: &CompanyId,
        id: &str,
        name: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> crate::Result<WorkspaceNode> {
        self.inner.rename_move(company, id, name, parent).await
    }
    async fn swap_files(
        &self,
        company: &CompanyId,
        expected_id: Option<&str>,
        replacement_id: &str,
        name: &str,
    ) -> crate::Result<Option<WorkspaceNode>> {
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

const SOURCE_TSX: &str = r#"
import * as React from "react";

export default function Page() {
  return <div>Revenue</div>;
}
"#;

const SAVED_MANIFEST: &str = "title = \"Revenue\"\ndescription = \"Monthly revenue\"\nicon = \"chart\"\nnav_visible = false\n";

fn company() -> CompanyId {
    CompanyId::new("acme")
}

async fn seeded(manifest_body: &str) -> (tempfile::TempDir, Arc<dyn WorkspaceStore>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let pages = CompanyPages::new(store.clone(), company(), "builder".to_string());
    let write = PagesWriteTool::new(pages.clone());
    let result = write
        .execute(json!({"slug": "revenue", "title": "Revenue", "source": SOURCE_TSX}))
        .await
        .expect("execute ok");
    assert!(!result.is_error, "seed write: {result:?}");
    let manifest = pages
        .page("revenue")
        .await
        .unwrap()
        .unwrap()
        .manifest
        .unwrap();
    store
        .write_with_revision(
            &company(),
            &manifest.id,
            manifest_body,
            WorkspaceOrigin::Agent {
                id: "builder".to_string(),
            },
            None,
        )
        .await
        .expect("seed manifest");
    (dir, store)
}

fn faulty(inner: &Arc<dyn WorkspaceStore>, fault: ManifestFault) -> CompanyPages {
    let store: Arc<dyn WorkspaceStore> = Arc::new(ManifestFaultStore {
        inner: inner.clone(),
        fault,
    });
    CompanyPages::new(store, company(), "builder".to_string())
}

async fn snapshot(store: &Arc<dyn WorkspaceStore>) -> (String, u64, String) {
    let pages = CompanyPages::new(store.clone(), company(), "builder".to_string());
    let bundle = pages.page("revenue").await.unwrap().unwrap();
    let manifest = bundle.manifest.unwrap();
    let source = bundle.source.unwrap();
    let body = store
        .read(&company(), &manifest.id)
        .await
        .unwrap()
        .unwrap()
        .1;
    let source_body = store.read(&company(), &source.id).await.unwrap().unwrap().1;
    (body, manifest.updated_at_millis, source_body)
}

#[tokio::test]
async fn pages_write_refuses_when_the_manifest_read_fails_and_writes_nothing() {
    for fault in [ManifestFault::Err, ManifestFault::Vanished] {
        let (_dir, store) = seeded(SAVED_MANIFEST).await;
        let before = snapshot(&store).await;
        let write = PagesWriteTool::new(faulty(&store, fault));
        let result = write
            .execute(json!({"slug": "revenue", "title": "Other", "nav_visible": true}))
            .await
            .expect("execute ok");
        assert!(result.is_error, "must refuse: {result:?}");
        let out = result.output();
        assert!(
            out.contains("Could not read page `revenue`'s manifest"),
            "{out}"
        );
        assert!(out.contains("Nothing was written"), "{out}");
        assert!(
            out.contains("do not recreate the page under another slug"),
            "{out}"
        );
        assert_eq!(snapshot(&store).await, before, "nothing may change");
    }
}

#[tokio::test]
async fn pages_write_refuses_an_unparseable_manifest_without_a_title() {
    let (_dir, store) = seeded("title = [not toml").await;
    let before = snapshot(&store).await;
    let pages = CompanyPages::new(store.clone(), company(), "builder".to_string());
    let result = PagesWriteTool::new(pages)
        .execute(json!({"slug": "revenue", "description": "x"}))
        .await
        .expect("execute ok");
    assert!(result.is_error, "must refuse: {result:?}");
    let out = result.output();
    assert!(out.contains("could not be parsed"), "{out}");
    assert!(out.contains("Nothing was written"), "{out}");
    assert!(out.contains("fields you omit will be reset"), "{out}");
    assert_eq!(snapshot(&store).await, before);
}

#[tokio::test]
async fn pages_write_replaces_an_unparseable_manifest_when_given_a_title() {
    let (_dir, store) = seeded("title = [not toml").await;
    let pages = CompanyPages::new(store.clone(), company(), "builder".to_string());
    let result = PagesWriteTool::new(pages)
        .execute(json!({"slug": "revenue", "title": "Fresh"}))
        .await
        .expect("execute ok");
    assert!(!result.is_error, "{result:?}");
    assert!(
        result
            .output()
            .contains("The old page.toml could not be parsed and was replaced"),
        "{result:?}"
    );
    let (body, _, _) = snapshot(&store).await;
    let manifest: PageManifest = toml::from_str(&body).expect("parses now");
    assert_eq!(manifest.title, "Fresh");
    assert!(manifest.description.is_none());
    assert!(manifest.nav_visible);
}

#[tokio::test]
async fn pages_write_keeps_a_readable_manifest_without_a_replacement_note() {
    let (_dir, store) = seeded(SAVED_MANIFEST).await;
    let pages = CompanyPages::new(store.clone(), company(), "builder".to_string());
    let result = PagesWriteTool::new(pages)
        .execute(json!({"slug": "revenue", "icon": "bars"}))
        .await
        .expect("execute ok");
    assert!(!result.is_error, "{result:?}");
    assert!(!result.output().contains("could not be parsed"));
    let (body, _, _) = snapshot(&store).await;
    let manifest: PageManifest = toml::from_str(&body).unwrap();
    assert_eq!(manifest.description.as_deref(), Some("Monthly revenue"));
    assert_eq!(manifest.icon.as_deref(), Some("bars"));
    assert!(!manifest.nav_visible);
}

#[tokio::test]
async fn pages_list_and_read_say_an_unreadable_manifest_is_unknown_not_empty() {
    let (_dir, store) = seeded(SAVED_MANIFEST).await;
    let pages = faulty(&store, ManifestFault::Err);
    let list = PagesListTool::new(pages.clone())
        .execute(json!({}))
        .await
        .unwrap()
        .output();
    assert!(
        list.contains("- revenue: (manifest could not be read:"),
        "{list}"
    );
    assert!(list.contains("unknown, not empty"), "{list}");

    let read = PagesReadTool::new(pages)
        .execute(json!({"slug": "revenue"}))
        .await
        .unwrap();
    assert!(!read.is_error);
    let out = read.output();
    assert!(out.contains("its manifest could not be read"), "{out}");
    assert!(out.contains("Do not rewrite it with pages_write"), "{out}");
    assert!(
        out.contains("--- BEGIN page.tsx ---"),
        "source still renders: {out}"
    );
}

#[tokio::test]
async fn pages_list_and_read_flag_an_unparseable_manifest() {
    let (_dir, store) = seeded("title = [not toml").await;
    let pages = CompanyPages::new(store.clone(), company(), "builder".to_string());
    let list = PagesListTool::new(pages.clone())
        .execute(json!({}))
        .await
        .unwrap()
        .output();
    assert!(
        list.contains("- revenue: \"revenue\" (page.toml could not be parsed"),
        "{list}"
    );
    let out = PagesReadTool::new(pages)
        .execute(json!({"slug": "revenue"}))
        .await
        .unwrap()
        .output();
    assert!(out.contains("page.toml could not be parsed"), "{out}");
    assert!(out.contains("unless you pass a `title`"), "{out}");
    assert!(!out.contains("title=\"revenue\""), "{out}");
}
