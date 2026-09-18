//! Workspace scaffold: per-agent folder minting, adoption, and error
//! handling when a member collides with something already there.

use std::sync::Arc;

use super::*;
use crate::store::FsOps;

fn agent(id: &str) -> WorkspaceOrigin {
    WorkspaceOrigin::Agent { id: id.to_string() }
}

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

// -- the lazy minters ---------------------------------------------------

/// The property #552's publish path depends on: minting on every publish
/// must be free after the first one, and must hand back the *same* parent
/// id so two deliverables land in one folder rather than two.
#[tokio::test]
async fn ensure_agent_folder_is_idempotent_and_stable() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    let first = ensure_agent_folder(ws.as_ref(), &company, "ceo")
        .await
        .unwrap();
    let second = ensure_agent_folder(ws.as_ref(), &company, "ceo")
        .await
        .unwrap();

    assert_eq!(first, second, "a second call minted a rival folder");
    assert_eq!(
        tree_paths(&ws, &company).await,
        vec![
            "agents",
            "agents/ceo",
            "artifacts",
            "artifacts/readme.md",
            "secrets",
            "secrets/readme.md"
        ]
    );
    let nodes = ws.tree(&company).await.unwrap();
    let ceo = nodes.iter().find(|n| n.name == "ceo").unwrap();
    assert_eq!(ceo.kind, NodeKind::Folder);
    assert_eq!(ceo.created_by, agent("ceo"));
}

/// One agent producing something must not conjure folders for the rest of
/// the roster — that is the whole difference from the eager design.
#[tokio::test]
async fn minting_one_agent_folder_leaves_the_roster_alone() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    ensure_agent_folder(ws.as_ref(), &company, "cmo")
        .await
        .unwrap();

    assert_eq!(
        tree_paths(&ws, &company).await,
        vec![
            "agents",
            "agents/cmo",
            "artifacts",
            "artifacts/readme.md",
            "secrets",
            "secrets/readme.md"
        ]
    );
}

/// A minter is also its own repair path: it creates the root when the
/// scaffold never ran, so a boot whose create fail-softed still ends up
/// with a usable `agents/` the first time an agent produces anything.
#[tokio::test]
async fn ensure_agent_folder_creates_the_root_it_needs() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");

    let id = ensure_agent_folder(ws.as_ref(), &company, "ceo")
        .await
        .unwrap();

    let nodes = ws.tree(&company).await.unwrap();
    assert_eq!(paths(&nodes), vec!["agents", "agents/ceo"]);
    let root = nodes.iter().find(|n| n.name == AGENTS_ROOT).unwrap();
    assert_eq!(root.created_by, WorkspaceOrigin::Seed);
    assert_eq!(nodes.iter().find(|n| n.id == id).unwrap().name, "ceo");
}

/// An operator's hand-made `Agents/ceo` is adopted, not duplicated.
#[tokio::test]
async fn ensure_agent_folder_adopts_an_existing_folder() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();
    let root_id = ws
        .tree(&company)
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.name == AGENTS_ROOT)
        .unwrap()
        .id;
    ws.create(
        &company,
        &WorkspaceNode {
            id: "hand-made".to_string(),
            name: "ceo".to_string(),
            kind: NodeKind::Folder,
            parent_id: Some(root_id),
            updated_at_millis: 1,
            created_by: WorkspaceOrigin::Operator,
            updated_by: WorkspaceOrigin::Operator,
            mime: None,
            size: None,
            sha256: None,
            adopted: false,
        },
        None,
    )
    .await
    .unwrap();

    let id = ensure_agent_folder(ws.as_ref(), &company, "ceo")
        .await
        .unwrap();

    assert_eq!(id, "hand-made");
    assert_eq!(
        ws.tree(&company)
            .await
            .unwrap()
            .iter()
            .find(|n| n.id == "hand-made")
            .unwrap()
            .created_by,
        WorkspaceOrigin::Operator,
        "adoption must not rewrite the operator's authorship"
    );
}

/// The minter has a caller waiting on an id, so a collision it cannot
/// resolve is an error rather than a warn-and-carry-on — there is no id to
/// hand back and pretending otherwise would strand the caller's write.
#[tokio::test]
async fn a_colliding_member_file_is_an_error_not_a_silent_skip() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();
    let root_id = ws
        .tree(&company)
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.name == AGENTS_ROOT)
        .unwrap()
        .id;
    ws.create(
        &company,
        &WorkspaceNode {
            id: "ceo-note".to_string(),
            name: "ceo".to_string(),
            kind: NodeKind::File,
            parent_id: Some(root_id),
            updated_at_millis: 1,
            created_by: WorkspaceOrigin::Operator,
            updated_by: WorkspaceOrigin::Operator,
            mime: None,
            size: None,
            sha256: None,
            adopted: false,
        },
        Some("# notes about the ceo"),
    )
    .await
    .unwrap();

    let err = ensure_agent_folder(ws.as_ref(), &company, "ceo")
        .await
        .expect_err("a colliding note must not resolve to a folder id");
    assert!(err.to_string().contains("ceo"), "{err}");
    assert_eq!(
        ws.tree(&company)
            .await
            .unwrap()
            .iter()
            .find(|n| n.name == "ceo")
            .unwrap()
            .kind,
        NodeKind::File,
        "the operator's note must not be shadowed by a folder of the same name"
    );
}

/// An id that is not a legal path segment would render an unaddressable or
/// traversal-shaped path, so it is refused before anything is created.
#[tokio::test]
async fn an_illegal_id_is_refused_and_creates_nothing() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");

    for id in ["../escape", "", ".", "a/b", "a\\b"] {
        ensure_agent_folder(ws.as_ref(), &company, id)
            .await
            .expect_err("`{id}` is not a legal path segment");
    }

    assert!(ws.is_empty(&company).await.unwrap());
}

/// Issue #1839: a folder a rival adopted survives that rival's rollback.
///
/// The residual half of #1801 removes a folder one caller minted and then
/// failed to write beneath. But a second caller can adopt the same folder in
/// the window — `adopt_or_create_folder` hands it back and stamps the lease —
/// and the minter's `rollback_empty_minted_folders` must then leave it
/// standing, because the adopter is about to write into it. The still-empty
/// guard alone could not tell the two apart; the lease is what does.
#[tokio::test]
async fn rollback_leaves_an_adopted_folder_but_sweeps_an_unadopted_one() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");

    // The minter creates `agents/cmo/` — its id is what a failed write would
    // roll back.
    let (adopted_id, created) = ensure_agent_folder_tracked(ws.as_ref(), &company, "cmo")
        .await
        .unwrap();
    assert!(created, "the first call minted the folder");

    // A rival publisher adopts the very same folder, taking the lease.
    let root_id = ws
        .tree(&company)
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.name == AGENTS_ROOT)
        .unwrap()
        .id;
    let claim = ws
        .adopt_or_create_folder(&company, Some(&root_id), "cmo", agent("cmo"))
        .await
        .unwrap();
    assert!(!claim.was_created(), "the rival adopted, it did not mint");
    assert!(claim.node().adopted, "adoption took the lease");

    // A second minted folder nobody adopts is the genuine #1801 leak.
    let (leaked_id, _) = ensure_agent_folder_tracked(ws.as_ref(), &company, "cto")
        .await
        .unwrap();

    // The minter's write failed; it rolls back both folders it minted.
    rollback_empty_minted_folders(
        ws.as_ref(),
        &company,
        &[adopted_id.clone(), leaked_id.clone()],
    )
    .await;

    let names: Vec<String> = ws
        .tree(&company)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert!(
        names.contains(&"cmo".to_string()),
        "an adopted empty folder must survive the minter's rollback: {names:?}"
    );
    assert!(
        !names.contains(&"cto".to_string()),
        "but an unadopted empty minted folder is still swept: {names:?}"
    );
}
