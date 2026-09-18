//! Multipart-upload test helpers shared by the `ops` write-plane split
//! test files (workspace uploads, chat uploads, oversize-boundary probes).
//! Split out of `write_test_support.rs` to stay under the 750-line cap.

use super::*;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use crate::AppState;
use crate::server::router;

pub(crate) async fn upload_file(
    state: &AppState,
    filename: &str,
    content_type: Option<&str>,
    bytes: &[u8],
    parent_id: Option<&str>,
) -> (StatusCode, Value) {
    const BOUNDARY: &str = "----opencompany553boundary";
    let mut body: Vec<u8> = Vec::new();
    if let Some(parent) = parent_id {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"parentId\"\r\n\r\n{parent}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    if let Some(ct) = content_type {
        body.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
    }
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());

    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/company/workspace/upload")
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let out = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if out.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&out).unwrap_or(Value::Null)
    };
    (status, value)
}

pub(crate) async fn blob_response(state: &AppState, id: &str) -> axum::response::Response {
    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/company/workspace/blob/{id}"))
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .body(Body::empty())
        .unwrap();
    router(state.clone()).oneshot(request).await.unwrap()
}

pub(crate) fn assert_not_executable(response: &axum::response::Response, context: &str) {
    let content_type = response.headers()["content-type"].to_str().unwrap();
    let disposition = response.headers()["content-disposition"].to_str().unwrap();
    let nosniff = response
        .headers()
        .get("x-content-type-options")
        .map(|v| v.to_str().unwrap().to_string());

    assert!(
        disposition.starts_with("attachment;"),
        "{context}: a browser must download this, not render it — got {disposition:?}"
    );
    assert_eq!(
        nosniff.as_deref(),
        Some("nosniff"),
        "{context}: without nosniff the type below is a suggestion"
    );
    for executable in [
        "text/html",
        "image/svg+xml",
        "application/xhtml+xml",
        "text/xml",
    ] {
        assert!(
            !content_type.starts_with(executable),
            "{context}: served as {content_type:?}, which a browser parses into a \
             document with a script context"
        );
    }
}

pub(crate) const OVERSIZE_BOUNDARY: &str = "----opencompany647boundary";

pub(crate) async fn post_upload(state: &AppState, body: Body) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/company/workspace/upload")
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={OVERSIZE_BOUNDARY}"),
        )
        .body(body)
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let out = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&out).unwrap_or(Value::Null);
    (status, value)
}

pub(crate) fn streamed_multipart(prefix: Vec<u8>, payload: usize, suffix: Vec<u8>) -> Body {
    const FRAME: usize = 1024 * 1024;
    let prefix = std::sync::Arc::new(prefix);
    let suffix = std::sync::Arc::new(suffix);
    let filler = bytes::Bytes::from(vec![0u8; FRAME]);
    let frames = payload.div_ceil(FRAME);

    let stream = futures::stream::unfold(0usize, move |step| {
        let prefix = prefix.clone();
        let suffix = suffix.clone();
        let filler = filler.clone();
        async move {
            tokio::task::yield_now().await;
            let frame = if step == 0 {
                bytes::Bytes::from(prefix.as_ref().clone())
            } else if step <= frames {
                filler.slice(..(payload - (step - 1) * FRAME).min(FRAME))
            } else if step == frames + 1 {
                bytes::Bytes::from(suffix.as_ref().clone())
            } else {
                return None;
            };
            Some((Ok::<_, std::io::Error>(frame), step + 1))
        }
    });
    Body::from_stream(stream)
}

pub(crate) fn file_part_prefix(filename: &str) -> Vec<u8> {
    format!(
        "--{OVERSIZE_BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
         filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes()
}

pub(crate) async fn tree_names(state: &AppState) -> Vec<String> {
    let (status, tree) = send(state, "GET", "/api/v1/company/workspace", None).await;
    assert_eq!(status, StatusCode::OK);
    provisioned_names(&tree)
}

pub(crate) fn text_file_part_prefix(filename: &str) -> Vec<u8> {
    format!(
        "--{OVERSIZE_BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
         filename=\"{filename}\"\r\nContent-Type: text/csv\r\n\r\n"
    )
    .into_bytes()
}

pub(crate) async fn detail_as(state: &AppState, id: &str, cookie: String) -> Value {
    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/company/tasks/{id}"))
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

pub(crate) async fn chat_upload(
    state: &AppState,
    filename: &str,
    content_type: Option<&str>,
    bytes: &[u8],
) -> (StatusCode, Value) {
    const BOUNDARY: &str = "----opencompany1682boundary";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    if let Some(ct) = content_type {
        body.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
    }
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());

    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/company/chat/upload")
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let out = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if out.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&out).unwrap_or(Value::Null)
    };
    (status, value)
}
