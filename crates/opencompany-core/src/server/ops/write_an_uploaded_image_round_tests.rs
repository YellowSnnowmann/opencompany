//! Integration tests for the `ops` write plane: tasks, memory, workspace,
//! skills, team, inbox-read, and desk chat — exercised end-to-end over the
//! router against a real fs-backed company.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

use super::write_test_support::*;
use crate::server::router;

/// The headline of #553 over HTTP: a PNG uploads, appears in the tree with its
/// metadata, and streams back byte-exactly — the round trip that used to be
/// impossible because the create route only took a JSON body.
#[tokio::test]
async fn an_uploaded_image_round_trips_through_the_blob_route() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    // Not valid UTF-8, so nothing on this path can be quietly routing it
    // through a `String`.
    let png: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe, 0x00,
    ];
    let (status, node) = upload_file(&state, "hero.png", Some("image/png"), &png, None).await;
    assert_eq!(status, StatusCode::OK, "{node}");
    assert_eq!(node["name"], "hero.png");
    assert_eq!(node["mime"], "image/png");
    assert_eq!(node["size"], png.len() as u64);
    let sha = node["sha256"]
        .as_str()
        .expect("a digest is returned")
        .to_string();
    assert_eq!(sha.len(), 64, "the store's digest, not the caller's");
    assert!(
        node["content"].is_null(),
        "a payload is never inlined into the node body"
    );
    let id = node["id"].as_str().unwrap().to_string();

    // It is in the tree, with its metadata, so the console can decide how to
    // render it without opening it.
    let (status, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed = tree
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id.as_str())
        .expect("the uploaded node is in the tree");
    assert_eq!(listed["mime"], "image/png");
    assert_eq!(listed["size"], png.len() as u64);

    // The payload streams back exactly, with the headers a browser needs.
    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/company/workspace/blob/{id}"))
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["etag"], format!("\"{sha}\""));
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("hero.png")
    );
    let got = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(got.to_vec(), png, "the bytes must survive the round trip");
}

/// Issue #666: the filesystem backend derives payload paths from sibling
/// names, so accepting the same name twice used to leave two ids pointing at
/// one file. The second upload overwrote the first while the first node kept
/// its old length and digest.
///
/// Refusing the colliding create is the filesystem backend's honest answer: it
/// preserves the first payload and prevents the blob route from serving bytes
/// under metadata computed for a different file.
#[tokio::test]
async fn a_same_name_upload_is_refused_without_overwriting_the_first_blob() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let first_bytes = vec![0x89, b'P', b'N', b'G', 0xff];
    let (status, first) =
        upload_file(&state, "chart.png", Some("image/png"), &first_bytes, None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let first_id = first["id"].as_str().unwrap().to_string();
    let first_sha = first["sha256"].as_str().unwrap().to_string();

    let second_bytes = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 0xff];
    let (status, refusal) =
        upload_file(&state, "chart.png", Some("image/png"), &second_bytes, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refusal}");
    assert_eq!(refusal["code"], "conflict", "{refusal}");
    assert!(
        refusal["error"]
            .as_str()
            .is_some_and(|message| message.contains("chart.png")),
        "the operator can identify the occupied name: {refusal}"
    );

    let response = blob_response(&state, &first_id).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-length"],
        first_bytes.len().to_string(),
        "the surviving node still describes its own payload"
    );
    assert_eq!(response.headers()["etag"], format!("\"{first_sha}\""));
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        body.as_ref(),
        first_bytes.as_slice(),
        "the refused upload must not overwrite the first file"
    );

    let (status, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        tree.as_array()
            .unwrap()
            .iter()
            .filter(|node| node["name"] == "chart.png")
            .count(),
        1,
        "a rejected collision must not leave a second metadata row"
    );

    // The physical paths differ when the parent differs, so this is not a
    // workspace-wide filename ban.
    let (_, folder) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "Archive", "kind": "folder"})),
    )
    .await;
    let folder_id = folder["id"].as_str().unwrap();
    let (status, nested) = upload_file(
        &state,
        "chart.png",
        Some("image/png"),
        &second_bytes,
        Some(folder_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{nested}");
    assert_eq!(nested["parentId"], folder_id);
}

/// A Markdown upload stays a **note**, not a payload. Storing it as bytes would
/// silently cost it the editor, the diff-free text read, backlinks and search —
/// so the decision is asserted rather than left to whichever branch ran.
#[tokio::test]
async fn a_markdown_upload_is_stored_as_a_note_not_as_bytes() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, node) = upload_file(
        &state,
        "brief.md",
        Some("text/markdown"),
        b"# Launch\n\nLinks to [[voice]].",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{node}");
    assert!(
        node["mime"].is_null(),
        "a note carries no mime — that field is what marks a node binary"
    );
    let id = node["id"].as_str().unwrap().to_string();

    // …and it reads back through the *text* route, with backlinks.
    let (status, file) = send(
        &state,
        "GET",
        &format!("/api/v1/company/workspace/file/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(file["content"].as_str().unwrap().contains("# Launch"));
}

/// A file *typed* as text whose bytes are not UTF-8 becomes a payload. The
/// decision is made on the bytes, so a mislabelled upload cannot be mangled
/// into a note.
#[tokio::test]
async fn a_mislabelled_text_upload_is_stored_as_bytes() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, node) = upload_file(
        &state,
        "notes.txt",
        Some("text/plain"),
        &[0xff, 0xfe, 0x01],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{node}");
    assert_eq!(
        node["mime"], "text/plain",
        "it keeps the declared type, but is stored as a payload"
    );
    assert_eq!(node["size"], 3);
}

/// The text read is for prose and says so when handed a payload, rather than
/// answering with an empty body. The blob read is the download of whatever the
/// node holds, so a note downloads through it as its own bytes.
#[tokio::test]
async fn the_text_read_refuses_a_payload_and_the_blob_read_downloads_a_note() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (_, image) = upload_file(&state, "chart.png", Some("image/png"), &[0x89, 0xff], None).await;
    let image_id = image["id"].as_str().unwrap().to_string();
    let (_, note) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "voice.md", "kind": "file", "content": "# Voice"})),
    )
    .await;
    let note_id = note["id"].as_str().unwrap().to_string();

    // Text read of a payload: refused, and it names the route that works.
    let (status, body) = send(
        &state,
        "GET",
        &format!("/api/v1/company/workspace/file/{image_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.to_string().contains("workspace/blob/"),
        "the refusal must point at the route that serves it: {body}"
    );

    // Blob read of a note: the note downloads, neutralised the same way every
    // payload this route serves is.
    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/company/workspace/blob/{note_id}"))
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    let got = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(String::from_utf8(got.to_vec()).unwrap(), "# Voice");
}

// ---------------------------------------------------------------------------
// The blob route never hands a browser a document (issue #667)
// ---------------------------------------------------------------------------

/// The vector in #667, end to end: a payload stored under a document media type
/// is not servable as a document.
///
/// The bytes carry a trailing `0xff` so the upload takes the **binary** branch —
/// a valid-UTF-8 `text/html` upload is stored as a prose note and never reaches
/// this route at all. That byte is not a contrivance to reach the branch: a
/// browser decoding these bytes substitutes U+FFFD for it and runs the script
/// exactly the same, so this is the real shape of the attack.
///
/// The assertion that matters is the one about the *stored* mime: it is still
/// `text/html` afterwards. The fix is on the read path precisely so that every
/// payload already sitting in a tree under a caller's chosen mime is covered,
/// which an upload-side sanitiser would not have been.
#[tokio::test]
async fn a_blob_stored_as_html_cannot_be_served_as_a_document() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let payload = b"<script>fetch('/api/v1/company/team')</script>\xff";
    let (status, node) = upload_file(&state, "payload.png", Some("text/html"), payload, None).await;
    assert_eq!(status, StatusCode::OK, "{node}");
    assert_eq!(
        node["mime"], "text/html",
        "the stored mime is untouched — the read path is what neutralises it"
    );
    let id = node["id"].as_str().unwrap().to_string();

    let response = blob_response(&state, &id).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream",
        "a type nobody vouched for is served as opaque bytes"
    );
    assert_not_executable(&response, "an html-typed payload");

    // Neutralised, not corrupted: an operator who downloads it still gets the
    // file they stored.
    let got = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(got.to_vec(), payload.to_vec());
}

/// SVG is why an `image/*` prefix rule would not have closed this.
///
/// It is an image the console previews and a document a browser executes, so the
/// two halves are answered separately: the type survives (an `<img>` will not
/// decode SVG without it, and inside an `<img>` the SVG spec's secure static
/// mode means no script runs), and the disposition becomes `attachment` so the
/// same bytes at the top of a tab are downloaded instead of rendered.
#[tokio::test]
async fn an_svg_keeps_its_type_for_the_console_but_is_never_rendered_as_a_document() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>fetch('/api/v1/company/team')</script></svg>"#;
    let (status, node) = upload_file(&state, "logo.svg", Some("image/svg+xml"), svg, None).await;
    assert_eq!(status, StatusCode::OK, "{node}");
    let id = node["id"].as_str().unwrap().to_string();

    let response = blob_response(&state, &id).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "image/svg+xml",
        "the console's <img> preview needs this exact type to decode the bytes"
    );
    let disposition = response.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        disposition.starts_with("attachment;"),
        "a top-level navigation must download an SVG, not render it: {disposition:?}"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
}

/// An arbitrary caller-declared type — one nobody has ever vetted — is opaque.
/// This is the closed-list half of the fix: the default arm is the safe one, so
/// a media type invented after this was written is downloaded, not rendered.
#[tokio::test]
async fn an_unrecognised_stored_type_is_served_as_opaque_bytes() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    for (name, declared) in [
        ("doc.xhtml", "application/xhtml+xml"),
        ("sheet.xml", "text/xml"),
        ("archive.zip", "application/zip"),
    ] {
        let (status, node) =
            upload_file(&state, name, Some(declared), &[0x50, 0x4b, 0xff], None).await;
        assert_eq!(status, StatusCode::OK, "{node}");
        let id = node["id"].as_str().unwrap().to_string();
        let response = blob_response(&state, &id).await;
        assert_eq!(
            response.headers()["content-type"],
            "application/octet-stream",
            "{declared} is not on the inline list"
        );
        assert_not_executable(&response, declared);
    }
}

/// The behaviour #611 built, pinned so the fix above cannot quietly cost it: an
/// image still arrives with its own type and `inline`, which is what makes the
/// console's preview and a direct navigation both show the picture.
#[tokio::test]
async fn an_image_still_renders_inline_with_its_own_type() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, node) = upload_file(
        &state,
        "hero.png",
        Some("image/png"),
        &[0x89, b'P', b'N', b'G', 0xff],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{node}");
    let id = node["id"].as_str().unwrap().to_string();

    let response = blob_response(&state, &id).await;
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let disposition = response.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        disposition.starts_with("inline;"),
        "an image must still render in place: {disposition:?}"
    );
    assert!(disposition.contains("hero.png"));
}

/// An upload lands under the folder it names, like any other node.
#[tokio::test]
async fn an_upload_can_target_a_parent_folder() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (_, folder) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "Shots", "kind": "folder"})),
    )
    .await;
    let folder_id = folder["id"].as_str().unwrap().to_string();

    let (status, node) = upload_file(
        &state,
        "a.png",
        Some("image/png"),
        &[0x89, 0x50],
        Some(&folder_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{node}");
    assert_eq!(node["parentId"], folder_id.as_str());
}

// ---------------------------------------------------------------------------
// An over-cap upload says "too large", not "malformed" (issue #647)
// ---------------------------------------------------------------------------
