use super::workflows_test_support::hosted_mode::*;
use super::workflows_test_support::*;
use super::*;
use crate::server::router;

/// `GET …/revisions` returns metadata only — id, name, version,
/// createdAtMillis — and never a graph body. Leaking the TOML/nodes here
/// would make the list as heavy as N graph reads and expose the raw
/// stored body the console never asked for.
#[tokio::test]
async fn revisions_list_is_metadata_only_and_newest_first() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    create_then_edit_greeter(&state).await;

    let response = router(state)
        .oneshot(request(
            "GET",
            "/api/v1/company/workflows/greeter/revisions",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let revs = body["revisions"].as_array().expect("revisions array");
    assert_eq!(revs.len(), 1, "one edit captured one revision: {body}");
    let row = &revs[0];
    assert!(row["id"].is_string());
    assert_eq!(row["name"], "Greeter");
    assert!(row["version"].is_string());
    assert!(row["createdAtMillis"].is_number());
    // No graph body leaks into the list.
    assert!(row.get("nodes").is_none(), "metadata only: {row}");
    assert!(row.get("edges").is_none(), "metadata only: {row}");
    assert!(
        row.get("toml").is_none(),
        "the raw body must never leak: {row}"
    );
}

/// `POST …/revisions/{rev}/restore` reverts the live graph to the
/// snapshot and answers with the restored body + a fresh token. The
/// captured revision was the schedule-less original, so the restore
/// removes the schedule the edit added.
#[tokio::test]
async fn restore_reverts_the_live_graph() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    // The token of the now-current (edited) graph — the one the restore
    // replaces, and which it must carry (required since #1013).
    let current = create_then_edit_greeter(&state).await;

    // Discover the revision id from the list.
    let list = json_body(
        router(state.clone())
            .oneshot(request(
                "GET",
                "/api/v1/company/workflows/greeter/revisions",
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let rev_id = list["revisions"][0]["id"].as_str().unwrap().to_string();

    // Restore it, carrying the current graph's token.
    let response = router(state.clone())
        .oneshot(request(
            "POST",
            &format!("/api/v1/company/workflows/greeter/revisions/{rev_id}/restore"),
            Some(serde_json::json!({ "expectedVersion": current })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let restored = json_body(response).await;
    // The schedule the edit introduced is gone — the original is back.
    assert!(
        restored["nodes"][0].get("schedule").is_none(),
        "restore should drop the edit's schedule: {restored}"
    );
    assert!(
        restored["version"].is_string(),
        "restore returns a fresh token"
    );

    // A fresh read agrees, and the restore itself was captured, so the
    // history now holds two snapshots (the original + the scheduled body
    // the restore replaced).
    let graph = json_body(
        router(state.clone())
            .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
            .await
            .unwrap(),
    )
    .await;
    assert!(graph["nodes"][0].get("schedule").is_none(), "{graph}");
    let list = json_body(
        router(state)
            .oneshot(request(
                "GET",
                "/api/v1/company/workflows/greeter/revisions",
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        list["revisions"].as_array().unwrap().len(),
        2,
        "the restore captured the body it replaced: {list}"
    );
}

/// Restoring a revision id that does not exist is a clean `404`.
#[tokio::test]
async fn restore_unknown_revision_is_not_found() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    // A token is required (issue #1013), so send one; the unknown revision
    // is resolved before the token is ever compared, so this stays a 404.
    let current = create_then_edit_greeter(&state).await;

    let response = router(state)
        .oneshot(request(
            "POST",
            "/api/v1/company/workflows/greeter/revisions/no-such-rev/restore",
            Some(serde_json::json!({ "expectedVersion": current })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// **The silent-clobber guard on restore (issue #1013).** A restore with
/// no token — like an omitted body — used to overwrite unconditionally,
/// so a stale editor could clobber a concurrent save. It is now a `400`
/// that tells the operator to re-read and send the `version`.
#[tokio::test]
async fn a_restore_without_a_token_is_rejected() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    create_then_edit_greeter(&state).await;

    // Discover a real revision id so the 400 is about the missing token,
    // not the revision.
    let list = json_body(
        router(state.clone())
            .oneshot(request(
                "GET",
                "/api/v1/company/workflows/greeter/revisions",
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let rev_id = list["revisions"][0]["id"].as_str().unwrap().to_string();

    let response = router(state)
        .oneshot(request(
            "POST",
            &format!("/api/v1/company/workflows/greeter/revisions/{rev_id}/restore"),
            Some(serde_json::json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    let message = body["error"].as_str().unwrap_or_default().to_lowercase();
    assert!(
        message.contains("version") && message.contains("read"),
        "the 400 must tell the operator to re-read and send the version: {body}"
    );
}

/// A workflow that was never edited has an empty history — `200 []`, not
/// a `404`.
#[tokio::test]
async fn revisions_of_an_unedited_workflow_are_empty() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    create_greeter(&state).await;

    let response = router(state)
        .oneshot(request(
            "GET",
            "/api/v1/company/workflows/greeter/revisions",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["revisions"].as_array().unwrap().len(), 0, "{body}");
}

/// **The silent-overwrite guard.** Two consoles hold the same graph; one
/// saves, then the other saves its stale copy. The second must be
/// refused, not silently win.
#[tokio::test]
async fn a_stale_expected_version_is_a_conflict() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    let stale = create_greeter(&state).await;

    // Console A saves first.
    let first = router(state.clone())
        .oneshot(request(
            "PUT",
            "/api/v1/company/workflows/greeter",
            Some(edited_body(Some(&stale))),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    // Console B saves with the token it loaded before A's write.
    let second = router(state.clone())
        .oneshot(request(
            "PUT",
            "/api/v1/company/workflows/greeter",
            Some(edited_body(Some(&stale))),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    // The message must tell the operator what to do, not just say no.
    let body = json_body(second).await;
    let message = body["error"].as_str().unwrap_or_default().to_lowercase();
    assert!(message.contains("reload"), "unhelpful 409: {body}");

    // A's edit is intact — the refusal changed nothing.
    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    let graph = json_body(response).await;
    assert_eq!(graph["description"], "Say hi, every morning.");
}

/// CONC-axis: restore inherits `PUT`'s optimistic-concurrency check —
/// the doc on [`restore_workflow_revision`] says so — but only `PUT`'s
/// own stale-token 409 was ever driven end to end. This drives
/// restore's own conflict path: two consoles racing a restore of the
/// same revision with the token each read, the second losing.
#[tokio::test]
async fn a_stale_expected_version_is_a_conflict_on_restore() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    let stale = create_then_edit_greeter(&state).await;

    let list = json_body(
        router(state.clone())
            .oneshot(request(
                "GET",
                "/api/v1/company/workflows/greeter/revisions",
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let rev_id = list["revisions"][0]["id"].as_str().unwrap().to_string();
    let before = list["revisions"].as_array().unwrap().len();

    // Console A restores first, carrying the token it read.
    let first = router(state.clone())
        .oneshot(request(
            "POST",
            &format!("/api/v1/company/workflows/greeter/revisions/{rev_id}/restore"),
            Some(serde_json::json!({ "expectedVersion": stale })),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    // Console B restores the SAME revision with the SAME token A already
    // spent — it named the graph before A's write, not after it.
    let second = router(state.clone())
        .oneshot(request(
            "POST",
            &format!("/api/v1/company/workflows/greeter/revisions/{rev_id}/restore"),
            Some(serde_json::json!({ "expectedVersion": stale })),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let body = json_body(second).await;
    let message = body["error"].as_str().unwrap_or_default().to_lowercase();
    assert!(message.contains("reload"), "unhelpful 409: {body}");

    // The refused restore must not have captured a second snapshot —
    // restoring the same revision twice would otherwise look identical
    // on the live graph (it is the same snapshot both times), so the
    // history length is what actually distinguishes "refused" from
    // "silently ran again".
    let list = json_body(
        router(state)
            .oneshot(request(
                "GET",
                "/api/v1/company/workflows/greeter/revisions",
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        list["revisions"].as_array().unwrap().len(),
        before + 1,
        "only the first restore may have run: {list}"
    );
}

/// **The silent-clobber guard, at the front door (issue #1013).** Omitting
/// the token used to be an unconditional write; a stale editor could then
/// overwrite a concurrent save without ever seeing a `409`. A tokenless
/// `PUT` is now a `400` that tells the operator to re-read and resend the
/// `version`.
#[tokio::test]
async fn an_edit_without_a_token_is_rejected() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    create_greeter(&state).await;

    let response = router(state.clone())
        .oneshot(request(
            "PUT",
            "/api/v1/company/workflows/greeter",
            Some(edited_body(None)),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    let message = body["error"].as_str().unwrap_or_default().to_lowercase();
    assert!(
        message.contains("version") && message.contains("read"),
        "the 400 must tell the operator to re-read and resend the version: {body}"
    );

    // The refusal changed nothing — the original description is intact.
    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    let graph = json_body(response).await;
    assert_eq!(graph["description"], "Say hi.");
}

/// A `PUT` that would rename the id is a 400, not a silent create — the
/// id keys the saved graph, its schedule and its run history.
#[tokio::test]
async fn an_id_mismatch_is_a_bad_request() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    create_greeter(&state).await;

    let mut body = edited_body(None);
    body["id"] = serde_json::json!("greeter-v2");
    let response = router(state.clone())
        .oneshot(request(
            "PUT",
            "/api/v1/company/workflows/greeter",
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Nothing was created under the new id.
    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows", None))
        .await
        .unwrap();
    let items = json_body(response).await;
    let own = own_rows(&items);
    assert_eq!(own.len(), 1, "{items}");
    assert_eq!(own[0]["id"], "greeter");
}

#[tokio::test]
async fn editing_an_unknown_workflow_is_not_found() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;

    // A token is required (issue #1013), so send one; the unknown id is
    // resolved before the token is ever compared, so this stays a 404.
    let mut body = edited_body(Some("deadbeef"));
    body["id"] = serde_json::json!("ghost");
    let response = router(state)
        .oneshot(request(
            "PUT",
            "/api/v1/company/workflows/ghost",
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A bad edit is refused on the same terms as a bad create — the shared
/// validation, at the HTTP boundary.
#[tokio::test]
async fn a_structurally_invalid_edit_is_a_bad_request() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    let version = create_greeter(&state).await;

    // No trigger node at all. Carries a valid token (required since #1013)
    // so the 400 comes from structural validation, not the token guard.
    let body = serde_json::json!({
        "id": "greeter",
        "name": "Greeter",
        "nodes": [ { "id": "done", "kind": "output", "name": "Report" } ],
        "edges": [],
        "expectedVersion": version
    });
    let response = router(state)
        .oneshot(request(
            "PUT",
            "/api/v1/company/workflows/greeter",
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// **The delete, and its durability.** A removed workflow leaves the
/// picker AND stays gone across a full state rebuild — the property that
/// matters, because `merge_enabled_workflows` (#208) re-derives the
/// enabled list at boot and would resurrect a half-delete.
#[tokio::test]
async fn delete_removes_it_from_the_picker_and_survives_a_rebuild() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;
    let version = create_greeter(&state).await;

    let response = router(state.clone())
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/company/workflows/greeter?expectedVersion={version}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Gone from the picker…
    let response = router(state.clone())
        .oneshot(request("GET", "/api/v1/company/workflows", None))
        .await
        .unwrap();
    let items = json_body(response).await;
    assert_eq!(own_rows(&items).len(), 0, "{items}");

    // …and from the graph read.
    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Rebuild everything from the same durable store: still gone.
    let rebuilt = state_over(&home, &id, false).await;
    let response = router(rebuilt)
        .oneshot(request("GET", "/api/v1/company/workflows", None))
        .await
        .unwrap();
    let items = json_body(response).await;
    assert_eq!(
        own_rows(&items).len(),
        0,
        "a deleted workflow must not come back on restart: {items}"
    );
}

/// **Run history is orphaned, not reaped.** What a workflow did stays
/// true after the workflow is gone, and the journal is append-only.
#[tokio::test]
async fn deleting_a_workflow_keeps_its_run_history() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;
    let version = create_greeter(&state).await;
    journal_run(
        &state,
        &id,
        "greeter",
        true,
        vec![undelivered_row("done")],
        None,
    )
    .await;

    let response = router(state.clone())
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/company/workflows/greeter?expectedVersion={version}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router(state)
        .oneshot(request(
            "GET",
            "/api/v1/company/workflows/runs?workflow=greeter",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let rows = body["runs"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "past runs must outlive the workflow: {body}");
    assert_eq!(rows[0]["workflowId"], "greeter");
}

#[tokio::test]
async fn deleting_with_a_stale_version_is_a_conflict() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    let stale = create_greeter(&state).await;

    // Someone edits after the console loaded the graph. This uses the
    // then-current token (`stale`); the edit moves the version, so the
    // delete below carries a now-stale one.
    let edited = router(state.clone())
        .oneshot(request(
            "PUT",
            "/api/v1/company/workflows/greeter",
            Some(edited_body(Some(&stale))),
        ))
        .await
        .unwrap();
    assert_eq!(edited.status(), StatusCode::OK);

    let response = router(state.clone())
        .oneshot(request(
            "DELETE",
            &format!("/api/v1/company/workflows/greeter?expectedVersion={stale}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // Still there.
    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn deleting_an_unknown_workflow_is_not_found() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;

    // A token is required (issue #1013), so send one; the unknown id is
    // resolved before the token is ever compared, so this stays a 404.
    let response = router(state)
        .oneshot(request(
            "DELETE",
            "/api/v1/company/workflows/ghost?expectedVersion=deadbeef",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// **The silent-clobber guard on delete (issue #1013).** A tokenless
/// `DELETE` used to remove unconditionally; a stale editor could drop a
/// workflow that changed underneath them. It is now a `400` that tells the
/// operator to re-read and pass the `version`, and removes nothing.
#[tokio::test]
async fn a_delete_without_a_token_is_rejected() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;
    create_greeter(&state).await;

    let response = router(state.clone())
        .oneshot(request("DELETE", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    let message = body["error"].as_str().unwrap_or_default().to_lowercase();
    assert!(
        message.contains("version") && message.contains("read"),
        "the 400 must tell the operator to re-read and pass the version: {body}"
    );

    // The refusal removed nothing — the workflow is still there.
    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
