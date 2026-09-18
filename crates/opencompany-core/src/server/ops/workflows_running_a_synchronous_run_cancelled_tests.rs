use super::workflows_test_support::running::*;
use super::*;

/// **A synchronous run can be cancelled mid-request, and its response
/// has to say so.**
///
/// Easy to miss, because "detached" and "cancellable" sound like the
/// same feature: the run id is registered the moment the task is
/// spawned, and the console learns it from the `workflow_run_started`
/// frame — so the cancel route is reachable well before the synchronous
/// response is written. The runner then resolves to a cancelled run
/// whose `output` is `null` with no approvals and no deliveries, which
/// is byte-identical to a run that legitimately produced nothing. This
/// caller was the last reader in the PR still left guessing.
#[tokio::test]
async fn a_synchronous_run_cancelled_mid_request_says_so_in_its_response() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;

    let mut running = Box::pin(
        c.app
            .clone()
            .oneshot(run_request(serde_json::json!({ "input": {} }))),
    );
    // Wait until the runner is actually parked, then find the run the
    // way the console does — by its id — and stop it.
    tokio::select! {
        _ = &mut running => panic!("the run answered before the runner was under way"),
        () = c.entered.notified() => {}
    }
    // Off the supervisor rather than the journal: this stub is the
    // `WorkflowRunner` port, so it never reaches the harness runner that
    // writes `WorkflowRunStarted`. The supervisor is the registration
    // the cancel route itself consults, which makes it the more direct
    // assertion anyway — the id is addressable while the request is open.
    let live = c.runtime.run_supervisor().live();
    assert_eq!(live.len(), 1, "the open synchronous run is registered");
    let (run_id, workflow_id) = live.into_iter().next().unwrap();
    assert_eq!(workflow_id, "demo");
    let response = c
        .app
        .clone()
        .oneshot(cancel_request(&run_id))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a synchronous run is cancellable while its request is open"
    );

    // The still-open request now answers, and must not read as a clean
    // empty success.
    let response = running.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["cancelled"], true, "{body}");
    assert_eq!(body["runId"], run_id.as_str(), "{body}");
    assert!(
        !c.completed.load(Ordering::SeqCst),
        "the run must not have completed its work"
    );
}

/// …and the flag is omitted entirely on a run nobody stopped, so an
/// existing caller's body is byte-unchanged.
#[tokio::test]
async fn an_uncancelled_synchronous_response_omits_the_flag() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;
    c.release.notify_one();

    let response = c
        .app
        .clone()
        .oneshot(run_request(serde_json::json!({ "input": {} })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(
        body.get("cancelled").is_none(),
        "a run nobody stopped carries no flag at all: {body}"
    );
}

/// The history fold reports it, so the console can render a stopped run
/// as stopped rather than as a clean success.
#[tokio::test]
async fn a_cancelled_run_reads_back_as_cancelled_and_not_running() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;

    let response = c
        .app
        .clone()
        .oneshot(run_request(serde_json::json!({ "detach": true })))
        .await
        .unwrap();
    let run_id = json_body(response).await["runId"]
        .as_str()
        .unwrap()
        .to_string();
    c.entered.notified().await;
    c.app
        .clone()
        .oneshot(cancel_request(&run_id))
        .await
        .unwrap();
    await_finished(&c.runtime).await.expect("settles");

    let response = c
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/company/workflows/runs")
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let rows = json_body(response).await;
    let row = &rows["runs"].as_array().expect("array")[0];
    assert_eq!(row["runId"], run_id.as_str(), "{rows}");
    assert_eq!(row["cancelled"], true, "{rows}");
    assert!(
        row.get("running").is_none(),
        "a settled run is not running: {rows}"
    );
    assert!(
        row.get("error").is_none(),
        "a stopped run carries no error: {rows}"
    );
}

/// Unknown and already-settled are the same `404`: there is nothing to
/// stop. Keeping a tombstone to tell them apart would mean choosing an
/// expiry for it, and the run history already says what became of a
/// settled run.
#[tokio::test]
async fn cancelling_an_unknown_or_settled_run_is_not_found() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;

    let response = c
        .app
        .clone()
        .oneshot(cancel_request("never-existed"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Now run one to completion and try again.
    let response = c
        .app
        .clone()
        .oneshot(run_request(serde_json::json!({ "detach": true })))
        .await
        .unwrap();
    let run_id = json_body(response).await["runId"]
        .as_str()
        .unwrap()
        .to_string();
    c.entered.notified().await;
    c.release.notify_one();
    await_finished(&c.runtime).await.expect("settles");

    let response = c
        .app
        .clone()
        .oneshot(cancel_request(&run_id))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a settled run is no longer cancellable"
    );
}

/// The cancel route is behind the same `ScopedCompany` guard as every
/// other route in this module — an unauthenticated caller cannot stop a
/// company's work.
#[tokio::test]
async fn cancelling_without_a_session_is_rejected() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;

    let response = c
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/company/workflows/runs/whatever/cancel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        response.status(),
        StatusCode::OK,
        "an unauthenticated cancel must not be accepted"
    );
    assert!(
        response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN,
        "expected an auth rejection, got {}",
        response.status()
    );
}

/// The cancel path is a static prefix under `/workflows`, and `runs` is
/// a syntactically valid workflow id — so this pins that it is not
/// shadowed by the dynamic `/workflows/{wid}` routes, the same guarantee
/// `run_history_is_not_shadowed_by_the_graph_read` makes for the GET.
#[tokio::test]
async fn the_cancel_route_is_not_shadowed_by_the_dynamic_workflow_routes() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;

    let response = c
        .app
        .clone()
        .oneshot(cancel_request("anything"))
        .await
        .unwrap();
    // 404 from the *cancel handler* (nothing to stop), not a 405 or a
    // route miss — reaching the handler at all is the assertion.
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = json_body(response).await;
    assert!(
        body.to_string().contains("workflow run"),
        "the 404 should come from the cancel handler: {body}"
    );
}
