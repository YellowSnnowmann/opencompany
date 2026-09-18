//! Publish and concurrent-publish tests: a deliverable landing under an
//! agent's artifacts folder, path normalization, binary payloads, and the
//! shape-change/first-publish races (split out of `artifact_mirror_tests.rs`).

use std::sync::Arc;

use super::artifact_mirror_tests_support::*;
use super::*;

/// The headline: a published deliverable lands in the shared tree, under
/// `artifacts/<agent-id>/`, attributed to the agent that published it.
///
/// The member folder is asserted rather than assumed because it does not
/// exist beforehand — member folders are minted on first use (#570), so
/// this proves `materialize` calls the minter instead of expecting a
/// folder somebody else laid down. The root it hangs off is the
/// deliverables root, never the publishing agent's scratch home: filing a
/// deliverable beside its author's working notes is what made "what has
/// this company produced?" unanswerable by navigation.
#[tokio::test]
async fn a_publish_lands_under_the_agents_own_artifacts_folder_it_mints() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let id = materialize(ws, &co, target("launch.md", "# Launch"))
        .await
        .expect("materialize")
        .node_id;

    assert_eq!(
        path_of(ws, &co, &id).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/launch.md")
    );
    let (node, body) = ws.read(&co, &id).await.unwrap().expect("the node exists");
    assert_eq!(body, "# Launch");
    assert_eq!(node.kind, NodeKind::File);
    assert_eq!(
        node.created_by,
        WorkspaceOrigin::Agent {
            id: "cmo".to_string()
        },
        "a published deliverable is the agent's work, and the tree must say so"
    );
}

/// A sandbox path with a space and a capital in it becomes a tree path
/// under the workspace naming rule.
///
/// The sandbox is the agent's own scratch and it names files however it
/// likes; the tree is what the operator reads, and one document there has
/// one spelling. Every interior segment goes through the rule too, not just
/// the file, or a deliverable would land in `specs/` beside `Specs/`.
#[tokio::test]
async fn a_published_path_is_normalized_into_the_tree() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let id = materialize(ws, &co, target("Specs/Launch Plan.md", "# Launch"))
        .await
        .expect("materialize")
        .node_id;

    assert_eq!(
        path_of(ws, &co, &id).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/specs/launch-plan.md")
    );
}

/// Two spellings of one sandbox path are one node in the tree, and the
/// second publish revises the first rather than opening a rival beside it.
///
/// Without this the normalization would be worse than no rule at all: a
/// path that resolved differently per publish is exactly the ambiguity the
/// mirror refuses everywhere else.
#[tokio::test]
async fn two_spellings_of_one_path_revise_one_node() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("Launch Plan.md", "v1"))
        .await
        .expect("first")
        .node_id;
    let second = materialize(ws, &co, target("launch-plan.md", "v2"))
        .await
        .expect("second")
        .node_id;

    assert_eq!(first, second, "one deliverable, one node");
    let (_, body) = ws.read(&co, &second).await.unwrap().expect("the node");
    assert_eq!(body, "v2");
}

/// A **binary** publish lands real bytes in the tree (issue #553).
///
/// This is the payoff of the whole issue: before it, a generated image
/// became a reference record naming a sandbox path, and wiping the sandbox
/// left the digest pointing at nothing. Now the same publish produces a
/// node the operator can open, on every backend.
#[tokio::test]
async fn a_binary_publish_lands_real_bytes_under_the_agents_folder() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();
    let png: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0xff, 0xfe, 0x00];

    let id = materialize(
        ws,
        &co,
        PublishTarget {
            payload: MirrorPayload::Bytes {
                bytes: &png,
                mime: "image/png",
            },
            ..target("shots/hero.png", "")
        },
    )
    .await
    .expect("materialize")
    .node_id;

    assert_eq!(
        path_of(ws, &co, &id).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/shots/hero.png")
    );
    let (node, stream) = ws
        .read_bytes(&co, &id)
        .await
        .unwrap()
        .expect("the payload is retrievable");
    assert_eq!(node.mime.as_deref(), Some("image/png"));
    assert_eq!(node.size, Some(png.len() as u64));
    assert_eq!(
        node.created_by,
        WorkspaceOrigin::Agent {
            id: "cmo".to_string()
        }
    );
    let mut got = Vec::new();
    {
        use futures::StreamExt;
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            got.extend_from_slice(&chunk.unwrap());
        }
    }
    assert_eq!(got, png, "the published bytes are the stored bytes");
}

/// Re-publishing the same path as a different shape replaces the node
/// rather than failing.
///
/// Neither write path converts a note into a payload or back — the store
/// refuses both — so a markdown draft later re-exported as a PDF would
/// otherwise error on every publish. The path is the deliverable's
/// identity, so the node is replaced and the history stays on the artifact
/// chain.
#[tokio::test]
async fn republishing_a_note_as_a_payload_replaces_the_node() {
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
    .expect("a shape change must not fail the publish")
    .node_id;

    let (node, _) = ws
        .read_bytes(&co, &second)
        .await
        .unwrap()
        .expect("the new node holds bytes");
    assert_eq!(node.mime.as_deref(), Some("application/pdf"));
    assert_eq!(
        path_of(ws, &co, &second).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/report.md"),
        "the deliverable keeps its path"
    );
    assert!(
        ws.read(&co, &first).await.unwrap().is_none(),
        "the superseded node is gone, not left beside its replacement"
    );
}

/// The reverse shape change is equally important: a generated payload can
/// later be republished as editable prose without asking either write API
/// to violate its type guard.
#[tokio::test]
async fn republishing_a_payload_as_a_note_replaces_the_node() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(
        ws,
        &co,
        PublishTarget {
            payload: MirrorPayload::Bytes {
                bytes: &[0x25, 0x50, 0x44, 0x46],
                mime: "application/pdf",
            },
            ..target("report.md", "")
        },
    )
    .await
    .unwrap()
    .node_id;
    let second = materialize(
        ws,
        &co,
        PublishTarget {
            existing_node_id: Some(&first),
            ..target("report.md", "# Editable")
        },
    )
    .await
    .expect("a binary-to-text shape change must succeed")
    .node_id;

    let (node, body) = ws.read(&co, &second).await.unwrap().unwrap();
    assert_eq!(body, "# Editable");
    assert!(!node.is_binary());
    assert_eq!(
        path_of(ws, &co, &second).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/report.md")
    );
    assert!(ws.read_bytes(&co, &first).await.unwrap().is_none());

    // …and the staging name does not survive. The text side stages through
    // plain `create` rather than `create_binary`, so it is a different pair
    // of store calls from the text→bytes case above: a swap that promoted
    // the node but left a sibling behind would satisfy every assertion
    // before this one.
    let nodes = ws.tree(&co).await.unwrap();
    let parent = node
        .parent_id
        .clone()
        .expect("the replacement has a parent");
    let siblings: Vec<&WorkspaceNode> = nodes
        .iter()
        .filter(|n| n.parent_id.as_deref() == Some(parent.as_str()))
        .collect();
    assert_eq!(
        siblings.iter().filter(|n| n.name == "report.md").count(),
        1,
        "exactly one node carries the deliverable's name: {siblings:?}"
    );
    assert!(
        !siblings.iter().any(|n| n.name.contains(".publishing-")),
        "no staging name may survive a successful publish: {siblings:?}"
    );
}

/// Two publishers can prepare the same shape-changing path concurrently.
/// Both must reach the real store's swap boundary before either proceeds;
/// exactly one wins, and the loser cannot create a duplicate final path or
/// leak its staging node.
#[tokio::test]
async fn concurrent_shape_changes_have_one_winner_and_no_duplicate_path() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("report.md", "# Draft"))
        .await
        .unwrap()
        .node_id;

    let racing = PausedSwap(ops.clone(), Arc::new(tokio::sync::Barrier::new(2)));
    let left = materialize(
        &racing,
        &co,
        PublishTarget {
            payload: MirrorPayload::Bytes {
                bytes: &[0x25, 0x50, 0x44, 0x46],
                mime: "application/pdf",
            },
            existing_node_id: Some(&first),
            ..target("report.md", "")
        },
    );
    let right = materialize(
        &racing,
        &co,
        PublishTarget {
            payload: MirrorPayload::Bytes {
                bytes: &[0x89, b'P', b'N', b'G'],
                mime: "image/png",
            },
            existing_node_id: Some(&first),
            ..target("report.md", "")
        },
    );
    let (left, right) = tokio::join!(left, right);
    let outcomes = [left, right];
    assert_eq!(
        outcomes.iter().filter(|result| result.is_ok()).count(),
        1,
        "exactly one compare-and-swap must win: {outcomes:?}"
    );
    let loser = outcomes
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one publisher loses");
    assert!(
        loser.to_string().contains("another publish"),
        "the refusal must say what happened: {loser}"
    );

    let nodes = ws.tree(&co).await.unwrap();
    let named: Vec<&WorkspaceNode> = nodes.iter().filter(|n| n.name == "report.md").collect();
    assert_eq!(
        named.len(),
        1,
        "the final path must continuously have one winner: {named:?}"
    );
    assert!(
        !nodes.iter().any(|n| n.name.contains(".publishing-")),
        "the losing compare-and-swap must consume its staged node: {nodes:?}"
    );
}

/// Issue #697, the sibling race: two **first** publishes of a path that
/// does not exist yet.
///
/// Both resolve the path to `None` — correctly, at the instant they look —
/// and before the fix both then created, leaving two nodes under one name.
/// That state does not decay: `resolve_file` answers a duplicated name with
/// `Conflict`, so a race lasting microseconds refuses every later publish to
/// that deliverable, for every agent, permanently.
///
/// Reuses `PausedSwap` unchanged, which is the point of routing creates
/// through the same primitive: both publishers are held at the store's
/// compare-and-swap boundary and released together, so the interleaving is
/// forced rather than hoped for. A test that merely ran two publishes
/// concurrently would pass on a machine that happened to serialize them.
#[tokio::test]
async fn two_first_publishes_of_one_path_have_one_winner_and_no_duplicate() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    // A different deliverable is published first, purely to mint the agent
    // and task folders the racers will share. Without it each publisher
    // walks `ensure_artifact_folder` / `resolve_folder` itself and mints its
    // OWN parent, so the two `report.md` nodes land under different folders
    // and never contend for one path — the test would pass while asserting
    // nothing about the race it names. (That folder walk is racy in its own
    // right; it is a separate defect from this one and is not what this
    // test pins.)
    materialize(ws, &co, target("seed.md", "# Seed"))
        .await
        .expect("seeding the shared folders");

    // The path under test must still not exist: this is the create arm.
    let before = ws.tree(&co).await.unwrap();
    assert!(
        !before.iter().any(|n| n.name == "report.md"),
        "the race is about a path that does not exist yet: {before:?}"
    );

    let racing = PausedSwap(ops.clone(), Arc::new(tokio::sync::Barrier::new(2)));
    let left = materialize(&racing, &co, target("report.md", "# From the left"));
    let right = materialize(&racing, &co, target("report.md", "# From the right"));
    let (left, right) = tokio::join!(left, right);
    let outcomes = [left, right];

    assert_eq!(
        outcomes.iter().filter(|result| result.is_ok()).count(),
        1,
        "exactly one first publish may create the path: {outcomes:?}"
    );
    let loser = outcomes
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one publisher loses");
    assert!(
        loser.to_string().contains("another publish"),
        "the refusal must say what happened: {loser}"
    );

    let nodes = ws.tree(&co).await.unwrap();
    let named: Vec<&WorkspaceNode> = nodes.iter().filter(|n| n.name == "report.md").collect();
    assert_eq!(
        named.len(),
        1,
        "one name, one node — a duplicate here is permanent: {named:?}"
    );
    assert!(
        !nodes.iter().any(|n| n.name.contains(".publishing-")),
        "the loser must consume its staged node: {nodes:?}"
    );

    // The path stays publishable. This is the assertion that speaks to why
    // the issue ranks the defect as it does: a duplicate would make every
    // future publish refuse, so proving the winner can still be revised is
    // proving the damage did not happen.
    materialize(ws, &co, target("report.md", "# A later revision"))
        .await
        .expect("the surviving path must still accept a publish");
    let after = ws.tree(&co).await.unwrap();
    assert_eq!(
        after.iter().filter(|n| n.name == "report.md").count(),
        1,
        "and revising it must not fork the path either: {after:?}"
    );
}
