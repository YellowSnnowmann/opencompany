use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};

use super::*;
use crate::company::CompanyManifest;
use crate::ports::types::{Actor, ActorKind, ApprovalId, CompanyId, Verdict};
use crate::ports::{WorkflowRun, WorkflowRunContext, WorkflowRunner};
use crate::runtime::RuntimeBuilder;
use crate::runtime::journal::{ApprovalConversation, TaskLink};

/// What the resume actually asked for.
#[derive(Clone, Debug)]
struct StartedRun {
    workflow_id: String,
    input: Value,
    run_id: String,
    started_by: crate::ports::types::StartedBy,
    /// Which way the run entered the engine — `NodeRestart` (a checkpoint
    /// resume) or `ReRunFromTrigger` — so a test can pin which one a
    /// fingerprint check chose without inspecting the engine's own
    /// behaviour. Read only by the `openhuman`-gated checkpoint-fingerprint
    /// tests below — `#[cfg]`'d rather than left unconditional, or the
    /// default lane (which never reads it) fails `-D dead_code`.
    #[cfg(feature = "openhuman")]
    resume_semantic: Option<crate::ports::ResumeSemantic>,
}

/// A runner that records every run it is handed and settles immediately.
#[derive(Default)]
struct RecordingRunner {
    started: Mutex<Vec<StartedRun>>,
}

impl RecordingRunner {
    fn started(&self) -> Vec<StartedRun> {
        self.started.lock().expect("recording runner").clone()
    }
}

#[async_trait]
impl WorkflowRunner for RecordingRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        workflow: &crate::company::WorkflowFile,
        input: Value,
        ctx: &WorkflowRunContext,
    ) -> crate::Result<WorkflowRun> {
        self.started
            .lock()
            .expect("recording runner")
            .push(StartedRun {
                workflow_id: workflow.id.clone(),
                input,
                run_id: ctx.run_id.clone(),
                started_by: ctx.started_by.clone(),
                #[cfg(feature = "openhuman")]
                resume_semantic: ctx.resume_semantic,
            });
        Ok(WorkflowRun {
            output: json!({ "ok": true }),
            pending_approvals: Vec::new(),
            deliveries: Vec::new(),
            cancelled: false,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        })
    }
}

const GATED_TOML: &str = r#"
id = "gated"
name = "Gated"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "gate"
kind = "output"
name = "Gate"
requires_approval = true
[[edge]]
from = "start"
to = "gate"
"#;

fn manifest() -> CompanyManifest {
    toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "ceo"
role = "Chief"

[policy]
mode = "full"
"#,
    )
    .expect("manifest parses")
}

fn operator() -> Actor {
    Actor {
        kind: ActorKind::Operator,
        id: "owner".into(),
    }
}

/// A seeded home whose `workflows/` directory holds the gated graph, so the
/// resume's loader finds it exactly as the console run route would.
fn seed_home() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("opencompany-resume-")
        .tempdir()
        .expect("tempdir");
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).expect("workflows dir");
    std::fs::write(workflows.join("gated.toml"), GATED_TOML).expect("seed graph");
    dir
}

/// A runtime with the recording runner installed and the graph on disk.
async fn runtime(
    home: &std::path::Path,
    with_runner: bool,
) -> (
    Arc<crate::company::runtime::CompanyRuntime>,
    Arc<RecordingRunner>,
) {
    let mut rt = RuntimeBuilder::new(home.to_path_buf(), manifest())
        .with_seed_dir(home.to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let runner = Arc::new(RecordingRunner::default());
    if with_runner {
        rt.set_workflow_runner(runner.clone());
    }
    (Arc::new(rt), runner)
}

/// Parks a gate card the way the workflow runner does, returning its id.
async fn park_gate(rt: &Arc<crate::company::runtime::CompanyRuntime>, input: Value) -> ApprovalId {
    park_gate_after(rt, input, &[]).await
}

/// [`park_gate`], for a run that delivered `deliveries` before it paused.
async fn park_gate_after(
    rt: &Arc<crate::company::runtime::CompanyRuntime>,
    input: Value,
    deliveries: &[DeliveryReport],
) -> ApprovalId {
    let effect = gate_effect(
        "gated",
        "gate",
        &input,
        "run-that-paused",
        deliveries,
        &[],
        None,
    );
    let id = rt
        .approvals
        .park(rt.id(), effect.clone())
        .await
        .expect("parks");
    rt.journal()
        .record_parked(
            &id,
            &effect,
            crate::ports::now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .expect("journals");
    id
}

/// [`park_gate`], stamping the card with `started_by` the way
/// `park_pending_gates` does in production (issue #1862 prerequisite) —
/// for pinning that a continuation carries it forward.
async fn park_gate_started_by(
    rt: &Arc<crate::company::runtime::CompanyRuntime>,
    input: Value,
    started_by: &crate::ports::types::StartedBy,
) -> ApprovalId {
    let mut effect = gate_effect("gated", "gate", &input, "run-that-paused", &[], &[], None);
    if let Value::Object(ref mut payload) = effect.payload {
        payload.insert(PAYLOAD_STARTED_BY.to_string(), json!(started_by));
    }
    let id = rt
        .approvals
        .park(rt.id(), effect.clone())
        .await
        .expect("parks");
    rt.journal()
        .record_parked(
            &id,
            &effect,
            crate::ports::now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .expect("journals");
    id
}

/// The resume spawns its run on a detached task, so give it a moment to be
/// recorded. Bounded so a genuine failure fails rather than hangs.
async fn wait_for_runs(runner: &Arc<RecordingRunner>, want: usize) -> Vec<StartedRun> {
    for _ in 0..200 {
        let started = runner.started();
        if started.len() >= want {
            return started;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    runner.started()
}

/// The headline: approving a parked gate starts a **new** run, carrying the
/// gate id in the trigger input's `approvals` so the node that paused now
/// proceeds.
#[tokio::test]
async fn approving_a_gate_starts_a_continuation_run() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_gate(&rt, json!({ "request": "quarterly numbers" })).await;
    assert_eq!(rt.pending_approvals().len(), 1);

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("resolves");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(started.len(), 1, "approving must start exactly one run");
    assert_eq!(started[0].workflow_id, "gated");
    // The gate is approved…
    assert_eq!(started[0].input["approvals"], json!(["gate"]));
    // …and the operator's original topic survives, so the re-run does the
    // same work rather than a blank one.
    assert_eq!(started[0].input["request"], "quarterly numbers");
    // A new causal root, not the paused run's id.
    assert_ne!(started[0].run_id, "run-that-paused");
    assert!(rt.pending_approvals().is_empty(), "the card is decided");
}

/// Issue #1862 prerequisite (a distinct gap from the trigger-site one
/// #1861 owns): approving a gate parked by an agent-triggered run must
/// carry that attribution into the continuation, not reset it to
/// `Operator`.
///
/// `WorkflowSpawn::spawn`'s `scheduled` is always `false` for a resume
/// (issue #542), which on its own stamps every continuation
/// `StartedBy::Operator` via `WorkflowRunContext::new`'s coarse default —
/// regardless of who or what actually started the run that paused. Before
/// the fix this assertion reads `Operator` even though the parked card
/// says `Agent("ceo")`.
#[tokio::test]
async fn a_continuation_run_carries_the_paused_runs_attribution() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_gate_started_by(
        &rt,
        json!({ "request": "quarterly numbers" }),
        &crate::ports::types::StartedBy::Agent("ceo".into()),
    )
    .await;

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("resolves");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0].started_by,
        crate::ports::types::StartedBy::Agent("ceo".into()),
        "the continuation must credit the same agent the paused run did: {:?}",
        started[0].started_by
    );
}

/// The other half of the same gap: a card parked before this field
/// existed carries no `PAYLOAD_STARTED_BY` at all, and must not fail the
/// resume — it degrades to the same `Operator` default the pre-fix
/// behaviour always produced.
#[tokio::test]
async fn a_pre_fix_gate_with_no_started_by_resumes_as_operator() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    // `park_gate` never stamps `PAYLOAD_STARTED_BY` — a stand-in for a card
    // parked before this field existed.
    let id = park_gate(&rt, json!({ "request": "legacy" })).await;

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("resolves");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0].started_by,
        crate::ports::types::StartedBy::Operator,
        "a card with no started_by payload must degrade to the old default, not error: {:?}",
        started[0].started_by
    );
}

/// The gate path resolves a global too.
///
/// The blocked-node twin is the reachable half today — no global declares
/// an approval gate — so this pins the other arm against the day one does,
/// and against a reader restoring the company-only loader on either.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_approved_gate_on_a_global_graph_continues() {
    let global = crate::globals::workflows()
        .first()
        .expect("the baseline ships at least one workflow");
    let home = seed_home();
    assert!(
        !home
            .path()
            .join("workflows")
            .join(format!("{}.toml", global.id))
            .exists(),
        "the fixture must not carry a company copy of {}",
        global.id
    );
    let (rt, runner) = runtime(home.path(), true).await;

    let effect = gate_effect(
        &global.id,
        "gather",
        &json!({ "request": "the week" }),
        "run-that-paused",
        &[],
        &[],
        None,
    );
    let id = rt
        .approvals
        .park(rt.id(), effect.clone())
        .await
        .expect("parks");
    rt.journal()
        .record_parked(
            &id,
            &effect,
            crate::ports::now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .expect("journals");

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("resolves");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(
        started
            .iter()
            .map(|run| run.workflow_id.as_str())
            .collect::<Vec<_>>(),
        vec![global.id.as_str()],
        "approving a gate on a global must start its continuation"
    );
}

/// Issue #438, over the real decide path: the run an approval starts is
/// handed the ledger of what its ancestor already delivered.
///
/// The unit tests above pin the ledger's arithmetic; this one pins that it
/// actually reaches a run — through the gate, the journal, `perform_effect`
/// and the spawn — because that is the hop where a threading mistake would
/// leave every other test green and still mail the report twice.
#[tokio::test]
async fn a_continuation_run_is_told_what_was_already_delivered() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_gate_after(
        &rt,
        json!({ "request": "quarterly numbers" }),
        &[DeliveryReport {
            node: "summary".into(),
            kind: "owner".into(),
            target: Some("ada@acme.test".into()),
            status: DeliveryStatus::Sent,
            detail: "emailed the company's admin".into(),
            reason: crate::ports::DeliveryReason::OwnerEmailed,
        }],
    )
    .await;

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("resolves");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(started.len(), 1);
    assert_eq!(
        delivered_in_input(&started[0].input),
        vec![DeliveredReport {
            node: "summary".into(),
            kind: "owner".into()
        }],
        "the continuation must know the summary already went out: {:?}",
        started[0].input
    );
}

/// Denying starts nothing. The paused run was already settled, so "nothing
/// runs" is the whole outcome — there is no task to cancel.
#[tokio::test]
async fn denying_a_gate_starts_nothing() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_gate(&rt, json!({ "request": "x" })).await;

    rt.resolve_approval(&id, Verdict::Deny, operator())
        .await
        .expect("resolves");

    // Give a spurious spawn the same window the approve test allows.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(runner.started().is_empty(), "a denied gate must not run");
    assert!(rt.pending_approvals().is_empty());
}

/// Approving twice — a double-click, or a retried request — starts one run.
/// At-most-once comes from `execute_effect_once`'s `approval:<id>` key; this
/// pins that the resume arm really is under it.
#[tokio::test]
async fn approving_twice_starts_one_run() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_gate(&rt, json!({ "request": "x" })).await;

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("first resolve");
    let _ = rt.resolve_approval(&id, Verdict::Approve, operator()).await;

    let started = wait_for_runs(&runner, 1).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        runner.started().len(),
        1,
        "a second approve must not start a second run: {started:?}"
    );
}

/// A host restart between the park and the approval must lose nothing. The
/// parked effect is self-contained and the journal replays it, so a fresh
/// runtime over the same home still resumes.
#[tokio::test]
async fn a_gate_parked_before_a_restart_still_resumes_after_it() {
    let home = seed_home();
    {
        let (rt, _) = runtime(home.path(), true).await;
        park_gate(&rt, json!({ "request": "survives" })).await;
    } // the "process" goes away

    // A fresh runtime over the same home, rehydrated from the journal.
    let (rt, runner) = runtime(home.path(), true).await;
    rt.recover().await.expect("replay rehydrates the park");
    let pending = rt.pending_approvals();
    let card = pending
        .iter()
        .find(|a| a.kind == WORKFLOW_APPROVE_KIND)
        .expect("the gate survived the restart");

    rt.resolve_approval(&card.id, Verdict::Approve, operator())
        .await
        .expect("resolves");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].input["approvals"], json!(["gate"]));
    assert_eq!(started[0].input["request"], "survives");
}

/// A gate nobody decided expires to a default deny, and an expired card can
/// never start a run.
///
/// The TTL clock is driven explicitly rather than by waiting: `sweep_expired`
/// takes `now` as a parameter, so a far-future reading exercises the real
/// expiry path on the real gate. What it proves is structural — expiry
/// removes the parked effect, so a later approve resolves to `NotParked` and
/// `settle_approved_effect` (the only route to `perform_effect`) is never
/// reached. This is the "nothing is ever held open" claim: the paused run
/// settled long ago, and its card ages out like any other.
#[tokio::test]
async fn an_undecided_gate_expires_and_starts_nothing() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_gate(&rt, json!({ "request": "x" })).await;

    let expired = rt
        .approval_gate
        .sweep_expired(crate::ports::now_millis() + crate::policy::DEFAULT_TTL_MILLIS + 1);
    assert_eq!(expired, vec![id.clone()], "the gate ages out like any card");

    // Approving after expiry is the already-resolved no-op, not a run.
    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("an expired card resolves as already-resolved, not an error");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        runner.started().is_empty(),
        "an expired gate must never start a continuation run"
    );
}

/// A build with no workflow execution says so at the moment the operator
/// clicks Approve, rather than leaving them watching for a run that will
/// never appear.
#[tokio::test]
async fn approving_on_a_build_with_no_runner_says_so() {
    let home = seed_home();
    let (rt, _) = runtime(home.path(), false).await;
    let id = park_gate(&rt, json!({ "request": "x" })).await;

    let err = rt
        .resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect_err("must surface the gap");
    assert!(
        err.to_string().contains("no workflow execution"),
        "the message must name the gap: {err}"
    );
}

/// A gate whose graph was deleted between parking and approving names that,
/// rather than failing with something the operator cannot act on.
#[tokio::test]
async fn approving_a_gate_whose_graph_is_gone_names_it() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_gate(&rt, json!({ "request": "x" })).await;
    std::fs::remove_file(home.path().join("workflows").join("gated.toml")).expect("delete");

    let err = rt
        .resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect_err("must surface the missing graph");
    assert!(err.to_string().contains("gated"), "{err}");
    assert!(runner.started().is_empty());
}

/// A workflow run whose whole approval batch is refused starts no
/// continuation — `resume_run`'s documented terminal case — and, since PR
/// #1991's review (`3903797619`), must also stop leaving that lineage's
/// checkpoint on disk forever: no other path ever comes back for a wholly
/// denied run's thread id.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_all_denied_batch_prunes_its_checkpoint_lineage() {
    use tinyflows::graph::Checkpointer;

    let home = seed_home();
    let mut rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_seed_dir(home.path().to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let checkpoints = Arc::new(
        crate::workflows::checkpoint_store::WorkflowCheckpointStore::new(
            home.path().join("checkpoints"),
        ),
    );
    checkpoints
        .put(tinyflows::graph::Checkpoint {
            thread_id: "run-that-paused".to_string(),
            checkpoint_id: "c1".to_string(),
            run_id: Some("run-that-paused".to_string()),
            parent_checkpoint_id: None,
            namespace: Vec::new(),
            state: json!({}),
            next_nodes: vec![tinyflows::graph::ids::NodeId::new("gate")],
            completed_tasks: Vec::new(),
            pending_writes: Vec::new(),
            interrupts: Vec::new(),
            pending_activations: None,
            barrier_arrivals: Vec::new(),
            metadata: Value::Null,
        })
        .await
        .expect("seed checkpoint");
    rt.set_workflow_checkpoints(checkpoints.clone());
    let rt = Arc::new(rt);

    let turn = workflow_turn_key("run-that-paused");
    let mut effect = gate_effect(
        "gated",
        "gate",
        &json!({ "request": "x" }),
        "run-that-paused",
        &[],
        &[],
        None,
    );
    if let Value::Object(ref mut payload) = effect.payload {
        payload.insert(PAYLOAD_THREAD_ID.to_string(), json!("run-that-paused"));
    }
    let id = ApprovalId::new("gate-1");
    rt.workflow_gates().arm(&turn, &id, &effect);
    rt.workflow_gates().decide(&turn, &id, Verdict::Deny);

    resume_run(&rt, &turn)
        .await
        .expect("an all-denied batch does not error");

    let remaining = checkpoints
        .get_thread("run-that-paused")
        .await
        .expect("checkpoint read");
    assert!(
        remaining.is_empty(),
        "a wholly refused batch starts no continuation, so its checkpoint lineage must be \
         pruned: {remaining:?}"
    );
}

/// A gate batch's continuation admission can fail terminally too, not
/// just resolve to no run at all. Here the batch has real approvals and a
/// live checkpoint, but the run supervisor is already at its ceiling when
/// `resume_run` tries to spawn the continuation — `spawn_continuation`'s
/// `begin(...)?` refuses with `WorkflowRunLimit`, and the release above
/// already took the batch out of `workflow_gates()` for good, so nothing
/// will ever retry this lineage. Its checkpoint must not outlive that
/// refusal on disk.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_gate_batch_prunes_its_checkpoint_when_admission_hits_the_run_ceiling() {
    use tinyflows::graph::Checkpointer;

    let home = seed_home();
    let mut rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_seed_dir(home.path().to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let runner = Arc::new(RecordingRunner::default());
    rt.set_workflow_runner(runner.clone());
    let checkpoints = Arc::new(
        crate::workflows::checkpoint_store::WorkflowCheckpointStore::new(
            home.path().join("checkpoints"),
        ),
    );
    checkpoints
        .put(tinyflows::graph::Checkpoint {
            thread_id: "run-that-paused".to_string(),
            checkpoint_id: "c1".to_string(),
            run_id: Some("run-that-paused".to_string()),
            parent_checkpoint_id: None,
            namespace: Vec::new(),
            state: json!({}),
            next_nodes: vec![tinyflows::graph::ids::NodeId::new("gate")],
            completed_tasks: Vec::new(),
            pending_writes: Vec::new(),
            interrupts: Vec::new(),
            pending_activations: None,
            barrier_arrivals: Vec::new(),
            metadata: Value::Null,
        })
        .await
        .expect("seed checkpoint");
    rt.set_workflow_checkpoints(checkpoints.clone());
    rt.set_run_supervisor(crate::runtime::RunSupervisor::with_limit(1));
    let rt = Arc::new(rt);

    // Occupy the run supervisor's only slot so `spawn_continuation`'s own
    // `begin(...)?` refuses once this test releases the gate batch below.
    let (_ctx, _guard) = rt
        .run_supervisor()
        .begin("someone-elses-run", false)
        .expect("the ceiling has room for the first run");

    let turn = workflow_turn_key("run-that-paused");
    let mut effect = gate_effect(
        "gated",
        "gate",
        &json!({ "request": "x" }),
        "run-that-paused",
        &[],
        &[],
        None,
    );
    if let Value::Object(ref mut payload) = effect.payload {
        payload.insert(PAYLOAD_THREAD_ID.to_string(), json!("run-that-paused"));
    }
    let id = ApprovalId::new("gate-1");
    rt.workflow_gates().arm(&turn, &id, &effect);
    rt.workflow_gates().decide(&turn, &id, Verdict::Approve);

    let err = resume_run(&rt, &turn)
        .await
        .expect_err("the run supervisor is already at its ceiling");
    assert!(
        matches!(err, OpenCompanyError::WorkflowRunLimit { .. }),
        "expected the ceiling refusal to surface rather than something else: {err}"
    );
    assert!(runner.started().is_empty(), "admission never happened");

    let remaining = checkpoints
        .get_thread("run-that-paused")
        .await
        .expect("checkpoint read");
    assert!(
        remaining.is_empty(),
        "a batch whose continuation could not be admitted is just as terminal for this \
         lineage as an all-denied batch, and its checkpoint must be pruned the same way: \
         {remaining:?}"
    );
}

/// Issue #1991 review (`3904304781`): `spawn_continuation`'s fallback to a
/// trigger re-run — reached here because
/// `graph_unchanged_since_park` just rejected a stale checkpoint — used to
/// leave `checkpoint_thread_id`'s lineage on disk forever: nothing else
/// ever comes back for it once this run re-dispatches on the trigger
/// input instead. Same leak class `an_all_denied_batch_prunes_its_checkpoint_lineage`
/// already covers for the wholly-refused exit; this is the fingerprint-
/// rejection exit.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_gate_refuses_a_stale_checkpoint_after_the_graph_is_edited_and_prunes_it() {
    use tinyflows::graph::Checkpointer;

    let home = seed_home();
    let parked_fingerprint = crate::company::parse_workflow(GATED_TOML)
        .expect("parses")
        .content_fingerprint();
    let mut rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_seed_dir(home.path().to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let runner = Arc::new(RecordingRunner::default());
    rt.set_workflow_runner(runner.clone());
    let checkpoints = Arc::new(
        crate::workflows::checkpoint_store::WorkflowCheckpointStore::new(
            home.path().join("checkpoints"),
        ),
    );
    checkpoints
        .put(tinyflows::graph::Checkpoint {
            thread_id: "run-that-paused".to_string(),
            checkpoint_id: "c1".to_string(),
            run_id: Some("run-that-paused".to_string()),
            parent_checkpoint_id: None,
            namespace: Vec::new(),
            state: json!({}),
            next_nodes: vec![tinyflows::graph::ids::NodeId::new("gate")],
            completed_tasks: Vec::new(),
            pending_writes: Vec::new(),
            interrupts: Vec::new(),
            pending_activations: None,
            barrier_arrivals: Vec::new(),
            metadata: Value::Null,
        })
        .await
        .expect("seed checkpoint");
    rt.set_workflow_checkpoints(checkpoints.clone());
    let rt = Arc::new(rt);

    // The edit: an author renames the gate node while this approval sits
    // pending.
    std::fs::write(
        home.path().join("workflows").join("gated.toml"),
        GATED_TOML.replace("name = \"Gate\"", "name = \"Gate — renamed\""),
    )
    .expect("edit graph on disk");

    let turn = workflow_turn_key("run-that-paused");
    let mut effect = gate_effect(
        "gated",
        "gate",
        &json!({ "request": "x" }),
        "run-that-paused",
        &[],
        &[],
        None,
    );
    if let Value::Object(ref mut payload) = effect.payload {
        payload.insert(PAYLOAD_THREAD_ID.to_string(), json!("run-that-paused"));
        payload.insert(
            PAYLOAD_WORKFLOW_FINGERPRINT.to_string(),
            json!(parked_fingerprint),
        );
    }
    let id = ApprovalId::new("gate-1");
    rt.workflow_gates().arm(&turn, &id, &effect);
    rt.workflow_gates().decide(&turn, &id, Verdict::Approve);

    resume_run(&rt, &turn)
        .await
        .expect("an approved batch still starts a continuation, just not a checkpoint resume");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(started.len(), 1, "exactly one continuation run must start");
    assert_eq!(
        started[0].resume_semantic,
        Some(crate::ports::ResumeSemantic::ReRunFromTrigger),
        "the graph changed since this gate parked, so the stale checkpoint must be refused"
    );

    let remaining = checkpoints
        .get_thread("run-that-paused")
        .await
        .expect("checkpoint read");
    assert!(
        remaining.is_empty(),
        "the refused lineage is unreachable from here on, so it must be pruned rather than \
         leaked: {remaining:?}"
    );
}

/// A blocked agent node's edited-graph twin of the gate-path proof above
/// (`an_edited_graph_no_longer_matches_its_parked_fingerprint`) — this is
/// the finding both `3904397452` (coderabbit) and `3904304754` (codex)
/// raised on `spawn_blocked_node_continuation`'s own `node_restart` check,
/// which used to consult checkpoint availability alone.
///
/// Pre-fix, `node_restart` was `checkpoint_resume_available(..)` with no
/// fingerprint term at all, so this test's checkpoint (seeded and
/// available) made it pick `NodeRestart` regardless of the edit below —
/// this assertion is what fails against that code.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_blocked_node_refuses_a_stale_checkpoint_after_the_graph_is_edited() {
    use tinyflows::graph::Checkpointer;

    let home = seed_home();
    let parked_fingerprint = crate::company::parse_workflow(GATED_TOML)
        .expect("parses")
        .content_fingerprint();
    let mut rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_seed_dir(home.path().to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let runner = Arc::new(RecordingRunner::default());
    rt.set_workflow_runner(runner.clone());
    let checkpoints = Arc::new(
        crate::workflows::checkpoint_store::WorkflowCheckpointStore::new(
            home.path().join("checkpoints"),
        ),
    );
    checkpoints
        .put(tinyflows::graph::Checkpoint {
            thread_id: "blocked-thread".to_string(),
            checkpoint_id: "c1".to_string(),
            run_id: Some("blocked-thread".to_string()),
            parent_checkpoint_id: None,
            namespace: Vec::new(),
            state: json!({}),
            next_nodes: vec![tinyflows::graph::ids::NodeId::new("gate")],
            completed_tasks: Vec::new(),
            pending_writes: Vec::new(),
            interrupts: Vec::new(),
            pending_activations: None,
            barrier_arrivals: Vec::new(),
            metadata: Value::Null,
        })
        .await
        .expect("seed checkpoint");
    rt.set_workflow_checkpoints(checkpoints.clone());
    let rt = Arc::new(rt);

    // The edit: an author renames the gate node while this block sits
    // pending — the same shape `FINGERPRINT_V2` gives the gate-path test,
    // applied to the graph on disk `spawn_blocked_node_continuation`
    // re-loads.
    std::fs::write(
        home.path().join("workflows").join("gated.toml"),
        GATED_TOML.replace("name = \"Gate\"", "name = \"Gate — renamed\""),
    )
    .expect("edit graph on disk");

    spawn_blocked_node_continuation(
        &rt,
        "blocked-turn",
        "gated",
        json!({ "request": "x" }),
        crate::ports::types::StartedBy::Operator,
        Some("blocked-thread".to_string()),
        Some(parked_fingerprint),
    )
    .await
    .expect("a blocked-node continuation still starts, just not as a checkpoint resume");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(started.len(), 1, "exactly one continuation run must start");
    assert_eq!(
        started[0].resume_semantic,
        Some(crate::ports::ResumeSemantic::ReRunFromTrigger),
        "the graph changed since this node parked, so the stale checkpoint must be refused \
         in favour of a trigger re-run, not resumed into the edited graph"
    );

    let remaining = checkpoints
        .get_thread("blocked-thread")
        .await
        .expect("checkpoint read");
    assert!(
        remaining.is_empty(),
        "the refused lineage is unreachable from here on, so it must be pruned rather than \
         leaked: {remaining:?}"
    );
}

/// A blocked node on a global graph resumes.
///
/// A global lives only in the static baseline, so a resume that reloads
/// the graph through the company's own two sources cannot find it: the
/// answer is banked, the continuation never starts, and the run ends
/// stranded carrying a message that says the graph no longer exists.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_blocked_node_on_a_global_graph_resumes() {
    let global = crate::globals::workflows()
        .first()
        .expect("the baseline ships at least one workflow");

    let home = seed_home();
    assert!(
        !home
            .path()
            .join("workflows")
            .join(format!("{}.toml", global.id))
            .exists(),
        "the fixture must not carry a company copy of {}, or the union loader would find \
         it and the global layer would go untested",
        global.id
    );

    let mut rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_seed_dir(home.path().to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let runner = Arc::new(RecordingRunner::default());
    rt.set_workflow_runner(runner.clone());
    let rt = Arc::new(rt);

    spawn_blocked_node_continuation(
        &rt,
        "blocked-turn",
        &global.id,
        json!({ "request": "x" }),
        crate::ports::types::StartedBy::Operator,
        None,
        None,
    )
    .await
    .expect("a blocked node on a global graph continues");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(
        started
            .iter()
            .map(|run| run.workflow_id.as_str())
            .collect::<Vec<_>>(),
        vec![global.id.as_str()],
        "answering the question must start the continuation, not strand the run"
    );
}

/// The positive control beside the test above: an unedited graph must
/// still resume its checkpoint — the fingerprint check must not become a
/// blanket refusal.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_blocked_node_resumes_its_checkpoint_when_the_graph_is_unchanged() {
    use tinyflows::graph::Checkpointer;

    let home = seed_home();
    let parked_fingerprint = crate::company::parse_workflow(GATED_TOML)
        .expect("parses")
        .content_fingerprint();
    let mut rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_seed_dir(home.path().to_path_buf())
        .build()
        .await
        .expect("runtime builds");
    let runner = Arc::new(RecordingRunner::default());
    rt.set_workflow_runner(runner.clone());
    let checkpoints = Arc::new(
        crate::workflows::checkpoint_store::WorkflowCheckpointStore::new(
            home.path().join("checkpoints"),
        ),
    );
    checkpoints
        .put(tinyflows::graph::Checkpoint {
            thread_id: "blocked-thread".to_string(),
            checkpoint_id: "c1".to_string(),
            run_id: Some("blocked-thread".to_string()),
            parent_checkpoint_id: None,
            namespace: Vec::new(),
            state: json!({}),
            next_nodes: vec![tinyflows::graph::ids::NodeId::new("gate")],
            completed_tasks: Vec::new(),
            pending_writes: Vec::new(),
            interrupts: Vec::new(),
            pending_activations: None,
            barrier_arrivals: Vec::new(),
            metadata: Value::Null,
        })
        .await
        .expect("seed checkpoint");
    rt.set_workflow_checkpoints(checkpoints.clone());
    let rt = Arc::new(rt);

    spawn_blocked_node_continuation(
        &rt,
        "blocked-turn",
        "gated",
        json!({ "request": "x" }),
        crate::ports::types::StartedBy::Operator,
        Some("blocked-thread".to_string()),
        Some(parked_fingerprint),
    )
    .await
    .expect("a blocked-node continuation starts");

    let started = wait_for_runs(&runner, 1).await;
    assert_eq!(started.len(), 1, "exactly one continuation run must start");
    assert_eq!(
        started[0].resume_semantic,
        Some(crate::ports::ResumeSemantic::NodeRestart),
        "an unedited graph must still resume its checkpoint — the fingerprint check must \
         not refuse a lineage that is still valid"
    );

    let remaining = checkpoints
        .get_thread("blocked-thread")
        .await
        .expect("checkpoint read");
    assert!(
        !remaining.is_empty(),
        "a resumed lineage must not be pruned out from under the run using it"
    );
}
