use super::workflows_test_support::hosted_mode::*;
use super::*;
use crate::server::router;

/// **The issue's durable half at the HTTP boundary.** A run's start,
/// its per-node rows and its outcome come back as ONE history entry
/// carrying the node trail — which is what makes a scheduled run's
/// failure point readable after the fact.
#[tokio::test]
async fn run_history_groups_a_runs_nodes_under_one_entry() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;

    journal_start(&state, &id, "digest", "run-1", true).await;
    journal_node(
        &state,
        &id,
        "digest",
        "run-1",
        "ceo",
        WorkflowNodeStatus::Ok,
    )
    .await;
    journal_node(
        &state,
        &id,
        "digest",
        "run-1",
        "send",
        WorkflowNodeStatus::Error,
    )
    .await;
    journal_finish(&state, &id, "digest", "run-1", true, Some("send failed")).await;

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/runs", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    let rows = body["runs"].as_array().expect("array");
    assert_eq!(rows.len(), 1, "four journal rows fold to one run: {body}");

    assert_eq!(rows[0]["runId"], "run-1");
    assert_eq!(rows[0]["error"], "send failed");
    assert!(rows[0].get("running").is_none(), "a settled run: {body}");
    assert!(rows[0]["startedAtMillis"].is_number(), "{body}");

    // In finish order, with the status and duration the canvas paints.
    let nodes = rows[0]["nodes"].as_array().expect("nodes");
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["nodeId"], "ceo");
    assert_eq!(nodes[0]["status"], "ok");
    assert_eq!(nodes[0]["elapsedMs"], 42);
    assert_eq!(nodes[1]["nodeId"], "send");
    assert_eq!(nodes[1]["status"], "error");
}

/// **The issue.** A run still in flight comes back naming the node it
/// is standing on, not just the ones it is done with.
///
/// Before this the fold read `WorkflowNodeStarted` nowhere, so an
/// in-flight run's only per-node facts were its finishes — and every
/// console that learned about a run from the history rather than from a
/// start frame (a reload, a cron fire, a reconnect, a workflow switch
/// and back) painted a graph with a gap where the working node was.
#[tokio::test]
async fn run_history_names_the_node_a_running_run_is_executing() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;

    // Registered on the supervisor, and the guard held across the read:
    // since #1009 a start with no finish whose id is NOT live is settled
    // by the read itself, so a genuinely in-flight run is the only way
    // to see `running: true` — and it is the case under test.
    let runtime = state.registry().get(&id).expect("registered");
    let (ctx, _guard) = runtime
        .run_supervisor()
        .begin("digest", true)
        .expect("under the default cap");
    let run = ctx.run_id.clone();
    journal_start(&state, &id, "digest", &run, true).await;
    journal_node_started(&state, &id, "digest", &run, "ceo").await;
    journal_node(&state, &id, "digest", &run, "ceo", WorkflowNodeStatus::Ok).await;
    // Started and NOT finished — the node the run is on. No finish is
    // journaled for it, which is the whole shape under test.
    journal_node_started(&state, &id, "digest", &run, "draft").await;

    let response = router(state.clone())
        .oneshot(request("GET", "/api/v1/company/workflows/runs", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    let rows = body["runs"].as_array().expect("array");
    assert_eq!(rows.len(), 1, "one run: {body}");
    assert_eq!(rows[0]["running"], true, "still in flight: {body}");
    assert_eq!(rows[0]["runId"], run, "{body}");

    // In start order, both brackets — the reader subtracts.
    let started = rows[0]["startedNodes"].as_array().expect("startedNodes");
    assert_eq!(
        started.len(),
        2,
        "both starts are recorded, finished or not: {body}"
    );
    assert_eq!(started[0], "ceo");
    assert_eq!(started[1], "draft");

    // Only the finished one has a node row, so "started minus finished"
    // is exactly the node executing now.
    let nodes = rows[0]["nodes"].as_array().expect("nodes");
    assert_eq!(nodes.len(), 1, "one node has finished: {body}");
    assert_eq!(nodes[0]["nodeId"], "ceo");
}

/// A start whose run has no entry is dropped, not turned into a run of
/// its own — the same rule the finish arm follows.
///
/// The `?workflow=` filter is the reachable way to produce this: the
/// start row for another workflow's run never opened an entry, so its
/// node brackets have nothing to attach to.
#[tokio::test]
async fn a_started_node_of_a_filtered_out_run_is_dropped() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;

    journal_start(&state, &id, "other", "run-other", false).await;
    journal_node_started(&state, &id, "other", "run-other", "ceo").await;
    journal_start(&state, &id, "digest", "run-mine", false).await;
    journal_node_started(&state, &id, "digest", "run-mine", "draft").await;

    let response = router(state)
        .oneshot(request(
            "GET",
            "/api/v1/company/workflows/runs?workflow=digest",
            None,
        ))
        .await
        .unwrap();
    let body = json_body(response).await;
    let rows = body["runs"].as_array().expect("array");
    assert_eq!(rows.len(), 1, "only the asked-for workflow: {body}");
    assert_eq!(rows[0]["runId"], "run-mine");
    let started = rows[0]["startedNodes"].as_array().expect("startedNodes");
    assert_eq!(started.len(), 1, "{body}");
    assert_eq!(started[0], "draft");
}

/// A run journaled before #382 — no starts at all — keeps the wire shape
/// it had: `startedNodes` is omitted entirely rather than sent empty.
#[tokio::test]
async fn a_run_with_no_started_rows_omits_the_field() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;

    journal_start(&state, &id, "digest", "run-old", false).await;
    journal_node(
        &state,
        &id,
        "digest",
        "run-old",
        "ceo",
        WorkflowNodeStatus::Ok,
    )
    .await;
    journal_finish(&state, &id, "digest", "run-old", false, None).await;

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/runs", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert!(
        body["runs"][0].get("startedNodes").is_none(),
        "an empty trail is absent, not `[]`: {body}"
    );
}

/// The receipt SURVIVES the finish, so a run that was cancelled or lost
/// mid-node still says which node it was standing on.
///
/// That id is the one fact neither list carries alone: `nodes` never
/// gets a row for a node that did not finish, and a cleared
/// `startedNodes` would throw away the only record that it began. The
/// console pairs this with `running` before painting anything live —
/// see `statesFromRun` — so keeping it cannot leave a settled run
/// spinning.
#[tokio::test]
async fn a_settled_run_keeps_the_node_it_was_standing_on() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;

    journal_start(&state, &id, "digest", "run-cut", false).await;
    journal_node_started(&state, &id, "digest", "run-cut", "ceo").await;
    journal_node(
        &state,
        &id,
        "digest",
        "run-cut",
        "ceo",
        WorkflowNodeStatus::Ok,
    )
    .await;
    // Begun, and then the run ended without it ever finishing.
    journal_node_started(&state, &id, "digest", "run-cut", "draft").await;
    journal_finish(&state, &id, "digest", "run-cut", false, Some("cancelled")).await;

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/runs", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert!(body["runs"][0].get("running").is_none(), "settled: {body}");
    let started = body["runs"][0]["startedNodes"]
        .as_array()
        .expect("startedNodes");
    assert_eq!(started.len(), 2, "{body}");
    assert_eq!(started[1], "draft");
    let nodes = body["runs"][0]["nodes"].as_array().expect("nodes");
    assert_eq!(nodes.len(), 1, "`draft` never finished: {body}");
}

/// Issues #881 / #880 at the HTTP boundary: a blocked run reads as
/// blocked in the history, and **its node chip is relabelled too**.
///
/// The node row is journaled live, node by node, long before anything
/// knows the run stopped for an approval rather than a fault — so the
/// durable `WorkflowNodeFinished` says `error`, honestly, in the
/// engine's own terms. Without the read-side relabel the panel would
/// show a run that says "blocked" beside a node chip that says
/// "failed", and an operator would go hunting for a bug that is not
/// there.
#[tokio::test]
async fn run_history_reports_a_blocked_node_and_the_approvals_it_parked() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;

    journal_start(&state, &id, "digest", "run-b", false).await;
    // The engine's own account: the capability returned an error, so
    // the observer reported `error`.
    journal_node(
        &state,
        &id,
        "digest",
        "run-b",
        "spec",
        WorkflowNodeStatus::Error,
    )
    .await;
    let runtime = state.registry().get(&id).expect("registered");
    runtime
        .events()
        .append(
            &id,
            CompanyEvent::WorkflowRunFinished {
                workflow_id: "digest".to_string(),
                scheduled: false,
                run_id: Some("run-b".to_string()),
                deliveries: Vec::new(),
                pending_approvals: vec!["spec".to_string()],
                // The whole point: a blocked run carries NO error.
                error: None,
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                blocked_nodes: vec![crate::ports::WorkflowBlockedNode {
                    node_id: "spec".to_string(),
                    tools: vec!["publish_artifact".to_string()],
                    approval_ids: vec!["appr-1".to_string()],
                    unparkable: 0,
                    stranded: 0,
                    blockers: 0,
                }],
                approvals: vec![crate::ports::WorkflowRunApprovalRow {
                    node_id: Some("spec".to_string()),
                    tool: Some("publish_artifact".to_string()),
                    outcome: crate::ports::WorkflowApprovalOutcome::Parked,
                    approval_id: Some("appr-1".to_string()),
                }],
            },
        )
        .await
        .expect("append");

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/runs", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    let rows = body["runs"].as_array().expect("array");
    assert_eq!(rows.len(), 1, "{body}");
    assert!(
        rows[0].get("error").is_none(),
        "a run waiting on a person did not fail: {body}"
    );
    assert_eq!(rows[0]["blockedNodes"][0]["nodeId"], "spec");
    assert_eq!(rows[0]["approvals"][0]["outcome"], "parked");
    assert_eq!(
        rows[0]["nodes"][0]["status"], "blocked",
        "the node chip must agree with the run's terminal reading: {body}"
    );
}

/// Issue #1143. The run's receipt names a card the queue no longer
/// holds, so the history says so instead of offering it as a decision.
///
/// This is the observed dead end: the drawer rendered "decide in
/// Approvals" links for `appr-gone`, the operator followed one, and
/// Approvals said "All clear". The run's `approvalIds` is a receipt and
/// is right to be immutable; what was missing is anything that reads it
/// against the live queue. Nothing is parked in this fixture, so the id
/// is stranded.
#[tokio::test]
async fn run_history_marks_a_blocked_approval_the_queue_no_longer_holds() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;

    journal_start(&state, &id, "digest", "run-s", false).await;
    let runtime = state.registry().get(&id).expect("registered");
    runtime
        .events()
        .append(
            &id,
            CompanyEvent::WorkflowRunFinished {
                workflow_id: "digest".to_string(),
                scheduled: true,
                run_id: Some("run-s".to_string()),
                deliveries: Vec::new(),
                pending_approvals: vec!["spec".to_string()],
                error: None,
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                blocked_nodes: vec![crate::ports::WorkflowBlockedNode {
                    node_id: "spec".to_string(),
                    tools: vec!["shell".to_string()],
                    approval_ids: vec!["appr-gone".to_string()],
                    unparkable: 0,
                    stranded: 0,
                    blockers: 0,
                }],
                approvals: Vec::new(),
            },
        )
        .await
        .expect("append");

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/runs", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert_eq!(
        body["runs"][0]["blockedNodes"][0]["stranded"], 1,
        "an approval id the journal no longer holds must read as stranded, \
         or the drawer goes on linking to an empty queue: {body}"
    );
    // Issue #1189, and the assertion that pins the reordering: the
    // verdict pass now runs AFTER this join, so it can see what the join
    // wrote. With the old ordering the row read `blocked` here — the
    // blocked-node list said "cannot be continued" and the run's own
    // one-word reading said "go and decide it", on the same row.
    assert_eq!(
        body["runs"][0]["verdict"], "stranded",
        "the verdict must be derived after the reconciliation, not before it: {body}"
    );
}

/// The other direction, and the reason the test above proves anything.
///
/// A field that marked *every* run stranded would satisfy the assertion
/// above and be worse than no field at all — it would retire live work.
/// Here the approval is genuinely parked, on both halves the runtime
/// reads (the gate's map and the journal), so `stranded` stays zero and
/// is skipped off the wire entirely.
#[tokio::test]
async fn run_history_leaves_a_live_approval_decidable() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;
    let runtime = state.registry().get(&id).expect("registered");

    let effect = crate::ports::types::Effect {
        kind: "filing.submit".into(),
        group: crate::ports::types::EffectGroup::Sign,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: Some("run-live".to_string()),
    };
    let approval_id = runtime
        .approvals
        .park(runtime.id(), effect.clone())
        .await
        .expect("park");
    runtime
        .journal()
        .record_parked(
            &approval_id,
            &effect,
            crate::ports::now_millis(),
            crate::runtime::journal::TaskLink::Unlinked,
            crate::runtime::journal::ApprovalConversation {
                thread: None,
                parent: None,
            },
            None,
        )
        .await
        .expect("record");

    journal_start(&state, &id, "digest", "run-live", false).await;
    runtime
        .events()
        .append(
            &id,
            CompanyEvent::WorkflowRunFinished {
                workflow_id: "digest".to_string(),
                scheduled: true,
                run_id: Some("run-live".to_string()),
                deliveries: Vec::new(),
                pending_approvals: vec!["spec".to_string()],
                error: None,
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                blocked_nodes: vec![crate::ports::WorkflowBlockedNode {
                    node_id: "spec".to_string(),
                    tools: vec!["shell".to_string()],
                    approval_ids: vec![approval_id.as_ref().to_string()],
                    unparkable: 0,
                    stranded: 0,
                    blockers: 0,
                }],
                approvals: Vec::new(),
            },
        )
        .await
        .expect("append");

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/runs", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert!(
        body["runs"][0]["blockedNodes"][0].get("stranded").is_none(),
        "a parked approval is still decidable, so nothing may be marked \
         stranded: {body}"
    );
    assert_eq!(
        body["runs"][0]["verdict"], "blocked",
        "a run with a live card is still blocked, not stranded: {body}"
    );
}

/// Issue #1189, THE regression test for the bigger half of the defect.
///
/// The marketing tenant's shape, verbatim: gate nodes on
/// `pendingApprovals`, `blockedNodes` empty, `approvals` empty, and an
/// empty queue. #1143's join is keyed on approval ids this shape never
/// records, so it could not touch these rows — 34 of the tenant's 60
/// runs went on scoring `awaiting-approval` about a person with nothing
/// to answer.
#[tokio::test]
async fn run_history_scores_a_gate_run_with_no_live_card_as_stranded() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;

    journal_start(&state, &id, "sports_blog", "run-g", true).await;
    let runtime = state.registry().get(&id).expect("registered");
    runtime
        .events()
        .append(
            &id,
            CompanyEvent::WorkflowRunFinished {
                workflow_id: "sports_blog".to_string(),
                scheduled: true,
                run_id: Some("run-g".to_string()),
                deliveries: Vec::new(),
                pending_approvals: vec!["fetch_bbc".to_string(), "fetch_espn".to_string()],
                error: None,
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                // The whole point of this shape: a paused gate writes
                // NEITHER of the two things #1143 reads.
                blocked_nodes: Vec::new(),
                approvals: Vec::new(),
            },
        )
        .await
        .expect("append");

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/runs", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert_eq!(
        body["runs"][0]["strandedApprovals"], 2,
        "both gates lost their card, and nothing else on this row says so: {body}"
    );
    assert_eq!(
        body["runs"][0]["verdict"], "stranded",
        "nothing in the queue is waiting on this run: {body}"
    );
}

/// The negative twin, and the reason the test above proves anything.
///
/// A join that marked every gate run stranded would satisfy it and be
/// far worse than no join: it would retire runs an operator can still
/// act on. Here the gate's card is genuinely parked — a real
/// `workflow.approve` effect carrying this run's id and this node's id,
/// which is the pair the join keys on — so the run stays awaiting and
/// `strandedApprovals` is skipped off the wire entirely.
#[tokio::test]
async fn run_history_leaves_a_gate_run_with_a_live_card_awaiting() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _store, id) = hosted_state(&home).await;
    let runtime = state.registry().get(&id).expect("registered");

    // Built by the same `gate_effect` the runner parks with, so the
    // shape the join reads is the shape the park writes.
    let effect = crate::runtime::workflow_resume::gate_effect(
        "sports_blog",
        "fetch_bbc",
        &serde_json::json!({}),
        "run-live-gate",
        &[],
        &[],
        None,
    );
    let approval_id = runtime
        .approvals
        .park(runtime.id(), effect.clone())
        .await
        .expect("park");
    runtime
        .journal()
        .record_parked(
            &approval_id,
            &effect,
            crate::ports::now_millis(),
            crate::runtime::journal::TaskLink::Unlinked,
            crate::runtime::journal::ApprovalConversation {
                thread: None,
                parent: None,
            },
            None,
        )
        .await
        .expect("record");

    journal_start(&state, &id, "sports_blog", "run-live-gate", true).await;
    runtime
        .events()
        .append(
            &id,
            CompanyEvent::WorkflowRunFinished {
                workflow_id: "sports_blog".to_string(),
                scheduled: true,
                run_id: Some("run-live-gate".to_string()),
                deliveries: Vec::new(),
                pending_approvals: vec!["fetch_bbc".to_string()],
                error: None,
                cancelled: false,
                notices: Vec::new(),
                board: Vec::new(),
                blocked_nodes: Vec::new(),
                approvals: Vec::new(),
            },
        )
        .await
        .expect("append");

    let response = router(state)
        .oneshot(request("GET", "/api/v1/company/workflows/runs", None))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert!(
        body["runs"][0].get("strandedApprovals").is_none(),
        "the gate's card is on the queue, so nothing may be marked stranded: {body}"
    );
    assert_eq!(
        body["runs"][0]["verdict"], "awaiting-approval",
        "a decidable gate is still awaiting a person: {body}"
    );
}
