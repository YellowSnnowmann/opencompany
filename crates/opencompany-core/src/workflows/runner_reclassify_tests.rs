use super::*;

use crate::company::parse_workflow;
use crate::ports::run_output::WorkflowRunOutputStore;
use crate::store::FsOps;

/// One node row, for the reclassification tests below — the three
/// structural scalars only, matching what `reclassify_capped_nodes` and
/// `reclassify_blocked` both read and write.
fn node_row(id: &str, status: WorkflowNodeStatus) -> crate::ports::WorkflowRunNodeRow {
    crate::ports::WorkflowRunNodeRow {
        node_id: id.to_string(),
        status,
        elapsed_ms: 10,
        diagnostics: Vec::new(),
    }
}

/// The third half of that reconciliation, and the one that was missing
/// (CodeRabbit review on #1905): what the **journal** records.
///
/// `reclassify_capped_nodes` below only ever reached the in-memory
/// `WorkflowRun.nodes`, so a capped node's durable `WorkflowNodeFinished`
/// kept the engine's `Ok`. `GET /workflows/runs` folds its rows from those
/// events, so the same run read back scored `ok` while the synchronous
/// response said `degraded` — one run, two verdicts, depending on which
/// surface you asked. The collector now consults `RunCappedNodes` before it
/// writes, so the event carries the relabelled status and both surfaces
/// derive the verdict from the same fact.
///
/// Pinned on `RunCappedNodes::contains` rather than by driving a whole run:
/// the read is the entire mechanism, and it has to answer without draining
/// — `take` would leave the settle-time relabel with an empty list, which
/// is the one way to "fix" history and break the live path instead.
#[test]
fn the_capped_read_answers_without_draining_the_settle_time_list() {
    let capped = super::super::caps::RunCappedNodes::default();
    capped.push("summarize".to_string());

    assert!(capped.contains("summarize"), "the journal write asks first");
    assert!(!capped.contains("fetch"), "and only about its own node");
    assert!(
        capped.contains("summarize"),
        "asking must not consume it — the settle-time relabel comes after"
    );

    let mut nodes = vec![node_row("summarize", WorkflowNodeStatus::Ok)];
    reclassify_capped_nodes(&mut nodes, &capped.take());
    assert_eq!(
        nodes[0].status,
        WorkflowNodeStatus::Error,
        "the in-memory row still gets its flip, so the two surfaces agree"
    );
}

/// Issue #1865: the run-level half of the iteration-cap reconciliation —
/// `caps::mod`'s own test
/// (`a_capped_turn_settles_failed_and_feeds_run_capped_nodes`) pins that a
/// capped turn feeds the node id into `RunCappedNodes`; this pins that
/// `reclassify_capped_nodes` turns that id into the row flip the run's
/// verdict needs (`WorkflowRunVerdict::of` reads `Error`, never a node
/// id list).
#[test]
fn reclassify_capped_nodes_flips_the_capped_row_to_error() {
    let mut nodes = vec![
        node_row("fetch", WorkflowNodeStatus::Ok),
        node_row("summarize", WorkflowNodeStatus::Ok),
    ];
    reclassify_capped_nodes(&mut nodes, &["summarize".to_string()]);
    assert_eq!(nodes[0].status, WorkflowNodeStatus::Ok, "untouched sibling");
    assert_eq!(
        nodes[1].status,
        WorkflowNodeStatus::Error,
        "the capped node's row must read Error, agreeing with its attempt"
    );
}

/// An empty capped list is a no-op — every row keeps whatever status the
/// engine (or `reclassify_blocked`) already gave it. The common case: most
/// runs cap no node at all.
#[test]
fn reclassify_capped_nodes_is_a_no_op_when_nothing_capped() {
    let mut nodes = vec![
        node_row("fetch", WorkflowNodeStatus::Ok),
        node_row("gate", WorkflowNodeStatus::Blocked),
    ];
    let before = nodes.clone();
    reclassify_capped_nodes(&mut nodes, &[]);
    assert_eq!(nodes, before);
}

/// The two reclassifications are structurally exclusive (a blocked node's
/// turn returns `Err` before the iteration-cap check is ever reached — see
/// `run_turn`'s `#881` block above the cap check), so this can never fire
/// against a real run. The guard is defensive anyway: a node the blocked
/// pass already relabelled must never be re-flipped by this one, because
/// `Blocked` is the more specific fact — a future caller that somehow
/// named one node in both lists must not have this hide a real approval
/// wait behind a plain failure.
#[test]
fn reclassify_capped_nodes_never_overrides_an_already_blocked_row() {
    let mut nodes = vec![node_row("gate", WorkflowNodeStatus::Blocked)];
    reclassify_capped_nodes(&mut nodes, &["gate".to_string()]);
    assert_eq!(
        nodes[0].status,
        WorkflowNodeStatus::Blocked,
        "a blocked node must never be relabelled Error"
    );
}

/// Coderabbit review on #1990: when `retry.max_attempts > 1`, tinyflows can
/// retry a node after its judge answered `halt_benign` on an earlier
/// attempt. If that retry itself hits the iteration cap, `RunCappedNodes`
/// picks up the same node id `reclassify_halted_nodes` already relabelled
/// `Declined` — and without this guard, `Blocked` was the only status this
/// function refused to override, so it would flip a correct benign-stop row
/// to `Error` and raise a false failure notice for a node that already
/// settled its more specific, correct fact.
#[test]
fn reclassify_capped_nodes_never_overrides_an_already_declined_row() {
    let mut nodes = vec![node_row("verify", WorkflowNodeStatus::Declined)];
    reclassify_capped_nodes(&mut nodes, &["verify".to_string()]);
    assert_eq!(
        nodes[0].status,
        WorkflowNodeStatus::Declined,
        "a benign-halt row must never be relabelled Error"
    );
}

#[test]
fn reclassify_halted_nodes_marks_only_the_benign_stop_declined() {
    let mut nodes = vec![
        node_row("prepare", WorkflowNodeStatus::Ok),
        node_row("verify", WorkflowNodeStatus::Error),
    ];
    reclassify_halted_nodes(&mut nodes, &["verify".to_string()]);
    assert_eq!(nodes[0].status, WorkflowNodeStatus::Ok);
    assert_eq!(nodes[1].status, WorkflowNodeStatus::Declined);
}

#[test]
fn a_halt_does_not_hide_an_unrelated_failure() {
    let nodes = vec![
        node_row("optional", WorkflowNodeStatus::Error),
        node_row("broken", WorkflowNodeStatus::Error),
    ];
    assert!(!only_expected_nodes_errored(
        &nodes,
        &[],
        &["optional".to_string()]
    ));
}

/// A workflow lane that records which agent it served. Its reply names the
/// lane so the run output proves the same routing decision as the call log.
pub(super) struct RecordingLane {
    pub(super) label: &'static str,
    pub(super) seen: std::sync::Mutex<Vec<String>>,
}

impl RecordingLane {
    pub(super) fn new(label: &'static str) -> Arc<Self> {
        Arc::new(Self {
            label,
            seen: std::sync::Mutex::new(Vec::new()),
        })
    }
}

/// A full workflow-node turn double that writes a real sandbox file before
/// either failing or parking an approval. It drives the real engine,
/// capability, mirror, output-store, and workspace-store seams; only model
/// inference is replaced.
struct ArtifactWritingTurn {
    workspace_root: std::path::PathBuf,
    approvals: crate::harness::policy::ApprovalRequestQueue,
    blocked: bool,
}

impl ArtifactWritingTurn {
    async fn execute(
        &self,
        company: &CompanyId,
        agent_id: &str,
    ) -> Result<crate::harness::TurnOutcome> {
        let workspace =
            crate::harness::build::agent_workspace(&self.workspace_root, company, agent_id);
        let report = workspace.join("reports/partial.md");
        tokio::fs::create_dir_all(report.parent().expect("report parent")).await?;
        tokio::fs::write(&report, b"# Partial report\n\nCaptured before settle.\n").await?;

        if !self.blocked {
            return Err(OpenCompanyError::Harness(
                "synthetic node failure after writing its file".to_string(),
            ));
        }

        self.approvals
            .push(crate::harness::policy::ApprovalRequest {
                tool: "shell".to_string(),
                reason: "synthetic approval after writing".to_string(),
                effect: crate::ports::types::Effect {
                    kind: "shell".to_string(),
                    group: crate::ports::types::EffectGroup::Other,
                    amount_usd: None,
                    established_thread: false,
                    first_time_counterparty: false,
                    payload: serde_json::json!({ "command": "finish-report" }),
                    agent: Some(agent_id.to_string()),
                    run_id: None,
                },
            });
        Ok(crate::harness::TurnOutcome {
            reply: "Waiting for approval.".to_string(),
            steps: Vec::new(),
            hit_iteration_cap: false,
            // Test fixture, not the ACP fold (PR #1880 review).
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        })
    }
}

#[async_trait]
impl crate::runtime::delegation::RunTurn for ArtifactWritingTurn {
    async fn run(
        &self,
        company: &CompanyId,
        agent_id: &str,
        _message: &str,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.execute(company, agent_id).await
    }

    async fn run_steered(
        &self,
        company: &CompanyId,
        agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.execute(company, agent_id).await
    }

    async fn run_steered_background(
        &self,
        company: &CompanyId,
        agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.execute(company, agent_id).await
    }
}

fn artifact_graph() -> WorkflowFile {
    parse_workflow(
        r#"
id = "artifact_capture"
name = "Artifact capture"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "work"
kind = "agent"
name = "Work"
summary = "Write a report."
agent = "ceo"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "work"
[[edge]]
from = "work"
to = "done"
"#,
    )
    .expect("artifact graph parses")
}

async fn assert_partial_run_artifact(blocked: bool) {
    use crate::ports::workspace::WorkspaceStore;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(FsOps::new(dir.path()));
    let (mut deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(
        "http://127.0.0.1:1/unused".to_string(),
        dir.path(),
    );
    deps.workspace = Some(store.clone());
    deps.run_output_store = Some(store.clone());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ArtifactWritingTurn {
        workspace_root: deps.workspace_root.clone(),
        approvals: deps.approval_requests.clone(),
        blocked,
    });
    let ctx = WorkflowRunContext::new(false);

    let result = run_workflow_lane_aware(
        turn,
        deps,
        &record,
        &artifact_graph(),
        serde_json::json!({ "request": "make the report" }),
        &ctx,
    )
    .await;
    if blocked {
        let run = result.expect("an approval-blocked run settles successfully");
        assert!(
            run.blocked_nodes.iter().any(|node| node.node_id == "work"),
            "the synthetic approval must block work: {run:?}"
        );
    } else {
        assert!(result.is_err(), "the synthetic failure must fail the run");
    }

    let stored = store
        .get_run_output(&record.id, &ctx.run_id)
        .await
        .expect("run-output read")
        .expect("failed and blocked runs both persist partial output");
    assert!(stored.partial, "capture must be marked partial: {stored:?}");
    let artifact = &stored.nodes["work"]["artifacts"][0];
    assert_eq!(artifact["source"], "reports/partial.md");
    let node_id = artifact["workspaceNodeId"]
        .as_str()
        .expect("capture links a workspace node");
    let (node, body) = WorkspaceStore::read(store.as_ref(), &record.id, node_id)
        .await
        .expect("workspace read")
        .expect("mirrored run artifact exists");
    assert_eq!(node.name, "partial.md");
    assert!(
        body.contains("Captured before settle"),
        "the mirrored node keeps the written body: {body:?}"
    );
}

#[tokio::test]
async fn a_failed_agent_node_keeps_the_file_it_wrote_as_a_run_artifact() {
    assert_partial_run_artifact(false).await;
}

#[tokio::test]
async fn a_blocked_agent_node_keeps_the_file_it_wrote_as_a_run_artifact() {
    assert_partial_run_artifact(true).await;
}

/// A genuinely failed checkpointed run has no continuation path — only an
/// approval or blocked-node resume reuses a run's thread id, and neither
/// applies to a plain failure — so its checkpoint lineage must be pruned
/// the same as a clean settle or a cancel, or it accumulates on disk
/// forever.
#[tokio::test]
async fn a_genuinely_failed_checkpointed_run_prunes_its_lineage() {
    use tinyflows::graph::Checkpointer;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(FsOps::new(dir.path()));
    let (mut deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(
        "http://127.0.0.1:1/unused".to_string(),
        dir.path(),
    );
    deps.workspace = Some(store.clone());
    deps.run_output_store = Some(store.clone());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ArtifactWritingTurn {
        workspace_root: deps.workspace_root.clone(),
        approvals: deps.approval_requests.clone(),
        blocked: false,
    });
    let checkpoints = Arc::new(
        crate::workflows::checkpoint_store::WorkflowCheckpointStore::new(
            dir.path().join("checkpoints"),
        ),
    );
    let ctx = WorkflowRunContext::new(false);
    let thread_id = ctx.run_id.clone();

    let result = run_workflow_lane_aware_checkpointed(
        turn,
        deps,
        &record,
        &artifact_graph(),
        serde_json::json!({ "request": "make the report" }),
        &ctx,
        Some(checkpoints.clone()),
    )
    .await;
    assert!(result.is_err(), "the synthetic failure must fail the run");

    let remaining = checkpoints
        .get_thread(&thread_id)
        .await
        .expect("checkpoint read");
    assert!(
        remaining.is_empty(),
        "a genuinely failed run has no continuation path, so its checkpoint lineage must be \
         pruned: {remaining:?}"
    );
}

/// A turn double for a two-node chain: `capped_agent` always truncates at
/// the iteration cap (`Ok`, `hit_iteration_cap: true` — the same signal
/// [`reclassify_capped_nodes`] reconciles), and `tail_agent` either fails
/// outright or parks an approval, depending on `blocked`. The chain is
/// strictly sequential (`start -> capped_work -> tail_work`), so
/// `capped_work` always settles — and pushes into `RunCappedNodes` — before
/// `tail_work` runs, with no race to arrange.
struct CappedThenSettlingTurn {
    approvals: crate::harness::policy::ApprovalRequestQueue,
    blocked: bool,
}

impl CappedThenSettlingTurn {
    async fn execute(&self, agent_id: &str) -> Result<crate::harness::TurnOutcome> {
        if agent_id == "capped_agent" {
            return Ok(crate::harness::TurnOutcome {
                reply: "partial answer, still going".to_string(),
                steps: Vec::new(),
                hit_iteration_cap: true,
                abnormal_stop: None,
                halted_for_spend: None,
                budget_paused: None,
            });
        }
        if !self.blocked {
            return Err(OpenCompanyError::Harness(
                "synthetic failure after a capped sibling already settled".to_string(),
            ));
        }
        self.approvals
            .push(crate::harness::policy::ApprovalRequest {
                tool: "shell".to_string(),
                reason: "synthetic approval after a capped sibling already settled".to_string(),
                effect: crate::ports::types::Effect {
                    kind: "shell".to_string(),
                    group: crate::ports::types::EffectGroup::Other,
                    amount_usd: None,
                    established_thread: false,
                    first_time_counterparty: false,
                    payload: serde_json::json!({ "command": "finish-report" }),
                    agent: Some(agent_id.to_string()),
                    run_id: None,
                },
            });
        Ok(crate::harness::TurnOutcome {
            reply: "Waiting for approval.".to_string(),
            steps: Vec::new(),
            hit_iteration_cap: false,
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        })
    }
}

#[async_trait]
impl crate::runtime::delegation::RunTurn for CappedThenSettlingTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        agent_id: &str,
        _message: &str,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.execute(agent_id).await
    }

    async fn run_steered(
        &self,
        _company: &CompanyId,
        agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.execute(agent_id).await
    }

    async fn run_steered_background(
        &self,
        _company: &CompanyId,
        agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> Result<crate::harness::TurnOutcome> {
        self.execute(agent_id).await
    }
}

fn capped_then_settling_graph() -> WorkflowFile {
    parse_workflow(
        r#"
id = "capped_then_settling"
name = "Capped then settling"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "capped_work"
kind = "agent"
name = "Capped work"
summary = "Loop until the iteration cap."
agent = "capped_agent"
[[node]]
id = "tail_work"
kind = "agent"
name = "Tail work"
summary = "Fail or block, depending on the test."
agent = "tail_agent"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "capped_work"
[[edge]]
from = "capped_work"
to = "tail_work"
[[edge]]
from = "tail_work"
to = "done"
"#,
    )
    .expect("capped-then-settling graph parses")
}

/// PR #1883 review (Codex #3877606126): `reclassify_capped_nodes` is only
/// ever called on the clean-settle arm at the bottom of
/// `run_workflow_inner` — the genuine-failure and blocked early returns a
/// few hundred lines above it build their `nodes`/`WorkflowRun` straight
/// from the collector's raw rows and return before that call is ever
/// reached. So a node upstream of the one that fails or blocks the run,
/// which itself only truncated at the iteration cap, keeps its `Ok` row
/// forever even though its own attempt already settled `Failed` — the
/// exact disagreement issue #1865 exists to close, just reachable from a
/// different exit than the one its unit tests cover.
async fn assert_capped_sibling_reclassified_before_early_return(blocked: bool) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(
        "http://127.0.0.1:1/unused".to_string(),
        dir.path(),
    );
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(CappedThenSettlingTurn {
        approvals: deps.approval_requests.clone(),
        blocked,
    });
    let ctx = WorkflowRunContext::new(false);

    let result = run_workflow_lane_aware(
        turn,
        deps,
        &record,
        &capped_then_settling_graph(),
        serde_json::json!({ "request": "go" }),
        &ctx,
    )
    .await;

    let nodes = if blocked {
        let run = result.expect("an approval-blocked run settles successfully");
        assert!(
            run.blocked_nodes.iter().any(|n| n.node_id == "tail_work"),
            "tail_work must block: {run:?}"
        );
        run.nodes
    } else {
        let err = result.expect_err("the synthetic failure must fail the run");
        let partial = err
            .partial_run()
            .expect("a genuine failure carries a partial run");
        partial.nodes.clone()
    };

    let capped_row = nodes
        .iter()
        .find(|n| n.node_id == "capped_work")
        .expect("the capped node's row must be in the partial run");
    assert_eq!(
        capped_row.status,
        WorkflowNodeStatus::Error,
        "a capped sibling's row must be reclassified Error even when the run leaves \
         through an early return (genuine failure or block), not only on the \
         clean-finish arm — {nodes:?}"
    );
}

#[tokio::test]
async fn a_capped_node_is_reclassified_even_when_a_later_node_fails_the_run() {
    assert_capped_sibling_reclassified_before_early_return(false).await;
}

#[tokio::test]
async fn a_capped_node_is_reclassified_even_when_a_later_node_blocks_the_run() {
    assert_capped_sibling_reclassified_before_early_return(true).await;
}

/// A turn double for `start -> ok_branch (-> done)`, in parallel with a
/// `bad_branch` tool_call that fails on its own (unknown slug, no model
/// call involved). `ok_branch`'s turn always reports a real, non-empty
/// reply; the scripted judge behind `deps.provider` is what answers
/// `halt_benign` for it.
pub(super) struct HaltOkTurn;
