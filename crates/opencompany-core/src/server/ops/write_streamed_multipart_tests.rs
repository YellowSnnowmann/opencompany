//! Integration tests for the `ops` write plane: tasks, memory, workspace,
//! skills, team, inbox-read, and desk chat — exercised end-to-end over the
//! router against a real fs-backed company.

use axum::body::Body;
use axum::http::StatusCode;
use serde_json::json;

use super::write_test_support::*;
use crate::ports::types::CompanyId;
use crate::store::FsCompanyStore;

/// The headline of #647: a file over the store's per-file cap is refused as
/// **too large**, with the sentence an operator can act on.
///
/// This failed before the fix, and not subtly — the route's `DefaultBodyLimit`
/// was the same 64 MiB as the cap, so it truncated the body first and the
/// truncation surfaced as a parse failure: `400 invalid request: unreadable
/// file part: Error parsing multipart/form-data request`. A correctly-formed
/// request, described as broken, for a reason the operator could not guess.
/// The store's refusal below existed the whole time and could never be reached
/// through this route.
#[tokio::test]
async fn a_file_over_the_per_file_cap_is_refused_as_too_large_not_as_malformed() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let before = tree_names(&state).await;

    // One megabyte over the 64 MiB default: enough to break the cap, nowhere
    // near the 256 MiB the route will now read.
    let oversize = 65 * 1024 * 1024;
    let (status, body) = post_upload(
        &state,
        streamed_multipart(
            file_part_prefix("hero.mov"),
            oversize,
            format!("\r\n--{OVERSIZE_BOUNDARY}--\r\n").into_bytes(),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["code"], "workspace_quota_exceeded", "{body}");
    let message = body["error"].as_str().expect("an error message");
    assert!(message.contains("hero.mov"), "names the file: {message}");
    assert!(message.contains("65.0 MiB"), "names its size: {message}");
    assert!(message.contains("64.0 MiB"), "names the limit: {message}");
    assert!(message.contains("Nothing was stored"), "{message}");

    // The two words the bug used to answer with. Asserting on their absence is
    // the regression guard: a future change that lets the body limit preempt
    // the store again would put them straight back.
    assert!(
        !message.contains("unreadable file part"),
        "the request was not unreadable: {message}"
    );
    assert!(
        !message.contains("Error parsing"),
        "nor was it malformed: {message}"
    );

    assert_eq!(tree_names(&state).await, before, "and nothing was stored");
}

/// Issue #665: an over-cap upload is refused even when its bytes are valid
/// UTF-8.
///
/// The store's quota decorator meters **binary payloads only**, and that is a
/// deliberate narrowing — `src/runtime/workspace_quota.rs` says so, on the
/// grounds that "a note is bounded by what a model will emit into a tool call".
/// That premise holds for every writer the decorator covers and is false for
/// this route, which is where arbitrary operator-supplied bytes enter the tree.
///
/// So a 65 MiB `.csv` — valid UTF-8, therefore classified as prose — used to be
/// stored with **no size check at all**, while the byte-identical payload under
/// a binary content type was refused. Same request, same size, opposite answer,
/// decided by whether the bytes happened to decode.
///
/// The narrowing itself is untouched: an agent's note is still unmetered, and
/// `tree_quota_gb` still counts binary payloads alone.
#[tokio::test]
async fn an_over_cap_upload_is_refused_even_when_its_bytes_are_valid_utf8() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let before = tree_names(&state).await;

    // NUL bytes: valid UTF-8, so `text_body` decodes them and the upload takes
    // the text branch. One megabyte over the 64 MiB default cap.
    let oversize = 65 * 1024 * 1024;
    let (status, body) = post_upload(
        &state,
        streamed_multipart(
            text_file_part_prefix("export.csv"),
            oversize,
            format!("\r\n--{OVERSIZE_BOUNDARY}--\r\n").into_bytes(),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["code"], "workspace_quota_exceeded", "{body}");
    let message = body["error"].as_str().expect("an error message");
    assert!(message.contains("export.csv"), "names the file: {message}");
    assert!(message.contains("65.0 MiB"), "names its size: {message}");
    assert!(message.contains("64.0 MiB"), "names the limit: {message}");
    assert!(message.contains("Nothing was stored"), "{message}");

    assert_eq!(tree_names(&state).await, before, "and nothing was stored");
}

/// The other half of #665, and the reason the fix is a cap rather than a
/// reclassification: an *under*-cap text upload is still stored as prose.
///
/// Refusing large text must not turn ordinary text uploads into opaque blobs — a
/// `.csv` an operator uploads is meant to stay searchable, backlinkable and
/// editable in the console. If this ever fails, the fix has started deciding
/// storage representation instead of bounding size.
#[tokio::test]
async fn an_under_cap_text_upload_is_still_stored_as_prose() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, node) = upload_file(
        &state,
        "notes.csv",
        Some("text/csv"),
        b"a,b,c\n1,2,3\n",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{node}");
    assert_eq!(
        node["content"], "a,b,c\n1,2,3\n",
        "a text upload keeps its body: {node}"
    );
    assert!(
        node.get("mime").is_none() || node["mime"].is_null(),
        "and is a prose note, not a binary payload: {node}"
    );
}

/// The route's own backstop is classified too, and it fires while *skipping* a
/// part — the reader can notice the limit anywhere it reads, not only where the
/// handler wants bytes.
///
/// Without this the classifier arm ships untested and a drift in axum's status
/// mapping would silently regress the answer to the old lying 400.
#[tokio::test]
async fn a_body_over_the_route_limit_is_classified_while_a_part_is_skipped() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let before = tree_names(&state).await;

    // A field the handler ignores by name, so it is drained rather than
    // buffered — and the drain runs past the 256 MiB the route will read.
    let prefix = format!(
        "--{OVERSIZE_BOUNDARY}\r\nContent-Disposition: form-data; name=\"ignored\"\r\n\r\n"
    )
    .into_bytes();
    let mut suffix = format!("\r\n--{OVERSIZE_BOUNDARY}\r\n").into_bytes();
    suffix.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\n\r\nxx\r\n",
    );
    suffix.extend_from_slice(format!("--{OVERSIZE_BOUNDARY}--\r\n").as_bytes());

    let (status, body) = post_upload(
        &state,
        streamed_multipart(prefix, 257 * 1024 * 1024, suffix),
    )
    .await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["code"], "workspace_quota_exceeded", "{body}");
    let message = body["error"].as_str().expect("an error message");
    assert!(
        message.contains("256.0 MiB"),
        "names the ceiling: {message}"
    );
    assert!(message.contains("Nothing was stored"), "{message}");
    // The size is deliberately absent: the body was cut off, so the true total
    // is not knowable here and a guess would be worse than silence.
    assert!(
        !message.contains("Error parsing"),
        "still not a parse failure: {message}"
    );

    assert_eq!(tree_names(&state).await, before, "and nothing was stored");
}

/// The same backstop, noticed at the other read site — while the handler is
/// pulling the `file` part's bytes rather than skipping past someone else's.
#[tokio::test]
async fn a_file_part_over_the_route_limit_is_classified_too() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let before = tree_names(&state).await;

    let (status, body) = post_upload(
        &state,
        streamed_multipart(
            file_part_prefix("enormous.bin"),
            257 * 1024 * 1024,
            format!("\r\n--{OVERSIZE_BOUNDARY}--\r\n").into_bytes(),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["code"], "workspace_quota_exceeded", "{body}");
    let message = body["error"].as_str().expect("an error message");
    assert!(
        message.contains("256.0 MiB"),
        "names the ceiling: {message}"
    );
    assert!(
        !message.contains("unreadable file part"),
        "the part was readable, just too long: {message}"
    );

    assert_eq!(tree_names(&state).await, before, "and nothing was stored");
}

/// The counter-test, and the half of the issue that is easiest to lose: a
/// genuinely malformed body still answers 400.
///
/// Classifying by size must not swallow the case the old message was right
/// about. These two shapes stay `invalid_request` — and stay distinguishable
/// from the 413s above, which is the whole point of the change.
#[tokio::test]
async fn a_malformed_multipart_body_is_still_a_400() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    // The declared boundary never appears.
    let (status, body) =
        post_upload(&state, Body::from(b"this is not a multipart body".to_vec())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_request", "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|m| m.contains("malformed multipart upload")),
        "{body}"
    );

    // A part that opens and never closes: headers, some bytes, no terminating
    // boundary. Truncated — the shape the body limit used to be mistaken for.
    let mut unterminated = file_part_prefix("half.bin");
    unterminated.extend_from_slice(b"partial bytes and then nothing");
    let (status, body) = post_upload(&state, Body::from(unterminated)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_request", "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|m| m.contains("unreadable file part")),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// First-run company setup (docs/spec/runtime/company-setup.md)
// ---------------------------------------------------------------------------

/// The e-commerce worked example, end to end over the router.
///
/// The default test build has no harness, so this is the unpolished path — and
/// that is exactly the contract worth pinning: a company with no inference
/// credential still gets a real industry roster rather than an empty page or an
/// error. Decision D3's floor, asserted at the surface an operator meets.
#[tokio::test]
async fn setup_proposes_a_real_roster_with_no_model_wired() {
    let home = home();
    let state = state_with_company(home.path()).await;

    let (status, body) = send(
        &state,
        "POST",
        "/api/v1/company/setup/roster",
        Some(json!({
            "industry": "E-commerce — I sell homeware online",
            "teamHint": "",
            "automate": "Social media posts, Meta ads, generating my reports, order dispatch",
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["template"], "ecommerce", "{body}");
    assert_eq!(
        body["source"], "fallback",
        "no harness is wired, so the curated team ships: {body}"
    );
    let agents = body["agents"].as_array().expect("agents array");
    assert!(
        (4..=6).contains(&agents.len()),
        "a proposal must be a workable team, got {}: {body}",
        agents.len()
    );
    let roles: Vec<&str> = agents
        .iter()
        .map(|a| a["role"].as_str().unwrap_or_default())
        .collect();
    assert!(roles.contains(&"Logistics Coordinator"), "{roles:?}");
    // Every row must be directly usable as a `POST …/team` body — the console
    // passes them straight through, so a missing field would surface as a
    // half-created teammate rather than as a validation error here.
    for agent in agents {
        for field in ["name", "role", "description"] {
            assert!(
                agent[field].as_str().is_some_and(|v| !v.trim().is_empty()),
                "agent is missing `{field}`: {agent}"
            );
        }
    }
}

/// Setup proposes; it does not create. The roster must be untouched afterwards,
/// because the console is what creates each teammate — and because the empty
/// roster is also the "has setup run?" signal (decision D4), a route that
/// created them itself would answer that question before the operator had seen
/// a single name.
#[tokio::test]
async fn setup_creates_no_teammates_of_its_own() {
    let home = home();
    let state = state_with_company(home.path()).await;

    let (before_status, before) = send(&state, "GET", "/api/v1/company/team", None).await;
    assert_eq!(before_status, StatusCode::OK);
    let before_len = before.as_array().expect("roster").len();

    let (status, _) = send(
        &state,
        "POST",
        "/api/v1/company/setup/roster",
        Some(json!({ "industry": "content creator", "automate": "daily posts" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, after) = send(&state, "GET", "/api/v1/company/team", None).await;
    assert_eq!(
        after.as_array().expect("roster").len(),
        before_len,
        "setup created teammates itself: {after}"
    );
}

/// The answers are persisted, because Phase 2 builds this company's workflows
/// from them and must not have to ask a second time.
#[tokio::test]
async fn setup_remembers_the_answers() {
    let home = home();
    let state = state_with_company(home.path()).await;

    let (status, _) = send(
        &state,
        "POST",
        "/api/v1/company/setup/roster",
        Some(json!({
            "industry": "E-commerce",
            "teamHint": "plus customer support",
            "automate": "Meta ads, order dispatch",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    use crate::ports::CompanyStore;
    let store = FsCompanyStore::new(home.path().to_path_buf());
    let record = store
        .load(&CompanyId::new("acme"))
        .await
        .expect("load")
        .expect("record");
    let answers = record.setup.expect("the answers were stored");
    assert_eq!(answers.industry, "E-commerce");
    assert_eq!(answers.team_hint, "plus customer support");
    assert_eq!(answers.automate, "Meta ads, order dispatch");
}

/// An operator who types nothing still gets a team. The three questions are
/// free text and the last two are skippable, so an empty body is a real request
/// rather than a client bug — and stranding someone on the setup screen is the
/// one outcome worse than a generic roster.
#[tokio::test]
async fn setup_answers_an_empty_body_with_the_generic_team() {
    let home = home();
    let state = state_with_company(home.path()).await;

    let (status, body) = send(
        &state,
        "POST",
        "/api/v1/company/setup/roster",
        Some(json!({})),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["template"], "generic", "{body}");
    assert!(
        body["agents"].as_array().expect("agents").len() >= 4,
        "{body}"
    );
}

/// `[workspace] max_blob_mb` above the default is a real knob again.
///
/// It never was one: the route stopped reading at 64 MiB whatever a company had
/// configured, so raising the cap bought nothing but a different way to fail.
/// A company at 128 MiB can now actually store a 65 MiB file.
#[tokio::test]
async fn a_company_that_raised_its_blob_cap_can_use_it() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_quota(
        &home,
        crate::runtime::WorkspaceQuota {
            max_blob_bytes: 128 * 1024 * 1024,
            tree_quota_bytes: None,
        },
    )
    .await;

    let size = 65 * 1024 * 1024;
    let (status, node) = post_upload(
        &state,
        streamed_multipart(
            file_part_prefix("raised.bin"),
            size,
            format!("\r\n--{OVERSIZE_BOUNDARY}--\r\n").into_bytes(),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{node}");
    assert_eq!(node["name"], "raised.bin");
    assert_eq!(node["size"], size as u64);
    assert!(tree_names(&state).await.contains(&"raised.bin".to_string()));
}

// ---------------------------------------------------------------------------
// Issue #705 — an irreversible effect's amount is admin-only
// ---------------------------------------------------------------------------
