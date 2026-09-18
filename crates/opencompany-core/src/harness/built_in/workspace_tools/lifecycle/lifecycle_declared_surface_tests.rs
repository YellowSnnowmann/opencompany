use std::sync::Arc;

use super::lifecycle_fixtures_tests::*;
use super::*;
use crate::harness::workspace_tools::tests::{text, ws};
use crate::ports::types::CompanyId;
use crate::ports::workspace::{
    BlobStream, FolderClaim, WorkspaceNode, WorkspaceOrigin, WorkspaceStore,
};

// ---------------------------------------------------------------------------
// The declared surface
// ---------------------------------------------------------------------------

/// Both tools are honest about what they do, and both answer a refusal as a
/// readable `ToolResult` rather than as an `Err` the harness renders as a
/// crash.
#[tokio::test]
async fn both_tools_declare_write_and_answer_refusals_as_results() {
    let home = own_home("acme").await;
    let delete = home.deleter();
    let rename = home.renamer();

    assert_eq!(delete.name(), WORKSPACE_DELETE_TOOL);
    assert_eq!(rename.name(), WORKSPACE_RENAME_TOOL);
    assert_eq!(delete.permission_level(), PermissionLevel::Write);
    assert_eq!(rename.permission_level(), PermissionLevel::Write);

    // Neither `path` nor `id`: the resolver's own message, delivered as a
    // result.
    for out in [
        delete
            .execute(json!({ "expected_updated_at": NOTE_REV }))
            .await
            .unwrap(),
        rename.execute(json!({ "new_name": "x.md" })).await.unwrap(),
    ] {
        assert!(out.is_error, "{}", text(&out));
        assert!(text(&out).contains("Invalid arguments"), "{}", text(&out));
    }

    // The schemas name only the arguments each tool actually reads.
    let delete_schema = delete.parameters_schema();
    assert_eq!(delete_schema["required"], json!(["expected_updated_at"]));
    assert_eq!(delete_schema["additionalProperties"], json!(false));
    let rename_schema = rename.parameters_schema();
    assert!(rename_schema["properties"]["new_parent"].is_object());
    assert_eq!(rename_schema["additionalProperties"], json!(false));
}

/// A workspace store that answers reads from a real one and makes the single
/// mutating call under test fail — either by reporting the node was already
/// gone (`Ok(false)`), or by erroring outright.
struct BrittleStore {
    inner: Arc<dyn WorkspaceStore>,
    vanished: bool,
}

#[async_trait::async_trait]
impl WorkspaceStore for BrittleStore {
    async fn tree(&self, company: &CompanyId) -> crate::Result<Vec<WorkspaceNode>> {
        self.inner.tree(company).await
    }

    async fn read(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> crate::Result<Option<(WorkspaceNode, String)>> {
        self.inner.read(company, id).await
    }

    async fn is_empty(&self, company: &CompanyId) -> crate::Result<bool> {
        self.inner.is_empty(company).await
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
    ) -> crate::Result<Option<(WorkspaceNode, BlobStream)>> {
        self.inner.read_bytes(company, id).await
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

    async fn delete(&self, _company: &CompanyId, _id: &str) -> crate::Result<bool> {
        if self.vanished {
            Ok(false)
        } else {
            Err(crate::error::OpenCompanyError::Store(
                "the workspace backend is unreachable".into(),
            ))
        }
    }

    async fn rename_move(
        &self,
        _company: &CompanyId,
        _id: &str,
        _new_name: Option<&str>,
        _new_parent: Option<Option<&str>>,
    ) -> crate::Result<WorkspaceNode> {
        Err(crate::error::OpenCompanyError::Store(
            "the workspace backend is unreachable".into(),
        ))
    }
}

/// The node survived the CAS check and then the store refused. The success
/// sentence claims a removal AND names whether the loss is permanent, so the
/// refusal has to reach the agent instead — as the store's stable error code
/// (per `store_reason`, the raw host-state string never does), and without
/// the node being reported gone.
#[tokio::test]
async fn a_delete_the_store_cannot_perform_is_refused_and_names_why() {
    let home = own_home("acme").await;
    let brittle: Arc<dyn WorkspaceStore> = Arc::new(BrittleStore {
        inner: home.store.clone(),
        vanished: false,
    });
    let deleter = WorkspaceDeleteTool::new(
        ws(brittle, home.company.clone()).with_artifacts(Some(home.artifacts.clone())),
    );

    let out = deleter
        .execute(json!({ "path": "agents/ceo/Draft.md", "expected_updated_at": NOTE_REV }))
        .await
        .unwrap();
    let message = text(&out);
    assert!(out.is_error, "{message}");
    assert!(
        message.contains("the workspace store failed"),
        "a store failure must be refused, not silently swallowed: {message}"
    );
    assert!(
        !message.contains("unreachable"),
        "the raw store reason is host state and must not reach the agent: {message}"
    );
    assert!(
        !message.contains("Deleted"),
        "a failed delete must not borrow the success sentence: {message}"
    );
    assert!(
        home.has("n-draft").await,
        "the note must still be there after a delete that did not happen"
    );
}

/// The CAS token narrows the window it does not close: between the tree
/// snapshot and the store call the node can go. `Ok(false)` is the store
/// saying exactly that, and it must not be read as a delete that worked.
#[tokio::test]
async fn a_node_that_vanished_between_the_check_and_the_delete_is_not_reported_deleted() {
    let home = own_home("acme").await;
    let brittle: Arc<dyn WorkspaceStore> = Arc::new(BrittleStore {
        inner: home.store.clone(),
        vanished: true,
    });
    let deleter = WorkspaceDeleteTool::new(
        ws(brittle, home.company.clone()).with_artifacts(Some(home.artifacts.clone())),
    );

    let out = deleter
        .execute(json!({ "path": "agents/ceo/Draft.md", "expected_updated_at": NOTE_REV }))
        .await
        .unwrap();
    let message = text(&out);
    assert!(out.is_error, "{message}");
    assert!(
        message.contains("already gone") && message.contains("Nothing was changed"),
        "the agent must be told the removal was somebody else's, not its own: {message}"
    );
}

/// The rename receipt hands back a new `rev` for the agent to reuse this turn.
/// A store that never performed the move has no rev to give, so the failure
/// must surface rather than the tool inventing a path it never landed at.
#[tokio::test]
async fn a_rename_the_store_cannot_perform_is_refused_and_leaves_the_path_alone() {
    let home = own_home("acme").await;
    let brittle: Arc<dyn WorkspaceStore> = Arc::new(BrittleStore {
        inner: home.store.clone(),
        vanished: false,
    });
    let renamer = WorkspaceRenameTool::new(ws(brittle, home.company.clone()));

    let out = renamer
        .execute(json!({ "path": "agents/ceo/Draft.md", "new_name": "final.md" }))
        .await
        .unwrap();
    let message = text(&out);
    assert!(out.is_error, "{message}");
    assert!(message.contains("the workspace store failed"), "{message}");
    assert!(
        !message.contains("unreachable"),
        "the raw store reason is host state and must not reach the agent: {message}"
    );
    assert!(
        !message.contains("Moved"),
        "a move that did not happen must not read as one: {message}"
    );
    let node = home.node("n-draft").await;
    assert_eq!(
        node.name, "Draft.md",
        "the node kept its name, and so must the report"
    );
}

/// `workspace_rename` carries no compare-and-swap token — the module header
/// argues one is unnecessary because a rename destroys nothing. That holds for
/// the *body*, and not for the ordering: two renames of one node are both
/// admitted against whatever the tree said when each of them looked, so the
/// second silently replaces the first's result and the first caller is told
/// its name landed. `workspace_delete` on the same node, in the same folder,
/// refuses exactly this with `expected_updated_at`.
///
/// The second rename here stands in for the concurrent one: it resolves the
/// node by id, sees a tree the first rename has already changed, and is
/// accepted anyway with nothing it could have passed to say which revision it
/// meant.
#[tokio::test]
#[ignore = "workspace_rename accepts no expected_updated_at, so a second rename of the same node cannot be ordered against the first"]
async fn a_second_rename_of_one_node_cannot_be_ordered_against_the_first() {
    let home = own_home("acme").await;
    let renamer = home.renamer();

    let first = renamer
        .execute(json!({ "id": "n-draft", "new_name": "launch-plan.md" }))
        .await
        .unwrap();
    assert!(!first.is_error, "{}", text(&first));

    let schema = renamer.parameters_schema();
    assert!(
        schema
            .get("properties")
            .and_then(|p| p.get("expected_updated_at"))
            .is_some(),
        "a rename must be able to say which revision of the node it meant, the way \
         `workspace_delete` does: {schema}"
    );

    let second = renamer
        .execute(json!({ "id": "n-draft", "new_name": "q3-plan.md" }))
        .await
        .unwrap();
    assert!(
        second.is_error,
        "a rename made against a view the first rename already invalidated must be refused, not \
         silently applied over it: {}",
        text(&second)
    );
}
