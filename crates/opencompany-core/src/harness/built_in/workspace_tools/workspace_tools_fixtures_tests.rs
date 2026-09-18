use super::*;
use crate::store::FsOps;

// -- helpers ------------------------------------------------------------

/// The agent every test writes as, so an authorship assertion has a name to
/// check against.
pub(super) const TEST_AGENT: &str = "ceo";

/// A [`CompanyWorkspace`] pinned to `company`, writing as [`TEST_AGENT`].
pub(super) fn ws(store: Arc<dyn WorkspaceStore>, company: CompanyId) -> CompanyWorkspace {
    CompanyWorkspace::new(store, company, TEST_AGENT.to_string())
}

/// This agent's origin — what a create or a write must stamp.
pub(super) fn agent_origin() -> WorkspaceOrigin {
    WorkspaceOrigin::Agent {
        id: TEST_AGENT.to_string(),
    }
}

pub(super) fn folder(id: &str, name: &str, parent: Option<&str>) -> WorkspaceNode {
    WorkspaceNode {
        id: id.to_string(),
        name: name.to_string(),
        kind: NodeKind::Folder,
        parent_id: parent.map(str::to_string),
        updated_at_millis: 1_000,
        created_by: WorkspaceOrigin::Operator,
        updated_by: WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    }
}

pub(super) fn file(id: &str, name: &str, parent: Option<&str>) -> WorkspaceNode {
    WorkspaceNode {
        id: id.to_string(),
        name: name.to_string(),
        kind: NodeKind::File,
        parent_id: parent.map(str::to_string),
        updated_at_millis: 2_000,
        created_by: WorkspaceOrigin::Operator,
        updated_by: WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    }
}

/// A live `FsOps`-backed workspace seeded with a small tree, plus the
/// tempdir keeping it alive.
pub(super) async fn seeded(company: &str) -> (tempfile::TempDir, Arc<dyn WorkspaceStore>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ops: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let id = CompanyId::new(company);
    ops.create(&id, &folder("f-standards", "standards", None), None)
        .await
        .expect("folder");
    ops.create(
        &id,
        &file("n-eng", "engineering-standards.md", Some("f-standards")),
        Some("# Engineering\nReview every PR."),
    )
    .await
    .expect("note");
    ops.create(&id, &file("n-readme", "readme.md", None), Some("# Root"))
        .await
        .expect("readme");
    (dir, ops)
}

pub(super) fn text(result: &ToolResult) -> String {
    result.output()
}
