use super::tests_cancel_delivery::deps_delivering_to_channel;
use super::tests_capped_halt::{GREET, deps, record};
use super::tests_delivery_gate::REPORT_TO_DESK;
use super::*;

use crate::company::parse_workflow;

/// **The committed negative control.** The identical journal state as the
/// test above — run 1 delivered and crashed — but run 2 runs with the
/// durable consult bypassed (`deps.events` unwired, so the fold never runs).
/// The report goes out a second time: the count reaches 2. This proves the
/// guard is load-bearing rather than incidental — without the consult, the
/// re-delivery the whole issue is about happens.
#[tokio::test]
async fn without_the_durable_consult_a_crashed_runs_report_is_re_delivered() {
    use crate::runtime::channel::RecordingChannel;

    let dir = tempfile::tempdir().unwrap();
    let events: Arc<dyn crate::ports::EventLog> =
        Arc::new(crate::store::FsEventLog::new(dir.path()));
    let channel = RecordingChannel::new("engineering");
    let rec = record();
    let file = parse_workflow(REPORT_TO_DESK).expect("parses");

    let deps1 = deps_delivering_to_channel(dir.path(), events.clone(), channel.clone(), true);
    let ctx1 = WorkflowRunContext::new(false);
    run_workflow(
        Arc::new(HarnessPool::new()),
        deps1,
        &rec,
        &file,
        serde_json::json!({ "brief": "quarterly numbers" }),
        &ctx1,
    )
    .await
    .expect("run 1 runs");
    assert_eq!(channel.sent().len(), 1);
    crate::runtime::sweep_interrupted_runs(&events, &rec.id).await;

    // Run 2: SAME journal, but the guard is off (`consult_journal = false`).
    let deps2 = deps_delivering_to_channel(dir.path(), events.clone(), channel.clone(), false);
    let ctx2 = WorkflowRunContext::new(false);
    let run2 = run_workflow(
        Arc::new(HarnessPool::new()),
        deps2,
        &rec,
        &file,
        serde_json::json!({ "brief": "quarterly numbers" }),
        &ctx2,
    )
    .await
    .expect("run 2 runs");

    assert_eq!(
        channel.sent().len(),
        2,
        "without consulting the durable ledger, the crashed run's report goes out again"
    );
    assert_eq!(
        run2.deliveries[0].status,
        crate::ports::DeliveryStatus::Sent,
        "the unguarded re-run delivers rather than skips"
    );
}

/// The cadence guarantee: a run that delivers AND finishes cleanly must not
/// suppress the next scheduled run. Run 1 delivers and its clean finish is
/// journaled; run 2 delivers again — a daily digest keeps going out every
/// day, because a clean finish clears the durable ledger.
#[tokio::test]
async fn a_clean_finish_lets_the_next_run_deliver_again() {
    use crate::runtime::channel::RecordingChannel;

    let dir = tempfile::tempdir().unwrap();
    let events: Arc<dyn crate::ports::EventLog> =
        Arc::new(crate::store::FsEventLog::new(dir.path()));
    let channel = RecordingChannel::new("engineering");
    let rec = record();
    let file = parse_workflow(REPORT_TO_DESK).expect("parses");

    // Run 1 delivers…
    let deps1 = deps_delivering_to_channel(dir.path(), events.clone(), channel.clone(), true);
    let ctx1 = WorkflowRunContext::new(false);
    let run1 = run_workflow(
        Arc::new(HarnessPool::new()),
        deps1,
        &rec,
        &file,
        serde_json::json!({ "brief": "quarterly numbers" }),
        &ctx1,
    )
    .await
    .expect("run 1 runs");
    assert_eq!(channel.sent().len(), 1);
    // …and the caller journals its clean finish, the way a real entry point
    // does once the run returns.
    crate::runtime::record_run_finished(&events, &rec.id, &file.id, true, &ctx1.run_id, Ok(&run1))
        .await;

    // Run 2 (the next day's fire) delivers again — never suppressed.
    let deps2 = deps_delivering_to_channel(dir.path(), events.clone(), channel.clone(), true);
    let ctx2 = WorkflowRunContext::new(false);
    let run2 = run_workflow(
        Arc::new(HarnessPool::new()),
        deps2,
        &rec,
        &file,
        serde_json::json!({ "brief": "quarterly numbers" }),
        &ctx2,
    )
    .await
    .expect("run 2 runs");

    assert_eq!(
        channel.sent().len(),
        2,
        "a clean finish must not suppress the next legitimate delivery"
    );
    assert_eq!(
        run2.deliveries[0].status,
        crate::ports::DeliveryStatus::Sent
    );
}

// --- issue #371: the per-node progress trail -----------------------------

/// Deps with a real filesystem journal wired, so the progress path is
/// exercised end to end rather than through a double: the claim under test
/// is that these events reach disk in an order a reader can rely on.
pub(super) fn deps_with_events(
    dir: &std::path::Path,
) -> (HarnessDeps, Arc<dyn crate::ports::EventLog>) {
    let events: Arc<dyn crate::ports::EventLog> = Arc::new(crate::store::FsEventLog::new(dir));
    let mut deps = deps(dir);
    deps.events = Some(events.clone());
    (deps, events)
}

/// Every event journaled for `company`, oldest first.
pub(super) async fn journaled(
    events: &Arc<dyn crate::ports::EventLog>,
    company: &CompanyId,
) -> Vec<CompanyEvent> {
    events
        .read_from(company, crate::ports::types::EventSeq::new(0), usize::MAX)
        .await
        .expect("read")
        .into_iter()
        .map(|s| s.event)
        .collect()
}

/// The ordering guarantee the whole read side rests on: a run journals its
/// start, then — for each non-trigger node, in execution order — a
/// `WorkflowNodeStarted` immediately followed by its `WorkflowNodeFinished`
/// (issue #382), and all of them are durable before `run_workflow` returns —
/// so the caller's `WorkflowRunFinished` can only ever land after them.
///
/// `GREET` is `start → ceo → done`; `start` is the trigger and the engine
/// reports no step for it, so exactly two nodes are owed, each with its own
/// started/finished pair. The **started-before-finished** ordering is the
/// #382 invariant: both frames ride one unbounded channel and the collector
/// drains it in order, so a node cannot settle on the journal before it
/// opens.
#[tokio::test]
async fn a_run_journals_a_start_then_a_started_finished_pair_per_non_trigger_node() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(HarnessPool::new());
    let rec = record();
    let (deps, events) = deps_with_events(dir.path());
    pool.ensure(&rec, &deps).await.expect("roster builds");

    let file = parse_workflow(GREET).expect("workflow parses");
    let ctx = WorkflowRunContext::new(false);
    run_workflow(pool, deps, &rec, &file, Value::Null, &ctx)
        .await
        .expect("workflow runs");

    let journal = journaled(&events, &rec.id).await;
    let trail: Vec<String> = journal
        .iter()
        .map(|e| match e {
            CompanyEvent::WorkflowRunStarted { .. } => "started".to_string(),
            CompanyEvent::WorkflowNodeStarted { node_id, .. } => format!("nodestart:{node_id}"),
            CompanyEvent::WorkflowNodeFinished { node_id, .. } => format!("node:{node_id}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        trail,
        vec![
            "started",
            "nodestart:ceo",
            "node:ceo",
            "nodestart:done",
            "node:done"
        ],
        "expected the run start, then a started→finished pair per non-trigger node in order"
    );

    // One run id across the whole trail — the correlation the fold groups on.
    for event in &journal {
        match event {
            CompanyEvent::WorkflowRunStarted {
                run_id,
                workflow_id,
                scheduled,
                started_by,
                ..
            } => {
                assert_eq!(run_id, &ctx.run_id);
                assert_eq!(workflow_id, "greet");
                assert!(!scheduled, "a manual run is not flagged scheduled");
                assert_eq!(
                    started_by,
                    &Some(ctx.started_by.clone()),
                    "the runner writes the context's started_by into the journal (issue #1862 prerequisite)"
                );
            }
            CompanyEvent::WorkflowNodeStarted {
                run_id,
                workflow_id,
                ..
            } => {
                assert_eq!(run_id, &ctx.run_id);
                assert_eq!(workflow_id, "greet");
            }
            CompanyEvent::WorkflowNodeFinished {
                run_id,
                status,
                workflow_id,
                ..
            } => {
                assert_eq!(run_id, &ctx.run_id);
                assert_eq!(workflow_id, "greet");
                assert_eq!(*status, WorkflowNodeStatus::Ok);
            }
            other => panic!("unexpected event on the journal: {other:?}"),
        }
    }
}

/// The scheduled flag rides the *start*, not only the outcome — which is
/// what lets the console mark a cron fire as such while it is still running.
#[tokio::test]
async fn a_scheduled_run_is_flagged_on_its_start_event() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(HarnessPool::new());
    let rec = record();
    let (deps, events) = deps_with_events(dir.path());
    pool.ensure(&rec, &deps).await.expect("roster builds");

    let file = parse_workflow(GREET).expect("workflow parses");
    run_workflow(
        pool,
        deps,
        &rec,
        &file,
        Value::Null,
        &WorkflowRunContext::new(true),
    )
    .await
    .expect("workflow runs");

    let journal = journaled(&events, &rec.id).await;
    let CompanyEvent::WorkflowRunStarted { scheduled, .. } = &journal[0] else {
        panic!("expected the start first, got {:?}", journal[0]);
    };
    assert!(scheduled);
}

/// The arm the issue is really about: a run that dies partway still leaves
/// the nodes that DID complete on the journal, under the same run id the
/// caller will stamp on the failure. That pairing is what lets the console
/// say how far a failed run got instead of only that it failed.
///
/// The graph is `start → ceo → fetch → done`, where `fetch` is an
/// `http_request` to loopback that the SSRF guard refuses. `on_error`
/// defaults to `stop`, so the run ends there — with `ceo` already recorded.
#[tokio::test]
async fn a_failed_run_still_journals_the_nodes_that_completed() {
    let src = r#"
id = "partial"
name = "Partial"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "ceo"
kind = "agent"
name = "CEO"
agent = "ceo"
[[node]]
id = "fetch"
kind = "http_request"
name = "Fetch"
[node.config]
method = "GET"
url = "http://127.0.0.1:9/"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "ceo"
[[edge]]
from = "ceo"
to = "fetch"
[[edge]]
from = "fetch"
to = "done"
"#;
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(HarnessPool::new());
    let rec = record();
    let (deps, events) = deps_with_events(dir.path());
    pool.ensure(&rec, &deps).await.expect("roster builds");

    let file = parse_workflow(src).expect("parses");
    let ctx = WorkflowRunContext::new(false);
    let outcome = run_workflow(pool, deps, &rec, &file, Value::Null, &ctx).await;
    assert!(outcome.is_err(), "the loopback fetch must fail the run");

    let journal = journaled(&events, &rec.id).await;
    // The start is there, and so is the node that got through before the
    // failure. `done` is not — an unreached node contributes no row, so
    // absence means "never reached", never "silently dropped".
    assert!(matches!(
        journal.first(),
        Some(CompanyEvent::WorkflowRunStarted { .. })
    ));
    let nodes: Vec<&String> = journal
        .iter()
        .filter_map(|e| match e {
            CompanyEvent::WorkflowNodeFinished { node_id, .. } => Some(node_id),
            _ => None,
        })
        .collect();
    assert!(nodes.contains(&&"ceo".to_string()), "{nodes:?}");
    assert!(!nodes.contains(&&"done".to_string()), "{nodes:?}");

    // **The failing node names itself.** A node that dies under the default
    // `stop` policy still reports a step, with `Error` status, before the
    // run ends — so failure attribution on the canvas is exact rather than
    // inferred from "the last node we saw running". Worth pinning: if the
    // engine ever stopped reporting the failing step, the console would
    // silently fall back to guessing, and nothing else would notice.
    let statuses: Vec<(&String, &WorkflowNodeStatus)> = journal
        .iter()
        .filter_map(|e| match e {
            CompanyEvent::WorkflowNodeFinished {
                node_id, status, ..
            } => Some((node_id, status)),
            _ => None,
        })
        .collect();
    assert_eq!(
        statuses,
        vec![
            (&"ceo".to_string(), &WorkflowNodeStatus::Ok),
            (&"fetch".to_string(), &WorkflowNodeStatus::Error),
        ],
        "the node that failed must be reported as the errored one"
    );

    // Every row shares the caller's id, so the `WorkflowRunFinished` the
    // caller journals for this failure groups with them.
    for event in &journal {
        let run_id = match event {
            CompanyEvent::WorkflowRunStarted { run_id, .. } => run_id,
            CompanyEvent::WorkflowNodeStarted { run_id, .. } => run_id,
            CompanyEvent::WorkflowNodeFinished { run_id, .. } => run_id,
            other => panic!("unexpected event: {other:?}"),
        };
        assert_eq!(run_id, &ctx.run_id);
    }
}

/// A build with no journal wired (the default runtime, and every other test
/// in this module) runs exactly as it did before #371 — no start, no
/// observer, no collector task. The progress path degrades to nothing
/// rather than to a half-written trail.
#[tokio::test]
async fn a_runtime_without_a_journal_records_no_progress() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(HarnessPool::new());
    let rec = record();
    let deps = deps(dir.path());
    assert!(deps.events.is_none(), "this is the default-build shape");
    pool.ensure(&rec, &deps).await.expect("roster builds");

    let file = parse_workflow(GREET).expect("workflow parses");
    let run = run_workflow(
        pool,
        deps,
        &rec,
        &file,
        Value::Null,
        &WorkflowRunContext::new(false),
    )
    .await
    .expect("workflow runs");
    assert!(run.pending_approvals.is_empty());
}

#[tokio::test]
async fn a_trigger_rerun_records_its_resume_semantic() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(HarnessPool::new());
    let rec = record();
    let (deps, events) = deps_with_events(dir.path());
    pool.ensure(&rec, &deps).await.expect("roster builds");
    let ctx = WorkflowRunContext::new(false)
        .with_resume_semantic(crate::ports::ResumeSemantic::ReRunFromTrigger);

    run_workflow(
        pool,
        deps,
        &rec,
        &parse_workflow(GREET).expect("workflow parses"),
        Value::Null,
        &ctx,
    )
    .await
    .expect("fallback run completes");

    assert!(matches!(
        journaled(&events, &rec.id).await.first(),
        Some(CompanyEvent::WorkflowRunStarted {
            resume_semantic: Some(crate::ports::ResumeSemantic::ReRunFromTrigger),
            ..
        })
    ));
}

// --- #383: stopping a run in flight ------------------------------------

/// A model that parks forever on its first call, after announcing that it
/// got there.
///
/// This is the lever the whole cancel test rests on, and it has to be an
/// **agent** node rather than an `http_request` one: a loopback stall server
/// is unreachable here by design, because the upstream `url_guard` refuses
/// private/loopback addresses regardless of the company's allowlist (see
/// `t5_http_request_to_loopback_is_ssrf_denied`). An agent node is the one
/// node kind whose executor this test can hold open deterministically —
/// which is also the realistic wedge: the run an operator actually wants to
/// stop is one sitting on a slow inference call.
pub(super) struct StallingProvider {
    pub(super) entered: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl tinyinference::model::ChatModel<()> for StallingProvider {
    async fn invoke(
        &self,
        _state: &(),
        _request: tinyinference::model::ModelRequest,
    ) -> tinyinference::Result<tinyinference::model::ModelResponse> {
        self.entered.notify_waiters();
        // Never returns. The run is stopped by the future being dropped,
        // which is the mechanism under test.
        std::future::pending::<()>().await;
        unreachable!("the stalling provider is never released")
    }
}

impl crate::harness::provider::HarnessModel for StallingProvider {
    fn telemetry_provider_id(&self) -> String {
        "stalling".to_string()
    }
}

/// `start → shape → ceo → done`: a transform that finishes instantly, then
/// an agent node that never will. Cancelling between the two is what proves
/// the trail keeps the completed node and only the completed node.
pub(super) const STALLS: &str = r#"
id = "stalls"
name = "Stalls"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "shape"
kind = "transform"
name = "Shape"
[[node]]
id = "ceo"
kind = "agent"
name = "CEO"
agent = "ceo"
prompt = "Think about it."
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "shape"
[[edge]]
from = "shape"
to = "ceo"
[[edge]]
from = "ceo"
to = "done"
"#;

/// **The keystone cancel test — the HARD-ABORT arm (issue #383/#398).** An
/// operator stops a run wedged on an agent node that never returns, so it can
/// never reach a node boundary and the clean token path (below) cannot settle
/// it. Four things have to be true at once:
///
/// 1. the run settles as `cancelled`, not as an error — a deliberate stop is
///    not a failure and must never land in the failure count;
/// 2. the journal keeps a node row for the node that **completed** and none
///    for the one that was still executing — "how far did it get before I
///    stopped it" is the question the trail exists to answer, and inventing
///    a row for the wedged node would answer it wrongly;
/// 3. **the grace window is actually spent** — a wedged node cannot be
///    hard-aborted before `CANCEL_HARD_ABORT_GRACE`, because the runner first
///    flips the engine token and waits that long for a clean wind-down;
/// 4. **once the grace is up, it comes back fast** — the hard abort must drop
///    the engine future *before* the observer, or the per-node handlers keep
///    their observer `Arc` clones, the progress channel stays open, and the
///    collector join blocks for the full `PROGRESS_DRAIN_TIMEOUT` on TOP of
///    the grace. That bug still passes 1–2 (the timeout swallows it and the
///    run settles correctly in the end); only the clock catches it, which is
///    why this asserts a bound rather than just an outcome.
#[tokio::test]
async fn a_cancelled_run_settles_fast_keeping_only_its_completed_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(HarnessPool::new());
    let rec = record();
    let entered = Arc::new(tokio::sync::Notify::new());
    let (mut deps, events) = deps_with_events(dir.path());
    deps.provider = Arc::new(StallingProvider {
        entered: entered.clone(),
    });
    deps.provider_slug = "stalling".to_string();
    pool.ensure(&rec, &deps).await.expect("roster builds");

    let file = parse_workflow(STALLS).expect("workflow parses");
    let ctx = WorkflowRunContext::new(false);
    let cancel = ctx.cancel.clone();

    // Registered *before* the run starts, so the wedged node cannot slip
    // past the notification and leave this test waiting forever.
    let reached_the_agent = entered.notified();

    let mut run = Box::pin(run_workflow(pool, deps, &rec, &file, Value::Null, &ctx));
    tokio::select! {
        _ = &mut run => panic!("the run finished, so the agent node did not stall"),
        () = reached_the_agent => {}
    }

    // The operator presses Cancel. From here the clock is the assertion.
    let pressed = std::time::Instant::now();
    cancel.cancel();
    let run = tokio::time::timeout(std::time::Duration::from_secs(30), run)
        .await
        .expect("the cancelled run never returned at all")
        .expect("a cancelled run is Ok, not Err");
    let elapsed = pressed.elapsed();

    assert!(run.cancelled, "the run must report that it was stopped");
    assert!(
        run.deliveries.is_empty(),
        "a cancelled run must not route reports for work it did not finish"
    );

    // **The grace was actually spent.** A wedged node cannot reach a boundary,
    // so the runner flips the token and waits the full `CANCEL_HARD_ABORT_GRACE`
    // before dropping the future. Landing below that would mean the token path
    // was skipped — a wedged run must never hard-abort early.
    assert!(
        elapsed >= CANCEL_HARD_ABORT_GRACE,
        "cancelling took {elapsed:?} — shorter than the grace window, so the clean node-boundary \
         wind-down was not attempted before the hard abort"
    );
    // **The drain-timeout guard.** Once the grace is up, the hard abort drops
    // the engine future, which must close the progress channel so the
    // collector join returns in milliseconds. If `drop(engine)` were missing
    // the join would stall for the full `PROGRESS_DRAIN_TIMEOUT` (10s) ON TOP
    // of the grace. This bounds the total at grace + a healthy drain, far
    // below grace + the drain timeout, so it fails loudly on that bug without
    // being flaky on a loaded CI box.
    assert!(
        elapsed < CANCEL_HARD_ABORT_GRACE + std::time::Duration::from_secs(2),
        "cancelling took {elapsed:?} — past the grace the progress channel did not close, so \
         the collector join stalled until the drain timeout. Check that the engine future is \
         dropped BEFORE the observer in `run_workflow_inner`."
    );
    assert!(
        elapsed < CANCEL_HARD_ABORT_GRACE + PROGRESS_DRAIN_TIMEOUT,
        "cancel latency reached the grace plus the full drain timeout"
    );

    // The trail: `shape` completed, `ceo` was still executing. Neither the
    // wedged node nor anything downstream may appear.
    let journal = journaled(&events, &rec.id).await;
    let nodes: Vec<String> = journal
        .iter()
        .filter_map(|e| match e {
            CompanyEvent::WorkflowNodeFinished { node_id, .. } => Some(node_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        nodes,
        vec!["shape".to_string()],
        "only the node that actually finished belongs on the trail"
    );
    // The start is still there and still correlates, so the caller's
    // `WorkflowRunFinished{cancelled}` groups with this trail rather than
    // stranding it.
    let CompanyEvent::WorkflowRunStarted { run_id, .. } = &journal[0] else {
        panic!("expected the start first, got {:?}", journal[0]);
    };
    assert_eq!(run_id, &ctx.run_id);
}
