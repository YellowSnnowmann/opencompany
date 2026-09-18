//! Workspace scaffold: the desks root, legacy (capitalised/raw-id) folder
//! adoption, and cross-root isolation.

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

/// The desk minter is the same shape one root over — and since issue #645
/// it is the *only* thing that ever creates `desks/`. Deliberately run with
/// no scaffold at all: the first call must mint the root and the member
/// folder together, which is what lets boot stop laying down an empty root
/// nothing was filling.
///
/// The root it mints stamps `Seed`, exactly as the boot scaffold used to,
/// so no consumer can tell a lazily-minted root from the old eager one. The
/// desk folder stamps `Seed` too, because a desk is not an agent and
/// `WorkspaceOrigin` has no way to name one.
#[tokio::test]
async fn ensure_desk_folder_mints_the_desks_root_on_first_use() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");

    let first = ensure_desk_folder(ws.as_ref(), &company, "creative_studio")
        .await
        .unwrap();
    let second = ensure_desk_folder(ws.as_ref(), &company, "creative_studio")
        .await
        .unwrap();

    assert_eq!(first, second, "a second call minted a rival folder");
    assert_eq!(
        tree_paths(&ws, &company).await,
        vec!["desks", "desks/creative-studio"],
        "the root appears with its first occupant, and brings nothing else"
    );
    let nodes = ws.tree(&company).await.unwrap();
    let desk = nodes.iter().find(|n| n.id == first).unwrap();
    assert_eq!(desk.kind, NodeKind::Folder);
    assert_eq!(desk.created_by, WorkspaceOrigin::Seed);
    let root = nodes.iter().find(|n| n.name == DESKS_ROOT).unwrap();
    assert_eq!(root.kind, NodeKind::Folder);
    assert_eq!(
        root.created_by,
        WorkspaceOrigin::Seed,
        "a lazily-minted root must carry the stamp boot used to give it"
    );
}

/// The migration story for every company that booted before issue #645: its
/// `desks/` root already exists, and the scaffold must leave it completely
/// alone rather than notice it is no longer managed and tidy it away.
///
/// The scaffold only ever looks up the names in `SYSTEM_ROOTS`, so a
/// `desks/` node is not even inspected — id, authorship and contents all
/// survive untouched.
#[tokio::test]
async fn a_pre_existing_desks_root_survives_the_scaffold_untouched() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("legacy");
    ws.create(
        &company,
        &WorkspaceNode {
            id: "legacy-desks".to_string(),
            name: DESKS_ROOT.to_string(),
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
    assert_eq!(
        paths(&nodes),
        vec![
            "agents",
            "artifacts",
            "artifacts/readme.md",
            "desks",
            "secrets",
            "secrets/readme.md"
        ],
        "dropping `desks/` from the scaffold must not delete an existing one"
    );
    let desks = nodes.iter().find(|n| n.name == DESKS_ROOT).unwrap();
    assert_eq!(desks.id, "legacy-desks", "the existing root must be kept");
    assert_eq!(
        desks.created_by,
        WorkspaceOrigin::Operator,
        "an unmanaged root's authorship must not be rewritten"
    );
}

/// The un-managed counterpart to `several_nodes_sharing_a_root_name_are_
/// left_alone`: duplicate `Desks` nodes are not a collision the scaffold
/// has to resolve any more, they are simply none of its business — and the
/// root it *does* manage still provisions beside them.
#[tokio::test]
async fn duplicate_desks_nodes_do_not_disturb_the_scaffold() {
    let (dir, ws) = store().await;
    let company = CompanyId::new("acme");
    seed_duplicate_roots(dir.path(), &company, DESKS_ROOT, &["dup-a", "dup-b"]).await;

    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    let nodes = ws.tree(&company).await.unwrap();
    assert_eq!(
        nodes.iter().filter(|n| n.name == DESKS_ROOT).count(),
        2,
        "an unmanaged name must be neither deduplicated nor added to"
    );
    assert_eq!(
        nodes.iter().filter(|n| n.name == AGENTS_ROOT).count(),
        1,
        "an odd name elsewhere is no reason to withhold a managed root"
    );
}

/// The two roots stay independent: minting a desk folder does not reach
/// into `agents/`, and vice versa.
#[tokio::test]
async fn the_two_roots_do_not_leak_into_each_other() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");

    ensure_agent_folder(ws.as_ref(), &company, "shared")
        .await
        .unwrap();
    ensure_desk_folder(ws.as_ref(), &company, "shared")
        .await
        .unwrap();

    assert_eq!(
        tree_paths(&ws, &company).await,
        vec!["agents", "agents/shared", "desks", "desks/shared"]
    );
}

/// The names the scaffold mints follow the workspace naming rule, so a
/// fresh company's tree is uniform from the first boot rather than mixing
/// `Agents/` with `playbooks/` the moment anybody puts something in it.
#[tokio::test]
async fn the_scaffolded_names_are_lowercase_and_dashed() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();
    ensure_agent_folder(ws.as_ref(), &company, "page_builder")
        .await
        .unwrap();

    let paths = tree_paths(&ws, &company).await;
    assert!(
        paths.contains(&"agents/page-builder".to_string()),
        "a snake_case roster id should mint a dashed folder: {paths:?}"
    );
    for path in &paths {
        assert_eq!(
            *path,
            crate::company::workspace_names::kebab_path(path),
            "the scaffold minted a name outside the rule: {path}"
        );
    }
}

/// A company created before the rule has `Agents/`, and must not grow a
/// second lowercase root beside it — that would put one agent's home in two
/// places, with neither view complete.
#[tokio::test]
async fn a_legacy_capitalised_root_is_adopted_not_duplicated() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    let legacy = ws
        .adopt_or_create_folder(&company, None, "Agents", WorkspaceOrigin::Operator)
        .await
        .unwrap()
        .into_node()
        .id;

    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();
    let home = ensure_agent_folder(ws.as_ref(), &company, "ceo")
        .await
        .unwrap();

    let nodes = ws.tree(&company).await.unwrap();
    assert_eq!(
        nodes
            .iter()
            .filter(|n| n.parent_id.is_none() && n.name.eq_ignore_ascii_case("agents"))
            .count(),
        1,
        "the legacy root should be adopted, not joined by a twin: {:?}",
        paths(&nodes)
    );
    assert_eq!(
        nodes.iter().find(|n| n.id == home).unwrap().parent_id,
        Some(legacy),
        "the member folder belongs under the root that already existed"
    );
}

/// The other half of the same upgrade: the member folder itself was named
/// by the roster id verbatim, which differs from its dashed form by a
/// character rather than by case, so `find` cannot see it.
#[tokio::test]
async fn a_legacy_member_folder_named_by_the_raw_id_is_adopted() {
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
    let legacy = ws
        .adopt_or_create_folder(
            &company,
            Some(&root_id),
            "page_builder",
            agent("page_builder"),
        )
        .await
        .unwrap()
        .into_node()
        .id;

    let adopted = ensure_agent_folder(ws.as_ref(), &company, "page_builder")
        .await
        .unwrap();

    assert_eq!(adopted, legacy, "one agent, one folder, across the upgrade");
    let nodes = ws.tree(&company).await.unwrap();
    assert!(
        !nodes.iter().any(|n| n.name == "page-builder"),
        "a rival dashed folder would split the agent's work: {:?}",
        paths(&nodes)
    );
}

// -- the created-vs-adopted signal and the compensating rollback (#1801) --

/// The tracked minter reports whether *this* call created the member folder:
/// `true` the first time, `false` once it is only adopting what stands. That
/// signal is what lets a failed write know which folder it, and only it,
/// brought into existence.
#[tokio::test]
async fn ensure_agent_folder_tracked_reports_created_then_adopted() {
    let (_dir, ws) = store().await;
    let company = CompanyId::new("acme");
    ensure_workspace_scaffold(ws.as_ref(), &company)
        .await
        .unwrap();

    let (first, created) = ensure_agent_folder_tracked(ws.as_ref(), &company, "ceo")
        .await
        .unwrap();
    assert!(created, "the first call mints the member folder");

    let (second, created_again) = ensure_agent_folder_tracked(ws.as_ref(), &company, "ceo")
        .await
        .unwrap();
    assert!(!created_again, "a second call adopts rather than minting");
    assert_eq!(first, second, "and hands back the same folder");
}
