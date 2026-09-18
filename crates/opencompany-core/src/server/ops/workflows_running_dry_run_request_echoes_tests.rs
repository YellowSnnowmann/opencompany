use super::workflows_test_support::running::*;
use super::*;

/// T8 — `{"dry_run":true}` answers 200 carrying `dryRun:true` and the
/// per-node `nodes`; a plain body carries neither `dryRun` (a real run's
/// shape an old host would produce) — the presence discriminator the
/// console reads instead of trusting what it asked for.
#[tokio::test]
async fn dry_run_request_echoes_the_marker_and_nodes_a_plain_body_omits_it() {
    let home_dir = home();
    let app = echo_company(home_dir.path()).await;

    let dry = app
        .clone()
        .oneshot(run_request(serde_json::json!({ "dry_run": true })))
        .await
        .unwrap();
    assert_eq!(dry.status(), StatusCode::OK);
    let body = json_body(dry).await;
    assert_eq!(body["dryRun"], serde_json::json!(true), "{body}");
    assert_eq!(body["nodes"][0]["nodeId"], "done", "{body}");
    assert_eq!(body["nodes"][0]["status"], "ok", "{body}");

    let plain = app
        .oneshot(run_request(serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    let body = json_body(plain).await;
    assert!(
        body.get("dryRun").is_none(),
        "a real run must carry no dryRun key: {body}"
    );
    // The node trail rides every settled run, dry or not.
    assert_eq!(body["nodes"][0]["nodeId"], "done", "{body}");
}

/// **The defect, at the HTTP boundary.** A run whose report was refused
/// answers `200` with every node `ok` and no error — and before this
/// there was nothing on the body that said otherwise, so a client
/// folding `nodes[].status` (the QA harness among them) scored it green.
#[tokio::test]
async fn a_run_whose_report_was_dropped_does_not_answer_as_a_clean_run() {
    let home_dir = home();
    let app = company_with_runner(home_dir.path(), Arc::new(DroppedReportRunner)).await;

    let response = app
        .oneshot(run_request(serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;

    assert_eq!(body["verdict"], "undelivered", "{body}");

    // …and the three facts the verdict must NOT have disturbed. A
    // delivery failure is not a broken graph: the node ran, so its
    // status stays `ok`, no `error` appears, and the run is not
    // cancelled. Flipping any of them would send the copilot's
    // fix-from-run at a graph that was fine.
    assert_eq!(body["nodes"][0]["nodeId"], "done", "{body}");
    assert_eq!(body["nodes"][0]["status"], "ok", "{body}");
    assert!(body.get("error").is_none(), "{body}");
    assert!(body.get("cancelled").is_none(), "{body}");
    // The row is still where the *reason* lives; the verdict is the
    // reading.
    assert_eq!(
        body["deliveries"][0]["reason"], "channel-not-wired",
        "{body}"
    );
}

/// Issue #981, the second half: a **test run** is not a run that lost
/// its report.
///
/// `deliver_outputs_dry` writes one `skipped`/`dry-run` row per routed
/// `output` node, so before this every single test run of a graph with
/// a destination answered `undelivered` — the console badged the safest
/// thing an operator can do as a failure, every time. The rows stay on
/// the body: they are what say *where* the report would have gone.
///
/// Asked for as a **real dry run** (`dry_run: true`) and the `dryRun`
/// discriminator asserted alongside the verdict, so this cannot pass on
/// a run that was not one — a stub runner returning a `dry-run` row is
/// only half the claim.
#[tokio::test]
async fn a_test_run_is_not_a_run_that_lost_its_report() {
    let home_dir = home();
    let app = company_with_runner(home_dir.path(), Arc::new(DryRunRunner)).await;

    let response = app
        .oneshot(run_request(serde_json::json!({ "dry_run": true })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;

    assert_eq!(body["verdict"], "ok", "{body}");
    assert_eq!(body["dryRun"], serde_json::json!(true), "{body}");
    assert_eq!(body["deliveries"][0]["reason"], "dry-run", "{body}");
    assert_eq!(body["deliveries"][0]["status"], "skipped", "{body}");
}

/// The other direction, which is the one that must not regress: a run
/// that delivered everything still reads `ok`.
#[tokio::test]
async fn a_run_that_delivered_fine_still_answers_ok() {
    let home_dir = home();
    let app = echo_company(home_dir.path()).await;

    let response = app
        .oneshot(run_request(serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["verdict"], "ok", "{body}");
}
