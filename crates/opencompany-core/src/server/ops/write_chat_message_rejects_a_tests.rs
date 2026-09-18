//! Integration tests for the `ops` write plane: tasks, memory, workspace,
//! skills, team, inbox-read, and desk chat — exercised end-to-end over the
//! router against a real fs-backed company.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

use super::write_test_support::*;
use crate::ports::types::CompanyId;
use crate::server::router;

/// A folder is still refused — and now the refusal says so, rather than
/// claiming a node the operator is looking at is absent from the workspace.
#[tokio::test]
async fn chat_message_rejects_a_folder_attachment() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, folder) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({ "name": "designs", "kind": "folder" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{folder}");
    let node_id = folder["id"].as_str().unwrap().to_string();

    let (status, body) = send(
        &state,
        "POST",
        "/api/v1/company/chat",
        Some(json!({ "message": "folder attached", "attachments": [node_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("folder"),
        "the refusal must name the real reason, got: {error}"
    );

    let (status, history) = send(&state, "GET", "/api/v1/company/chat/history", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        history
            .as_array()
            .expect("history is a list")
            .iter()
            .all(|m| m["text"] != "folder attached"),
        "a refused attachment message still reached the transcript: {history}"
    );
}

/// The download half: the blob route serves a prose note's bytes exactly, as a
/// neutralised download — never inline, never under a type a caller chose — so
/// the chip an attached note renders has a working download behind it. A
/// folder and an unknown id still 404 identically.
#[tokio::test]
async fn workspace_blob_serves_a_prose_note_as_a_download() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let content = "# Plan\n\nShip it.\n";
    let (status, created) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({ "name": "plan.md", "kind": "file", "content": content })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let node_id = created["id"].as_str().unwrap().to_string();

    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/company/workspace/blob/{node_id}"))
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers["content-type"], "application/octet-stream",
        "a note is served under a neutral type, not one a caller influenced"
    );
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment;"),
        "a note must never be served inline on the console's origin"
    );
    assert_eq!(headers["content-length"], content.len().to_string());
    assert!(
        !headers.contains_key("etag"),
        "a prose note has no stored digest to answer a conditional request with"
    );
    let got = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        String::from_utf8(got.to_vec()).unwrap(),
        content,
        "the note's bytes must survive the round trip"
    );

    // A folder and an id naming nothing still 404 identically.
    let (status, folder) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({ "name": "archive", "kind": "folder" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{folder}");
    for id in [folder["id"].as_str().unwrap(), "01JZZZNOTAREALNODE00000000"] {
        let request = Request::builder()
            .method("GET")
            .uri(format!("/api/v1/company/workspace/blob/{id}"))
            .header("cookie", crate::server::test_support::fixed_cookie("acme"))
            .body(Body::empty())
            .unwrap();
        let response = router(state.clone()).oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{id} must not be servable as a blob"
        );
    }
}

/// A note past the extraction cap still **attaches** — the reference is what
/// the operator asked for — it simply carries no extracted text, the same
/// answer an oversized binary gets.
#[tokio::test]
async fn chat_attachment_oversized_note_attaches_without_extracted_text() {
    use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceOrigin, WorkspaceStore};

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).expect("company");

    // Written straight to the store: the JSON create route's body limit is far
    // below the extraction cap, so this size cannot arrive through it.
    let huge = "x".repeat(5 * 1024 * 1024);
    WorkspaceStore::create(
        runtime.workspace().as_ref(),
        &company,
        &WorkspaceNode {
            id: "node-oversized-note".to_string(),
            name: "huge.md".to_string(),
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
        Some(&huge),
    )
    .await
    .expect("seed the oversized note");

    let (status, body) = send(
        &state,
        "POST",
        "/api/v1/company/chat",
        Some(json!({ "message": "big note", "attachments": ["node-oversized-note"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let journaled = runtime
        .events()
        .read_from(runtime.id(), crate::ports::types::EventSeq::new(0), 10_000)
        .await
        .unwrap()
        .into_iter()
        .find_map(|s| match s.event {
            crate::ports::types::CompanyEvent::OperatorMessage { attachments, .. }
                if !attachments.is_empty() =>
            {
                Some(attachments)
            }
            _ => None,
        })
        .expect("the message with an attachment is in the journal");
    assert_eq!(journaled.len(), 1);
    assert_eq!(journaled[0].size, huge.len() as u64);
    assert_eq!(
        journaled[0].extracted_text, None,
        "a note past the extraction cap attaches with no text, rather than not at all"
    );
}

/// The ceiling on a prose attachment holds at read time, not after.
///
/// The binary half of this path has never had to buffer what it will discard:
/// `size` rides the node, so an over-cap payload is refused on metadata and
/// `read_bytes` is never called. A note carries no `size`, so the same
/// discipline needs the store to answer the length and withhold the body in one
/// step — otherwise the cap is applied to a `String` that has already been
/// allocated, which is the allocation the cap exists to prevent, and one
/// message may carry twenty of them.
///
/// Recorded through the whole runtime stack, so a decorator that stopped
/// forwarding `read_capped` fails here too.
#[tokio::test]
async fn an_over_cap_note_attachment_is_never_fully_read() {
    use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceOrigin, WorkspaceStore};

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let recorder = std::sync::Arc::new(RecordingReads::new(std::sync::Arc::new(
        crate::company::workspace_repair::loose_store::LooseWorkspace::default(),
    )));
    let state = state_with_workspace(&home, recorder.clone()).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).expect("company");

    let huge = "x".repeat(5 * 1024 * 1024);
    WorkspaceStore::create(
        runtime.workspace().as_ref(),
        &company,
        &WorkspaceNode {
            id: "node-oversized".to_string(),
            name: "huge.md".to_string(),
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
        Some(&huge),
    )
    .await
    .expect("seed the oversized note");
    let seeded = recorder.bytes_read("node-oversized");

    let (status, body) = send(
        &state,
        "POST",
        "/api/v1/company/chat",
        Some(json!({ "message": "big note", "attachments": ["node-oversized"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(
        recorder.bytes_read("node-oversized") - seeded,
        0,
        "resolving an over-cap attachment must not pull the note's body through \
         the unbounded read"
    );

    // And it still attaches, with the length the store measured and no text.
    let journaled = runtime
        .events()
        .read_from(runtime.id(), crate::ports::types::EventSeq::new(0), 10_000)
        .await
        .unwrap()
        .into_iter()
        .find_map(|s| match s.event {
            crate::ports::types::CompanyEvent::OperatorMessage { attachments, .. }
                if !attachments.is_empty() =>
            {
                Some(attachments)
            }
            _ => None,
        })
        .expect("the message with an attachment is in the journal");
    assert_eq!(journaled.len(), 1);
    assert_eq!(journaled[0].size, huge.len() as u64);
    assert_eq!(journaled[0].extracted_text, None);
}

/// A note under the cap is read once and reaches the brain whole — the bound
/// above must not be paid for by an attachment that fits.
#[tokio::test]
async fn an_under_cap_note_attachment_still_carries_its_text() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let recorder = std::sync::Arc::new(RecordingReads::new(std::sync::Arc::new(
        crate::company::workspace_repair::loose_store::LooseWorkspace::default(),
    )));
    let state = state_with_workspace(&home, recorder.clone()).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).expect("company");

    let (status, created) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({
            "name": "brief.md",
            "kind": "file",
            "content": "Q3 revenue grew 12% year over year.",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let node_id = created["id"].as_str().unwrap().to_string();

    let (status, body) = send(
        &state,
        "POST",
        "/api/v1/company/chat",
        Some(json!({ "message": "small note", "attachments": [node_id] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let journaled = runtime
        .events()
        .read_from(runtime.id(), crate::ports::types::EventSeq::new(0), 10_000)
        .await
        .unwrap()
        .into_iter()
        .find_map(|s| match s.event {
            crate::ports::types::CompanyEvent::OperatorMessage { attachments, .. }
                if !attachments.is_empty() =>
            {
                Some(attachments)
            }
            _ => None,
        })
        .expect("the message with an attachment is in the journal");
    assert_eq!(
        journaled[0].extracted_text.as_deref(),
        Some("Q3 revenue grew 12% year over year."),
    );
    assert_eq!(
        journaled[0].size,
        "Q3 revenue grew 12% year over year.".len() as u64
    );
}
