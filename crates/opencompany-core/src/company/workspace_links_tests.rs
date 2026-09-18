use std::sync::Arc;

use super::*;
use crate::ports::workspace::WorkspaceOrigin;
use crate::store::FsOps;

async fn note(ws: &Arc<dyn WorkspaceStore>, company: &CompanyId, name: &str, body: &str) -> String {
    let id = crate::ports::generate_id();
    ws.create(
        company,
        &crate::ports::workspace::WorkspaceNode {
            id: id.clone(),
            name: name.to_string(),
            kind: NodeKind::File,
            parent_id: None,
            updated_at_millis: 1,
            created_by: WorkspaceOrigin::Operator,
            updated_by: WorkspaceOrigin::Operator,
            mime: None,
            size: None,
            sha256: None,
            adopted: false,
        },
        Some(body),
    )
    .await
    .expect("create");
    id
}

/// Link text is how a document reads; the file name is what the tree is
/// kept in. `[[Close checklist]]` therefore has to find `close-checklist.md`
/// — otherwise the lowercase-dashed naming rule would have silently
/// unresolved every wiki link in every seeded company on the day it landed.
#[tokio::test]
async fn a_link_written_as_prose_backlinks_a_dashed_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let company = CompanyId::new("acme");

    let target = note(&ws, &company, "close-checklist.md", "# Close").await;
    note(
        &ws,
        &company,
        "readme.md",
        "Follow the [[Close checklist]] every month.",
    )
    .await;

    let (_, _, backlinks) = file_with_backlinks(ws.as_ref(), &company, &target)
        .await
        .expect("reads")
        .expect("the note exists");

    assert_eq!(
        backlinks
            .iter()
            .map(|n| n.name.as_str())
            .collect::<Vec<_>>(),
        vec!["readme.md"],
    );
}

/// The converse, for a company that predates the rule: a note still named
/// `Close checklist.md` is found by a link written in the new spelling.
#[tokio::test]
async fn a_dashed_link_backlinks_a_legacy_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let company = CompanyId::new("acme");

    let target = note(&ws, &company, "Close checklist.md", "# Close").await;
    note(&ws, &company, "readme.md", "See [[close-checklist]].").await;

    let (_, _, backlinks) = file_with_backlinks(ws.as_ref(), &company, &target)
        .await
        .expect("reads")
        .expect("the note exists");

    assert_eq!(backlinks.len(), 1, "{backlinks:?}");
}

/// Two notes that differ by more than separators stay unlinked — the rule
/// normalizes spelling, it does not make matching fuzzy.
#[tokio::test]
async fn an_unrelated_link_is_not_a_backlink() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let company = CompanyId::new("acme");

    let target = note(&ws, &company, "close-checklist.md", "# Close").await;
    note(&ws, &company, "readme.md", "See [[open-checklist]].").await;

    let (_, _, backlinks) = file_with_backlinks(ws.as_ref(), &company, &target)
        .await
        .expect("reads")
        .expect("the note exists");

    assert!(backlinks.is_empty(), "{backlinks:?}");
}
