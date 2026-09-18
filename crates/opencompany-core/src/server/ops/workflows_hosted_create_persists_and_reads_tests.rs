use super::workflows_test_support::hosted_mode::*;
use super::workflows_test_support::*;
use super::*;
use crate::server::router;

/// **The #168 regression test.** Creating a workflow on a tenant with no
/// (writable) source directory used to fail with
/// `Read-only file system (os error 30)` — the handler wrote the graph
/// into the crate's read-only company source tree. It now persists on
/// the record, so the create succeeds, the graph lists under its real
/// name, and the full body reads back.
#[tokio::test]
async fn create_persists_and_reads_back_with_no_source_dir() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;

    // POST → 200 with the graph echoed back.
    let response = router(state.clone())
        .oneshot(request(
            "POST",
            "/api/v1/company/workflows",
            Some(create_body()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let created = json_body(response).await;
    assert_eq!(created["id"], "greeter");
    assert_eq!(created["nodes"].as_array().unwrap().len(), 2);

    // GET list → the real name, not the id fallback.
    let response = router(state.clone())
        .oneshot(request("GET", "/api/v1/company/workflows", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let listed = json_body(response).await;
    let items = own_rows(&listed);
    assert_eq!(items.len(), 1, "body: {listed}");
    assert_eq!(items[0]["id"], "greeter");
    assert_eq!(items[0]["name"], "Greeter");
    assert_eq!(items[0]["description"], "Say hi.");

    // GET one → the full graph.
    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let graph = json_body(response).await;
    assert_eq!(graph["id"], "greeter");
    assert_eq!(graph["edges"][0]["label"], "ok");
}

/// Codex review on #1937 (issue #1866, thread 1) — the RED-on-old
/// proof for BOTH halves the finding names: `CreateNode` never
/// deserialized `postcondition` (a caller's declared gate was
/// silently discarded on save), and `WorkflowNode` never serialized
/// it back out (a `GET` → edit → `PUT` round trip would clear any
/// postcondition already present, since the editor never even saw it
/// to carry forward).
///
/// This is a genuine round trip through the real HTTP handlers: POST
/// create, GET and assert the field survived the write AND the read,
/// then PUT back the **exact GET body** unchanged (the shape a
/// console editor sends when nothing else on the node changed) and
/// GET once more to prove the postcondition is still there — not
/// silently cleared by the round trip. On the code as it stood
/// before this fix, the first GET's `nodes[1]["postcondition"]`
/// assertion already fails: `WorkflowNode` had no such field to
/// serialize.
#[tokio::test]
async fn a_postcondition_survives_create_get_put_get() {
    let home_dir = home();
    let state = desk_state(home_dir.path()).await;

    let created = post_create(state.clone(), body_with_postcondition()).await;
    assert_eq!(created.status(), StatusCode::OK, "{:?}", created);

    let response = router(state.clone())
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut graph = json_body(response).await;
    assert_eq!(
        graph["nodes"][1]["postcondition"],
        serde_json::json!({ "require": "non_empty" }),
        "a postcondition declared on create must be readable back: {graph}"
    );

    // The console's edit flow: take exactly what GET returned, add the
    // version token it must echo back, and PUT it — unchanged — as a
    // no-op save.
    let version = graph["version"]
        .as_str()
        .expect("GET returns a version")
        .to_string();
    graph["expectedVersion"] = serde_json::json!(version);
    let put_response = router(state.clone())
        .oneshot(request(
            "PUT",
            "/api/v1/company/workflows/greeter",
            Some(graph),
        ))
        .await
        .unwrap();
    assert_eq!(put_response.status(), StatusCode::OK, "{:?}", put_response);

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    let graph_after_put = json_body(response).await;
    assert_eq!(
        graph_after_put["nodes"][1]["postcondition"],
        serde_json::json!({ "require": "non_empty" }),
        "a GET -> PUT round trip must not silently clear an existing \
         postcondition: {graph_after_put}"
    );
}

/// **The #981 story, resolved by #1757.** `operator` was in the picker
/// the console showed the author while delivery refused it by name — so
/// the graph saved, ran green, and dropped its report. Now `operator` is a
/// durable, journal-backed channel that lands in the standing Operator
/// feed, so routing a report to it is legitimate and the save succeeds.
#[tokio::test]
async fn a_report_routed_to_operator_saves() {
    let home_dir = home();
    let state = desk_state(home_dir.path()).await;

    let response = post_create(
        state.clone(),
        body_with_destination("channel", Some("operator")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    let graph = json_body(response).await;
    assert_eq!(graph["nodes"][1]["destination"]["kind"], "channel");
    assert_eq!(graph["nodes"][1]["destination"]["target"], "operator");
}

/// A channel nobody wired is refused the same way. The author's typo and
/// the author's `operator` are the same mistake — a destination this
/// company cannot deliver to — and get one answer.
#[tokio::test]
async fn a_report_routed_to_an_unwired_channel_is_refused_at_save() {
    let home_dir = home();
    let state = desk_state(home_dir.path()).await;

    let response = post_create(state, body_with_destination("channel", Some("enginering"))).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let message = json_body(response).await.to_string();
    assert!(
        message.contains("is not an automation delivery channel"),
        "{message}"
    );
    assert!(message.contains("engineering"), "{message}");
}

/// Issue #1191: the refusal is the SAME envelope every sibling node-config
/// rule answers with — `workflow_invalid` plus a `problems` array whose
/// entry names the node and the field.
///
/// It used to be `invalid_request` with no array at all, because the rule
/// sat outside the `problems` accumulator on the write route. So the
/// console, which reads `problems` to highlight the offending node
/// (#1123), got a flat banner for exactly the class of error #836 was
/// filed about. Asserting the envelope rather than the sentence is the
/// point: the sentence never changed.
#[tokio::test]
async fn an_undeliverable_channel_answers_with_a_located_problem() {
    let home_dir = home();
    let state = desk_state(home_dir.path()).await;

    let response = post_create(
        state,
        body_with_destination("channel", Some("engineering-desk")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["code"], "workflow_invalid", "{body}");
    assert_eq!(body["problems"][0]["node_id"], "done", "{body}");
    assert_eq!(body["problems"][0]["field"], "destination.target", "{body}");
    assert!(
        body["problems"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("is not an automation delivery channel"),
        "{body}"
    );
}

/// The sibling rule on the same field: a `channel` destination with no
/// `target` is located too (issue #1191). It was always a
/// `workflow_invalid`, but the entry carried `node_id: null` and
/// `field: null` because the load path mints it as a flat string.
#[tokio::test]
async fn a_channel_destination_with_no_target_is_located_too() {
    let home_dir = home();
    let state = desk_state(home_dir.path()).await;

    let response = post_create(state, body_with_destination("channel", None)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["code"], "workflow_invalid", "{body}");
    assert_eq!(body["problems"][0]["node_id"], "done", "{body}");
    assert_eq!(body["problems"][0]["field"], "destination.target", "{body}");
    assert!(
        body["problems"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("name the channel to post the report to"),
        "{body}"
    );
}

/// The guard refuses what delivery would refuse and nothing more: a real
/// desk saves, and reads back with its destination intact.
#[tokio::test]
async fn a_report_routed_to_a_real_desk_saves() {
    let home_dir = home();
    let state = desk_state(home_dir.path()).await;

    let response = post_create(
        state.clone(),
        body_with_destination("channel", Some("engineering")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/greeter", None))
        .await
        .unwrap();
    let graph = json_body(response).await;
    assert_eq!(graph["nodes"][1]["destination"]["kind"], "channel");
    assert_eq!(graph["nodes"][1]["destination"]["target"], "engineering");
}

/// An edit is a save too. The create route was never the only way in —
/// `PUT` replaces the graph wholesale, so a destination refused on
/// create must not be reachable by saving a clean graph and then
/// editing it.
#[tokio::test]
async fn an_edit_cannot_introduce_an_undeliverable_destination() {
    let home_dir = home();
    let state = desk_state(home_dir.path()).await;

    let created = post_create(state.clone(), create_body()).await;
    assert_eq!(created.status(), StatusCode::OK);
    // Carry the created graph's token (required since #1013) so the 400
    // comes from the destination guard, not the missing-token guard.
    let version = json_body(created).await["version"]
        .as_str()
        .expect("create returns a version")
        .to_string();

    // A genuinely unwired desk (issue #1757: `operator` is now a real
    // target, so it can no longer stand in for an undeliverable one).
    let mut body = body_with_destination("channel", Some("marketing"));
    body["expectedVersion"] = serde_json::json!(version);
    let response = router(state)
        .oneshot(request(
            "PUT",
            "/api/v1/company/workflows/greeter",
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let message = json_body(response).await.to_string();
    assert!(
        message.contains("is not an automation delivery channel"),
        "{message}"
    );
}

/// A company with no desks and no provider channels has nowhere to
/// deliver (#963), and says so in its own words rather than trailing off
/// after `has: `. The destinations that do NOT depend on a channel still
/// save — the guard is about channels, and a company with no desks can
/// still mail its owner.
#[tokio::test]
async fn a_company_with_no_desks_still_offers_the_operator_channel() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;

    // A desk nobody wired is still refused — but the runtime is not
    // channel-less: since #1757 it always has the Operator channel, so the
    // refusal names `operator` as what would work.
    let response = post_create(
        state.clone(),
        body_with_destination("channel", Some("engineering")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let message = json_body(response).await.to_string();
    assert!(
        message.contains("is not an automation delivery channel"),
        "{message}"
    );
    assert!(message.contains("operator"), "{message}");

    // …and the picker it offers is `["operator"]`, never empty.
    let response = router(state.clone())
        .oneshot(request(
            "GET",
            "/api/v1/company/workflows/wired-channels",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(response).await["channels"],
        serde_json::json!(["operator"])
    );

    // An `owner` report saves — it needs no channel, and its no-mailbox
    // fallback lands in that same Operator channel.
    let response = post_create(state, body_with_destination("owner", None)).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an `owner` report needs no channel"
    );
}

/// The guard stays out of everything that is not a channel destination
/// on an `output` node: a graph that routes nowhere saves, and a
/// `destination` on a non-`output` node is still `parse_workflow`'s
/// refusal to report — reporting the wrong problem first would send an
/// author looking for a channel that was never their mistake (#947).
#[tokio::test]
async fn the_guard_leaves_non_channel_graphs_alone() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;

    // No destination at all: unchanged, saves.
    assert_eq!(
        post_create(state.clone(), create_body()).await.status(),
        StatusCode::OK
    );

    // A channel destination on the TRIGGER node, on a company with no
    // delivery channels at all: still rejected for being on the wrong
    // kind of node, not for the channel.
    let mut body = create_body();
    body["id"] = serde_json::Value::String("second".into());
    body["name"] = serde_json::Value::String("Second".into());
    body["nodes"][0]["destination"] =
        serde_json::json!({ "kind": "channel", "target": "engineering" });
    let response = post_create(state, body).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let message = json_body(response).await.to_string();
    assert!(
        message.contains("only `output` nodes route a report"),
        "the structural problem must be the one reported: {message}"
    );
}

/// A duplicate id is a clean 409, not a 500 — the id-uniqueness check
/// that replaced the filesystem's `create_new(true)`.
#[tokio::test]
async fn duplicate_create_is_a_conflict() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;

    let first = router(state.clone())
        .oneshot(request(
            "POST",
            "/api/v1/company/workflows",
            Some(create_body()),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = router(state)
        .oneshot(request(
            "POST",
            "/api/v1/company/workflows",
            Some(create_body()),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
}

/// Issue #753: an empty description is a `400` on **both** scope forms —
/// which also proves the route is wired under each (a route-miss would be
/// a `404`, not the `400` the handler returns before it ever looks for a
/// builder). The empty check runs ahead of the capability gate, so this
/// holds on every build.
#[tokio::test]
async fn draft_from_description_rejects_empty_on_both_scope_forms() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;

    for uri in [
        "/api/v1/company/workflows/draft-from-description",
        "/api/v1/companies/acme/workflows/draft-from-description",
    ] {
        let response = router(state.clone())
            .oneshot(request(
                "POST",
                uri,
                Some(serde_json::json!({ "description": "   " })),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "empty description must 400 on {uri}"
        );
    }
}

/// Issue #753: with a real description but no builder wired on the running
/// runtime, the copilot classifies the gap exactly as the run route does —
/// a `not_wired` 404 or a `restart_required` / `inference_required` 409,
/// each carrying its `code` — rather than a bare failure. The hosted test
/// runtime wires no harness, so this is the gap path.
#[tokio::test]
async fn draft_from_description_reports_a_builder_gap() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;

    let response = router(state)
        .oneshot(request(
            "POST",
            "/api/v1/company/workflows/draft-from-description",
            Some(serde_json::json!({
                "description": "email the weekly digest every Monday"
            })),
        ))
        .await
        .unwrap();
    let status = response.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::CONFLICT,
        "a builder gap is a 404/409, got {status}"
    );
    let body = json_body(response).await;
    let code = body["code"].as_str().unwrap_or_default();
    assert!(
        matches!(
            code,
            "not_wired" | "restart_required" | "inference_required"
        ),
        "gap response carries a known code, got: {body}"
    );
}

/// Issue #840 (PR-3): with a real body but no builder wired, the
/// fix-from-run route classifies the gap exactly as the draft + run routes
/// do — a `not_wired` 404 or a `restart_required` / `inference_required`
/// 409 — on **both** scope forms. Also proves the sub-resource route is
/// wired (a route-miss would be a bare 404 with no `code`). The hosted test
/// runtime wires no harness, so this is the gap path.
#[tokio::test]
async fn fix_from_run_reports_a_builder_gap_on_both_scope_forms() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, _id) = hosted_state(&home).await;

    for uri in [
        "/api/v1/company/workflows/weekly-digest/fix-from-run",
        "/api/v1/companies/acme/workflows/weekly-digest/fix-from-run",
    ] {
        let response = router(state.clone())
            .oneshot(request(
                "POST",
                uri,
                Some(serde_json::json!({
                    "runId": "run-1",
                    "errorHint": "it failed at the search node"
                })),
            ))
            .await
            .unwrap();
        let status = response.status();
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::CONFLICT,
            "a builder gap is a 404/409 on {uri}, got {status}"
        );
        let body = json_body(response).await;
        let code = body["code"].as_str().unwrap_or_default();
        assert!(
            matches!(
                code,
                "not_wired" | "restart_required" | "inference_required"
            ),
            "gap response carries a known code on {uri}, got: {body}"
        );
    }
}
