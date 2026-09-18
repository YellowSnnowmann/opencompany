//! Task-folder-sharing and replacement tests: two publishers needing one
//! folder, failed-replacement rollback, and the operator-version /
//! retired-node lookup rules (split out of `artifact_mirror_tests.rs`).

use super::artifact_mirror_tests_support::*;
use super::*;

/// Issue #759, the folder half of the publish walk: two publishers needing
/// the same **task folder** that does not exist yet.
///
/// The filenames differ on purpose. Issue #697 already made two publishers
/// contending for one *file* path resolve to a single winner, so a test
/// using one filename would be satisfied by that fix alone and would say
/// nothing about the folder above it. Different names means both files must
/// land — and they can only both land if the folder they land in is one
/// folder.
///
/// The last assertion is the one that speaks to severity. A duplicated
/// folder is not a transient: `resolve_folder`'s ambiguity arm answers
/// `Conflict` for every later publish beneath that path, for every agent,
/// permanently. Proving a third publish still works is proving the momentary
/// race did not become a standing outage.
#[tokio::test]
async fn two_publishes_needing_one_task_folder_share_it_rather_than_duplicating_it() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    // Seed only the agent folder, by publishing something for a different
    // task. The task folder under test must still be absent — that is the
    // create arm this test exists for.
    materialize(
        ws,
        &co,
        PublishTarget {
            task_id: "t-seed",
            task_title: None,
            ..target("seed.md", "# Seed")
        },
    )
    .await
    .expect("seeding `agents/cmo/`");
    let agent_folder = ws
        .tree(&co)
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.name == "cmo")
        .expect("the agent folder exists")
        .id;
    assert!(
        named_children(&ws.tree(&co).await.unwrap(), &agent_folder, "t-9").is_empty(),
        "the race is about a task folder that does not exist yet"
    );

    // Four arrivals: each publisher reads the tree twice on this path — once
    // in the member-folder minter, once in `materialize` itself — and the
    // second rendezvous is the one that puts both of them on a snapshot with
    // no `t-9` in it.
    let racing = PausedTreeRead::new(ops.clone(), 4);
    let left = materialize(
        &racing,
        &co,
        PublishTarget {
            task_id: "t-9",
            task_title: None,
            ..target("left.md", "# From the left")
        },
    );
    let right = materialize(
        &racing,
        &co,
        PublishTarget {
            task_id: "t-9",
            task_title: None,
            ..target("right.md", "# From the right")
        },
    );
    let (left, right) = tokio::join!(left, right);
    let left = left.expect("the left publish must succeed");
    let right = right.expect("the right publish must succeed");

    let nodes = ws.tree(&co).await.unwrap();
    let task_folders = named_children(&nodes, &agent_folder, "t-9");
    assert_eq!(
        task_folders.len(),
        1,
        "one task folder, or every later publish beneath it is refused forever: {nodes:?}"
    );
    let task_folder = &task_folders[0];

    for (id, name) in [(&left.node_id, "left.md"), (&right.node_id, "right.md")] {
        let node = nodes
            .iter()
            .find(|n| &n.id == id)
            .unwrap_or_else(|| panic!("the published node for {name} is in the tree"));
        assert_eq!(node.name, name);
        assert_eq!(
            node.parent_id.as_deref(),
            Some(task_folder.as_str()),
            "both deliverables must land in the one task folder"
        );
    }

    // …and the path stays publishable, which a duplicate would have ended.
    materialize(
        ws,
        &co,
        PublishTarget {
            task_id: "t-9",
            task_title: None,
            ..target("later.md", "# A later publish")
        },
    )
    .await
    .expect("the shared task folder must still accept a publish");
    let after = ws.tree(&co).await.unwrap();
    assert_eq!(
        named_children(&after, &agent_folder, "t-9").len(),
        1,
        "and it must still be one folder: {after:?}"
    );
}

/// The same race one level up, on the folders the *scaffold* mints:
/// `agents/` and `agents/<agent-id>/`.
///
/// Nothing is seeded, so both publishers read an empty tree and both need
/// the root and the agent's own folder. Different task ids keep the task
/// folders apart, so the only thing they can contend for is the pair
/// `ensure_member_folder` claims — which is what pins that conversion
/// independently of `resolve_folder`'s.
///
/// Two arrivals rather than four: the rendezvous that matters is the member
/// minter's tree read, and budgeting only that one leaves a publisher that
/// fails there (the unfixed behaviour) unable to strand its partner.
#[tokio::test]
async fn two_publishers_minting_one_agent_folder_share_it_rather_than_duplicating_it() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();
    assert!(ws.is_empty(&co).await.unwrap(), "nothing is seeded");

    let racing = PausedTreeRead::new(ops.clone(), 2);
    let left = materialize(
        &racing,
        &co,
        PublishTarget {
            task_id: "t-left",
            task_title: None,
            ..target("left.md", "# From the left")
        },
    );
    let right = materialize(
        &racing,
        &co,
        PublishTarget {
            task_id: "t-right",
            task_title: None,
            ..target("right.md", "# From the right")
        },
    );
    let (left, right) = tokio::join!(left, right);
    left.expect("the left publish must succeed");
    right.expect("the right publish must succeed");

    let nodes = ws.tree(&co).await.unwrap();
    let roots: Vec<&WorkspaceNode> = nodes
        .iter()
        .filter(|n| n.parent_id.is_none() && n.name == ARTIFACTS_ROOT)
        .collect();
    assert_eq!(
        roots.len(),
        1,
        "one `{ARTIFACTS_ROOT}` root — two would make every agent folder ambiguous: {nodes:?}"
    );
    assert_eq!(
        named_children(&nodes, &roots[0].id, "cmo").len(),
        1,
        "one folder for the agent, or the agent can never publish again: {nodes:?}"
    );

    // The whole subtree stays usable afterwards.
    materialize(
        ws,
        &co,
        PublishTarget {
            task_id: "t-later",
            task_title: None,
            ..target("later.md", "# A later publish")
        },
    )
    .await
    .expect("the agent's folder must still accept a publish");
}

/// Issue #662. A shape-changing publish whose replacement **fails** must
/// leave the previous deliverable exactly where it was.
///
/// This is the defect: the old code deleted first, so a refused create —
/// over `max_blob_mb`, over `tree_quota_gb`, any store error — destroyed a
/// deliverable that was fine, and left the artifact record pointing at a
/// node id that no longer resolved. Quota refusal is a *designed* outcome of
/// this path, so the window was never theoretical.
#[tokio::test]
async fn a_failed_replacement_leaves_the_previous_deliverable_intact() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("report.md", "# Draft"))
        .await
        .unwrap()
        .node_id;
    let before = path_of(ws, &co, &first).await;

    // The same publish as the test above, but the store refuses to create
    // the binary node — the shape every quota refusal takes here.
    let refusing = RefusingCreate(ops.clone());
    let err = materialize(
        &refusing,
        &co,
        PublishTarget {
            payload: MirrorPayload::Bytes {
                bytes: &[0x25, 0x50, 0x44, 0x46],
                mime: "application/pdf",
            },
            existing_node_id: Some(&first),
            ..target("report.md", "")
        },
    )
    .await
    .expect_err("the refused create must fail the publish");
    assert!(
        err.to_string().contains("over quota"),
        "the store's own refusal must reach the caller: {err}"
    );

    let (node, body) = ws
        .read(&co, &first)
        .await
        .unwrap()
        .expect("the previous deliverable must still exist");
    assert_eq!(body, "# Draft", "its content is untouched");
    assert_eq!(node.name, "report.md");
    assert_eq!(
        path_of(ws, &co, &first).await,
        before,
        "and it still sits at the deliverable's path"
    );
}

/// The other half: a refused replacement must not leave the staged node
/// behind either. The workspace has to look exactly as it did before the
/// publish was attempted — one node at the path, and nothing beside it.
#[tokio::test]
async fn a_failed_replacement_leaves_no_staged_node_behind() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("report.md", "# Draft"))
        .await
        .unwrap()
        .node_id;
    // Sorted: `tree` promises the set, not an order, so comparing raw
    // sequences would fail on a reshuffle that changed nothing.
    let before = sorted_ids(ws, &co).await;

    let refusing = RefusingCreate(ops.clone());
    let _ = materialize(
        &refusing,
        &co,
        PublishTarget {
            payload: MirrorPayload::Bytes {
                bytes: &[0x25, 0x50, 0x44, 0x46],
                mime: "application/pdf",
            },
            existing_node_id: Some(&first),
            ..target("report.md", "")
        },
    )
    .await;

    let after = sorted_ids(ws, &co).await;
    assert_eq!(
        before, after,
        "a refused publish must change nothing at all"
    );
}

/// The success path still ends with exactly ONE node at the path — the
/// staging name is an implementation detail that must never survive.
///
/// Without this, the staged replacement could silently ship under
/// `report.md.publishing-<id>` and every assertion about the failure paths
/// would still pass.
#[tokio::test]
async fn a_successful_replacement_leaves_one_node_at_the_path() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("report.md", "# Draft"))
        .await
        .unwrap()
        .node_id;
    let second = materialize(
        ws,
        &co,
        PublishTarget {
            payload: MirrorPayload::Bytes {
                bytes: &[0x25, 0x50, 0x44, 0x46],
                mime: "application/pdf",
            },
            existing_node_id: Some(&first),
            ..target("report.md", "")
        },
    )
    .await
    .expect("the replacement lands")
    .node_id;

    let nodes = ws.tree(&co).await.unwrap();
    let parent = nodes
        .iter()
        .find(|n| n.id == second)
        .and_then(|n| n.parent_id.clone())
        .expect("the replacement has a parent");
    let named: Vec<&WorkspaceNode> = nodes
        .iter()
        .filter(|n| n.parent_id.as_deref() == Some(parent.as_str()))
        .collect();
    assert_eq!(
        named.iter().filter(|n| n.name == "report.md").count(),
        1,
        "exactly one node carries the deliverable's name: {named:?}"
    );
    assert!(
        !named.iter().any(|n| n.name.contains(".publishing-")),
        "no staging name may survive a successful publish: {named:?}"
    );
}

/// Interior segments become folders. Flattening to the basename would make
/// two genuinely different deliverables of one task collide on one node —
/// so the same basename in two directories must be two nodes.
#[tokio::test]
async fn the_same_basename_in_two_directories_is_two_nodes() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let spec = materialize(ws, &co, target("specs/a.md", "spec body"))
        .await
        .unwrap()
        .node_id;
    let doc = materialize(ws, &co, target("docs/a.md", "doc body"))
        .await
        .unwrap()
        .node_id;

    assert_ne!(spec, doc, "one node for two paths would lose a deliverable");
    assert_eq!(
        path_of(ws, &co, &spec).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/specs/a.md")
    );
    assert_eq!(
        path_of(ws, &co, &doc).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/docs/a.md")
    );
    assert_eq!(ws.read(&co, &spec).await.unwrap().unwrap().1, "spec body");
    assert_eq!(ws.read(&co, &doc).await.unwrap().unwrap().1, "doc body");
}

/// Re-publishing with the node from last time revises **that** node, so the
/// operator's open tab, deep link and backlinks all keep working. Nothing
/// new is created.
#[tokio::test]
async fn a_republish_revises_the_same_node() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("launch.md", "draft one"))
        .await
        .unwrap()
        .node_id;
    let before = ws.tree(&co).await.unwrap().len();

    let again = materialize(
        ws,
        &co,
        PublishTarget {
            existing_node_id: Some(&first),
            ..target("launch.md", "draft two")
        },
    )
    .await
    .unwrap()
    .node_id;

    assert_eq!(again, first, "a re-publish must not open a rival node");
    assert_eq!(ws.tree(&co).await.unwrap().len(), before, "nothing created");
    assert_eq!(ws.read(&co, &first).await.unwrap().unwrap().1, "draft two");
}

/// The operator's deletions stick. A re-publish whose remembered node is
/// gone mints a fresh one rather than resurrecting the old id — and the
/// path is the same, so the deliverable reappears where it belongs.
#[tokio::test]
async fn a_republish_after_the_operator_deleted_the_node_mints_a_fresh_one() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("launch.md", "draft one"))
        .await
        .unwrap()
        .node_id;
    assert!(ws.delete(&co, &first).await.unwrap());

    let again = materialize(
        ws,
        &co,
        PublishTarget {
            existing_node_id: Some(&first),
            ..target("launch.md", "draft two")
        },
    )
    .await
    .unwrap()
    .node_id;

    assert_ne!(again, first, "a deleted node must not be resurrected by id");
    assert_eq!(
        path_of(ws, &co, &again).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/launch.md"),
        "the replacement belongs at the same path"
    );
    assert_eq!(ws.read(&co, &again).await.unwrap().unwrap().1, "draft two");
}

/// Losing the id but not the node — a pre-#552 record re-published — must
/// adopt what is already at the path rather than mint a duplicate beside
/// it. Two nodes on one path is precisely the ambiguity the tool layer's
/// resolver then refuses for every agent.
#[tokio::test]
async fn a_publish_over_an_existing_path_adopts_it_rather_than_duplicating() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("launch.md", "draft one"))
        .await
        .unwrap()
        .node_id;
    // `existing_node_id: None` is exactly what a pre-#552 artifact carries.
    let again = materialize(ws, &co, target("launch.md", "draft two"))
        .await
        .unwrap()
        .node_id;

    assert_eq!(again, first, "the node already at the path must be adopted");
    assert_eq!(ws.read(&co, &first).await.unwrap().unwrap().1, "draft two");
}

/// A folder sitting where the note should go is refused, not overwritten:
/// the deliverable would otherwise vanish into a name that resolves to
/// something else entirely.
#[tokio::test]
async fn a_folder_in_the_notes_place_is_refused() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    // Publish once to lay down `agents/cmo/t-1/`, then put a folder where
    // the next publish's note wants to be.
    let sibling = materialize(ws, &co, target("other.md", "x"))
        .await
        .unwrap()
        .node_id;
    let parent = ws
        .read(&co, &sibling)
        .await
        .unwrap()
        .unwrap()
        .0
        .parent_id
        .unwrap();
    ws.adopt_or_create_folder(&co, Some(&parent), "launch.md", WorkspaceOrigin::Operator)
        .await
        .expect("claim the folder standing in the note's place");

    let refused = materialize(ws, &co, target("launch.md", "body"))
        .await
        .expect_err("a folder in the note's place must be refused");
    assert!(
        refused.to_string().contains("already exists as a folder"),
        "unexpected refusal: {refused}"
    );
}

/// A traversal segment reaching `create` as a node *name* would render a
/// path the console cannot navigate, and the sqlite/mongodb backends do not
/// reject one — so the guard lives here.
#[tokio::test]
async fn a_traversal_segment_is_refused() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    for bad in ["../escape.md", "specs/../../escape.md", "   "] {
        assert!(
            materialize(ws, &co, target(bad, "body")).await.is_err(),
            "`{bad}` must not name a workspace path"
        );
    }
}

// -- mirror_node_edit ---------------------------------------------------

/// An edit to a published node is recorded on the artifact chain, as a new
/// version by the editing author — which is what keeps `human_edit_diff`
/// answerable after the operator revises a deliverable in the console.
#[tokio::test]
async fn an_edit_to_a_published_node_appends_an_operator_version() {
    let (_dir, ops, co) = stores();
    let artifacts: &dyn ArtifactStore = ops.as_ref();

    let mut record = ArtifactRecord::new(
        "a-1",
        "t-1",
        "Launch",
        ArtifactKind::Markdown,
        "agent draft",
        "cmo",
        1,
    );
    record.stamp_workspace_node("node-1");
    artifacts.upsert(&co, &record).await.unwrap();

    let MirrorOutcome::Recorded(mirrored) = mirror_node_edit(
        artifacts,
        &co,
        "node-1",
        "operator draft",
        ArtifactAuthor::Operator,
        "operator",
        Some("operator edit before approval".to_string()),
    )
    .await
    .unwrap() else {
        panic!("a published node's edit is recorded");
    };

    assert_eq!(mirrored.artifact_id, "a-1");
    assert_eq!(mirrored.version, 2);

    let stored = artifacts.get(&co, "a-1").await.unwrap().unwrap();
    assert_eq!(stored.latest().unwrap().body, "operator draft");
    assert_eq!(stored.latest().unwrap().author, ArtifactAuthor::Operator);
    assert_eq!(
        stored.workspace_node_id(),
        Some("node-1"),
        "the appended version must carry the node too, or the NEXT edit's \
         reverse lookup finds nothing and mirroring silently stops"
    );
    assert!(
        stored.human_edit_diff().is_some(),
        "the whole reason the chain must see console edits"
    );
}

/// Most of the tree is ordinary notes. Editing one touches no artifact and
/// is not an error — the common answer, and deliberately silent.
#[tokio::test]
async fn an_edit_to_an_unpublished_node_records_nothing() {
    let (_dir, ops, co) = stores();
    let artifacts: &dyn ArtifactStore = ops.as_ref();

    let mut published = ArtifactRecord::new(
        "a-1",
        "t-1",
        "Launch",
        ArtifactKind::Markdown,
        "body",
        "cmo",
        1,
    );
    published.stamp_workspace_node("node-1");
    artifacts.upsert(&co, &published).await.unwrap();

    let mirrored = mirror_node_edit(
        artifacts,
        &co,
        "some-other-note",
        "new body",
        ArtifactAuthor::Operator,
        "operator",
        None,
    )
    .await
    .unwrap();

    assert!(matches!(mirrored, MirrorOutcome::Ordinary), "{mirrored:?}");
    let stored = artifacts.get(&co, "a-1").await.unwrap().unwrap();
    assert_eq!(
        stored.versions.len(),
        1,
        "an unrelated note must not append"
    );
}

/// The lookup matches the **latest** version's node, not any version's. An
/// artifact whose node the operator deleted and which was re-published into
/// a new one must mirror into the new node — matching on the stale id would
/// write today's edit into yesterday's history.
#[tokio::test]
async fn the_lookup_matches_the_current_node_not_a_retired_one() {
    let (_dir, ops, co) = stores();
    let artifacts: &dyn ArtifactStore = ops.as_ref();

    let mut record = ArtifactRecord::new(
        "a-1",
        "t-1",
        "Launch",
        ArtifactKind::Markdown,
        "v1",
        "cmo",
        1,
    );
    record.stamp_workspace_node("node-old");
    record.push_version("v2", ArtifactAuthor::Agent, "cmo", 2, None);
    record.stamp_workspace_node("node-new");
    artifacts.upsert(&co, &record).await.unwrap();

    assert!(
        matches!(
            mirror_node_edit(
                artifacts,
                &co,
                "node-old",
                "edit",
                ArtifactAuthor::Operator,
                "operator",
                None,
            )
            .await
            .unwrap(),
            MirrorOutcome::Ordinary
        ),
        "the retired node no longer addresses this artifact"
    );
    assert!(matches!(
        mirror_node_edit(
            artifacts,
            &co,
            "node-new",
            "edit",
            ArtifactAuthor::Operator,
            "operator",
            None,
        )
        .await
        .unwrap(),
        MirrorOutcome::Recorded(_)
    ));
}
