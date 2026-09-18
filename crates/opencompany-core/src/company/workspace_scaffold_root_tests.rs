//! Workspace scaffold: system-root provisioning, idempotency, and
//! adopting or leaving alone pre-existing filesystem state.

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

/// Seeds root folders that share `name` by writing the workspace index
/// directly.
///
/// The filesystem store refuses to *create* two siblings under one name,
/// because on that backend they would resolve to one path (issue #666).
/// The trees below are the ones that check what the scaffold does when it
/// nevertheless *finds* an ambiguous root — an index written before that
/// refusal existed, or one an id-keyed backend can still represent legally.
/// So the state is written rather than requested: going through `create`
/// would only re-assert the store's refusal and never reach the scaffold.
async fn seed_duplicate_roots(
    dir: &std::path::Path,
    company: &CompanyId,
    name: &str,
    ids: &[&str],
) {
    let index: std::collections::HashMap<String, WorkspaceNode> = ids
        .iter()
        .map(|id| {
            (
                (*id).to_string(),
                WorkspaceNode {
                    id: (*id).to_string(),
                    name: name.to_string(),
                    kind: NodeKind::Folder,
                    parent_id: None,
                    updated_at_millis: 1,
                    created_by: WorkspaceOrigin::Operator,
                    updated_by: WorkspaceOrigin::Operator,
                    mime: None,
                    size: None,
                    sha256: None,
                    adopted: false,
                },
            )
        })
        .collect();
    let bundle = crate::store::Bundle::new(dir.to_path_buf(), company);
    tokio::fs::create_dir_all(bundle.workspace_dir())
        .await
        .expect("workspace dir");
    tokio::fs::write(
        bundle.workspace_index_json(),
        serde_json::to_vec(&index).expect("index json"),
    )
    .await
    .expect("seed index");
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

fn scaffold_paths() -> Vec<&'static str> {
    vec![
        "agents",
        "artifacts",
        "artifacts/readme.md",
        "secrets",
        "secrets/readme.md",
    ]
}

/// The scaffold has an empty agent root plus the operator-only secrets
/// folder and its explanatory note. It never creates roster member folders
/// or the unused `desks/` root.
#[tokio::test]
async fn it_provisions_one_empty_system_root() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");

    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    let nodes = ws.tree(&company).await.unwrap();
    assert_eq!(
        paths(&nodes),
        scaffold_paths(),
        "`desks/` has no producer, so boot must not lay it down"
    );
    for node in nodes.iter().filter(|node| node.kind == NodeKind::Folder) {
        assert_eq!(
            node.created_by,
            WorkspaceOrigin::Seed,
            "{} is runtime scaffolding, not anybody's writing",
            node.name
        );
    }
    // Both notes, by path: two roots now carry a `readme.md`, so a
    // find-by-name would assert against whichever the store happened to
    // return first and pass while one of them held the other's text.
    for (path, expected) in [
        ("secrets/readme.md", SECRETS_README),
        ("artifacts/readme.md", ARTIFACTS_README),
    ] {
        let readme = nodes
            .iter()
            .find(|node| path_of(&nodes, node) == path)
            .unwrap_or_else(|| panic!("{path} is missing from the scaffold"));
        let (_, body) = ws.read(&company, &readme.id).await.unwrap().unwrap();
        assert_eq!(body, expected, "{path}");
    }
}

/// The scaffold takes no roster and asks for none: a company with no agents
/// at all still gets the shape of its workspace. (This reverses the earlier
/// eager design, where an empty roster deliberately created nothing —
/// there, a root with no children was a stray; here it is the point.)
#[tokio::test]
async fn a_company_with_no_roster_still_gets_the_agents_root() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("solo");

    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    assert_eq!(tree_paths(&ws, &company).await, scaffold_paths());
}

/// The deliverables root is scaffolded; a teammate's folder beneath it is
/// not, and appears only when that teammate publishes.
///
/// The asymmetry is the whole design: the root says the company has
/// somewhere to put deliverables, a member folder says *this* teammate
/// delivered. An eager folder per roster member would make the second claim
/// on behalf of teammates that have produced nothing.
#[tokio::test]
async fn an_artifact_folder_is_minted_on_demand_beneath_a_scaffolded_root() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");

    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();
    assert!(
        !tree_paths(&ws, &company)
            .await
            .contains(&"artifacts/cmo".to_string()),
        "boot must not mint a folder for a teammate that has published nothing"
    );

    let first = ensure_artifact_folder(ws.as_ref(), &company, "cmo")
        .await
        .unwrap();
    let second = ensure_artifact_folder(ws.as_ref(), &company, "cmo")
        .await
        .unwrap();
    assert_eq!(first, second, "a second call minted a rival folder");

    let nodes = ws.tree(&company).await.unwrap();
    let mine = nodes.iter().find(|node| node.id == first).unwrap();
    assert_eq!(path_of(&nodes, mine), "artifacts/cmo");
    assert_eq!(mine.kind, NodeKind::Folder);
    assert_eq!(mine.created_by, agent("cmo"));
    assert!(
        !nodes
            .iter()
            .any(|node| path_of(&nodes, node) == "agents/cmo"),
        "publishing must not also mint the agent's scratch home"
    );
}

/// The property that lets this run on every boot.
#[tokio::test]
async fn it_is_idempotent() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");

    for _ in 0..3 {
        ensure_workspace_scaffold(ws.as_ref(), &company)
            .await
            .unwrap();
    }

    assert_eq!(tree_paths(&ws, &company).await, scaffold_paths());
}

/// An operator-made `Agents/` folder is adopted as-is rather than
/// duplicated — identity is by path, so a second root would make every
/// `agents/...` path permanently ambiguous.
#[tokio::test]
async fn an_existing_root_folder_is_adopted() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ws.create(
        &company,
        &WorkspaceNode {
            id: "hand-made".to_string(),
            name: AGENTS_ROOT.to_string(),
            kind: NodeKind::Folder,
            parent_id: None,
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

    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    let nodes = ws.tree(&company).await.unwrap();
    assert_eq!(paths(&nodes), scaffold_paths());
    let root = nodes.iter().find(|n| n.name == AGENTS_ROOT).unwrap();
    assert_eq!(root.id, "hand-made", "the operator's folder must be reused");
    assert_eq!(
        root.created_by,
        WorkspaceOrigin::Operator,
        "adoption must not rewrite the operator's authorship"
    );
}

/// Fail-closed: a root *file* named `Agents` is a collision this module has
/// no honest way to resolve, so it leaves it alone rather than shadowing
/// the operator's note with a rival folder of the same name — and creates
/// nothing else in its place.
#[tokio::test]
async fn a_root_file_is_left_alone_rather_than_shadowed() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ws.create(
        &company,
        &WorkspaceNode {
            id: "note".to_string(),
            name: AGENTS_ROOT.to_string(),
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
        Some("# not a folder"),
    )
    .await
    .unwrap();

    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    let nodes = ws.tree(&company).await.unwrap();
    assert_eq!(
        paths(&nodes),
        scaffold_paths(),
        "the collision must not be shadowed; unrelated scaffold still provisions"
    );
    assert_eq!(
        nodes.iter().find(|n| n.name == AGENTS_ROOT).unwrap().kind,
        NodeKind::File,
        "the operator's note must not be shadowed by a folder of the same name"
    );
}

/// Several root nodes sharing a reserved name is the other unresolvable
/// shape: adding a third would make it worse, so nothing is created.
#[tokio::test]
async fn several_nodes_sharing_a_root_name_are_left_alone() {
    let (dir, ws) = store().await;
    let company = CompanyId::new("acme");
    seed_duplicate_roots(dir.path(), &company, AGENTS_ROOT, &["dup-a", "dup-b"]).await;

    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    let nodes = ws.tree(&company).await.unwrap();
    assert_eq!(
        nodes.iter().filter(|n| n.name == AGENTS_ROOT).count(),
        2,
        "an ambiguous root must not gain a third candidate"
    );
    assert_eq!(
        paths(&nodes),
        vec![
            "agents",
            "agents",
            "artifacts",
            "artifacts/readme.md",
            "secrets",
            "secrets/readme.md"
        ],
        "only the unrelated secrets scaffold may be created beside the collision"
    );
}

/// The tree is company-scoped: scaffolding one company leaves another's
/// workspace untouched.
#[tokio::test]
async fn scaffolding_is_per_company() {
    let (_dir, ws) = store().await;
    let acme = CompanyId::new("acme");
    let other = CompanyId::new("other");

    ensure_workspace_scaffold(ws.as_ref(), &acme).await.unwrap();

    assert!(ws.is_empty(&other).await.unwrap());
}
