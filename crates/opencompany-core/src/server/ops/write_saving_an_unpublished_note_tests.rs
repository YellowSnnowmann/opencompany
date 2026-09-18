//! Integration tests for the `ops` write plane: tasks, memory, workspace,
//! skills, team, inbox-read, and desk chat — exercised end-to-end over the
//! router against a real fs-backed company.

use axum::http::StatusCode;
use serde_json::json;

use super::write_test_support::*;
use crate::ports::types::CompanyId;
use crate::store::FsCompanyStore;

/// Nearly every note in the tree is an ordinary note, not a deliverable.
/// Saving one must append nothing anywhere — the reverse lookup answering
/// "no artifact owns this" is the common case, and deliberately silent.
#[tokio::test]
async fn saving_an_unpublished_note_appends_no_artifact_version() {
    use crate::ports::artifacts::{ArtifactKind, ArtifactRecord, ArtifactStore};

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = state.registry().list()[0].clone();
    let runtime = state.registry().get(&company).expect("company");

    // A published artifact exists, but points at a DIFFERENT node.
    let mut published = ArtifactRecord::new(
        "art-1",
        "t-1",
        "Launch spec",
        ArtifactKind::Markdown,
        "deliverable",
        "ceo",
        1,
    );
    published.stamp_workspace_node("some-other-node");
    ArtifactStore::upsert(runtime.artifacts().as_ref(), &company, &published)
        .await
        .expect("seed");

    let (_, note) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "notes.md", "kind": "file", "content": "just a note"})),
    )
    .await;
    let node_id = note["id"].as_str().unwrap().to_string();

    let (status, _) = send(
        &state,
        "PUT",
        &format!("/api/v1/company/workspace/file/{node_id}"),
        Some(json!({"content": "still just a note"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, artifact) = send(&state, "GET", "/api/v1/company/artifacts/art-1", None).await;
    assert_eq!(
        artifact["versions"].as_array().unwrap().len(),
        1,
        "an ordinary note's save must not touch an unrelated artifact"
    );
}

/// The other direction of the same invariant: appending a version through the
/// Artifacts tab must push the new body into the deliverable's workspace note,
/// or the tree keeps serving a draft the history has superseded.
#[tokio::test]
async fn appending_an_artifact_version_updates_its_workspace_note() {
    use crate::ports::artifacts::{ArtifactKind, ArtifactRecord, ArtifactStore};
    use crate::ports::workspace::WorkspaceStore;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = state.registry().list()[0].clone();
    let runtime = state.registry().get(&company).expect("company");

    let (_, note) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "launch.md", "kind": "file", "content": "v1"})),
    )
    .await;
    let node_id = note["id"].as_str().unwrap().to_string();

    let mut published = ArtifactRecord::new(
        "art-1",
        "t-1",
        "Launch spec",
        ArtifactKind::Markdown,
        "v1",
        "ceo",
        1,
    )
    .with_source("launch.md");
    published.stamp_workspace_node(&node_id);
    ArtifactStore::upsert(runtime.artifacts().as_ref(), &company, &published)
        .await
        .expect("seed");

    let (status, appended) = send(
        &state,
        "POST",
        "/api/v1/company/artifacts/art-1/versions",
        Some(json!({"body": "v2, edited in the Artifacts tab"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        appended["versions"][1]["workspaceNodeId"], node_id,
        "the appended version keeps naming the node it lives in"
    );

    let (_, body) = WorkspaceStore::read(runtime.workspace().as_ref(), &company, &node_id)
        .await
        .unwrap()
        .expect("the note exists");
    assert_eq!(
        body, "v2, edited in the Artifacts tab",
        "the shared tree must not keep serving a superseded draft"
    );
}

/// An artifact with no workspace note — a legacy capture, or one recorded
/// while no tree was wired — appends exactly as it always did, with no node
/// write attempted and nothing invented for it.
#[tokio::test]
async fn appending_to_an_unmirrored_artifact_touches_no_note() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, created) = send(
        &state,
        "POST",
        "/api/v1/company/artifacts",
        Some(json!({"taskId": "t-1", "title": "Draft", "body": "v1"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_string();

    let (status, appended) = send(
        &state,
        "POST",
        &format!("/api/v1/company/artifacts/{id}/versions"),
        Some(json!({"body": "v2"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(appended["versions"].as_array().unwrap().len(), 2);
    assert!(
        appended["versions"][1].get("workspaceNodeId").is_none(),
        "nothing may invent a node for an artifact that has none"
    );
}

/// Issue #552 made every note save consult the artifact store, and an ordinary
/// note must not inherit that store's health.
///
/// Nearly the whole tree is ordinary notes. They own no artifact chain, and
/// their save touches the artifact store for one reason only — to ask whether
/// they are a deliverable. When that question cannot be answered, refusing the
/// save would discard an operator's typing to protect a chain the note does not
/// have.
#[tokio::test]
async fn an_ordinary_note_still_saves_when_the_artifact_store_cannot_be_read() {
    use crate::ports::workspace::WorkspaceStore;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, company) = state_with_faulty_artifacts(
        &home,
        FaultyArtifacts {
            listed: Vec::new(),
            list_fails: true,
            upsert_fails: false,
        },
    )
    .await;
    let runtime = state.registry().get(&company).expect("company");

    let (_, note) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "notes.md", "kind": "file", "content": "just a note"})),
    )
    .await;
    let node_id = note["id"].as_str().expect("node id").to_string();

    let (status, _) = send(
        &state,
        "PUT",
        &format!("/api/v1/company/workspace/file/{node_id}"),
        Some(json!({"content": "the operator kept typing"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an unreadable artifact store must not reject a plain note's save"
    );

    let (_, body) = WorkspaceStore::read(runtime.workspace().as_ref(), &company, &node_id)
        .await
        .unwrap()
        .expect("the note still exists");
    assert_eq!(
        body, "the operator kept typing",
        "the edit must actually land, not merely report success"
    );
}

/// The other direction, and the one the availability fix must not have cost:
/// once the store *has* answered and named this node a published deliverable,
/// a version that cannot be recorded still refuses the save.
///
/// This is the fail-closed guarantee the module exists for. A node written
/// behind a version that was never appended is the silent, permanent direction
/// — `human_edit_diff` would answer for a draft the operator had already
/// rewritten.
#[tokio::test]
async fn a_published_note_refuses_the_save_when_its_version_cannot_be_recorded() {
    use crate::ports::artifacts::{ArtifactKind, ArtifactRecord};
    use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceOrigin, WorkspaceStore};

    let home_dir = home();
    let home = home_dir.path().to_path_buf();

    // The store answers the lookup — this node IS a deliverable — but refuses
    // the append.
    let mut published = ArtifactRecord::new(
        "art-1",
        "t-1",
        "Launch spec",
        ArtifactKind::Markdown,
        "the agent's draft",
        "ceo",
        1,
    );
    published.stamp_workspace_node("node-published");
    let (state, company) = state_with_faulty_artifacts(
        &home,
        FaultyArtifacts {
            listed: vec![published],
            list_fails: false,
            upsert_fails: true,
        },
    )
    .await;
    let runtime = state.registry().get(&company).expect("company");

    // The node the artifact points at, created directly so its id is the one
    // the record was stamped with.
    WorkspaceStore::create(
        runtime.workspace().as_ref(),
        &company,
        &WorkspaceNode {
            id: "node-published".to_string(),
            name: "launch.md".to_string(),
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
        Some("the agent's draft"),
    )
    .await
    .expect("seed the node");

    let (status, _) = send(
        &state,
        "PUT",
        "/api/v1/company/workspace/file/node-published",
        Some(json!({"content": "the operator's rewrite"})),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "a deliverable whose version cannot be recorded must not have its node written"
    );

    let (_, body) = WorkspaceStore::read(runtime.workspace().as_ref(), &company, "node-published")
        .await
        .unwrap()
        .expect("the note still exists");
    assert_eq!(
        body, "the agent's draft",
        "the node must be untouched — writing it would strand the chain behind it"
    );
}

// ---------------------------------------------------------------------------
// The plan → workflow bridge: apply / reject a proposal (issue #580)
// ---------------------------------------------------------------------------

/// Applying a manual-trigger proposal creates the workflow, stamps the card's
/// output link to the build attempt, finishes the card in Done, and clears the
/// proposal — the whole happy path in one assertion set.
#[tokio::test]
async fn applying_a_proposal_creates_the_workflow_and_finishes_the_card() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let id = seed_proposal_card(&state, digest_ops(None)).await;

    let (status, card) = send(
        &state,
        "POST",
        &format!("/api/v1/company/tasks/{id}/workflow-proposal/apply"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{card}");
    // Done is reached — the create path is the human approval the epic requires.
    assert_eq!(card["column"], "done");
    // The proposal is consumed, and the card links to the workflow it created and
    // to the attempt that built it (issue #339).
    assert!(card.get("workflowProposal").is_none(), "{card}");
    assert_eq!(card["output"]["runId"], "run-build-1");
    assert_eq!(
        card["output"]["workflows"][0]["workflowId"],
        "weekly-digest"
    );
    assert_eq!(card["output"]["workflows"][0]["action"], "created");

    // The workflow now exists in the company's list — and, with no schedule, it
    // is armed (nothing to disarm).
    let (status, workflows) = send(&state, "GET", "/api/v1/company/workflows", None).await;
    assert_eq!(status, StatusCode::OK);
    let created = workflows
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == "weekly-digest")
        .expect("the created workflow is listed");
    assert_eq!(created["enabled"], true, "a manual trigger is not disarmed");
}

/// Issue #1862 prerequisite: a proposal that names no `ownerDesk` defaults to
/// the proposing card's assignee's desk. `seed_proposal_card` assigns the
/// card to `ceo`, and `desk_manifest` seats `ceo` on the `engineering` desk —
/// so the created workflow must come out owned by `engineering` even though
/// `digest_ops` never mentions it.
///
/// This reads the default back off the persisted overlay TOML directly,
/// rather than the `GET …/workflows/{id}` response (which now also projects
/// `ownerDesk`, see `WorkflowGraph::owner_desk`) — pinning the actual stored
/// effect of the defaulting logic, independent of the read projection.
#[tokio::test]
async fn applying_a_proposal_defaults_the_owner_desk_from_the_assignees_desk() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(&home, desk_manifest()).await;
    let id = seed_proposal_card(&state, digest_ops(None)).await;

    let (status, card) = send(
        &state,
        "POST",
        &format!("/api/v1/company/tasks/{id}/workflow-proposal/apply"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{card}");

    use crate::ports::CompanyStore;
    let store = FsCompanyStore::new(home.clone());
    let record = store.load(&CompanyId::new("acme")).await.unwrap().unwrap();
    let overlay = record
        .overlay_workflows
        .iter()
        .find(|w| w.id == "weekly-digest")
        .expect("the created workflow is saved as an overlay");
    let file = crate::company::parse_workflow(&overlay.toml).expect("saved TOML parses");
    assert_eq!(
        file.owner_desk.as_deref(),
        Some("engineering"),
        "the assignee's desk fills the omitted owner_desk"
    );
}
