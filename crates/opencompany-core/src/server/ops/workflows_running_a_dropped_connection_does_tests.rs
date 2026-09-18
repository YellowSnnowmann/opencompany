use super::workflows_test_support::running::*;
use super::*;

/// **The keystone.** A client that walks away mid-run must not take the
/// run with it.
///
/// The route used to await the run *inside* the request future, and
/// hyper drops that future when the peer closes — so `record_run_finished`
/// never ran and the run produced no history entry at all. Post-#385
/// that is strictly worse: the run has already journaled a
/// `WorkflowRunStarted`, so the fold reports `running: true` forever and
/// `sweep_interrupted_runs` is boot-only. "Workflow Run produces no
/// run-history entry" is that bug, reported from staging.
///
/// `Router::oneshot` reproduces the cancellation by the same mechanism
/// hyper uses rather than by analogy: the handler future is owned by the
/// future being polled, so dropping the latter drops the former.
#[tokio::test]
async fn a_dropped_connection_does_not_cancel_a_synchronous_run() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;

    let mut running = Box::pin(c.app.clone().oneshot(run_request(
        serde_json::json!({ "input": { "request": "go" } }),
    )));
    tokio::select! {
        _ = &mut running => panic!("the run answered before the runner was under way"),
        () = c.entered.notified() => {}
    }
    // The client hangs up, exactly as a proxy does when it gives up.
    drop(running);

    // The run must still be there to finish.
    c.release.notify_one();
    let finished = await_finished(&c.runtime).await.expect(
        "the run died with the dropped connection: nothing was journaled, so the \
                 history shows it running forever",
    );
    assert!(
        c.completed.load(Ordering::SeqCst),
        "the runner never got to finish its work"
    );
    let CompanyEvent::WorkflowRunFinished {
        error, cancelled, ..
    } = finished
    else {
        unreachable!()
    };
    assert!(error.is_none(), "a client hanging up is not a run failure");
    assert!(!cancelled, "nobody cancelled this run");
}

/// The same proof over a **real socket**, so the keystone rests on
/// hyper's actual behaviour rather than on `oneshot` modelling it well.
///
/// **The pause after the close is load-bearing.** Hyper does not learn
/// the peer is gone when the client calls `shutdown` — it learns when
/// its connection task next polls the socket and reads EOF. Release the
/// runner before that happens and the run finishes on its own merits, so
/// the test passes against the broken code and proves nothing.
#[tokio::test]
async fn a_real_socket_close_does_not_cancel_a_synchronous_run() {
    use tokio::io::AsyncWriteExt;

    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = c.app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, app).await });

    let body = serde_json::json!({ "input": {} }).to_string();
    let request = format!(
        "POST /api/v1/company/workflows/demo/run HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Cookie: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        crate::server::test_support::fixed_cookie("acme"),
        body.len(),
    );
    let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
    socket.write_all(request.as_bytes()).await.unwrap();
    socket.flush().await.unwrap();

    c.entered.notified().await;
    socket.shutdown().await.unwrap();
    drop(socket);
    // See the doc comment: without this, hyper has not yet noticed.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    c.release.notify_one();
    assert!(
        await_finished(&c.runtime).await.is_some(),
        "a real peer close cancelled the run: nothing was journaled"
    );
    assert!(c.completed.load(Ordering::SeqCst));
    server.abort();
}

/// `detach: true` answers `202` with the run id while the run is
/// demonstrably still going — the half that removes the wait.
#[tokio::test]
async fn a_detached_run_answers_202_before_the_run_finishes() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;

    let response = c
        .app
        .clone()
        .oneshot(run_request(serde_json::json!({ "detach": true })))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = json_body(response).await;
    assert_eq!(body["detached"], true, "{body}");
    assert!(
        body["runId"].as_str().is_some_and(|s| !s.is_empty()),
        "the id is the whole point of the response: {body}"
    );
    assert!(
        body.get("output").is_none(),
        "a detached response must not look settled: {body}"
    );
    assert!(
        !c.completed.load(Ordering::SeqCst),
        "the response arrived before the run finished, which is the point"
    );

    // And it settles on its own, with nobody waiting.
    c.release.notify_one();
    assert!(await_finished(&c.runtime).await.is_some());
}

/// The wire-compat guarantee in the other direction: a caller that sends
/// no `detach` gets exactly the response it always got — a `200`
/// carrying the settled run — so an older console is untouched.
#[tokio::test]
async fn a_body_without_detach_still_gets_the_synchronous_response() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;
    // Pre-release, so the runner never parks and this is a plain
    // start-to-finish call — the shape an older console makes.
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
        body.get("output").is_some(),
        "the settled shape carries `output`: {body}"
    );
    assert!(
        body.get("detached").is_none(),
        "the synchronous response must not carry the detach discriminator: {body}"
    );
    assert!(body["runId"].as_str().is_some(), "{body}");
}

/// Codex review finding on PR #2140 (`3952230576`): the emergency stop
/// is a separate switch from `lifecycle` (a stopped company still
/// reports `running`), so `ensure_running` alone missed it here. This
/// POST was the one manual admission door
/// `CompanyRuntime::ensure_not_emergency_stopped`'s own doc did not
/// enumerate, because a workflow run never reaches `run_cycle`,
/// `spawn_follow_up`, or the boot reconciler.
#[tokio::test]
async fn an_emergency_stopped_company_refuses_a_manual_run() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;
    c.runtime
        .emergency_pause(
            crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::Operator,
                id: "owner".into(),
            },
            None,
        )
        .await
        .expect("pause");

    let response = c
        .app
        .clone()
        .oneshot(run_request(serde_json::json!({ "input": {} })))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "a stopped company must refuse a manual run exactly as it refuses chat"
    );
    assert!(
        !c.completed.load(Ordering::SeqCst),
        "the refusal must return before the runner ever ran, let alone finished"
    );
}

/// Cancel a live run: `200`, and it settles as cancelled rather than as
/// an error.
#[tokio::test]
async fn cancelling_a_live_run_stops_it_and_records_it_as_cancelled() {
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

    let response = c
        .app
        .clone()
        .oneshot(cancel_request(&run_id))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["cancelling"], true);

    let CompanyEvent::WorkflowRunFinished {
        cancelled,
        error,
        run_id: journaled_id,
        ..
    } = await_finished(&c.runtime)
        .await
        .expect("a cancelled run still journals a finish")
    else {
        unreachable!()
    };
    assert!(cancelled, "the outcome must say it was stopped");
    assert!(
        error.is_none(),
        "a deliberate stop is not a failure: {error:?}"
    );
    assert_eq!(
        journaled_id.as_deref(),
        Some(run_id.as_str()),
        "the finish carries the id the run route handed back — no second identifier"
    );
    assert!(
        !c.completed.load(Ordering::SeqCst),
        "the run must not have completed its work"
    );
}

/// **B-121: deleting a workflow stops the run of it still in flight.**
///
/// Delete used to take the schedule and the revisions and leave the run
/// executing — and, worse, leave it *uncontrollable*: the only Stop
/// button in the product is on the workflow detail page the delete
/// removes, so the run went on calling models and spending with nothing
/// anywhere able to reach it, still reporting `running: true` under a
/// workflow that no longer existed.
///
/// The assertion that carries the weight is `completed`: the stalled
/// runner finishes its work only when released, so a run that reaches
/// its own completion here is one the delete failed to stop.
#[tokio::test]
async fn deleting_a_workflow_stops_the_run_of_it_still_in_flight() {
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
    assert_eq!(
        c.runtime.run_supervisor().live().len(),
        1,
        "the run has to be genuinely live, or this proves nothing"
    );

    let response = c.app.clone().oneshot(get_workflow_request()).await.unwrap();
    let version = json_body(response).await["version"]
        .as_str()
        .expect("the graph carries its version token")
        .to_string();
    let response = c
        .app
        .clone()
        .oneshot(delete_workflow_request(&version))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // CodeRabbit review (PR #2053): the count the console's toast now
    // reads has to be the sweep's own answer, not a client-side guess —
    // pin it here so a regression back to "no body" or a miscounted
    // sweep shows up as a body assertion rather than only as a wrong
    // toast nobody is testing.
    assert_eq!(json_body(response).await["stoppedRuns"], 1);

    let CompanyEvent::WorkflowRunFinished {
        cancelled,
        error,
        run_id: journaled_id,
        ..
    } = await_finished(&c.runtime)
        .await
        .expect("the deleted workflow's run settles rather than running on")
    else {
        unreachable!()
    };
    assert!(
        cancelled,
        "the run of a deleted workflow settles stopped, on the Stop button's own path"
    );
    assert!(
        error.is_none(),
        "a stop that follows from a delete is not a failure: {error:?}"
    );
    assert_eq!(
        journaled_id.as_deref(),
        Some(run_id.as_str()),
        "the same run the run route handed back — no second identifier"
    );
    assert!(
        !c.completed.load(Ordering::SeqCst),
        "the run must not have gone on to finish the work of a workflow that no \
         longer exists"
    );
}

/// The mirror: a delete with **no** run in flight cancels nothing.
/// Without it the sweep above could quietly grow into "delete stops
/// something" for a company that had nothing to stop.
#[tokio::test]
async fn deleting_an_idle_workflow_stops_nothing() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;

    assert!(c.runtime.run_supervisor().live().is_empty());
    let response = c.app.clone().oneshot(get_workflow_request()).await.unwrap();
    let version = json_body(response).await["version"]
        .as_str()
        .expect("version")
        .to_string();
    let response = c
        .app
        .clone()
        .oneshot(delete_workflow_request(&version))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // CodeRabbit review (PR #2053): the response body is what the
    // console's toast now reads, so pin the zero here too — the same
    // reason the in-flight case above pins its 1.
    assert_eq!(json_body(response).await["stoppedRuns"], 0);
    assert_eq!(
        c.runtime.stop_runs_of_workflow("demo"),
        0,
        "nothing was in flight, so nothing was stopped"
    );
}
