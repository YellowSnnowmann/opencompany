//! Compile and drive a company workflow on the tinyflows engine.
//!
//! [`run_workflow`] is the free driver: [`translate`](super::translate) the
//! [`WorkflowFile`] into a tinyflows graph, [`compile`](tinyflows::compiler)
//! it, build the [`Capabilities`](super::caps) bundle (agent nodes → harness
//! pool), and [`run`](tinyflows::engine) it to completion. [`HarnessWorkflowRunner`]
//! is the [`WorkflowRunner`] port implementation the runtime holds: it owns the
//! shared pool/deps/record, ensures the roster is resident, then delegates.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::Result;
use crate::company::WorkflowFile;
use crate::error::OpenCompanyError;
use crate::harness::{HarnessDeps, HarnessPool};
use crate::ports::types::{CompanyEvent, CompanyId, CompanyRecord, WorkflowNodeStatus};
use crate::ports::{WorkflowRun, WorkflowRunContext, WorkflowRunner};

/// How deeply a workflow may re-enter itself before the run is refused
/// (issue #151 part a).
///
/// One level of nesting is legitimate and useful — a `sub_workflow` node, or a
/// workflow whose agent node asks the orchestrator to run a second, different
/// graph. Beyond that a chain is almost certainly a cycle rather than a plan,
/// and the cost of being wrong is asymmetric: refusing a deep run returns a
/// readable tool error, while allowing it aborts the host.
pub(crate) const MAX_WORKFLOW_DEPTH: usize = 4;

/// How long a settling run waits for its node-progress events to finish
/// reaching the journal (issue #371).
///
/// The drain is normally instant — the channel is already closed and only
/// in-flight appends remain — so this bound never fires in practice. It exists
/// to keep a progress-reporting stall from ever becoming a *run* stall.
const PROGRESS_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a stopped run is given to wind down cleanly at a node boundary
/// before the runner falls back to the hard abort (issue #398).
///
/// When an operator stops a run, the runner flips the engine's
/// [`CancellationToken`](tinyflows::engine::CancellationToken): the engine checks
/// it before each node and, once a node in flight finishes, winds the run down
/// and returns a real (partial) [`RunOutcome`] with `cancelled` set. That is the
/// clean path — the collected node trail is kept and nothing is dropped
/// mid-await.
///
/// But a node wedged mid-await on a stalled external call never reaches the next
/// boundary, so the token alone could hang the stop forever (see the
/// `StallingProvider` test). This bound caps the wait: if the run has not wound
/// down within it, the runner drops the engine future — the pre-#398 hard abort
/// — so a wedged run stays killable. Generous enough that a healthy node
/// crossing a boundary always makes it, short enough that a stuck stop is not an
/// eternity.
const CANCEL_HARD_ABORT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

tokio::task_local! {
    /// How many workflow runs are already on this call chain.
    ///
    /// A task-local, not a counter on the runner, and that distinction is the
    /// point: a workflow run, the agent turns inside it, and any tool those
    /// turns call all execute inline on **one** tokio task (the only `spawn` on
    /// the path is the progress-event collector, which runs nothing re-entrant).
    /// So this counts exactly one causal chain. A shared counter would instead
    /// count *concurrent* runs and refuse two operators running unrelated
    /// workflows at the same time.
    static WORKFLOW_DEPTH: usize;
}

/// The current re-entry depth, `0` outside any workflow run.
fn current_workflow_depth() -> usize {
    WORKFLOW_DEPTH.try_with(|d| *d).unwrap_or(0)
}

/// Runs `workflow` for the company described by `record` on the tinyflows engine
/// with the trigger `input`, returning the final run state and any nodes left
/// pending approval.
///
/// `record` (not a bare [`CompanyId`]) is threaded through so the outside-world
/// capabilities — the `tool_call` toolbelt and the `http_request` SSRF guard —
/// can read the company's `[policy].mode`, `[tools].allow` grants, and
/// `[tools].web_allowed_domains` (see [`super::caps::build_capabilities`]).
///
/// The caller is responsible for having the company's roster resident in `pool`
/// (agent nodes address it by teammate id) — [`HarnessWorkflowRunner::run`] does
/// this via [`HarnessPool::ensure`] before delegating here.
pub async fn run_workflow(
    pool: Arc<HarnessPool>,
    deps: HarnessDeps,
    record: &CompanyRecord,
    workflow: &WorkflowFile,
    input: Value,
    ctx: &WorkflowRunContext,
) -> Result<WorkflowRun> {
    // A single pool is the single-harness case: the router is just the default
    // lane over that pool. Kept as its own entrypoint so the many single-pool
    // tests (and any single-harness caller) need not hand-assemble a router.
    let turn: Arc<dyn crate::runtime::delegation::RunTurn> = Arc::new(
        crate::harness::built_in::run_turn::HarnessRunTurn::new(pool, Arc::new(deps.clone())),
    );
    run_workflow_lane_aware(turn, deps, record, workflow, input, ctx).await
}

/// [`run_workflow`] over an already-assembled, lane-aware router.
///
/// The production [`HarnessWorkflowRunner`] takes this path so a workflow
/// `agent` node addressing a named-harness agent routes to that harness's
/// engine instead of the default pool.
pub async fn run_workflow_lane_aware(
    turn: Arc<dyn crate::runtime::delegation::RunTurn>,
    deps: HarnessDeps,
    record: &CompanyRecord,
    workflow: &WorkflowFile,
    input: Value,
    ctx: &WorkflowRunContext,
) -> Result<WorkflowRun> {
    run_workflow_lane_aware_checkpointed(turn, deps, record, workflow, input, ctx, None).await
}

async fn run_workflow_lane_aware_checkpointed(
    turn: Arc<dyn crate::runtime::delegation::RunTurn>,
    deps: HarnessDeps,
    record: &CompanyRecord,
    workflow: &WorkflowFile,
    input: Value,
    ctx: &WorkflowRunContext,
    checkpoint_store: Option<Arc<super::checkpoint_store::WorkflowCheckpointStore>>,
) -> Result<WorkflowRun> {
    // Issue #151 part a: refuse an unbounded re-entry before it takes the host
    // down. `run_workflow` is an orchestrator tool, and a workflow `agent` node
    // may address the orchestrator — so a graph whose agent node runs a
    // workflow that reaches the orchestrator again recurses with no bound. Each
    // level is a whole agent turn plus an engine run, so the process dies on a
    // stack overflow rather than returning an error, taking every other tenant
    // on the host with it. `MAX_DELEGATIONS_PER_TURN` caps fan-out *within* one
    // turn and does nothing about depth.
    let depth = current_workflow_depth();
    if depth >= MAX_WORKFLOW_DEPTH {
        tracing::warn!(
            company = %record.id,
            workflow = %workflow.id,
            depth,
            "workflow: refusing a run past the re-entry limit"
        );
        return Err(OpenCompanyError::Harness(format!(
            "workflow `{}` was not run: it is already {depth} workflow runs deep, at the \
             re-entry limit of {}. A workflow whose agent node runs another workflow that \
             reaches back here will loop forever — break the cycle, or run the inner \
             workflow on its own.",
            workflow.id, MAX_WORKFLOW_DEPTH
        )));
    }

    WORKFLOW_DEPTH
        .scope(
            depth + 1,
            run_workflow_inner(turn, deps, record, workflow, input, ctx, checkpoint_store),
        )
        .await
}

/// One per-node progress frame, as the engine's observer callbacks hand it over
/// to the async collector (issue #371 for the finish, issue #382 for the start).
///
/// A node produces a [`Started`](Self::Started) frame just before its first
/// attempt and a [`Finished`](Self::Finished) frame as it settles, both on the
/// **same** channel, so the collector sees them in that order and a node's
/// started event always journals ahead of its finished one.
///
/// Deliberately no whole `ExecutionStep`. A `Started` frame carries the node id
/// alone; the node has not run, so there is no status, duration, or output to
/// carry either. What is not carried cannot leak.
///
/// A `Finished` frame carries the node's `output` items (issue #1008) **as well
/// as** its status and duration — but that payload is used for exactly one thing
/// and is walled off from the journal: the collector accumulates it into a
/// side map that feeds ONLY the durable, console-facing run-output store on the
/// **failure/blocked arms**, where the engine returns no `outcome.output` to
/// persist from. The journal still receives only `{node_id, status,
/// elapsed_ms}` (see the `Finished` arm of the collector, which builds its
/// `WorkflowNodeFinished` from those three scalars and never touches `output`) —
/// the same stance the live turn frames take on tool args. Before #1008 the
/// failure paths threw this output away, so a failed/blocked run's inspector
/// wrongly claimed the run predated output capture.
enum NodeProgress {
    /// A node began executing (issue #382) — the opening bracket. Id only.
    Started { node_id: String },
    /// A node finished (issue #371) — its status, wall-clock duration, and the
    /// items it emitted (issue #1008; [`Value::Null`] on an error step). The
    /// output is consumed only by the run-output persist on the failure arms;
    /// it never reaches the journal.
    Finished {
        node_id: String,
        status: WorkflowNodeStatus,
        elapsed_ms: u64,
        output: Value,
        /// What the harness did inside this node, in order — empty for every
        /// non-agent node and for a turn with no steps to fold.
        ///
        /// Rides this channel exactly as `output` does, and for the same reason:
        /// it is per-node content the run response and the output snapshot want,
        /// not a scalar the journal carries. Bounded per entry by the engine's
        /// `TranscriptEntry::bounded` before it ever reaches here.
        transcript: Vec<tinyflows::transcript::TranscriptEntry>,
        /// Issue #1014: the config paths of this node's null-resolved
        /// `=`-expressions — the engine's own broken-wiring list, lifted off
        /// `ExecutionStep.diagnostics`. Paths only (each
        /// `NullResolution.location`), never a resolved value: a null resolution
        /// has no value, and the location is config the author wrote, so nothing
        /// a node produced rides this channel. Carried onto the node row and the
        /// `WorkflowNodeFinished` event; like the scalars beside it, never the
        /// `output`.
        diagnostics: Vec<String>,
    },
}

/// A [`RunObserver`](tinyflows::observability::RunObserver) that forwards each
/// node start and finish onto an unbounded channel.
///
/// The channel is the whole reason this type exists. Observer callbacks are
/// **synchronous** (the engine invokes them inline, across threads) while
/// [`EventLog::append`] is async, so the callback cannot journal directly. It
/// also must not block: a node handler stalled on a disk write would make
/// observability change the run's timing, which is exactly what an observer is
/// not allowed to do. Unbounded is safe at this volume — two messages per
/// non-trigger node, ~16 for a six-node graph — and it means a slow journal can
/// never apply backpressure to the engine.
struct ProgressObserver {
    tx: tokio::sync::mpsc::UnboundedSender<NodeProgress>,
}

impl tinyflows::observability::RunObserver for ProgressObserver {
    fn on_step_start(&self, node_id: &str) {
        // Issue #382: the node's opening bracket, sent on the SAME channel and
        // therefore BEFORE its finish — the collector processes the channel in
        // order, so a node's started event is always journaled ahead of its
        // finished one. A closed receiver (the run is settling, or `deps.events`
        // was never wired) drops the frame, exactly as the finish arm does:
        // progress reporting must never disturb the run.
        let _ = self.tx.send(NodeProgress::Started {
            node_id: node_id.to_string(),
        });
    }

    fn on_step_finish(&self, step: &tinyflows::observability::ExecutionStep) {
        // A closed receiver means the collector already stopped (the run is
        // settling, or `deps.events` was never wired). Dropping the frame is
        // correct: progress reporting must never disturb the run.
        let _ = self.tx.send(NodeProgress::Finished {
            node_id: step.node_id.clone(),
            status: match step.status {
                tinyflows::observability::StepStatus::Success => WorkflowNodeStatus::Ok,
                tinyflows::observability::StepStatus::Error => WorkflowNodeStatus::Error,
            },
            // `u128` millis is the engine's type; a node running longer than
            // 584 million years is not the failure mode worth a `Result`.
            elapsed_ms: u64::try_from(step.duration_ms).unwrap_or(u64::MAX),
            // Issue #1014: the config path of every `=`-expression this node's
            // execution resolved to `null` — the engine's own broken-wiring
            // list. Only `NullResolution.location` (a dotted config path the
            // author wrote, e.g. `args.to`) is taken; the `expression` and any
            // resolved value are dropped here, so nothing but the wiring's
            // address leaves the observer. Empty on error steps and for nodes
            // with no expression config.
            diagnostics: step
                .diagnostics
                .iter()
                .map(|d| d.location.clone())
                .collect(),
            // Issue #1008: the node's emitted items, carried so the collector can
            // accumulate a partial per-node output map for the failure/blocked
            // arms (which have no `outcome.output` to persist from). A success
            // step's `output` is the items array; an error step's is
            // `Value::Null`. This clone rides the same channel as the scalars
            // and never touches the journal.
            output: step.output.clone(),
            // The engine copied this off the `AgentRunOutcome` the harness
            // returned. Cloned like `output` beside it; the observer must stay
            // allocation-cheap but must not borrow past the callback.
            transcript: step.transcript.clone(),
        });
    }
}

/// The run itself, always executed inside a [`WORKFLOW_DEPTH`] scope so a
/// nested run sees this one on the chain.
async fn run_workflow_inner(
    turn: Arc<dyn crate::runtime::delegation::RunTurn>,
    deps: HarnessDeps,
    record: &CompanyRecord,
    workflow: &WorkflowFile,
    input: Value,
    ctx: &WorkflowRunContext,
    checkpoint_store: Option<Arc<super::checkpoint_store::WorkflowCheckpointStore>>,
) -> Result<WorkflowRun> {
    let mut graph = super::translate::translate(workflow);
    // Policy-generated workflow HITL is disabled. Preserve an author's own
    // `requires_approval = true`, but add no gates merely because a tool call
    // matches company policy. Agent-driven approvals enter through explicit
    // approval-producing tools instead.
    let gated = super::gate::policy_hitl_disabled(&mut graph);
    // Issue #846: a node whose call already left the building in an earlier run
    // of this lineage replays its recorded result instead of calling again.
    // Driven entirely off the trigger input's ledger, so a first run rewrites
    // nothing and the graph stays byte-identical.
    //
    // **After the gate pass, and the order is load-bearing.** The gate pass
    // classifies a node by its slug; running it second would have it classify
    // the host's replay sentinel — an inert slug no policy has an opinion about
    // — and either gate a node that does nothing or fail to gate one that does.
    // Nothing is lost by this order: a replayed node was necessarily executed by
    // an earlier run, so its id is already in the input's `approvals` array and
    // the gate it still carries falls straight through.
    let replayed = super::replay::replay_performed(&mut graph, &input);
    if !replayed.is_empty() {
        tracing::info!(
            company = %record.id,
            workflow = %workflow.id,
            run_id = %ctx.run_id,
            nodes = ?replayed,
            "workflow: this continuation replays calls an earlier run in its lineage already \
             made, rather than repeating them"
        );
    }
    let compiled = tinyflows::compiler::compile(&graph).map_err(map_engine_error)?;
    // Issue #371: the caller's run id, not a freshly minted one. Correlating the
    // run's progress events with the `WorkflowRunFinished` the caller journals
    // requires both halves to share an id, and only the caller can supply one
    // that survives the error arm (where this function returns nothing at all).
    // A side win: the run's `_workflow/` workspace directory, which is named
    // from this id, becomes correlatable with the journal for the first time.
    let run_id = ctx.run_id.clone();
    let checkpoint_thread_id = ctx
        .checkpoint_resume
        .as_ref()
        .map_or_else(|| run_id.clone(), |resume| resume.thread_id.clone());
    // Issue #154: the operator's run request rides the trigger payload. Pull it
    // out before the input is handed to the engine so every agent node's turn
    // message carries the topic — a node's authored `prompt` is the same on
    // every run and cannot say what was asked this time.
    let run_request = super::caps::run_request_text(&input);
    // Issue #395: the trigger payload, kept before the engine consumes it. A
    // paused gate's approval card has to carry the input the run was started
    // with, because resuming means re-running the graph with that same input
    // plus the approval — see `crate::runtime::workflow_resume`.
    let trigger_input = input.clone();
    // Issue #170: the delivery ports are read off `deps` BEFORE it moves into
    // the capability bundle. Delivery is host-side and post-engine, so it is not
    // a capability — the engine never learns a report has a destination.
    let delivery = deps.delivery.clone();
    // Issue #371: likewise read the journal off `deps` before it moves. `None`
    // (the default build, and every existing test) degrades the whole progress
    // path to a no-op — no started event, a `NoopObserver`, no collector.
    let events = deps.events.clone();
    // Issue #596: the durable, console-facing run-output store, read off `deps`
    // before it moves into the capability bundle — like `events`/`delivery`
    // above. `None` (default build, unwired tests) degrades the persist to a
    // no-op. Kept beside `events` so it rides the same "read the host-side ports
    // out before the engine takes deps" pattern.
    let run_output_store = deps.run_output_store.clone();
    // Issue #542: the mode. A dry run walks the same real graph over stubbed
    // effectful capabilities (see `caps::dry_run`) and, host-side, skips every
    // durable effect around the engine — the started/finished/node journal
    // writes, the delivery dispatch, and gate parking. Read once here.
    let dry_run = ctx.dry_run;
    // Issue #661 (L2): a failed per-run-workspace mkdir aborts the run here,
    // BEFORE the WorkflowRunStarted journal append below — so a workspace the
    // effects cannot be rooted at leaves no orphaned started row, and the caller
    // sees the real cause instead of a later, further-removed effect failure.
    // Issue #638: where an agent node leaves an operator-facing notice. Owned
    // here, by the run, because that is the only scope that outlives the nodes
    // and reaches `WorkflowRun`.
    let notices = super::caps::RunNotices::default();
    // Issue #661 (M5): where the run's board writes are recorded. Owned here for
    // the same reason `notices` is — the nodes come and go, and this is the only
    // scope that outlives them and reaches `WorkflowRun`. Critically it is owned
    // *outside* the capability bundle, so a hard abort that drops the engine future
    // (and with it the bundle and its board claim) still leaves every row already
    // collected readable here: a card is real once written, so it must stay listed.
    // Issue #976: a graph whose only node is its trigger has nothing to execute.
    // The engine runs it happily — there is no stage to fail — so it settles as
    // an ordinary finished run, and a run row that says nothing is its own small
    // lie: `QA Test Pipeline` on staging banked six of them. Said here, through
    // the channel #638 built for exactly this shape of fact.
    //
    // A notice rather than an error, for the reason `notices` exists at all: an
    // empty graph is not a failure. Nothing broke, nothing was attempted, and
    // marking it failed would put a half-authored stub into the failure count
    // next to runs that genuinely went wrong. Same call `NoDestinationConfigured`
    // makes one level down (#925) — state the reason instead of leaving it to be
    // inferred from an absence.
    if !workflow.has_runnable_node() {
        notices.push(crate::company::STAGELESS_WORKFLOW_NOTICE.to_string());
        tracing::warn!(
            company = %record.id,
            workflow = %workflow.id,
            "workflow run: the graph has no runnable node, so this run could not do anything"
        );
    }
    let board = super::caps::RunBoard::default();
    // Issue #881 / #880: the two sideways channels an agent node reports through
    // — that it blocked, and what it parked. Owned out here for exactly the
    // reason `board` is: a blocked node halts the run, which drops the engine
    // future and the capability bundle with it, and both of these facts have to
    // survive that. An approval card is durable the moment it is written, so a
    // run that ended badly must still be able to say it opened one.
    let blocks = super::caps::RunBlocks::default();
    // Issue #1865: the sideways channel an agent node's turn reports through
    // when it truncates at the `max_tool_iterations` cap — owned out here like
    // `blocks`, for the same reason: the node's attempt row already settles
    // `Failed` for this, and the run-level row must be told to agree before the
    // capability bundle (and the fact it is carrying) drops.
    let capped = super::caps::RunCappedNodes::default();
    let halted = super::caps::RunHaltedNodes::default();
    let approvals = super::caps::RunApprovals::default();
    // Card-less files written by agent nodes. Kept outside the capability
    // bundle so a failed/blocked engine future cannot drop the capture before
    // the durable run-output snapshot is assembled.
    let run_artifacts = super::caps::RunArtifacts::default();
    // Owned out here like `blocks` and `approvals`: the journal write that needs
    // it happens in the collector task, which outlives the capability bundle the
    // engine future drops.
    let attempts = super::caps::RunAttempts::default();
    // Read before `deps` moves into the builder below. A dry run records
    // nothing: it makes no effects, so an attempt row would be a receipt for
    // work that never happened.
    let attempt_runs = (!dry_run).then(|| deps.workflow_runs.clone()).flatten();
    let attempt_deep = (!dry_run).then(|| deps.deep_trace.clone()).flatten();
    // Issue #617: one per-run record of every child the resolver gates. The
    // resolver (invoked by the engine mid-run) writes it; the parking path
    // (after the engine returns) reads it to name a child pause. Created out
    // here and owned past the engine call — the engine future and the
    // capability bundle drop before parking runs.
    let child_gates = Arc::new(super::caps::resolver::ChildGateRegistry::default());
    // Issue #1991 review (`3904397452`/`3904304754`): the graph's fingerprint
    // as loaded for this run attempt, stamped onto a blocked node's stash the
    // same way `park_pending_gates` stamps it onto a gate's parked effect, so
    // `spawn_blocked_node_continuation` can refuse a checkpoint resume into a
    // graph an editor changed while the block sat pending.
    let workflow_fingerprint = workflow.content_fingerprint();
    let capabilities = super::caps::build_capabilities(
        turn,
        deps,
        record,
        super::caps::RunContext {
            workflow_id: &workflow.id,
            run_id: &run_id,
            checkpoint_thread_id: &checkpoint_thread_id,
            workflow_fingerprint: &workflow_fingerprint,
            run_request,
            trigger_input: &trigger_input,
            started_by: ctx.started_by.clone(),
            dry_run,
            notices: notices.clone(),
            board: board.clone(),
            blocks: blocks.clone(),
            capped: capped.clone(),
            halted: halted.clone(),
            approvals: approvals.clone(),
            artifacts: run_artifacts.clone(),
            // A dry run records nothing: it makes no effects, so an attempt row
            // for it would be a receipt for work that never happened.
            runs: attempt_runs,
            deep: attempt_deep,
            attempts: attempts.clone(),
            child_gates: child_gates.clone(),
        },
    )
    .await?;

    // The opening bracket, appended BEFORE the engine call so a run killed
    // mid-flight leaves a start with no finish — which is precisely the shape
    // the boot sweep looks for. Best-effort, like every other journal write on
    // this path: losing the record is worth a log line, never a failed run.
    //
    // Issue #542: skipped entirely for a dry run. A test run writes NOTHING
    // durable — no started row, so no boot sweep ever adopts it and no `running`
    // row ever appears in the history; the settled response body is its whole
    // record.
    if let Some(events) = events.as_ref().filter(|_| !dry_run) {
        let started = CompanyEvent::WorkflowRunStarted {
            workflow_id: workflow.id.clone(),
            run_id: run_id.clone(),
            scheduled: ctx.scheduled,
            // Issue #1862 prerequisite: who triggered this run, stamped as a
            // fact at start rather than guessed later at failure time.
            started_by: Some(ctx.started_by.clone()),
            resume_semantic: ctx.resume_semantic,
        };
        if let Err(err) = events.append(&record.id, started).await {
            tracing::warn!(
                company = %record.id,
                workflow = %workflow.id,
                %run_id,
                %err,
                "workflow: run-started progress event could not be journaled; the run is unaffected"
            );
        }
    }

    // Issue #371 + #542: ALWAYS drive with a progress observer, so the per-node
    // timeline is collected for **every** run — it feeds `WorkflowRun.nodes` on
    // all paths, and for a dry run (which journals nothing) it is the only trail
    // the run leaves. The collector accumulates one `WorkflowRunNodeRow` per
    // node and, *additionally*, appends a `WorkflowNodeFinished` to the journal
    // only when there is a journal AND this is not a dry run.
    //
    // The journal is passed to the collector only in that case; `None` there
    // means "collect the rows, write nothing" — the shape the default build and
    // every dry run take.
    //
    // Issue #1008: the collector *additionally* accumulates a per-node output
    // map (`{ "<id>": { "items": [ … ] } }`) from each `Finished` frame's
    // output. This map exists solely to feed the durable run-output persist on
    // the failure/blocked arms, where the engine returns no `outcome.output`. It
    // is returned beside the rows and never journaled — the journal invariant
    // (no node output) is preserved because the `WorkflowNodeFinished` event is
    // still built from the three scalars alone.
    let journal_nodes = events.clone().filter(|_| !dry_run);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<NodeProgress>();
    let collector = tokio::spawn({
        let company = record.id.clone();
        let workflow_id = workflow.id.clone();
        let run_id = run_id.clone();
        let collector_attempts = attempts.clone();
        // Issue #1865 (CodeRabbit review on #1905): read by the journal write
        // below so the durable event records the same status the settle-time
        // `reclassify_capped_nodes` will put on the in-memory row.
        let collector_capped = capped.clone();
        let collector_halted = halted.clone();
        async move {
            let mut rows: Vec<crate::ports::WorkflowRunNodeRow> = Vec::new();
            // Issue #1008: node_id -> `{ "items": [ … ] }`, canonical-shaped so
            // the console's `parseNodeMessages` reads it exactly like a clean
            // run's `outcome.output["nodes"]`.
            let mut partial_nodes = serde_json::Map::new();
            let attempts = collector_attempts;
            // node_id -> the ordered transcript entries that node produced.
            let mut node_transcripts = serde_json::Map::new();
            while let Some(progress) = rx.recv().await {
                match progress {
                    // Issue #382: the node's opening bracket. Journaled only when
                    // there is a journal AND this is not a dry run — the same
                    // gate the finish arm applies. It contributes NO response
                    // row: `WorkflowRun.nodes` stays finished-only, so a started
                    // node with no finish never masquerades as a completed one.
                    NodeProgress::Started { node_id } => {
                        if let Some(events) = journal_nodes.as_ref() {
                            let event = CompanyEvent::WorkflowNodeStarted {
                                workflow_id: workflow_id.clone(),
                                run_id: run_id.clone(),
                                node_id,
                            };
                            if let Err(err) = events.append(&company, event).await {
                                tracing::warn!(
                                    %company,
                                    workflow = %workflow_id,
                                    %run_id,
                                    %err,
                                    "workflow: node-started progress event could not be journaled; \
                                     the run is unaffected"
                                );
                            }
                        }
                    }
                    NodeProgress::Finished {
                        node_id,
                        status,
                        elapsed_ms,
                        output,
                        transcript,
                        diagnostics,
                    } => {
                        // Issue #1865: the engine reports a turn that
                        // truncated at `max_tool_iterations` (or paused for
                        // budget) as `Ok` — in its own terms it is, the node
                        // returned — and the host relabels it at settle via
                        // `reclassify_capped_nodes`. That relabel only ever
                        // reached the in-memory `WorkflowRun.nodes`, so the
                        // journal kept saying `Ok` and a run re-read from
                        // history scored `ok` where the synchronous response
                        // said `degraded`: one run, two verdicts, depending on
                        // which surface you asked.
                        //
                        // Written correctly here rather than carried on
                        // `WorkflowRunFinished` and relabelled on the read (the
                        // shape `relabel_blocked` uses): the fact is already
                        // known at this point — the cap is pushed inside the
                        // turn, strictly before the engine emits this node's
                        // `Finished` — so the durable event can simply be right
                        // the first time instead of being corrected by every
                        // reader forever.
                        let journaled_status = if collector_halted.contains(&node_id) {
                            crate::ports::WorkflowNodeStatus::Declined
                        } else if collector_capped.contains(&node_id) {
                            crate::ports::WorkflowNodeStatus::Error
                        } else {
                            status
                        };
                        if let Some(events) = journal_nodes.as_ref() {
                            let event = CompanyEvent::WorkflowNodeFinished {
                                workflow_id: workflow_id.clone(),
                                run_id: run_id.clone(),
                                node_id: node_id.clone(),
                                status: journaled_status,
                                elapsed_ms,
                                // Issue #1014: the broken-wiring paths ride the
                                // durable event too, so a re-read run (folded
                                // from the journal) shows the same diagnostics
                                // the synchronous response did.
                                diagnostics: diagnostics.clone(),
                                // The join, on the durable event: a console
                                // folding this run from the journal reaches the
                                // node's step trace without a second lookup.
                                agent_run_id: attempts.get(&node_id),
                            };
                            if let Err(err) = events.append(&company, event).await {
                                tracing::warn!(
                                    %company,
                                    workflow = %workflow_id,
                                    %run_id,
                                    %err,
                                    "workflow: node progress event could not be journaled; the run \
                                     is unaffected"
                                );
                            }
                        }
                        // Issue #1008: accumulate this node's output into the
                        // partial map under the canonical `{ "items": [ … ] }`
                        // shape. A success step's `output` is the items array; an
                        // error (or any non-array) step contributes an empty
                        // `items`, so the failing node still appears as "produced
                        // none" rather than vanishing. Keyed before `node_id`
                        // moves into the row below.
                        let items = match output {
                            Value::Array(_) => output,
                            _ => Value::Array(Vec::new()),
                        };
                        partial_nodes
                            .insert(node_id.clone(), serde_json::json!({ "items": items }));
                        // Kept beside the output map rather than inside it: a
                        // clean settle persists the ENGINE's `outcome.output`,
                        // not this map, so the transcripts have to survive
                        // separately and be merged in at each persist site.
                        // Folding them in here would land them on the failure
                        // arms only — which is the whole bug this avoids.
                        if !transcript.is_empty() {
                            node_transcripts.insert(
                                node_id.clone(),
                                serde_json::to_value(&transcript).unwrap_or(Value::Null),
                            );
                        }
                        // Collected for the response on every path — status is
                        // `Copy`, `node_id` moves in after its clone (if any)
                        // went to the event.
                        rows.push(crate::ports::WorkflowRunNodeRow {
                            node_id,
                            status,
                            elapsed_ms,
                            // Issue #1014: the node's null-resolved config paths,
                            // carried on the run response's per-node timeline.
                            // `diagnostics` moves in after its clone above went
                            // to the event.
                            diagnostics,
                        });
                    }
                }
            }
            (rows, partial_nodes, node_transcripts)
        }
    });

    let observer: Arc<dyn tinyflows::observability::RunObserver> =
        Arc::new(ProgressObserver { tx });
    // Issue #383/#398: the engine call is raced against the run's stop signal.
    //
    // # The engine's own token, with a bounded hard-abort fallback
    //
    // tinyflows exposes `run_cancellable_with_observer`, which takes a
    // `CancellationToken` **and** an observer — so a cancellable run keeps the
    // per-node progress trail (#371/#382) instead of trading it away, which the
    // old host-side "drop the future" race had to. The engine checks the token
    // before each node, so cancelling stops the run at the next **node boundary**
    // rather than mid-await: a node already executing runs to completion, its
    // finish is journaled, and the run winds down carrying a real (partial)
    // `RunOutcome` with `cancelled` set. That is the clean path.
    //
    // # Why the fallback survives (decision locked)
    //
    // A node wedged mid-await on a stalled external call never reaches the next
    // boundary, so the token alone could hang the stop forever (the
    // `StallingProvider` test is exactly this). So the stop path flips the token,
    // gives the run a bounded `CANCEL_HARD_ABORT_GRACE` to wind down cleanly, and
    // ONLY if it does not settle in that window drops the engine future — the
    // pre-#398 hard abort. Dropping stops the run mid-await: a node part way
    // through an external side effect stays part way through it, the same class
    // of outcome as the host being killed, which the boot sweep already settles.
    // Keeping this fallback is what guarantees a wedged run stays killable.
    //
    // `Box::pin` because the losing branch must be droppable, which a
    // `tokio::pin!`ed local is not.
    let token = tinyflows::engine::CancellationToken::new();
    let checkpointed = checkpoint_store.is_some() && !dry_run;
    // Only an actual resume lacks a cancellation path into the engine —
    // `resume_with_checkpointer_journaled_observed` takes no token. A
    // checkpointed *initial* run still goes through the grace-then-drop wait
    // below: the token itself is a no-op against the checkpointer engine
    // call, but the wait still lets a node that is genuinely close to
    // finishing (not wedged) settle before the future is dropped.
    let resuming = checkpointed && ctx.checkpoint_resume.is_some();
    let mut engine: std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = tinyflows::error::Result<tinyflows::engine::RunOutcome>,
                > + Send
                + '_,
        >,
    > = if let Some(store) = checkpoint_store.as_ref().filter(|_| !dry_run) {
        let checkpointer: Arc<dyn tinyflows::graph::Checkpointer<Value>> = store.clone();
        let graph_journal = Arc::new(tinyflows::engine::InMemoryGraphEventJournal::new());
        if let Some(resume) = &ctx.checkpoint_resume {
            let compiled = &compiled;
            let capabilities = &capabilities;
            let observer = &observer;
            let checkpoint_thread_id = checkpoint_thread_id.clone();
            let approved = resume.approved.clone();
            let rejected = resume.rejected.clone();
            Box::pin(async move {
                tinyflows::engine::resume_with_checkpointer_journaled_observed(
                    compiled,
                    capabilities,
                    checkpointer,
                    &checkpoint_thread_id,
                    approved,
                    rejected,
                    graph_journal,
                    observer,
                )
                .await
                .map(|journaled| journaled.outcome)
            })
        } else {
            let compiled = &compiled;
            let capabilities = &capabilities;
            let observer = &observer;
            let checkpoint_thread_id = checkpoint_thread_id.clone();
            Box::pin(async move {
                tinyflows::engine::run_with_checkpointer_journaled_observed(
                    compiled,
                    input,
                    capabilities,
                    checkpointer,
                    &checkpoint_thread_id,
                    graph_journal,
                    observer,
                )
                .await
                .map(|journaled| journaled.outcome)
            })
        }
    } else {
        Box::pin(tinyflows::engine::run_cancellable_with_observer(
            &compiled,
            input,
            &capabilities,
            token.clone(),
            &observer,
        ))
    };
    // Set only on the checkpointed-initial-run branch of the cancel arm below,
    // where `token.cancel()` is a no-op against the checkpointer engine call
    // (see `resuming`'s doc comment above). When that is the branch taken, a
    // finish inside the grace window comes back as an ordinary successful
    // `RunOutcome` — the engine was never told to stop — so `outcome.cancelled`
    // alone cannot be trusted to say whether this cancel actually landed. The
    // match on `outcome_opt` below consults this flag to still honour the
    // cancel instead of falling through to delivery as though it never
    // happened.
    let mut cancel_raced_noop_checkpointer_token = false;
    let outcome_opt = tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => {
            if resuming {
                None
            } else {
            // Node-boundary stop: flip the engine's token so it winds down
            // cleanly, then bound the wait. A run that crosses a boundary within
            // the grace returns its real `cancelled` outcome; a wedged one times
            // out — the `Err` becomes `None`, falling through to the hard abort
            // below.
            cancel_raced_noop_checkpointer_token = checkpointed;
            token.cancel();
            tokio::time::timeout(CANCEL_HARD_ABORT_GRACE, &mut engine)
                .await
                .ok()
            }
        }
        outcome = &mut engine => Some(outcome),
    };
    // **Drop the engine future before the observer.** It only matters on the
    // hard-abort arm (`None`): there the future still owns the observer `Arc`
    // clones its per-node handlers hold, so dropping `observer` alone would NOT
    // close the channel — the collector below would then block until the drain
    // timeout on every wedged stop, and the timeout would hide it. On the clean
    // arms (completion or a wound-down cancel) the graph already died inside the
    // call and those clones are gone, so this drop is a no-op.
    //
    // Today the **borrow checker enforces this ordering**: the engine future
    // borrows `observer`, so removing this line does not compile. That is a
    // happy accident of the current signature taking `&Arc`, not a guarantee —
    // an engine that took the `Arc` by value would close the compile-time hole
    // and re-open the runtime one, silently.
    // `a_cancelled_run_settles_fast_keeping_only_its_completed_nodes` asserts a
    // cancel-latency bound for exactly that case; it was verified to fail (at
    // 10.004s, the full drain timeout) against a deliberately leaked observer
    // clone.
    drop(engine);

    // Drop the last sender, then join — in that order, and before anything else.
    // The drop closes the channel so the collector's `recv()` returns `None` and
    // its loop ends; the join then waits for every already-sent frame to reach
    // the journal (when journaling) and returns the collected rows either way.
    // This is what makes the ordering guarantee true: every
    // `WorkflowNodeFinished` is durably appended before the caller's
    // `WorkflowRunFinished`, so a reader folding the journal never sees a run
    // settle before its nodes land.
    //
    // The drop closes the channel only if ours is the **last** `Arc` — true
    // today, because the engine's per-node handler clones die with the graph
    // inside `run_with_observer`. The bounded wait keeps that a *performance*
    // assumption rather than a liveness one: if a future engine ever parked a
    // clone somewhere longer-lived, this would log and move on with an empty
    // trail instead of wedging the run (and the host behind it) forever.
    drop(observer);
    // Issue #1008: the collector now returns both the response rows and the
    // accumulated per-node output map. The map feeds ONLY the run-output persist
    // on the failure/blocked arms below; `nodes` stays the output-free row list
    // the journal and `WorkflowRun.nodes` carry. A drain failure yields an empty
    // map, so a persist on that path simply records "produced none".
    let (mut nodes, partial_nodes, node_transcripts): (
        Vec<crate::ports::WorkflowRunNodeRow>,
        serde_json::Map<String, Value>,
        serde_json::Map<String, Value>,
    ) = match tokio::time::timeout(PROGRESS_DRAIN_TIMEOUT, collector).await {
        Ok(Ok(collected)) => collected,
        Ok(Err(err)) => {
            tracing::warn!(
                company = %record.id,
                workflow = %workflow.id,
                %run_id,
                %err,
                "workflow: the node-progress collector did not shut down cleanly"
            );
            (Vec::new(), serde_json::Map::new(), serde_json::Map::new())
        }
        Err(_) => {
            tracing::warn!(
                company = %record.id,
                workflow = %workflow.id,
                %run_id,
                "workflow: node progress events did not drain in time; the run's finished \
                 record may be journaled ahead of them"
            );
            (Vec::new(), serde_json::Map::new(), serde_json::Map::new())
        }
    };
    // The post-turn capture is a sideways channel because failed and blocked
    // agent nodes return no engine output. Drain it only after the engine and
    // progress collector are gone, then fold the same rows into every settle
    // arm below.
    let captured_artifacts = run_artifacts.take();

    // Resolved only AFTER the drain above, so a cancelled run's completed nodes
    // are journaled exactly like a completed run's before the caller writes the
    // finish.
    //
    // `None` is the **hard-abort** arm (issue #398): the run was wedged past the
    // grace window and its future was dropped, so there is no outcome to read —
    // `cancelled_run()` reports the stop with an empty body, and the trail is the
    // journal, not this return.
    let mut outcome = match outcome_opt {
        Some(Ok(mut outcome)) => {
            // The checkpointed-initial-run race (issue #1991 review): the
            // token above never reached the engine, so a finish inside the
            // grace window settles as an ordinary success with
            // `outcome.cancelled == false` even though the operator asked to
            // stop. Overriding it here routes into the `outcome.cancelled`
            // arm below — partial output persisted, no deliveries, no
            // pending approvals listed — instead of treating an operator's
            // cancel as though it never happened.
            if cancel_raced_noop_checkpointer_token && !outcome.cancelled {
                outcome.cancelled = true;
            }
            outcome
        }
        // Issue #881: the engine failed the run. Before deciding that is what
        // happened, ask whether every node that errored was one the host
        // *blocked* on an approval — because `on_error` defaults to `"stop"`,
        // a blocked agent node reaches the caller as exactly this `Err`.
        //
        // The containment check is what keeps this from hiding a real failure:
        // a run whose blocked node is joined by a genuinely broken one still
        // reports the error, and the block survives on the approval receipts.
        Some(Err(err)) => {
            let blocked = blocks.take();
            let halted_nodes = halted.take();
            // Issue #1008: the engine returns no `outcome.output` on this arm, so
            // the run's per-node output lives ONLY in the map the progress
            // observer accumulated. Persist that, flagged `partial`, on BOTH the
            // genuine-failure and the blocked branches — before #1008 both threw
            // it away, so the inspector wrongly reported "this run predates output
            // capture" for every failed or blocked run.
            let partial_output = merge_run_artifacts(
                merge_transcripts(&Value::Object(partial_nodes), &node_transcripts),
                &captured_artifacts,
            );
            // `only_expected_nodes_errored` reads `nodes[].status == Error` to
            // decide whether every errored row belongs to a blocked node, so it
            // MUST run before the capped-node reclassification below: a capped
            // node's row is still `Ok` at this point, and reclassifying first
            // would add its fresh `Error` row to this check and misread an
            // otherwise-clean block as a genuine failure.
            let is_genuine_failure = !only_expected_nodes_errored(&nodes, &blocked, &halted_nodes);
            // Issue #1865 (PR #1883 review): the sibling reclassification the
            // clean-finish arm applies near the bottom of this function, reached
            // here too — both branches below are early returns that used to skip
            // straight past that sole `capped.take()` and reconciliation. A node
            // that only truncated at the iteration cap (or paused for budget)
            // alongside a genuinely failed or blocked sibling kept its `Ok` row
            // forever even though its own attempt already settled `Failed`; this
            // closes that gap for both of this arm's exits, not only the one the
            // unit tests around `reclassify_capped_nodes` already covered.
            let mut nodes = nodes;
            reclassify_capped_nodes(&mut nodes, &capped.take());
            reclassify_halted_nodes(&mut nodes, &halted_nodes);
            // Codex review on #1990 (#3904894275): scrubbed and noticed here,
            // before branching on `is_genuine_failure`, not only in the
            // halt-only/halt-plus-block exits below. A halted sibling that
            // shares this run with a genuinely failed node still gets its row
            // reclassified `Declined` two lines up, but until this moved up
            // here the `partial_output` this arm persists and returns kept
            // the halted node's rejected reply verbatim — the failed run's
            // snapshot presented that reply as produced output with no
            // `Declined` explanation, unlike every other exit from this
            // function.
            let partial_output = without_node_ids(partial_output, &halted_nodes);
            for node_id in &halted_nodes {
                notices.push(format!(
                    "The step \"{node_id}\" concluded that no further work was needed."
                ));
            }
            if is_genuine_failure {
                // No continuation ever reuses a genuinely-failed run's thread
                // id — only an approval or blocked-node resume does, and
                // neither applies here — so its checkpoint lineage is prunable
                // exactly like a clean settle or cancel.
                prune_checkpoint_lineage(checkpoint_store.as_deref(), &checkpoint_thread_id).await;
                // A genuine failure. Persist the partial capture so the inspector
                // shows what the nodes that ran produced.
                if !persist_run_output(
                    run_output_store.as_deref(),
                    &record.id,
                    &workflow.id,
                    &run_id,
                    &partial_output,
                    true,
                )
                .await
                {
                    // This branch used to be log-only, because it returns `Err`
                    // and an `Err` had no `WorkflowRun` to hang a notice on. It
                    // has one now (below), so the operator hears about a lost
                    // snapshot on the failure arm exactly as on the blocked one.
                    notices.push(run_output_persist_failed_notice());
                }
                // Reclassify capped nodes before they move into the partial run,
                // the same as the settled arm does. A node that hit the iteration
                // cap reports Ok but settled Failed, so both must agree on Error.
                // Ordered after `reclassify_blocked` but the two cannot collide:
                // `run_turn` returns `Err` on the blocked arm before reaching
                // max_tool_iterations, so no node is both blocked and capped.
                reclassify_capped_nodes(&mut nodes, &capped.take());
                // Issue #1008 (second half): the failure carries the partial run
                // rather than only a message. The nodes that ran before the break
                // really did open board cards, park approvals and raise notices,
                // and those are durable facts by now — so the caller's
                // `record_run_finished` can list them instead of journaling a
                // failed run as an empty one. See `OpenCompanyError::partial_run`.
                return Err(OpenCompanyError::WorkflowRunFailed {
                    source: Box::new(map_engine_error(err)),
                    partial: Box::new(WorkflowRun {
                        output: partial_output,
                        pending_approvals: Vec::new(),
                        deliveries: Vec::new(),
                        cancelled: false,
                        nodes,
                        notices: notices.take(),
                        board: board.take(),
                        // Non-empty exactly when a real failure and a block
                        // happened together — the case the containment check
                        // above refuses to relabel as a plain block.
                        blocked_nodes: blocked,
                        approvals: approvals.take(),
                    }),
                });
            }
            if blocked.is_empty() {
                if !persist_run_output(
                    run_output_store.as_deref(),
                    &record.id,
                    &workflow.id,
                    &run_id,
                    &partial_output,
                    true,
                )
                .await
                {
                    notices.push(run_output_persist_failed_notice());
                }
                return Ok(WorkflowRun {
                    output: serde_json::json!({ "nodes": partial_output }),
                    pending_approvals: Vec::new(),
                    deliveries: Vec::new(),
                    cancelled: false,
                    nodes,
                    notices: notices.take(),
                    board: board.take(),
                    blocked_nodes: Vec::new(),
                    approvals: approvals.take(),
                });
            }
            tracing::info!(
                company = %record.id,
                workflow = %workflow.id,
                %run_id,
                nodes = ?blocked.iter().map(|b| &b.node_id).collect::<Vec<_>>(),
                "workflow: the run stopped because a node is waiting on an operator, not because \
                 it failed"
            );
            // Issue #1008: minus the blocked nodes' own entries, and that
            // subtraction is issue #881's invariant rather than tidiness. A node
            // refused inside its model's tool loop ends its turn by writing
            // *prose about being blocked*, and the engine records that prose as
            // the step's output. #881 stopped that apology travelling to the next
            // node; letting it ride the run inspector as the node's product would
            // re-open the same lie one surface over. The node's `blocked` chip and
            // the run's notice already say what happened.
            // Remove the blocked node's refusal prose, then restore only the
            // files it actually wrote. A partial artifact is a deliverable; an
            // apology about why the turn stopped is not.
            let partial_output =
                merge_run_artifacts(without_nodes(partial_output, &blocked), &captured_artifacts);
            // A blocked run DOES return a `WorkflowRun`, so a failed persist adds
            // an operator-facing notice rather than only a log line (Part 6).
            if !persist_run_output(
                run_output_store.as_deref(),
                &record.id,
                &workflow.id,
                &run_id,
                &partial_output,
                true,
            )
            .await
            {
                notices.push(run_output_persist_failed_notice());
            }
            // Issue #899 (Stage 1): this is the block-settle arm the DEFAULT
            // `on_error = "stop"` takes — a blocked agent node reaches here as an
            // `Err` the containment check reclassifies. Stash each blocked node's
            // continuation facts so approving its parked calls re-dispatches the
            // run. (The `on_error = "continue"/"route"` case settles on the main
            // arm below, which arms the same stash there.)
            stash_blocked_agent_nodes_checkpointed(
                delivery.as_ref(),
                &workflow.id,
                &run_id,
                &trigger_input,
                &blocked,
                &ctx.started_by,
                Some(&checkpoint_thread_id),
                Some(&workflow_fingerprint),
            )
            .await;
            // Reclassify capped nodes before they move into the blocked run, the
            // same as the settled arm does. A node that hit the iteration cap
            // reports Ok but settled Failed, so both must agree on Error.
            reclassify_capped_nodes(&mut nodes, &capped.take());
            return Ok(blocked_run(BlockedRun {
                nodes,
                blocked,
                notices,
                board: board.take(),
                approvals: approvals.take(),
                // Issue #1008: the same capture the durable snapshot took, so a
                // live run drawer and a run reopened from History show one thing
                // rather than disagreeing about what the run produced. Wrapped
                // under `nodes` because that is the shape every reader of
                // `WorkflowRun.output` already parses — the durable record stores
                // the bare map, the run body carries the engine's envelope.
                output: serde_json::json!({ "nodes": partial_output }),
            }));
        }
        None => {
            prune_checkpoint_lineage(checkpoint_store.as_deref(), &checkpoint_thread_id).await;
            return Ok(cancelled_run(
                notices.take(),
                board.take(),
                approvals.take(),
            ));
        }
    };

    let halted_nodes = halted.take();
    reclassify_halted_nodes(&mut nodes, &halted_nodes);
    if let Some(raw_nodes) = outcome.output.get_mut("nodes") {
        *raw_nodes = without_node_ids(std::mem::take(raw_nodes), &halted_nodes);
    }
    for node_id in &halted_nodes {
        notices.push(format!(
            "The step \"{node_id}\" concluded that no further work was needed."
        ));
    }

    // Issue #398: the **clean** node-boundary cancel. The engine observed the
    // flipped token and wound down at a boundary, so unlike the hard-abort arm
    // above there IS a real (partial) outcome and the collected node rows are
    // meaningful — carry them. A stop still routes nothing and parks no gate: an
    // operator who stopped a run is asking neither to deliver its half-finished
    // reports nor to be asked about gates it never reached. `pending_approvals`
    // is emptied for the same reason `cancelled_run()` empties it — listing gates
    // this run will not continue would imply it is still waiting on them.
    if outcome.cancelled {
        // Issue #596: a cleanly-cancelled run still produced real partial output
        // for the nodes that completed — persist it so the console inspector can
        // show how far the run got and what each finished node made. This is an
        // outcome-bearing arm (unlike the hard-abort return above, which has no
        // outcome and persists nothing).
        //
        // Issue #1008: this is a CLEAN cancel with a real `outcome.output`, so it
        // persists that canonical map with `partial = false` — the "partial"
        // flag is reserved for the failure/blocked arms that have no outcome and
        // fall back to the observer's accumulated capture. A failed write adds an
        // operator notice (Part 6), since this arm returns a `WorkflowRun`.
        let raw_nodes = outcome.output.get("nodes").cloned().unwrap_or(Value::Null);
        // The transcripts the observer collected are not in the engine's run state,
        // so a clean settle would otherwise persist a snapshot that says what every
        // node emitted and nothing about what its agent did.
        let raw_nodes = merge_run_artifacts(
            merge_transcripts(&raw_nodes, &node_transcripts),
            &captured_artifacts,
        );
        if !persist_run_output(
            run_output_store.as_deref(),
            &record.id,
            &workflow.id,
            &run_id,
            &raw_nodes,
            false,
        )
        .await
        {
            notices.push(run_output_persist_failed_notice());
        }
        // Issue #1865 (PR #1883 review): the third sibling reclassification —
        // `94c8e0507` closed this gap on the genuine-failure/blocked `Err` arm
        // above, but a clean node-boundary cancel is its own early return with
        // its own `nodes`, reached without ever passing through that arm or the
        // clean-finish arm at the bottom of this function. A node that only
        // truncated at the iteration cap (or paused for budget) before an
        // operator cancelled a later/parallel node kept its `Ok` row here too,
        // even though its own attempt already settled `Failed` — the same
        // disagreement, a third exit.
        let mut nodes = nodes;
        reclassify_capped_nodes(&mut nodes, &capped.take());
        prune_checkpoint_lineage(checkpoint_store.as_deref(), &checkpoint_thread_id).await;
        return Ok(WorkflowRun {
            output: merge_run_artifacts_envelope(outcome.output, &captured_artifacts),
            pending_approvals: Vec::new(),
            deliveries: Vec::new(),
            cancelled: true,
            nodes,
            // A stopped run still reports what its completed nodes had to say
            // — a discarded-overflow notice describes calls that were already
            // refused before the stop, and withholding it would leave the
            // operator with fewer cards than were gated and no explanation.
            notices: notices.take(),
            // Issue #661 (M5): a cancelled run's board writes SURVIVE and stay
            // listed. A card is a durable write the moment the drain performs it,
            // so an operator who stopped the run still has the card in front of
            // them — dropping the row would leave a card on the board that no run
            // admits to opening. (A run cancelled *mid-turn* staged writes that
            // were never drained, so it has no card and no row: consistent, and the
            // same judgement `park_gated_calls` documents for gated calls.)
            board: board.take(),
            // Issue #881: a node the host blocked cannot be reached on this arm
            // — a block halts the run, which is the `Err` arm above, not a
            // clean node-boundary cancel. Taken rather than hard-coded empty so
            // the claim stays a test's to make.
            blocked_nodes: blocks.take(),
            // Issue #880: NOT zeroed, unlike `deliveries` / `pending_approvals`
            // one field up. Those describe what the run would still do; this
            // describes what it already did. A run stopped after parking two
            // approvals really did park them — the cards are on the operator's
            // Approvals page right now — so dropping the rows would leave two
            // cards no run admits to opening. Same argument as `board` above.
            approvals: approvals.take(),
        });
    }

    // Issue #542: a dry run STOPS here on the effect side. Route the reached
    // `output` nodes so the operator sees WHERE each report would have gone —
    // that routing is exactly what a test run is meant to prove — but dispatch
    // nothing, journal nothing, and park no gate. The per-node timeline in
    // `nodes` and this settled body are the whole record.
    if dry_run {
        let deliveries = super::delivery::deliver_outputs_dry(record, workflow, &outcome.output);
        return Ok(WorkflowRun {
            output: outcome.output,
            pending_approvals: outcome.pending_approvals,
            deliveries,
            cancelled: false,
            nodes,
            notices: notices.take(),
            // Issue #661 (M5): empty by construction, not by this line. A dry run's
            // bundle wires `DryRunAgent`, so `HarnessAgentRunner` — the only thing
            // that ever takes a board claim or drains one — is never built. Taken
            // rather than hard-coded empty so the claim is a *test's* to make (see
            // `a_dry_run_of_a_spawning_graph_writes_no_card`), following #542.
            board: board.take(),
            // Issues #881 / #880: empty by the same construction. `DryRunAgent`
            // runs no turn, so no tool call is gated, so nothing blocks and
            // nothing parks. Taken rather than hard-coded for the same reason.
            blocked_nodes: blocks.take(),
            approvals: approvals.take(),
        });
    }

    // Route every reached `output` node's report to its configured destination.
    // Deliberately here rather than in the HTTP handler: the orchestrator's
    // `run_workflow` tool and the trigger scheduler drive this same path, and a
    // scheduled run is exactly the case where nobody is watching the console's
    // run-result drawer. Never fails the run — each attempt is reported instead.
    //
    // Issue #438: the per-lineage half of the guard. `delivered_in_input` is
    // what a *continuation* must NOT send again — read off the trigger input,
    // where the approval that started this run threaded it. Empty on a run nobody
    // resumed.
    //
    // Issue #529: unioned with the durable half. A crashed run journals its
    // sends write-behind but never its finish, so its deliveries are stranded in
    // the journal with nothing on any trigger input to carry them. An
    // *independent* re-run (the operator pressing Run again, or a schedule
    // firing) has an empty per-lineage ledger and would re-mail every already-
    // sent report. `delivered_by_unsettled_runs` folds those stranded deliveries
    // back so the re-run skips them. Consulted only when the journal is wired
    // (`events` is `Some`) — the default build and every unwired test degrade to
    // the pre-#529 per-lineage behaviour, delivering as before.
    let mut already_delivered = crate::runtime::workflow_resume::delivered_in_input(&trigger_input);
    if let Some(events) = events.as_ref() {
        for entry in
            crate::runtime::delivered_by_unsettled_runs(events, &record.id, &workflow.id).await
        {
            // Deduped by node — the two ledgers may name the same report, and a
            // reached node is skipped on the first match regardless.
            if !already_delivered
                .iter()
                .any(|prior| prior.node == entry.node)
            {
                already_delivered.push(entry);
            }
        }
    }
    let deliveries = super::delivery::deliver_outputs(
        delivery.as_ref(),
        record,
        workflow,
        &run_id,
        &outcome.output,
        &already_delivered,
    )
    .await;

    // Issue #395: turn every gate the engine paused on into a decidable
    // approval. Deliberately here, beside the delivery call and for the same
    // reason: all three entry points — the console route, the cron scheduler,
    // and the orchestrator's `run_workflow` tool — come through this function,
    // and a scheduled run is exactly the case where nobody is watching the
    // response that used to be the only place these ids appeared.
    //
    // **After delivery, and that ordering is load-bearing** (issue #438). The
    // parked card carries the ledger of what this run delivered, so it can only
    // be built once delivery has happened. Nothing is lost by the swap: the two
    // steps are independent — parking reads `pending_approvals` and delivery
    // reads reached `output` nodes, and a node past a gate was never reached, so
    // no delivery could ever depend on a gate having been parked first.
    //
    // Skipped for a cancelled run, which returns above: an operator who stopped
    // a run is not asking to be asked about the gates it never reached. (A dry
    // run also never reaches here — it returned above, having parked nothing.)
    // Issue #846: what this run's `tool_call` nodes sent outside the company,
    // and what it could not record. Computed here, beside the delivery ledger
    // and for the same reason — the card an approval is decided from has to
    // carry both, or approving repeats one of them.
    //
    // Only when the run actually paused: a run that reached the end has no
    // continuation coming, so there is nothing to guard against and nothing to
    // warn about.
    let (performed, mut unreplayable) = if outcome.pending_approvals.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        super::replay::outward_calls_performed(&graph, &outcome.output, workflow)
    };
    // Issue #617: a child that paused namespaced gates (`sub::work`) will
    // restart when its gate is approved, and a restart re-runs the child's
    // ungated outward calls. Their results are not carried up when the child
    // pauses, so nothing can be replayed — report them exactly like the
    // top-level unreplayable calls above, so the operator is warned that
    // approving restarts the child from the top.
    if !outcome.pending_approvals.is_empty() {
        unreplayable.extend(super::replay::child_calls_to_repeat(
            &graph,
            &outcome.pending_approvals,
            &child_gates,
            &trigger_input,
        ));
    }
    for call in &unreplayable {
        tracing::warn!(
            company = %record.id,
            workflow = %workflow.id,
            %run_id,
            node = %call.node_id,
            tool = %call.slug,
            why = call.why,
            "workflow: an outward call this run made cannot be replayed, so approving a gate \
             below it will repeat it"
        );
        notices.push(call.notice());
    }
    park_pending_gates(
        delivery.as_ref(),
        record,
        &workflow.id,
        &run_id,
        PausedGates {
            trigger_input: &trigger_input,
            pending: &outcome.pending_approvals,
            deliveries: &deliveries,
            performed: &performed,
            gated: &gated,
            graph: &graph,
            // Issue #596: the reached-node output + the graph's edges, so each
            // parked gate can carry the verbatim upstream content awaiting
            // sign-off.
            output: &outcome.output,
            edges: &workflow.edges,
            // Issue #617: the resolver's per-child gate record, so a namespaced
            // child gate's card can name the child's tool and reason.
            child_gates: &child_gates,
            started_by: &ctx.started_by,
            checkpoint_thread_id: &checkpoint_thread_id,
            workflow,
        },
    )
    .await;

    // Issue #596: persist this settled run's per-node output durably (normal
    // completion AND the paused-with-`pending_approvals` case both reach here).
    // Best-effort, upstream of the WorkflowRun below so console/scheduled/
    // agent-tool runs all persist through this one site.
    //
    // Issue #1008: a clean settle carries the real `outcome.output`, so it
    // persists that canonical map with `partial = false`; a failed write adds an
    // operator notice (Part 6) since this arm returns a `WorkflowRun`.
    let raw_nodes = outcome.output.get("nodes").cloned().unwrap_or(Value::Null);
    let raw_nodes = merge_run_artifacts(raw_nodes, &captured_artifacts);
    if !persist_run_output(
        run_output_store.as_deref(),
        &record.id,
        &workflow.id,
        &run_id,
        &raw_nodes,
        false,
    )
    .await
    {
        notices.push(run_output_persist_failed_notice());
    }

    // Issue #881: a node can block and the run still reach here — an author who
    // wrote `on_error = "continue"` or `"route"` asked for the branch to survive
    // the block, and gets it. The run-level record stays truthful either way, so
    // the post-pass runs on this arm too rather than only on the halted one.
    let mut pending_approvals = outcome.pending_approvals;
    let blocked_nodes = blocks.take();
    reclassify_blocked(&mut nodes, &mut pending_approvals, &blocked_nodes);
    // Issue #1865: the sibling reclassification — a node whose turn truncated
    // at the `max_tool_iterations` cap reports `Success` at the engine
    // boundary (see `RunCappedNodes`), so its row lands here as `Ok` while its
    // attempt already settled `Failed`. Relabel it `Error` so the two agree,
    // exactly as `reclassify_blocked` relabels a parked node `Blocked` so ITS
    // row agrees with what actually happened. Ordered after `reclassify_blocked`
    // but the two cannot collide: `run_turn` returns `Err` on the blocked arm
    // before it ever reaches the iteration-cap check, so no node is ever both.
    reclassify_capped_nodes(&mut nodes, &capped.take());
    // Issue #899 (Stage 1): stash the workflow id and trigger input each blocked
    // agent node's continuation needs, keyed by the same per-(run, node) turn key
    // its parked calls armed `ContinuationQueue` under. The resolve path releases
    // both together and re-dispatches the run once the last call is decided.
    stash_blocked_agent_nodes_checkpointed(
        delivery.as_ref(),
        &workflow.id,
        &run_id,
        &trigger_input,
        &blocked_nodes,
        &ctx.started_by,
        Some(&checkpoint_thread_id),
        Some(&workflow_fingerprint),
    )
    .await;
    // Issue #900: `blocked_run` (the halt arm) tells the operator what blocked
    // via a `notices` sentence, not only via the node's own status — the
    // per-node chip is easy to miss on a run that otherwise looks fine, and a
    // continued run finishing "green" beside a blocked node is exactly that
    // case. Same sentence, same source (`blocked_notice`), so the two arms
    // cannot drift into disagreement about what a block reads as.
    for b in &blocked_nodes {
        notices.push(blocked_notice(b));
    }
    // Issue #1865: a node under `on_error = "continue"`/`"route"` errors and
    // the graph keeps going — the run reaches this arm (not `blocked_run`)
    // carrying an `Error` row the reclassification above deliberately left
    // alone, because it is not waiting on anyone. `WorkflowRunVerdict::of`
    // now folds that into a `degraded` verdict, but a verdict is one word —
    // this is the sentence naming *which* node, so an operator does not have
    // to open the canvas to find the red chip. One notice per errored node,
    // in the order the nodes finished, the same order `nodes` already carries.
    for row in nodes
        .iter()
        .filter(|n| n.status == WorkflowNodeStatus::Error)
    {
        notices.push(errored_node_notice(&row.node_id));
    }

    if pending_approvals.is_empty() && blocked_nodes.is_empty() {
        prune_checkpoint_lineage(checkpoint_store.as_deref(), &checkpoint_thread_id).await;
    }

    Ok(WorkflowRun {
        output: merge_run_artifacts_envelope(outcome.output, &captured_artifacts),
        pending_approvals,
        deliveries,
        cancelled: false,
        nodes,
        // Issue #638: whatever the nodes had to tell the operator. Empty for
        // every run that did not overflow the approval cap, which is nearly all
        // of them.
        notices: notices.take(),
        // Issue #661 (M5): every card this run's nodes opened or re-owned. Empty
        // for every run whose nodes touched no card, which is nearly all of them.
        board: board.take(),
        blocked_nodes,
        // Issue #880: every approval this run's nodes parked, whether or not
        // any node blocked. A run can park a card and still finish — the
        // author's `on_error` decides — and the receipt is owed in both cases.
        approvals: approvals.take(),
    })
}

async fn prune_checkpoint_lineage(
    store: Option<&super::checkpoint_store::WorkflowCheckpointStore>,
    thread_id: &str,
) {
    if let Some(store) = store
        && let Err(error) = store.prune_settled(thread_id).await
    {
        tracing::warn!(%thread_id, %error, "workflow: failed to prune settled checkpoints");
    }
}

/// Whether every node that reported an error is one the host blocked.
///
/// The guard on reclassifying an engine failure as a block (issue #881). A run
/// where a blocked node is joined by a genuinely broken one must still report
/// the failure: hiding a real error behind "waiting on approval" is the same
/// class of lie #881 exists to remove, pointed the other way.
///
/// Requires **at least one** errored row before it will vouch for the
/// reclassification (issue #900). `Iterator::all` is vacuously `true` on an
/// empty iterator, so without this an engine failure that named no node at
/// all — a setup or validation error the engine raised before any node ran —
/// would satisfy the check by default and get relabelled as a plain block,
/// dropping the real failure exactly as the doc comment above says this guard
/// exists to prevent.
#[cfg(test)]
fn only_blocked_nodes_errored(
    nodes: &[crate::ports::WorkflowRunNodeRow],
    blocked: &[crate::ports::WorkflowBlockedNode],
) -> bool {
    let mut errored = nodes
        .iter()
        .filter(|row| row.status == WorkflowNodeStatus::Error)
        .peekable();
    errored.peek().is_some() && errored.all(|row| blocked.iter().any(|b| b.node_id == row.node_id))
}

fn only_expected_nodes_errored(
    nodes: &[crate::ports::WorkflowRunNodeRow],
    blocked: &[crate::ports::WorkflowBlockedNode],
    halted: &[String],
) -> bool {
    let mut errored = nodes
        .iter()
        .filter(|row| row.status == WorkflowNodeStatus::Error)
        .peekable();
    errored.peek().is_some()
        && errored.all(|row| {
            blocked.iter().any(|b| b.node_id == row.node_id)
                || halted.iter().any(|id| id == &row.node_id)
        })
}

/// Reclassifies a blocked node's row and lists it as something the run is
/// waiting on (issue #881).
///
/// Host-side on purpose. The engine reported `Error` and that report is honest —
/// the capability *did* return an error, which is what halted the branch. What
/// the engine cannot know is *why*, so the host, which does, relabels the row
/// on the way out. The
/// [`ExecutionStep` → `NodeProgress`](ProgressObserver::on_step_finish) mapping
/// is deliberately left alone: it is the engine's own account of what happened,
/// and rewriting it there would make the live progress frames disagree with the
/// engine.
///
/// The blocked ids are **unioned into** `pending_approvals` rather than replacing
/// it: a run can both pause at a `requires_approval` gate and block an agent
/// node, and the console renders every entry as a node name.
fn reclassify_blocked(
    nodes: &mut [crate::ports::WorkflowRunNodeRow],
    pending_approvals: &mut Vec<String>,
    blocked: &[crate::ports::WorkflowBlockedNode],
) {
    if blocked.is_empty() {
        return;
    }
    for row in nodes.iter_mut() {
        if blocked.iter().any(|b| b.node_id == row.node_id) {
            row.status = WorkflowNodeStatus::Blocked;
        }
    }
    for b in blocked {
        if !pending_approvals.contains(&b.node_id) {
            pending_approvals.push(b.node_id.clone());
        }
    }
}

/// Relabels a capped agent node's row `Error` so it agrees with its attempt
/// (issue #1865).
///
/// Host-side, exactly on [`reclassify_blocked`]'s terms and right beside it:
/// `tinyflows::observability` reports `StepStatus::Success` for a turn that
/// truncated at the `max_tool_iterations` cap — the model produced a reply,
/// the node "finished" from the engine's point of view — so the row this
/// function receives already carries [`WorkflowNodeStatus::Ok`]. Only the
/// host, via [`super::caps::RunCappedNodes`], knows the reply was a partial
/// checkpoint rather than a completed answer, which is the same shape of gap
/// `reclassify_blocked` closes for a parked node: the engine reported the only
/// thing it could see, and the host relabels the row on the way out rather
/// than rewriting the engine's own account of what happened.
///
/// `capped` names node ids, not rows with a status to overwrite — unlike
/// `blocked`, which carries [`WorkflowBlockedNode`](crate::ports::WorkflowBlockedNode)
/// structs the caller already built. A plain id list is all
/// [`RunCappedNodes`](super::caps::RunCappedNodes) needs to carry: there is no
/// second fact about a capped node the run-level record is missing, the way
/// `blocked_nodes` carries `tools`/`approval_ids` for the notice
/// `blocked_notice` composes.
fn reclassify_capped_nodes(nodes: &mut [crate::ports::WorkflowRunNodeRow], capped: &[String]) {
    if capped.is_empty() {
        return;
    }
    for row in nodes.iter_mut() {
        // `Blocked` is deliberately never overridden here, even though
        // `run_turn` structurally never puts one node in both lists (see this
        // function's own doc): a node waiting on a person is the more
        // specific fact, and a future caller that somehow did name one in
        // both must not have this flip hide the approval behind a plain
        // failure — the same direction `only_expected_nodes_errored`'s guard
        // already leans in.
        //
        // Coderabbit review on #1990: `Declined` joins that exemption for the
        // same reason. When `retry.max_attempts > 1`, a node whose judge
        // halted it benignly can be retried, and if that retry then hits the
        // iteration cap, `RunCappedNodes` names the same node id
        // `reclassify_halted_nodes` already settled `Declined` — a correct,
        // more specific fact that a plain `Error` must not overwrite.
        if row.status != WorkflowNodeStatus::Blocked
            && row.status != WorkflowNodeStatus::Declined
            && capped.iter().any(|id| id == &row.node_id)
        {
            row.status = WorkflowNodeStatus::Error;
        }
    }
}

/// Relabels the engine's capability-error row as an intentional benign stop.
fn reclassify_halted_nodes(nodes: &mut [crate::ports::WorkflowRunNodeRow], halted: &[String]) {
    for row in nodes.iter_mut() {
        if halted.iter().any(|id| id == &row.node_id) {
            row.status = WorkflowNodeStatus::Declined;
        }
    }
}

/// What a run halted by a blocked node settles with (issue #881).
///
/// Bundled rather than passed as five arguments to [`blocked_run`], the same
/// choice [`super::caps::RunContext`] makes and for the same reason: every field
/// is one run's, and none of them means anything without the others.
struct BlockedRun {
    nodes: Vec<crate::ports::WorkflowRunNodeRow>,
    blocked: Vec<crate::ports::WorkflowBlockedNode>,
    notices: super::caps::RunNotices,
    board: Vec<crate::ports::WorkflowRunBoardRow>,
    approvals: Vec<crate::ports::WorkflowRunApprovalRow>,
    /// Issue #1008: what the nodes upstream of the block produced, in the
    /// engine's `{ "nodes": { "<id>": { "items": [ … ] } } }` envelope, with the
    /// blocked nodes' own entries already removed by the caller.
    output: Value,
}

/// Settles a run that stopped because a node is waiting on an operator (issue
/// #881).
///
/// **`Ok`, and no `error`.** The engine returned `Err`, because a capability
/// error under the default `on_error = "stop"` is how a branch halts — but a
/// node waiting for a human is not a node that failed, and journalling it as one
/// would put every blocked run in the failure count and hide real failures among
/// them. This is precisely the reclassification
/// [`WorkflowRun::cancelled`](crate::ports::WorkflowRun) already performs for a
/// deliberate stop: "a cancelled run is not a failed one", and neither is a
/// blocked one.
///
/// Each emptiness below is a claim rather than a shrug:
///
/// * **no `deliveries`** — `deliver_outputs` runs off the settled output, which
///   does not exist here. An absent row already means "not reached" everywhere
///   else, and a run that stopped short must not mail anybody a report of work
///   it did not finish.
///
/// # `output` is threaded in, not emptied (issue #1008)
///
/// It used to be `Value::Null`, on the argument that "the engine returned an
/// error, not a final state". That holds for the engine's *merged* state and not
/// for the nodes: everything upstream of the block ran to completion and
/// produced real items, which the progress observer collected on the way past.
/// Reporting none of it made the run inspector say "no output for this node"
/// about a node that had just written a draft. The caller threads its collected
/// capture in — the same value it persisted — having first removed the blocked
/// nodes' own entries, so "the blocked node produced nothing" stays literally
/// true here.
///
/// `notices` carries the operator-facing sentence, composed from the structural
/// blocked rows so the wording lives in one place and no model prose or store
/// error text can ride it.
fn blocked_run(settled: BlockedRun) -> WorkflowRun {
    let BlockedRun {
        mut nodes,
        blocked,
        notices,
        board,
        approvals,
        output,
    } = settled;
    for b in &blocked {
        notices.push(blocked_notice(b));
    }
    let mut pending_approvals = Vec::new();
    reclassify_blocked(&mut nodes, &mut pending_approvals, &blocked);
    WorkflowRun {
        output,
        pending_approvals,
        deliveries: Vec::new(),
        cancelled: false,
        nodes,
        notices: notices.take(),
        board,
        blocked_nodes: blocked,
        approvals,
    }
}

/// Drops `blocked`'s nodes from a collected `{ "<id>": { "items": [ … ] } }`
/// capture (issue #1008).
///
/// A blocked node produced nothing, so it must have no entry — see the call site
/// for why that is issue #881's invariant and not a cosmetic choice. A value of
/// another shape is returned untouched: this narrows a map it recognises and
/// never invents one.
fn without_nodes(mut output: Value, blocked: &[crate::ports::WorkflowBlockedNode]) -> Value {
    if blocked.is_empty() {
        return output;
    }
    if let Value::Object(nodes) = &mut output {
        for b in blocked {
            nodes.remove(&b.node_id);
        }
    }
    output
}

fn without_node_ids(mut output: Value, ids: &[String]) -> Value {
    if let Value::Object(nodes) = &mut output {
        for id in ids {
            nodes.remove(id);
        }
    }
    output
}

/// The operator's sentence for one blocked node (issue #881).
///
/// Composed here, from the structural row, rather than lifted off the
/// capability's error string: that string reaches host logs, and the two
/// audiences want different things. Worded as a **receipt** — "parked N
/// approvals", never "waiting on N" — because a settle-time count of what is
/// still outstanding is stale the moment the operator approves one, while a
/// record of what this run parked is true forever.
pub(super) fn blocked_notice(blocked: &crate::ports::WorkflowBlockedNode) -> String {
    let tools = if blocked.tools.is_empty() {
        "a tool call".to_string()
    } else {
        blocked
            .tools
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let parked = blocked.approval_ids.len();
    let choices = crate::ports::blockers::BLOCKER_VERDICT_CHOICES;
    let mut tail = String::new();
    if parked > 0 {
        let plural = if parked == 1 { "" } else { "s" };
        let them = if parked == 1 { "it" } else { "them" };
        // A gated call continues the run when approved; a blocker is answered
        // with one of four verdicts, and two of them do not run the step again.
        // Only the all-gated shape may promise an automatic continue.
        tail.push_str(&if blocked.blockers == 0 {
            format!(
                " It parked {parked} approval{plural}; approve {them} in Approvals and this run \
                 continues on its own. Approving re-runs the step, so if the agent's next turn \
                 differs it may ask again."
            )
        } else if blocked.blockers == parked {
            format!(" It parked {parked} question{plural}; answer {them} in Approvals — {choices}.")
        } else {
            format!(
                " It parked {parked} approval{plural} in Approvals. Approving a gated tool call \
                 continues this run on its own; the questions among them are answered instead — \
                 {choices}."
            )
        });
    }
    if blocked.unparkable > 0 {
        tail.push_str(&format!(
            " {} call{} could not be queued for approval at all, so you will not be asked about \
             {}.",
            blocked.unparkable,
            if blocked.unparkable == 1 { "" } else { "s" },
            if blocked.unparkable == 1 {
                "it"
            } else {
                "them"
            }
        ));
    }
    format!(
        "The step \"{}\" needed your approval for {tools}, so it produced nothing and the steps \
         after it did not run.{tail}",
        blocked.node_id
    )
}

/// Names a step that settled `Error` on a run that otherwise kept going (issue
/// #1865) — a node under `on_error = "continue"`/`"route"` whose capability
/// genuinely errored, or an agent node whose turn was relabelled `Error`
/// because it truncated at the `max_tool_iterations` cap (see
/// `reclassify_capped_nodes`). Both settle the run without an `error`, and
/// both are exactly what `Degraded` exists to stop hiding.
///
/// The sentence half of `degraded`: `WorkflowRunVerdict::Degraded` says one
/// word about the whole run, and this says which node — mirroring how
/// `blocked_notice` is the sentence behind a `blocked`/`stranded` verdict.
/// Worded to cover either cause without claiming which one it was, since the
/// row carries no reason text (issue #371's no-`String`-arm invariant). Called
/// once per errored, non-blocked row in `nodes`, in finish order, so a graph
/// with more than one recovering branch names every one of them rather than
/// only the first — the console's `failedNodeOf` picks one node for a
/// *stopped* run's headline, but this run did not stop.
fn errored_node_notice(node_id: &str) -> String {
    format!(
        "The step \"{node_id}\" did not finish cleanly, and the run continued past it — check \
         its output for details."
    )
}

/// Persists a run's per-node output to the durable, console-facing store (issue
/// #596; failure/blocked capture added in #1008), best-effort.
///
/// One helper, called from every outcome-bearing arm of `run_workflow_inner`, so
/// the bounding + write live in exactly one place. `store` is `None` on the
/// default build and every unwired test — then this is a no-op. A write failure
/// is logged at `warn` and never fails the run: the run's work is already done
/// and correct, and losing an inspector snapshot must not discard it.
///
/// The caller hands `raw_nodes` — the `{ "<id>": { "items": [ … ] } }` map — in
/// directly: on the clean arms that is `outcome.output["nodes"]` (the same
/// capture the in-process
/// [`RunOutputCache`](crate::harness::orchestrator::RunOutputCache) reads), and
/// on the failure/blocked arms (issue #1008) it is the map the progress observer
/// accumulated, flagged `partial`. Bounding happens inside
/// [`WorkflowRunOutputRecord::from_raw_nodes`](crate::ports::WorkflowRunOutputRecord::from_raw_nodes),
/// which clips (never refuses) so the durable record always exists.
///
/// Returns `true` when the snapshot is safely stored (or there is no store to
/// store it in — a no-op is not a failure), and `false` only when a store was
/// present and the write errored. The notice-bearing callers (which return a
/// `WorkflowRun`) use `false` to add an operator-facing notice; the genuine-`Err`
/// caller has no `WorkflowRun` to hang one on and lets the `warn!` stand alone
/// (issue #1008, Part 3).
/// Folds the observer's per-node transcripts into a run-output `nodes` map.
///
/// The two halves arrive from different places and only meet here. A clean
/// settle's `nodes` map is the ENGINE's own run state (`outcome.output`), which
/// knows what each node emitted but nothing about what a harness did inside one;
/// the transcripts come off `ExecutionStep.transcript`, which the progress
/// observer collected. Merging at the persist site is what puts them on the same
/// record on every arm — a clean run, a failed one, and a blocked one alike.
///
/// Non-destructive by construction: it only ever adds a `transcript` key to a
/// node object that already exists in `nodes`, so a node the engine did not
/// report cannot be invented here, and nothing the engine wrote is overwritten.
/// A non-object `nodes` (or a node whose slot is not an object) passes through
/// untouched rather than being coerced into one.
fn merge_transcripts(nodes: &Value, transcripts: &serde_json::Map<String, Value>) -> Value {
    if transcripts.is_empty() {
        return nodes.clone();
    }
    let Value::Object(map) = nodes else {
        return nodes.clone();
    };
    let mut merged = map.clone();
    for (node_id, transcript) in transcripts {
        let Some(Value::Object(slot)) = merged.get_mut(node_id) else {
            continue;
        };
        slot.insert("transcript".to_string(), transcript.clone());
    }
    Value::Object(merged)
}

/// Adds card-less run artifacts to their node slots without changing items.
///
/// Unlike transcripts, artifacts may legitimately belong to a node that ended
/// in error and therefore has no engine output slot. Such a slot is created
/// with an empty `items` array so the inspector can surface the files while
/// still truthfully showing no reply text.
fn merge_run_artifacts(nodes: Value, artifacts: &serde_json::Map<String, Value>) -> Value {
    if artifacts.is_empty() {
        return nodes;
    }
    let mut nodes = match nodes {
        Value::Object(nodes) => nodes,
        _ => serde_json::Map::new(),
    };
    for (node_id, rows) in artifacts {
        let slot = nodes
            .entry(node_id.clone())
            .or_insert_with(|| serde_json::json!({ "items": [] }));
        if let Value::Object(slot) = slot {
            slot.insert("artifacts".to_string(), rows.clone());
        }
    }
    Value::Object(nodes)
}

/// Applies [`merge_run_artifacts`] to a full engine output envelope.
fn merge_run_artifacts_envelope(
    mut output: Value,
    artifacts: &serde_json::Map<String, Value>,
) -> Value {
    if artifacts.is_empty() {
        return output;
    }
    let Value::Object(envelope) = &mut output else {
        return serde_json::json!({
            "nodes": merge_run_artifacts(Value::Null, artifacts),
        });
    };
    let nodes = envelope.remove("nodes").unwrap_or(Value::Null);
    envelope.insert("nodes".to_string(), merge_run_artifacts(nodes, artifacts));
    output
}

async fn persist_run_output(
    store: Option<&dyn crate::ports::run_output::WorkflowRunOutputStore>,
    company: &CompanyId,
    workflow_id: &str,
    run_id: &str,
    raw_nodes: &Value,
    partial: bool,
) -> bool {
    let Some(store) = store else {
        return true;
    };
    let record = crate::ports::WorkflowRunOutputRecord::from_raw_nodes(
        run_id,
        workflow_id,
        crate::ports::now_millis(),
        raw_nodes,
        partial,
    );
    match store.put_run_output(company, &record).await {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(
                company = %company,
                workflow = %workflow_id,
                %run_id,
                %err,
                "workflow: could not persist the run's per-node output; the run is unaffected"
            );
            false
        }
    }
}

/// The operator-facing notice a notice-bearing arm adds when
/// [`persist_run_output`] could not write the snapshot (issue #1008, Part 3).
///
/// The run's result itself is unaffected — this only warns that reopening the
/// run later may show no output for its nodes, so the black-box silence has a
/// visible cause instead of masquerading as "this run predates output capture".
fn run_output_persist_failed_notice() -> String {
    "This run's per-node output could not be saved, so reopening it later may show no output \
     for some steps. The run itself was unaffected."
        .to_string()
}

/// What a settled run left for the operator to decide.
///
/// Grouped rather than passed as four more parameters because they only make
/// sense together: the gates the engine paused on, the input a continuation has
/// to be started with, what this run already delivered (so approving does not
/// re-send it), and which of those gates the company's policy raised rather
/// than an author (issue #460), so the card can name the call.
struct PausedGates<'a> {
    /// The trigger payload the paused run was started with.
    trigger_input: &'a Value,
    /// The node ids the engine reported on `pending_approvals`.
    pending: &'a [String],
    /// What this run actually routed (issue #438).
    deliveries: &'a [crate::ports::DeliveryReport],
    /// What this run already sent outside the company (issue #846), so
    /// approving replays it rather than repeating it.
    performed: &'a [crate::runtime::workflow_resume::PerformedCall],
    /// The policy-raised gates, so a card can say which tool and **why**. An
    /// authored gate has no entry here — nobody stated a reason — and issue #846
    /// reads its call off the graph instead, so the card still names it.
    gated: &'a [super::gate::GatedCall],
    /// The run's graph, for the authored-gate description above (issue #846).
    graph: &'a tinyflows::model::WorkflowGraph,
    /// Issue #596: the run's reached-node output and the graph's edges, so a
    /// parked gate's card can carry the verbatim upstream content awaiting
    /// sign-off. Additive to the #460 struct — the pre-existing fields are
    /// untouched.
    output: &'a Value,
    edges: &'a [crate::company::WorkflowEdgeDef],
    /// Issue #617: the resolver's per-child gate record, so a namespaced child
    /// gate (`sub::work`) is described from what the gate pass classified
    /// rather than falling back to an unclassified parent-graph lookup.
    child_gates: &'a super::caps::resolver::ChildGateRegistry,
    /// Issue #1862 prerequisite: the paused run's own attribution, stamped on
    /// every card this pass parks so `spawn_continuation` can carry it into the
    /// continuation instead of resetting to `Operator`.
    started_by: &'a crate::ports::types::StartedBy,
    checkpoint_thread_id: &'a str,
    /// The paused graph itself, fingerprinted and stamped on every card this
    /// pass parks so `spawn_continuation` can refuse to resume a stale
    /// checkpoint into a graph an editor changed while the approval sat
    /// pending — see `PAYLOAD_WORKFLOW_FINGERPRINT`.
    workflow: &'a crate::company::WorkflowFile,
}

/// Parks one approval card per gate the run paused on (issue #395).
///
/// # The hole this closes
///
/// A node marked `requires_approval` pauses the run, and the engine reports the
/// gate's node id on `RunOutcome::pending_approvals`. Those ids flowed into
/// exactly two places — the run route's HTTP response and the
/// `WorkflowRunFinished` journal line — and **neither is an approval**. The
/// Approvals page reads the journal's parked [`Effect`](crate::ports::types::Effect)s,
/// so it was empty by construction: the run paused, the ids were recorded as
/// trivia, and nothing an operator could act on ever existed. That is why a QA
/// run with a `requires_approval` node left the page reading "All clear".
///
/// # Best-effort, and never fails the run
///
/// The engine has already settled by the time this runs; the graph's work is
/// done and correct whatever happens here. A park that fails is logged loudly —
/// it is the only trace of a decision the operator will never be asked for —
/// and the next gate is still attempted. Failing the run instead would discard
/// a completed run's output over an approvals-queue write.
///
/// # Dedupe
///
/// A run is re-runnable, and resuming one is itself a re-run, so the same gate
/// on the same input will be reached again and again. Without a dedupe the
/// queue fills with identical cards for one decision — which is exactly how an
/// operator learns to rubber-stamp the queue. Identity is the gate, not the run;
/// see [`already_parked`](crate::runtime::workflow_resume::already_parked).
///
/// # The delivery ledger (issue #438)
///
/// `deliveries` is what this run actually routed, and it rides the card so the
/// continuation an approval starts knows what has already left the process.
/// Without it, approving a gate re-mails every report upstream of it — the
/// re-run semantics above applied to a side effect that reaches a real person.
/// Stashes the facts each blocked agent node's continuation needs — the workflow
/// id and this run's trigger input — keyed by the per-(run, node) turn key its
/// gated calls armed `ContinuationQueue` under at park time (issue #899, Stage 1).
///
/// # Why the durable mirror is still written here, not where the calls are parked
///
/// The in-memory arm itself now happens where the calls are parked —
/// `HarnessAgentRunner` is built with the run's trigger input (issue #1825, P1
/// follow-up) precisely so `park_gated_calls` can call `arm` before a card is
/// journaled and clickable, closing the race this function's own comment above
/// describes. What that call site does *not* have is the settled `blocked`
/// list — a node only shows up there once the engine has decided the run
/// stopped for it — so the durable journal record still has to be written
/// from here, once per this run's whole batch of blocked nodes, exactly as
/// `park_pending_gates` is the one place a gate's facts come together. A node
/// with no parked approval id is skipped: nothing can be decided, so nothing
/// will ever release a stash.
///
/// A build with no approvals queue wired stashes nothing and is silent — the
/// same node already logged its own "could not be parked" line, and there is no
/// resolve path to release a stash to.
#[cfg(test)]
async fn stash_blocked_agent_nodes(
    delivery: Option<&super::delivery::WorkflowDeliveryDeps>,
    workflow_id: &str,
    run_id: &str,
    trigger_input: &serde_json::Value,
    blocked: &[crate::ports::WorkflowBlockedNode],
    started_by: &crate::ports::types::StartedBy,
) {
    stash_blocked_agent_nodes_checkpointed(
        delivery,
        workflow_id,
        run_id,
        trigger_input,
        blocked,
        started_by,
        None,
        None,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn stash_blocked_agent_nodes_checkpointed(
    delivery: Option<&super::delivery::WorkflowDeliveryDeps>,
    workflow_id: &str,
    run_id: &str,
    trigger_input: &serde_json::Value,
    blocked: &[crate::ports::WorkflowBlockedNode],
    started_by: &crate::ports::types::StartedBy,
    checkpoint_thread_id: Option<&str>,
    workflow_fingerprint: Option<&str>,
) {
    let Some(parking) = delivery.and_then(|delivery| delivery.parking.as_ref()) else {
        return;
    };

    // Issue #1816 (Stage 2 follow-up): arm every eligible node's in-memory
    // stash BEFORE awaiting any durable journal write. This settle can name
    // several blocked nodes at once (a fan-out that pauses on more than one
    // gate), and their approval cards are already parked and clickable from
    // agent execution — an operator can act on any of them the instant this
    // function starts. The old loop interleaved the synchronous `arm()` with
    // an awaited `record_blocked_node_stashed()` call per node, which opens a
    // window, per node, where that node's card is clickable but its stash is
    // not armed yet: a decision landing in that window finds nothing to
    // release, consumes the approval anyway, and a later arm for that same
    // turn then stashes facts with no remaining decision to release them —
    // stranding the approved run. Arming every node up front (a purely
    // in-memory, non-awaiting pass) closes that window for the whole batch at
    // once instead of leaving it open per node; only the durable mirroring
    // below still awaits.
    //
    // Issue #1825 (P1 follow-up): `arm` below is now a no-op for every node in
    // `blocked` whose calls actually parked — `HarnessAgentRunner::park_gated_calls`
    // arms the same stash itself, at node park time, before the first card is
    // journaled and clickable (see `crate::runtime::blocked_nodes`'s module
    // doc). This function ran only after the agent returned and the engine
    // settled, which — even with the synchronous pass above — was still after
    // every card for this run's blocked nodes had gone live; an operator fast
    // enough to approve inside that outer window hit the same empty-stash race
    // the paragraph above closed for the inner one. The pass stays here
    // because it is still the only place with the full settled `blocked` list
    // the durable mirror below needs, and `arm`'s first-write-wins semantics
    // make the redundant call free.
    //
    // Issue #1825 (P2, third follow-up — found by chatgpt-codex-connector):
    // "free" stopped being true the moment park time started performing both
    // arms. Every node reaching this filter has a non-empty `approval_ids`,
    // which `park_gated_calls` only ever populates *after* its own
    // unconditional arm has already run for this exact turn — so `is_armed`
    // being false here cannot mean "never armed"; the only way to get here
    // is a decision that resolved the node's whole batch and released the
    // stash before this settle pass got to it (an operator fast enough to
    // decide the *last* card in the window between the agent returning and
    // this function running). Re-arming a released turn resurrects it with
    // no decision left to redeem it, and the durable write two lines down
    // would then append a `BlockedNodeStashed` *after* the `BlockedNodeReleased`
    // that already retired it — durable on replay, and a late approval-bank
    // retry landing on the resurrection can mark it approved and have a
    // future boot's `reconcile_stranded_blocked_nodes` dispatch the run a
    // second time. Filtering on `is_armed` here, before either write, is
    // cheap and catches it before either arm happens rather than sweeping up
    // after.
    let turns: Vec<_> = blocked
        .iter()
        .filter(|node| !node.approval_ids.is_empty())
        .filter_map(|node| {
            let turn =
                crate::runtime::workflow_resume::workflow_node_turn_key(run_id, &node.node_id);
            if !parking.blocked_nodes.is_armed(&turn) {
                tracing::info!(
                    %workflow_id,
                    %run_id,
                    node = %node.node_id,
                    "[approval] skipping this blocked node's settle-time stash: it is no \
                     longer armed, meaning its whole batch was already decided and released \
                     before this settle pass ran — re-arming it now would resurrect a turn \
                     with nothing left to redeem it"
                );
                return None;
            }
            parking.blocked_nodes.arm_checkpointed(
                &turn,
                workflow_id,
                trigger_input,
                started_by,
                checkpoint_thread_id,
                workflow_fingerprint,
            );
            Some((turn, &node.node_id))
        })
        .collect();

    for (turn, node_id) in turns {
        // Issue #1825 (P2, fourth follow-up — found by chatgpt-codex-connector):
        // the `is_armed` check above ran once, synchronously, before this loop
        // awaited anything — it only proves each turn was still live at the
        // moment the whole batch was collected. For every turn but the first,
        // a sibling's own awaited durable write (right below, one iteration
        // ago) gives a landing decision time to run `retire_blocked_stash`
        // in between: that clears this turn's in-memory stash and appends its
        // `BlockedNodeReleased` before this iteration ever reaches it. Without
        // this re-check, the write below would then append a `BlockedNodeStashed`
        // behind that terminal record — durable on replay — resurrecting an
        // already-dispatched turn for a future boot's `reconcile_stranded_blocked_nodes`
        // to redeem a second time. Re-checking immediately before the write
        // narrows the window to the same shape every other park-time write in
        // this module already accepts (see the P1 fourth follow-up's own
        // residual in `caps::park_gated_calls`), not the whole width of this
        // batch's sibling I/O.
        if !parking.blocked_nodes.is_armed(&turn) {
            tracing::info!(
                %workflow_id,
                %run_id,
                node = %node_id,
                "[approval] skipping this blocked node's durable stash write: a decision \
                 released it while an earlier sibling in this same settle batch was still \
                 being durably written — re-appending now would resurrect an already-dispatched \
                 turn"
            );
            continue;
        }
        // Issue #1816 (Stage 2): mirror the in-memory arm into the durable
        // journal so an approval landing after a process/host replacement can
        // still locate this run. Best-effort — a failed write leaves the
        // in-memory stash serving the no-restart case, and failing the settled
        // run over an approvals-queue write is the wrong trade (same stance as
        // the park itself). The in-memory arm itself (with `started_by`) already
        // happened in the synchronous pass above that built `turns`.
        if let Err(error) = parking
            .journal
            .record_blocked_node_stashed_checkpointed(
                &turn,
                workflow_id,
                trigger_input,
                started_by,
                checkpoint_thread_id,
                workflow_fingerprint,
            )
            .await
        {
            tracing::warn!(
                %workflow_id,
                %run_id,
                node = %node_id,
                %error,
                "[approval] a blocked node's continuation facts could not be durably \
                 stashed; the in-memory stash still covers a resolve without a restart"
            );
        }
    }
}

async fn park_pending_gates(
    delivery: Option<&super::delivery::WorkflowDeliveryDeps>,
    record: &CompanyRecord,
    workflow_id: &str,
    run_id: &str,
    paused: PausedGates<'_>,
) {
    let PausedGates {
        trigger_input,
        pending,
        deliveries,
        performed,
        gated,
        graph,
        // Issue #596: the reached-node output + the graph's edges, so each parked
        // gate's card can carry the verbatim upstream content awaiting sign-off.
        output,
        edges,
        // Issue #617: the resolver's per-child gate record, for a namespaced
        // child gate's card.
        child_gates,
        started_by,
        checkpoint_thread_id,
        workflow,
    } = paused;
    if pending.is_empty() {
        return;
    }
    // Issue #1991 review: computed once per park pass (not per run) — hashed
    // only once this run is actually known to be pausing on a gate.
    let workflow_fingerprint = workflow.content_fingerprint();
    let Some(parking) = delivery.and_then(|delivery| delivery.parking.as_ref()) else {
        // Fails closed and loud, the same stance `deliver_outputs` takes for an
        // unwired destination: the run genuinely paused, and on this build
        // nobody can be asked to un-pause it.
        tracing::error!(
            company = %record.id,
            workflow = %workflow_id,
            %run_id,
            gates = pending.len(),
            "workflow: the run paused for approval but this runtime has no approvals queue \
             wired, so the gates cannot be parked — the run cannot be continued"
        );
        return;
    };

    // Issue #978: the gates this lineage has already refused. A denied node is
    // decided and final — replaying into it must not raise the question a second
    // time, or a mixed verdict still nets new cards and "approving never
    // increases pending approvals" is false again.
    let denied = crate::runtime::workflow_resume::denied_in_input(trigger_input);

    // Issue #978: every gate this run parks shares ONE turn key, so the N of a
    // fan-out are one decision batch owed exactly one continuation. Keyed on the
    // run because the run is what gets re-dispatched.
    let turn = crate::runtime::workflow_resume::workflow_turn_key(run_id);

    // Issue #1825 (P1, found by chatgpt-codex-connector): hold this turn's
    // `ContinuationQueue` counter open across the whole loop below, the same
    // shape of fix `park_gated_calls`'s fourth follow-up applied in
    // `workflows::caps` for the identical race.
    //
    // The loop below parks this run's gates one at a time, and each successful
    // `park_and_journal` arms the counter for its own card (issue #469/#978's
    // per-card mechanism, unchanged, and closed against its OWN in-flight
    // window by the fifth follow-up above `park_and_journal`). With no hold
    // here, an operator can resolve an EARLIER card — already parked, already
    // counted — while a LATER card in this loop is still being attempted. If
    // that later park then fails, `park_and_journal`'s own error branch
    // releases the slot it armed for it; when that release happens to be the
    // batch's last decrement, the batch it hands back — every sibling decided
    // while this loop was still running — is dropped inside that failure
    // branch, with no caller left to route it through `resume_workflow_run`.
    // The already-decided siblings are not lost from the approval journal
    // (each was resolved and recorded independently of this counter); what is
    // lost is the automatic re-dispatch, silently, with nothing telling the
    // operator the run is now stranded.
    //
    // The hold pins outstanding at least 1 above the count of decided cards
    // until every gate here has actually been attempted, so no single card's
    // own arm/release pair — including a failed one's — can ever be the
    // batch's last word while a sibling is still in flight. Released below,
    // once the loop is done.
    //
    // Skipped for a single-gate run: there is no "rest of the batch" to
    // protect against, and holding would only insert an extra decrement
    // between that lone card's approval and its release.
    let holds_continuation = pending.len() > 1;
    if holds_continuation {
        parking.continuations.arm(&turn);
    }

    for node_id in pending {
        if denied.iter().any(|refused| refused == node_id) {
            tracing::info!(
                company = %record.id,
                workflow = %workflow_id,
                node = %node_id,
                %run_id,
                "workflow: this gate was already refused, so it is not asked about again"
            );
            continue;
        }
        // Issue #460: when the policy is what stopped this node, the card says
        // which tool and why.
        //
        // Issue #846: when the **author** stopped it, the card still says which
        // tool — read off the graph, which has known the node's slug and
        // arguments all along. Only the reason is policy-specific, and it is the
        // one thing an authored gate genuinely does not have. Falling back rather
        // than merging: a policy-raised gate already carries the same call, so
        // consulting the graph for it would be a second answer to a question that
        // already has one.
        let described;
        let gate = match gated.iter().find(|gate| gate.node_id == *node_id) {
            Some(gate) => Some(gate),
            None => {
                // Issue #617: a namespaced id (`sub::work`, nested one level
                // per child as `sub::nested::work`) names a gate inside a child
                // the resolver ran. The parent graph has no node with that id,
                // so describe it from the child's own gate record, descending
                // the registry through the `sub_workflow` nodes — resolving an
                // expression-bound `workflow_id` against the trigger input
                // where the engine's `once` scope allows — so the child's tool
                // and reason reach the card the way a top-level policy gate's
                // do. Falls back to the parent-graph read for every other gate
                // (an authored child gate, which the registry never sees, keeps
                // its pre-existing behaviour).
                described = super::caps::resolver::child_gate_call(
                    child_gates,
                    graph,
                    node_id,
                    Some(trigger_input),
                )
                .or_else(|| super::gate::describe_call(graph, node_id));
                described.as_ref()
            }
        };
        let call = gate.map(|gate| crate::runtime::workflow_resume::GateCall {
            tool: gate.slug.as_str(),
            // Empty means "nobody wrote one", which `describe_call` documents;
            // the key is then absent from the payload rather than present and
            // blank, so a console can tell an unstated reason from an empty one.
            reason: Some(gate.reason.as_str()).filter(|reason| !reason.is_empty()),
            args: Some(&gate.args),
            target: gate.target.as_deref(),
        });
        let mut effect = crate::runtime::workflow_resume::gate_effect(
            workflow_id,
            node_id,
            trigger_input,
            run_id,
            deliveries,
            performed,
            call,
        );
        // Issue #596: enrich the card with the verbatim output of this gate's
        // upstream nodes — the content awaiting sign-off. A self-contained
        // addition on top of the effect `gate_effect` already built; the dedupe
        // below keys on explicit payload keys only (NOT this content), so two
        // parks differing only in content still collapse to one card.
        crate::runtime::workflow_resume::attach_upstream_content(
            &mut effect,
            output,
            edges,
            node_id,
        );
        // Issue #1862 prerequisite: stamp the paused run's own attribution on
        // the card, so approving it can carry the same `StartedBy` into the
        // continuation instead of resetting to `Operator` (see
        // `started_by_of`/`spawn_continuation`). Outside `is_same_gate`'s
        // dedupe identity, same as the ledgers below it — the decision is the
        // same decision however it later gets re-parked.
        if let Value::Object(ref mut payload) = effect.payload {
            payload.insert(
                crate::runtime::workflow_resume::PAYLOAD_STARTED_BY.to_string(),
                serde_json::json!(started_by),
            );
            payload.insert(
                crate::runtime::workflow_resume::PAYLOAD_THREAD_ID.to_string(),
                serde_json::json!(checkpoint_thread_id),
            );
            payload.insert(
                crate::runtime::workflow_resume::PAYLOAD_WORKFLOW_FINGERPRINT.to_string(),
                serde_json::json!(workflow_fingerprint),
            );
        }
        if crate::runtime::workflow_resume::already_parked(&parking.journal, &effect) {
            tracing::debug!(
                company = %record.id,
                workflow = %workflow_id,
                node = %node_id,
                "workflow: this gate is already waiting on the operator; not asking twice"
            );
            continue;
        }
        // A workflow run has no board card behind it and no conversation to
        // raise the request in — the same two facts the delivery park records
        // (#333, #379).
        match parking
            .park_and_journal(
                &record.id,
                effect,
                crate::runtime::journal::TaskLink::Unlinked,
                None,
                Some(turn.clone()),
            )
            .await
        {
            Ok(approval_id) => tracing::info!(
                company = %record.id,
                workflow = %workflow_id,
                node = %node_id,
                %run_id,
                %approval_id,
                "workflow: parked a paused gate for operator approval; approving it starts a \
                 continuation run"
            ),
            Err(err) => tracing::error!(
                company = %record.id,
                workflow = %workflow_id,
                node = %node_id,
                %run_id,
                %err,
                "workflow: a paused gate could NOT be parked for approval; the run cannot be \
                 continued"
            ),
        }
    }

    // Issue #1825 (P1): release the hold armed above, now that every gate in
    // this run has actually been attempted — whether it parked or failed.
    // `Some(batch)` back means this release was itself the batch's last
    // decision: every card this loop parked was already decided by the time
    // the loop finished attempting the rest, which needs an operator (or an
    // API caller) faster than this function's own sequential parks.
    //
    // `park_pending_gates` runs deep inside the engine with no handle back to
    // the `CompanyRuntime` that owns `resume_workflow_run` — the only place
    // that spawns this run's continuation — short of re-entering this run's
    // own execution while it is still mid-run, which is a worse hazard than
    // the one this hold exists to close (a duplicate dispatch, just moved).
    // So, exactly like `park_gated_calls`'s own release in `workflows::caps`,
    // this rare batch is left exactly as the loop above already left it: every
    // decision durably recorded in the approval journal, independent of this
    // counter. Only the automatic re-dispatch is deferred — and unlike a
    // blocked agent node, a workflow run has no boot-time reconciliation sweep
    // of its own yet, so nothing recovers this automatically; the operator has
    // to notice the run is still `Blocked` and re-run it. An empty batch
    // (every gate below failed to park, so nothing was ever decided) needs no
    // warning — there is nothing stranded.
    if holds_continuation
        && let Some(batch) = parking.continuations.decide(&turn, None)
        && !batch.is_empty()
    {
        tracing::warn!(
            company = %record.id,
            workflow = %workflow_id,
            %run_id,
            decisions = batch.len(),
            "workflow: every gate this run parked was already decided before the rest of the \
             batch finished parking; the approvals are recorded but the run will not \
             auto-resume — re-run the workflow to pick it back up"
        );
    }
}

/// What a run stopped by an operator settles with on the **hard-abort** arm
/// (issue #383/#398) — the wedged-node path, where the engine future was dropped
/// because the run did not wind down within `CANCEL_HARD_ABORT_GRACE`.
///
/// (A run that stopped **cleanly** at a node boundary does not come here: it has
/// a real partial outcome and is settled inline in `run_workflow_inner`, carrying
/// its collected node rows. This is only the dropped-future case.)
///
/// Empty on every field but the flag and the two the caller threads in, and each
/// emptiness is a claim rather than a shrug:
///
/// * **no `output`** — the engine future was dropped, so there is no final state
///   to report. A partial one would be a new shape nothing downstream parses;
/// * **no `deliveries`** — `deliver_outputs` is deliberately not called. A
///   cancelled run must not email anybody a report of work it did not finish,
///   and an absent row already means "not reached" everywhere else;
/// * **no `pending_approvals`** — approvals earlier nodes already parked are
///   journal-backed and independent of the run, so they stay in the queue and
///   an operator may still approve or deny them. Listing them here would imply
///   this run is still waiting on them, which it is not.
///
/// # The two arguments are the exceptions, and they are the point
///
/// `notices` and `board` are **threaded in rather than emptied** (issue #661 /
/// M5). Everything above is empty because it describes the run's *result*, which
/// a dropped future does not have. These two describe what its nodes already
/// **did** before it wedged, and both are durable facts by the time this is
/// reached: a notice records tool calls that were already refused, and a board row
/// records a card that is already on the operator's board. Emptying them would
/// leave a card nothing admits to opening — which is why this signature changed
/// instead of the constructor keeping its convenient `Vec::new()`s. (`notices` was
/// dropped here before, silently; that is fixed by the same change.)
fn cancelled_run(
    notices: Vec<String>,
    board: Vec<crate::ports::WorkflowRunBoardRow>,
    approvals: Vec<crate::ports::WorkflowRunApprovalRow>,
) -> WorkflowRun {
    WorkflowRun {
        output: Value::Null,
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        cancelled: true,
        // Empty for the same reason as the fields above: a stopped run reports
        // no result. Its completed nodes were still journaled as they finished
        // (the drain runs before this returns), so "how far did it get?" is
        // answered by the history, not by this settled body.
        nodes: Vec::new(),
        notices,
        board,
        // Issue #881: a hard abort drops the engine future mid-await, so no
        // node ever reported a block — and a block is reported only by a node
        // that finished its turn. Empty by construction, not by omission.
        blocked_nodes: Vec::new(),
        // Issue #880: threaded in, NOT emptied, for exactly the reason `board`
        // is. An approval card is durable the moment it is written, so a run
        // that parked two and was then hard-aborted really did park them —
        // zeroing the rows would leave two cards on the operator's Approvals
        // page that no run admits to opening.
        approvals,
    }
}

/// Maps a tinyflows [`EngineError`](tinyflows::error::EngineError) onto the crate
/// error: a structural validation failure is a caller-facing bad request; every
/// other engine/capability failure is a harness error.
fn map_engine_error(err: tinyflows::error::EngineError) -> OpenCompanyError {
    use tinyflows::error::EngineError;
    match err {
        EngineError::Validation(v) => {
            OpenCompanyError::InvalidRequest(format!("workflow graph is invalid: {v}"))
        }
        other => OpenCompanyError::Harness(other.to_string()),
    }
}

/// The [`WorkflowRunner`] port backed by the embedded harness: it holds the
/// lane-aware router so every agent node dispatches to the engine its harness
/// demands, plus the deps and company record it needs to warm every lane before
/// a run.
pub struct HarnessWorkflowRunner {
    turn: Arc<dyn crate::runtime::delegation::RunTurn>,
    deps: HarnessDeps,
    checkpoint_store: Option<Arc<super::checkpoint_store::WorkflowCheckpointStore>>,
    /// The company record as of this runner's construction (runtime build /
    /// rebuild). The run re-reads the live record from the store so a console
    /// policy PUT since then reaches the run's gate; this snapshot is the
    /// fallback when the store is unwired or the row is gone.
    record: CompanyRecord,
}

impl HarnessWorkflowRunner {
    /// Builds a runner dispatching through `turn`, sharing `deps` with the
    /// rest of the harness surface for the company described by `record`.
    pub fn new(
        turn: Arc<dyn crate::runtime::delegation::RunTurn>,
        deps: HarnessDeps,
        record: CompanyRecord,
    ) -> Self {
        Self {
            turn,
            deps,
            checkpoint_store: None,
            record,
        }
    }

    pub fn with_checkpoint_store(
        mut self,
        checkpoint_store: Arc<super::checkpoint_store::WorkflowCheckpointStore>,
    ) -> Self {
        self.checkpoint_store = Some(checkpoint_store);
        self
    }

    /// The effective record for a run: the store's current one when it can be
    /// read, the build-time snapshot otherwise.
    ///
    /// Issue #1455: the snapshot this runner was built with carries the policy
    /// as of runtime build / rebuild. The cycle refreshes its record at the top
    /// of every cycle, but a workflow run is dispatched outside that cadence —
    /// an operator who tightens the spend cap and then starts an authored
    /// workflow without rebuilding the runtime would otherwise have the run's
    /// tool-call gate classified under the *old* cap, auto-approving what the
    /// new cap would park. Re-reading here keeps the graph-level gate on the
    /// same policy the roster rebuilds against. A store fault falls back to the
    /// snapshot rather than failing the run, preserving the pre-#1455
    /// behaviour of never touching the store on this path.
    async fn effective_record(&self) -> Result<CompanyRecord> {
        Ok(match self.deps.store.load(&self.record.id).await {
            Ok(Some(record)) => record,
            Ok(None) | Err(_) => self.record.clone(),
        })
    }
}

#[async_trait]
impl WorkflowRunner for HarnessWorkflowRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        workflow: &WorkflowFile,
        input: Value,
        ctx: &WorkflowRunContext,
    ) -> Result<WorkflowRun> {
        // Issue #1455: a console policy PUT/DELETE since this runner was built
        // must reach this run's gate and the roster it warms. The store's live
        // record carries the current overlay; the snapshot is only the fallback.
        let record = self.effective_record().await?;
        // Idempotent: builds the roster on first use, a no-op after. Warmed
        // through the router so every lane's pool — not just the default's — is
        // populated before a node addresses it. The run addresses the record's
        // own company; `_company` is the routed scope, which the runtime
        // resolves to this same record.
        self.turn.ensure(&record).await?;
        run_workflow_lane_aware_checkpointed(
            self.turn.clone(),
            self.deps.clone(),
            &record,
            workflow,
            input,
            ctx,
            self.checkpoint_store.clone(),
        )
        .await
    }
}

#[cfg(test)]
#[path = "runner_cancel_delivery_tests.rs"]
mod tests_cancel_delivery;
#[cfg(test)]
#[path = "runner_capped_halt_tests.rs"]
mod tests_capped_halt;
#[cfg(test)]
#[path = "runner_checkpoint_cancel_tests.rs"]
mod tests_checkpoint_cancel;
#[cfg(test)]
#[path = "runner_delivery_gate_tests.rs"]
mod tests_delivery_gate;
#[cfg(test)]
#[path = "runner_dry_run_tests.rs"]
mod tests_dry_run;
#[cfg(test)]
#[path = "runner_journal_tests.rs"]
mod tests_journal;
#[cfg(test)]
#[path = "runner_node_kinds_tests.rs"]
mod tests_node_kinds;
#[cfg(test)]
#[path = "runner_node_output_tests.rs"]
mod tests_node_output;
#[cfg(test)]
#[path = "runner_reclassify_tests.rs"]
mod tests_reclassify;
#[cfg(test)]
#[path = "runner_settle_tests.rs"]
mod tests_settle;
#[cfg(test)]
#[path = "runner_sub_workflow_tests.rs"]
mod tests_sub_workflow;
