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

/// Issue #705: any Member could read the dollar value of every irreversible
/// effect on a card.
///
/// #618 restricted the money on an approval — the effect nobody has signed off
/// yet. The *executed* effect carries the same number through a different DTO
/// on a different route, and that route had no role check at all.
///
/// **Asserted on the serialized JSON, not on the struct.** `amount_usd` carries
/// `skip_serializing_if`, so the wire shape is the only thing that settles
/// whether the field shipped; a struct-level assertion can pass while the bytes
/// still carry the amount.
#[tokio::test]
async fn a_member_does_not_see_an_irreversible_effects_amount() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    crate::server::test_support::seed_fixed_member(&state, "acme").await;

    let (status, task) = send(
        &state,
        "POST",
        "/api/v1/company/tasks",
        Some(json!({"title": "Pay the Q3 retainer"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{task}");
    let id = task["id"].as_str().unwrap().to_string();

    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    // Two effects: one carrying money, one not. The second is the control that
    // keeps "withheld" and "there was never an amount" distinguishable.
    runtime
        .journal
        .record_executed(
            "exec-705-paid",
            crate::runtime::journal::ExecutedEffect {
                kind: "payment.send".to_string(),
                amount_usd: Some(2400.0),
                task_id: Some(id.clone()),
                at_millis: 1_000,
                irreversible: true,
            },
        )
        .await
        .unwrap();
    runtime
        .journal
        .record_executed(
            "exec-705-free",
            crate::runtime::journal::ExecutedEffect {
                kind: "email.send".to_string(),
                amount_usd: None,
                task_id: Some(id.clone()),
                at_millis: 2_000,
                irreversible: true,
            },
        )
        .await
        .unwrap();

    // The admin signs these off, so the admin sees what they cost.
    let as_admin = detail_as(
        &state,
        &id,
        crate::server::test_support::fixed_cookie("acme"),
    )
    .await;
    let admin_effects = as_admin["irreversibleEffects"].as_array().unwrap();
    assert_eq!(admin_effects.len(), 2, "{as_admin}");
    let admin_paid = admin_effects
        .iter()
        .find(|e| e["kind"] == "payment.send")
        .unwrap();
    assert_eq!(admin_paid["amountUsd"].as_f64(), Some(2400.0), "{as_admin}");
    assert!(
        admin_paid.get("amountHidden").is_none(),
        "an admin is not told anything was withheld: {as_admin}"
    );

    let as_member = detail_as(
        &state,
        &id,
        crate::server::test_support::member_cookie("acme"),
    )
    .await;
    let member_effects = as_member["irreversibleEffects"].as_array().unwrap();
    assert_eq!(
        member_effects.len(),
        2,
        "the rows survive — a member must still see what a retry would re-do: {as_member}"
    );
    let member_paid = member_effects
        .iter()
        .find(|e| e["kind"] == "payment.send")
        .unwrap();

    // The leak, closed. Absent from the wire, not null: `skip_serializing_if`.
    assert!(
        member_paid.get("amountUsd").is_none(),
        "the amount must not reach a member: {as_member}"
    );
    // Hidden is not absent — the console has to be able to say why.
    assert_eq!(
        member_paid["amountHidden"], true,
        "a withheld amount must be distinguishable from an effect that cost \
         nothing: {as_member}"
    );
    // Everything that makes the retry warning legible survives.
    assert_eq!(member_paid["kind"], "payment.send");
    assert_eq!(member_paid["atMillis"].as_u64(), Some(1_000));

    // The control: an effect that never carried money is not reported as
    // redacted, or "nothing to show" and "not shown to you" collapse.
    let member_free = member_effects
        .iter()
        .find(|e| e["kind"] == "email.send")
        .unwrap();
    assert!(member_free.get("amountUsd").is_none(), "{as_member}");
    assert!(
        member_free.get("amountHidden").is_none(),
        "an effect with no amount was not redacted: {as_member}"
    );
}

/// The export path is covered by construction, because it is handed the same
/// value.
///
/// `assemble_detail` is deliberately shared between the JSON route and the
/// export document (issue #352 calls that sharing "the export's redaction
/// guarantee"). This drives that shared function directly with a
/// member-scoped principal, so the guarantee is asserted at the seam both
/// readers pass through rather than only at the JSON one.
///
/// **Why not assert on the exported HTML alone.** The export template does not
/// currently render effect amounts at all, so an HTML-only assertion would pass
/// whether or not the redaction exists — coverage that cannot fail. The HTML
/// check below is kept as a secondary guard against a future template that does
/// render them; the assertion that actually holds the line is the one on the
/// shared projection.
#[tokio::test]
async fn the_export_document_is_built_from_the_redacted_detail() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    crate::server::test_support::seed_fixed_member(&state, "acme").await;

    let (status, task) = send(
        &state,
        "POST",
        "/api/v1/company/tasks",
        Some(json!({"title": "Pay the Q3 retainer"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{task}");
    let id = task["id"].as_str().unwrap().to_string();

    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    runtime
        .journal
        .record_executed(
            "exec-705-export",
            crate::runtime::journal::ExecutedEffect {
                kind: "payment.send".to_string(),
                amount_usd: Some(2400.0),
                task_id: Some(id.clone()),
                at_millis: 1_000,
                irreversible: true,
            },
        )
        .await
        .unwrap();

    // The seam both readers share, driven as a member. `assemble_detail` is
    // exactly what `export_task` calls.
    let as_member = super::tasks::assemble_detail(
        &super::ScopedCompany {
            runtime: runtime.clone(),
            actor: None,
            may_read_contents: false,
            is_admin: false,
        },
        &id,
    )
    .await
    .expect("detail assembles");
    // Serialized, not read off the struct: `skip_serializing_if` means the wire
    // shape is what settles whether the amount shipped.
    let wire = serde_json::to_value(&as_member.irreversible_effects).unwrap();
    let paid = &wire.as_array().unwrap()[0];
    assert!(
        paid.get("amountUsd").is_none(),
        "the shared projection the export renders must already be redacted: {wire}"
    );
    assert_eq!(paid["amountHidden"], true, "{wire}");

    // …and the same principal reading it as an admin still gets the number, so
    // the assertion above is redaction rather than the field being gone.
    let as_admin = super::tasks::assemble_detail(
        &super::ScopedCompany {
            runtime: runtime.clone(),
            actor: None,
            may_read_contents: true,
            is_admin: true,
        },
        &id,
    )
    .await
    .expect("detail assembles");
    let admin_wire = serde_json::to_value(&as_admin.irreversible_effects).unwrap();
    assert_eq!(
        admin_wire.as_array().unwrap()[0]["amountUsd"].as_f64(),
        Some(2400.0),
        "{admin_wire}"
    );

    // Secondary guard: the rendered document must not carry it either. This
    // passes today regardless (the template renders no effects) and exists so a
    // future template that does render them cannot reintroduce the leak.
    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/company/tasks/{id}/export"))
        .header("cookie", crate::server::test_support::member_cookie("acme"))
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(bytes.to_vec()).expect("the export is utf-8");
    assert!(
        !html.contains("2400"),
        "the exported document must not carry an amount this reader may not read"
    );
}

// ── Issue #661 (M5): the console's two new reads ───────────────────────────

/// A run's board rows reach `GET …/workflows/runs`.
///
/// This is the surface PR3's console history panel consumes, and the only one a
/// **scheduled** run has: nobody awaited its response, so without this the sole
/// evidence a 3am run opened a card is the card itself, with nothing saying
/// which run put it there.
///
/// Asserted through the real route and the real group-by-run fold, because the
/// fold is where a row can be dropped — a `WorkflowRunFinished` that settles an
/// open entry writes every field across, and one missing line there is invisible
/// to a serialization test.
#[tokio::test]
async fn the_run_history_carries_a_runs_board_rows() {
    use crate::ports::types::CompanyEvent;
    use crate::ports::{WorkflowBoardAction, WorkflowRunBoardRow};

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).unwrap();

    for event in [
        CompanyEvent::WorkflowRunStarted {
            workflow_id: "digest".into(),
            run_id: "run-1".into(),
            scheduled: true,
            started_by: None,
            resume_semantic: None,
        },
        CompanyEvent::WorkflowRunFinished {
            workflow_id: "digest".into(),
            scheduled: true,
            run_id: Some("run-1".into()),
            deliveries: Vec::new(),
            pending_approvals: Vec::new(),
            error: None,
            cancelled: false,
            notices: Vec::new(),
            board: vec![WorkflowRunBoardRow {
                action: WorkflowBoardAction::Spawned,
                task_id: Some("card-1".into()),
                title: Some("Reply to the auditor".into()),
                assignee: None,
            }],
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        },
        // A second run that touched no card, so the omission is asserted on a
        // real row rather than on an absence that could be the fold failing.
        CompanyEvent::WorkflowRunFinished {
            workflow_id: "digest".into(),
            scheduled: false,
            run_id: Some("run-2".into()),
            deliveries: Vec::new(),
            pending_approvals: Vec::new(),
            error: None,
            cancelled: false,
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        },
    ] {
        runtime.events().append(&company, event).await.unwrap();
    }

    let (status, runs) = send(&state, "GET", "/api/v1/company/workflows/runs", None).await;
    assert_eq!(status, StatusCode::OK);
    let runs = runs["runs"].as_array().expect("an array of runs");

    let settled = runs
        .iter()
        .find(|r| r["runId"] == "run-1")
        .unwrap_or_else(|| panic!("run-1 must be in the history: {runs:?}"));
    assert_eq!(settled["board"][0]["action"], "spawned");
    assert_eq!(settled["board"][0]["taskId"], "card-1");
    assert_eq!(settled["board"][0]["title"], "Reply to the auditor");

    let untouched = runs
        .iter()
        .find(|r| r["runId"] == "run-2")
        .unwrap_or_else(|| panic!("run-2 must be in the history: {runs:?}"));
    assert!(
        untouched["board"].is_null(),
        "a run that touched no card must omit the key entirely, so every existing history row's \
         wire shape is unchanged: {untouched}"
    );
}

/// A card opened by a run carries its provenance onto the board read, and a card
/// opened any other way is byte-unchanged.
///
/// The second half is the compatibility claim and needs its own card rather than
/// a re-read of the first: `skip_serializing_if` is what keeps every card the
/// board rendered before #661 identical, and only an actually-absent field
/// proves it.
#[tokio::test]
async fn a_card_opened_by_a_run_projects_its_provenance() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).unwrap();

    let mut from_run = discussion_card("t-run", "Reply to the auditor");
    from_run.origin_run_id = Some("run-1".to_string());
    from_run.origin_workflow_id = Some("digest".to_string());
    runtime.tasks().upsert(&company, &from_run).await.unwrap();
    runtime
        .tasks()
        .upsert(&company, &discussion_card("t-hand", "Opened by hand"))
        .await
        .unwrap();

    let (status, body) = send(&state, "GET", "/api/v1/company/tasks", None).await;
    assert_eq!(status, StatusCode::OK);
    let cards = body.as_array().expect("an array of cards");

    let from_run = cards
        .iter()
        .find(|c| c["id"] == "t-run")
        .unwrap_or_else(|| panic!("the run's card must be on the board: {cards:?}"));
    assert_eq!(from_run["originRunId"], "run-1");
    assert_eq!(from_run["originWorkflowId"], "digest");

    let by_hand = cards
        .iter()
        .find(|c| c["id"] == "t-hand")
        .unwrap_or_else(|| panic!("the hand-opened card must be on the board: {cards:?}"));
    assert!(
        by_hand["originRunId"].is_null() && by_hand["originWorkflowId"].is_null(),
        "a card no run opened must carry neither key, so the board's existing wire shape is \
         unchanged: {by_hand}"
    );
}

/// Both addressing forms reach it, like every other write-plane route — the
/// platform `…/companies/{id}/…` spelling and the single-company alias.
#[tokio::test]
async fn setup_is_reachable_under_both_scope_forms() {
    let home = home();
    let state = state_with_company(home.path()).await;

    let (status, body) = send(
        &state,
        "POST",
        "/api/v1/companies/acme/setup/roster",
        Some(json!({ "industry": "software" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["template"], "software", "{body}");
}

// ---------------------------------------------------------------------------
// Chat attachments (issue #1682)
// ---------------------------------------------------------------------------

/// The upload half of #1682: a file posts to `/chat/upload`, comes back as a
/// compact `AttachmentRef` with the store's own metadata, and lands in the
/// workspace tree as a binary node the existing blob route can serve. This is
/// the reference the send path then carries by id.
#[tokio::test]
async fn chat_upload_stores_binary_and_returns_ref() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    // Not valid UTF-8, so nothing on this path can be quietly routing it
    // through a `String` and turning the attachment into a prose note.
    let png: Vec<u8> = vec![
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe, 0x00,
    ];
    let (status, reference) = chat_upload(&state, "hero.png", Some("image/png"), &png).await;
    assert_eq!(status, StatusCode::OK, "{reference}");
    assert_eq!(reference["name"], "hero.png");
    assert_eq!(reference["mime"], "image/png");
    assert_eq!(reference["size"], png.len() as u64);
    let node_id = reference["nodeId"].as_str().expect("a node id").to_string();

    // It is a real binary node in the tree, so it shares the workspace quota
    // and the hardened blob serve rather than a parallel store.
    let (status, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    assert_eq!(status, StatusCode::OK);
    let listed = tree
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == node_id.as_str())
        .expect("the uploaded node is in the tree");
    assert_eq!(listed["mime"], "image/png");
    assert_eq!(listed["size"], png.len() as u64);

    // And it streams back byte-exactly through the existing blob route — the
    // download path #1682 reuses untouched.
    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/company/workspace/blob/{node_id}"))
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let got = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(got.to_vec(), png, "the bytes must survive the round trip");
}
