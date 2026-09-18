use std::sync::Arc;

use super::loose_store::LooseWorkspace;
use super::*;
use crate::ports::workspace::WorkspaceOrigin;

fn node(id: &str, name: &str, parent: Option<&str>, updated: u64) -> WorkspaceNode {
    WorkspaceNode {
        id: id.to_string(),
        name: name.to_string(),
        kind: NodeKind::Folder,
        parent_id: parent.map(str::to_string),
        updated_at_millis: updated,
        created_by: WorkspaceOrigin::Seed,
        updated_by: WorkspaceOrigin::Seed,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    }
}

fn folder(id: &str, name: &str, parent: Option<&str>) -> WorkspaceNode {
    node(id, name, parent, 1)
}

fn file(id: &str, name: &str, parent: Option<&str>) -> WorkspaceNode {
    WorkspaceNode {
        kind: NodeKind::File,
        ..folder(id, name, parent)
    }
}

fn moved_ids(folder: &MergedFolder) -> Vec<&str> {
    folder.moved.iter().map(|m| m.id.as_str()).collect()
}

/// The tree a `LooseWorkspace` holds, as `id → parent`, so an assertion says
/// where everything ended up rather than counting.
async fn placement(ws: &Arc<dyn WorkspaceStore>, company: &CompanyId) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = ws
        .tree(company)
        .await
        .unwrap()
        .into_iter()
        .map(|node| (node.id, node.parent_id.unwrap_or_else(|| "-".to_string())))
        .collect();
    out.sort();
    out
}

// -- the plan, on shapes the `fs` backend refuses to create ---------------

/// The whole point, in one tree: the race left two `reports/` folders, each
/// holding a different deliverable. The older one keeps its id, the newer
/// one's file moves across, and the emptied folder goes.
#[test]
fn a_duplicate_pair_folds_into_the_older_folder() {
    let nodes = vec![
        folder("root", "Agents", None),
        node("keep", "reports", Some("root"), 100),
        node("dupe", "reports", Some("root"), 200),
        file("a", "q1.md", Some("keep")),
        file("b", "q2.md", Some("dupe")),
    ];

    let plan = duplicate_folder_plan(&nodes);

    assert_eq!(plan.folders.len(), 1, "{plan:?}");
    let fold = &plan.folders[0];
    assert_eq!((fold.id.as_str(), fold.into_id.as_str()), ("dupe", "keep"));
    assert_eq!(moved_ids(fold), vec!["b"], "the newer twin's file moves");
    assert!(fold.removed, "the emptied duplicate goes");
    assert!(plan.residuals.is_empty(), "{:?}", plan.residuals);
}

/// The tiebreak, stated on its own: two folders written in the same
/// millisecond — which sqlite's millisecond clock makes ordinary for a race
/// — must still produce one stable answer, or a preview and the confirm that
/// follows it could disagree about which folder survives.
#[test]
fn a_tie_on_the_timestamp_is_broken_by_the_node_id() {
    let nodes = vec![
        node("zzz", "reports", None, 50),
        node("aaa", "reports", None, 50),
    ];

    let plan = duplicate_folder_plan(&nodes);

    assert_eq!(plan.folders.len(), 1);
    assert_eq!(plan.folders[0].into_id, "aaa");
    assert_eq!(plan.folders[0].id, "zzz");
}

/// The fixpoint: the duplicate holds a folder whose name is taken inside the
/// survivor, so that pair merges too — in the same run, and deep enough that
/// the emptiness has to cascade two levels for both folders to go.
#[test]
fn nested_folder_collisions_iterate_to_a_fixpoint() {
    let nodes = vec![
        node("keep", "reports", None, 100),
        node("dupe", "reports", None, 200),
        node("keep-q1", "q1", Some("keep"), 100),
        node("dupe-q1", "q1", Some("dupe"), 200),
        file("stray", "notes.md", Some("dupe-q1")),
    ];

    let plan = duplicate_folder_plan(&nodes);

    let folds: Vec<(&str, &str, bool)> = plan
        .folders
        .iter()
        .map(|f| (f.id.as_str(), f.into_id.as_str(), f.removed))
        .collect();
    assert_eq!(
        folds,
        vec![("dupe", "keep", true), ("dupe-q1", "keep-q1", true)],
        "both layers fold, and both empty out: {plan:?}"
    );
    assert_eq!(
        moved_ids(&plan.folders[1]),
        vec!["stray"],
        "the file two levels down moves into the surviving `q1`"
    );
    assert!(plan.residuals.is_empty(), "{:?}", plan.residuals);
}

/// The irreducible remainder. Both duplicates hold a `summary.md`; those are
/// two documents, not one, so neither moves and neither is overwritten — the
/// file is named as a residual and the duplicate folder stays standing
/// around it.
#[test]
fn a_file_collision_is_reported_and_moves_nothing() {
    let nodes = vec![
        node("keep", "reports", None, 100),
        node("dupe", "reports", None, 200),
        file("mine", "summary.md", Some("keep")),
        file("theirs", "summary.md", Some("dupe")),
        file("safe", "appendix.md", Some("dupe")),
    ];

    let plan = duplicate_folder_plan(&nodes);

    let fold = &plan.folders[0];
    assert_eq!(
        moved_ids(fold),
        vec!["safe"],
        "the child whose name is free still moves"
    );
    assert!(
        !fold.removed,
        "a duplicate still holding a document must not be deleted"
    );
    assert_eq!(
        plan.residuals,
        vec![Residual {
            id: "theirs".to_string(),
            name: "summary.md".to_string(),
            parent_id: Some("dupe".to_string()),
            cause: ResidualCause::FileInTheWay,
        }],
        "the operator is told exactly which document is still theirs to settle"
    );
}

/// A file and a folder sharing a name is a broken path too, and one this
/// pass cannot fix: merging the folders would leave the file sitting on the
/// path regardless. So the group is left entirely alone and reported.
#[test]
fn a_file_sharing_the_name_leaves_the_whole_group_untouched() {
    let nodes = vec![
        node("keep", "reports", None, 100),
        node("dupe", "reports", None, 200),
        file("note", "reports", None),
        file("inside", "q1.md", Some("dupe")),
    ];

    let plan = duplicate_folder_plan(&nodes);

    assert!(
        plan.folders.is_empty(),
        "nothing may move while a file shares the name: {plan:?}"
    );
    let reported: Vec<(&str, ResidualCause)> = plan
        .residuals
        .iter()
        .map(|r| (r.id.as_str(), r.cause))
        .collect();
    assert_eq!(
        reported,
        vec![
            ("keep", ResidualCause::FileSharesTheName),
            ("dupe", ResidualCause::FileSharesTheName),
            ("note", ResidualCause::FileSharesTheName),
        ]
    );
}

/// A healthy tree is not a duplicate set: same name under different parents,
/// and a folder beside a file it does not share a name with, are both
/// ordinary.
#[test]
fn a_tree_without_duplicates_yields_an_empty_plan() {
    let nodes = vec![
        folder("agents", "Agents", None),
        folder("desks", "Desks", None),
        folder("ceo", "reports", Some("agents")),
        folder("cmo", "reports", Some("desks")),
        file("note", "reports.md", Some("agents")),
    ];

    assert_eq!(duplicate_folder_plan(&nodes), RepairPlan::default());
}

/// Issue #1839: a node whose `parent_id` resolves to nothing is a true
/// orphan — the Race-2 shape a lockless backend leaves when a child insert
/// commits after its parent was read-deleted. The plan names every orphan,
/// both kinds, so the preview can show them and the apply half can act.
#[test]
fn a_dangling_parent_is_reported_as_a_residual() {
    let nodes = vec![
        file("orphan-file", "lost.md", Some("ghost")),
        folder("orphan-dir", "lost", Some("ghost")),
    ];

    let plan = duplicate_folder_plan(&nodes);

    assert!(
        plan.folders.is_empty(),
        "there is nothing to merge: {plan:?}"
    );
    let reported: Vec<(&str, ResidualCause)> = plan
        .residuals
        .iter()
        .map(|r| (r.id.as_str(), r.cause))
        .collect();
    assert_eq!(
        reported,
        vec![
            ("orphan-dir", ResidualCause::DanglingParent),
            ("orphan-file", ResidualCause::DanglingParent),
        ],
        "both orphans are named, sorted by id"
    );
}

/// A node whose parent *does* exist is not an orphan, so a healthy tree — and
/// the duplicate shapes above — never gain a spurious dangling residual.
#[test]
fn a_present_parent_is_never_dangling() {
    let nodes = vec![
        folder("root", "agents", None),
        folder("child", "cmo", Some("root")),
    ];

    assert_eq!(duplicate_folder_plan(&nodes), RepairPlan::default());
}

/// A parent cycle is the kind of damage this module gets called in to look
/// at, so the ancestor walk must terminate rather than hang the request.
#[test]
fn a_parent_cycle_does_not_hang_the_plan() {
    let nodes = vec![
        node("a", "reports", Some("b"), 100),
        node("b", "reports", Some("a"), 200),
    ];

    let plan = duplicate_folder_plan(&nodes);

    assert!(
        plan.folders.is_empty(),
        "neither twin can be moved into the other: {plan:?}"
    );
}

// -- the real run, over a store that permits the broken state ------------

fn store() -> Arc<dyn WorkspaceStore> {
    Arc::new(LooseWorkspace::default())
}

async fn seed(ws: &Arc<dyn WorkspaceStore>, company: &CompanyId, nodes: &[WorkspaceNode]) {
    for node in nodes {
        ws.create(company, node, Some("")).await.unwrap();
    }
}

/// The apply path end to end: the child relocates, keeps its id, and the
/// emptied duplicate is deleted.
#[tokio::test]
async fn it_merges_over_a_store_and_keeps_every_node_id() {
    let ws = store();
    let company = CompanyId::new("acme");
    seed(
        &ws,
        &company,
        &[
            node("keep", "reports", None, 100),
            node("dupe", "reports", None, 200),
            file("a", "q1.md", Some("keep")),
            file("b", "q2.md", Some("dupe")),
        ],
    )
    .await;

    let done = merge_duplicate_folders(ws.as_ref(), &company, false)
        .await
        .unwrap();

    assert!(done.folders[0].removed);
    assert_eq!(
        placement(&ws, &company).await,
        vec![
            ("a".to_string(), "keep".to_string()),
            ("b".to_string(), "keep".to_string()),
            ("keep".to_string(), "-".to_string()),
        ],
        "both files sit under the survivor, with the ids they were published as"
    );
}

/// A dry run is the confirm dialog's evidence: it names the fold and leaves
/// the tree exactly as it found it.
#[tokio::test]
async fn a_dry_run_names_the_fold_and_changes_nothing() {
    let ws = store();
    let company = CompanyId::new("acme");
    seed(
        &ws,
        &company,
        &[
            node("keep", "reports", None, 100),
            node("dupe", "reports", None, 200),
            file("b", "q2.md", Some("dupe")),
        ],
    )
    .await;
    let before = placement(&ws, &company).await;

    let preview = merge_duplicate_folders(ws.as_ref(), &company, true)
        .await
        .unwrap();

    assert_eq!(moved_ids(&preview.folders[0]), vec!["b"]);
    assert!(preview.folders[0].removed, "it would go");
    assert_eq!(placement(&ws, &company).await, before);
}

/// Acceptance criterion: a second run changes nothing. The pass is a
/// function of the current tree and holds no state, so an operator's double
/// click is harmless.
#[tokio::test]
async fn running_it_twice_changes_nothing_the_second_time() {
    let ws = store();
    let company = CompanyId::new("acme");
    seed(
        &ws,
        &company,
        &[
            node("keep", "reports", None, 100),
            node("dupe", "reports", None, 200),
            file("b", "q2.md", Some("dupe")),
        ],
    )
    .await;

    merge_duplicate_folders(ws.as_ref(), &company, false)
        .await
        .unwrap();
    let after_first = placement(&ws, &company).await;
    let second = merge_duplicate_folders(ws.as_ref(), &company, false)
        .await
        .unwrap();

    assert_eq!(
        second,
        RepairPlan::default(),
        "the second run found nothing"
    );
    assert_eq!(
        placement(&ws, &company).await,
        after_first,
        "and changed nothing either"
    );
}

/// The residual, over a real store: the rival document is still there, under
/// the id it always had, and the duplicate folder holding it is still
/// standing.
#[tokio::test]
async fn a_file_collision_survives_the_real_run_untouched() {
    let ws = store();
    let company = CompanyId::new("acme");
    seed(
        &ws,
        &company,
        &[
            node("keep", "reports", None, 100),
            node("dupe", "reports", None, 200),
            file("mine", "summary.md", Some("keep")),
            file("theirs", "summary.md", Some("dupe")),
        ],
    )
    .await;

    let done = merge_duplicate_folders(ws.as_ref(), &company, false)
        .await
        .unwrap();

    assert!(!done.folders[0].removed);
    assert_eq!(
        done.residuals
            .iter()
            .map(|r| (r.id.as_str(), r.cause))
            .collect::<Vec<_>>(),
        vec![("theirs", ResidualCause::FileInTheWay)]
    );
    assert_eq!(
        placement(&ws, &company).await,
        vec![
            ("dupe".to_string(), "-".to_string()),
            ("keep".to_string(), "-".to_string()),
            ("mine".to_string(), "keep".to_string()),
            ("theirs".to_string(), "dupe".to_string()),
        ],
        "both documents are exactly where they were"
    );
}

/// Issue #671's discipline, at this module's boundary: the emptiness that
/// authorises a delete is re-read, not remembered. A child that lands in the
/// duplicate between the plan and the delete keeps its folder standing —
/// and the port's `delete` is recursive, so this is the difference between
/// leaving a folder behind and taking a deliverable with it.
#[tokio::test]
async fn a_child_that_arrives_mid_merge_keeps_its_folder() {
    let loose = Arc::new(LooseWorkspace::default());
    let ws: Arc<dyn WorkspaceStore> = loose.clone();
    let company = CompanyId::new("acme");
    seed(
        &ws,
        &company,
        &[
            node("keep", "reports", None, 100),
            node("dupe", "reports", None, 200),
            file("b", "q2.md", Some("dupe")),
        ],
    )
    .await;

    // A publish lands inside the duplicate the instant its last child has
    // been relocated — the exact window the second read exists to see.
    loose.on_next_move(|nodes| nodes.push(file("late", "late.md", Some("dupe"))));

    let done = merge_duplicate_folders(ws.as_ref(), &company, false)
        .await
        .unwrap();

    assert!(
        !done.folders[0].removed,
        "the duplicate gained a deliverable and must survive: {done:?}"
    );
    assert!(
        ws.read(&company, "late").await.unwrap().is_some(),
        "and the deliverable itself must still be there"
    );
}

/// Issue #1839, the guaranteed net: a real run reaps a provably-empty orphan
/// **folder** and surfaces an orphan **file** rather than destroying it.
/// Both are injected past the store's parent check, because a lawful create
/// refuses a missing parent — the orphan only arises from the Race-2 the
/// reaper exists to clean up.
#[tokio::test]
async fn the_orphan_reaper_reaps_an_empty_folder_and_surfaces_a_file() {
    let loose = Arc::new(LooseWorkspace::default());
    let ws: Arc<dyn WorkspaceStore> = loose.clone();
    let company = CompanyId::new("acme");
    loose.inject(
        &company,
        vec![
            file("orphan-file", "lost.md", Some("ghost")),
            folder("orphan-dir", "lost", Some("ghost")),
        ],
    );

    let done = merge_duplicate_folders(ws.as_ref(), &company, false)
        .await
        .unwrap();

    // The empty orphan folder is gone from the residual list; the file stays.
    let surfaced: Vec<(&str, ResidualCause)> = done
        .residuals
        .iter()
        .map(|r| (r.id.as_str(), r.cause))
        .collect();
    assert_eq!(
        surfaced,
        vec![("orphan-file", ResidualCause::DanglingParent)],
        "the orphan file is surfaced; the empty orphan folder is not: {done:?}"
    );

    let remaining: Vec<String> = ws
        .tree(&company)
        .await
        .unwrap()
        .into_iter()
        .map(|node| node.id)
        .collect();
    assert!(
        remaining.contains(&"orphan-file".to_string()),
        "the orphan file must be surfaced, never destroyed: {remaining:?}"
    );
    assert!(
        !remaining.contains(&"orphan-dir".to_string()),
        "the empty orphan folder must be reaped: {remaining:?}"
    );
}

/// The reaper honours the module's standing invariant: a **non-empty** orphan
/// folder is not deleted. It stays surfaced, and its child — whose own parent
/// now resolves — is left where it is.
#[tokio::test]
async fn a_non_empty_orphan_folder_is_surfaced_not_reaped() {
    let loose = Arc::new(LooseWorkspace::default());
    let ws: Arc<dyn WorkspaceStore> = loose.clone();
    let company = CompanyId::new("acme");
    loose.inject(
        &company,
        vec![
            folder("orphan-dir", "lost", Some("ghost")),
            file("inside", "kept.md", Some("orphan-dir")),
        ],
    );

    let done = merge_duplicate_folders(ws.as_ref(), &company, false)
        .await
        .unwrap();

    // Only the top orphan is dangling — `inside`'s parent resolves — and it
    // holds a document, so `delete_if_empty` refuses it and it stays a
    // residual.
    assert_eq!(
        done.residuals
            .iter()
            .map(|r| (r.id.as_str(), r.cause))
            .collect::<Vec<_>>(),
        vec![("orphan-dir", ResidualCause::DanglingParent)],
    );
    assert!(
        ws.read(&company, "inside").await.unwrap().is_some(),
        "the child of a non-empty orphan folder is untouched"
    );
}
