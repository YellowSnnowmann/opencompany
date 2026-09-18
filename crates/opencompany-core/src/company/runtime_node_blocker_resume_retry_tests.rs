//! Runtime node-blocker resume: retry/amend/skip/cancel re-entry into a workflow node.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::company::CompanyManifest;
use crate::company::runtime::CompanyRuntime;
use crate::company::task_intent::BlockerReplyIntent;
use crate::ports::blockers::{
    BlockerKind, BlockerPayload, BlockerSource, BlockerStep, BlockerVerdict,
};
use crate::ports::types::{CompanyId, Effect, EffectGroup};
use crate::ports::{WorkflowRun, WorkflowRunContext, WorkflowRunner};
use crate::runtime::RuntimeBuilder;
use crate::runtime::journal::{ApprovalConversation, TaskLink};
use crate::runtime::workflow_resume::{blocker_answer_for, workflow_node_turn_key};

const RUN_ID: &str = "run-that-blocked";
const NODE_ID: &str = "draft";
const WORKFLOW_TOML: &str = r#"
id = "reporting"
name = "Reporting"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "draft"
kind = "output"
name = "Draft"
[[edge]]
from = "start"
to = "draft"
"#;

#[derive(Clone, Debug)]
struct StartedRun {
    workflow_id: String,
    input: Value,
}

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
        _ctx: &WorkflowRunContext,
    ) -> crate::Result<WorkflowRun> {
        self.started
            .lock()
            .expect("recording runner")
            .push(StartedRun {
                workflow_id: workflow.id.clone(),
                input,
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

fn manifest() -> CompanyManifest {
    toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n\
         [[agent]]\nid = \"eng\"\nrole = \"Engineer\"\n",
    )
    .expect("manifest")
}

fn seed_home() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("opencompany-node-blocker-")
        .tempdir()
        .expect("tempdir");
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).expect("workflows dir");
    std::fs::write(workflows.join("reporting.toml"), WORKFLOW_TOML).expect("seed graph");
    dir
}

async fn runtime(
    home: &std::path::Path,
    with_runner: bool,
) -> (Arc<CompanyRuntime>, Arc<RecordingRunner>) {
    let mut rt = RuntimeBuilder::new(home.to_path_buf(), manifest())
        .with_id(CompanyId::new("acme"))
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

/// Parks a node blocker exactly as `park_node_blocker_as` does, and
/// arms the same per-(run, node) stash its `stash_node_blocker_resume`
/// writes at park time.
async fn park_node_blocker(rt: &Arc<CompanyRuntime>, input: Value) -> String {
    park_node_blocker_stashed(rt, input, true, None).await
}

async fn park_node_blocker_stashed(
    rt: &Arc<CompanyRuntime>,
    input: Value,
    stash: bool,
    thread_id: Option<&str>,
) -> String {
    let payload = BlockerPayload {
        kind: BlockerKind::Infrastructure,
        source: BlockerSource::Provider,
        step: Some(BlockerStep::Node {
            run_id: RUN_ID.to_string(),
            node_id: NODE_ID.to_string(),
        }),
        reason: "the model id `gpt-nope` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    let effect = Effect {
        kind: payload.effect_kind(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::to_value(&payload).expect("payload"),
        agent: None,
        run_id: Some(RUN_ID.to_string()),
    };
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
    if stash {
        let turn = workflow_node_turn_key(RUN_ID, NODE_ID);
        rt.blocked_nodes.arm_checkpointed(
            &turn,
            "reporting",
            &input,
            &crate::ports::types::StartedBy::from_scheduled(false),
            thread_id,
            None,
        );
        rt.journal()
            .record_blocked_node_stashed_checkpointed(
                &turn,
                "reporting",
                &input,
                &crate::ports::types::StartedBy::from_scheduled(false),
                thread_id,
                None,
            )
            .await
            .expect("stashes");
    }
    id.to_string()
}

/// [`park_node_blocker_stashed`], but for a caller that needs its own
/// run id — a batch mixing a stashed and an unstashed member must not
/// have them collide on one turn key.
async fn park_node_blocker_on_run(
    rt: &Arc<CompanyRuntime>,
    run_id: &str,
    input: Value,
    stash: bool,
) -> String {
    let payload = BlockerPayload {
        kind: BlockerKind::Infrastructure,
        source: BlockerSource::Provider,
        step: Some(BlockerStep::Node {
            run_id: run_id.to_string(),
            node_id: NODE_ID.to_string(),
        }),
        reason: "the model id `gpt-nope` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    let effect = Effect {
        kind: payload.effect_kind(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::to_value(&payload).expect("payload"),
        agent: None,
        run_id: Some(run_id.to_string()),
    };
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
    if stash {
        let turn = workflow_node_turn_key(run_id, NODE_ID);
        rt.blocked_nodes.arm_checkpointed(
            &turn,
            "reporting",
            &input,
            &crate::ports::types::StartedBy::from_scheduled(false),
            None,
            None,
        );
        rt.journal()
            .record_blocked_node_stashed_checkpointed(
                &turn,
                "reporting",
                &input,
                &crate::ports::types::StartedBy::from_scheduled(false),
                None,
                None,
            )
            .await
            .expect("stashes");
    }
    id.to_string()
}

async fn answer(rt: &Arc<CompanyRuntime>, id: &str, intent: BlockerReplyIntent, text: &str) {
    let ids = vec![crate::ports::types::ApprovalId::from(id.to_string())];
    rt.apply_blocker_reply(&ids, intent, text, None)
        .await
        .expect("applies");
}

/// The acceptance headline: a workflow parked at a failed node and
/// answered `retry` re-runs, and the answer is on the trigger input the
/// re-run carries — not banked in the DM and dropped.
#[tokio::test]
async fn retry_re_runs_the_node_with_the_answer_in_the_trigger_input() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_node_blocker(&rt, json!({ "topic": "quarterly numbers" })).await;

    answer(&rt, &id, BlockerReplyIntent::Retry, "go ahead and retry").await;

    let started = runner.started();
    assert_eq!(started.len(), 1, "a retry re-enters the node exactly once");
    assert_eq!(started[0].workflow_id, "reporting");
    assert_eq!(
        started[0].input["topic"], "quarterly numbers",
        "the blocked run's own trigger input is replayed"
    );
    let carried = blocker_answer_for(&started[0].input, NODE_ID)
        .expect("readable")
        .expect("the answer rides the trigger input");
    assert_eq!(carried.verdict, BlockerVerdict::Retry);
}

/// An amend carries the operator's words into the re-run, the workflow
/// twin of the card path's note append.
#[tokio::test]
async fn amend_carries_the_operators_words_into_the_re_run() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_node_blocker(&rt, json!({ "topic": "quarterly numbers" })).await;

    answer(
        &rt,
        &id,
        BlockerReplyIntent::Amend,
        "use gpt-4o-mini instead",
    )
    .await;

    let started = runner.started();
    assert_eq!(started.len(), 1);
    let carried = blocker_answer_for(&started[0].input, NODE_ID)
        .expect("readable")
        .expect("the answer rides the trigger input");
    assert_eq!(carried.verdict, BlockerVerdict::Amend);
    assert_eq!(
        carried.answer, "use gpt-4o-mini instead",
        "the correction must reach the node, or the re-run repeats the failure"
    );
}

/// A skip proceeds past the node: the run is re-entered carrying a
/// verdict the node reads as "waived", rather than being abandoned.
#[tokio::test]
async fn skip_continues_the_run_past_the_node() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_node_blocker(&rt, json!({ "topic": "quarterly numbers" })).await;

    answer(&rt, &id, BlockerReplyIntent::Skip, "skip it").await;

    let started = runner.started();
    assert_eq!(started.len(), 1, "a skip still continues the run");
    let carried = blocker_answer_for(&started[0].input, NODE_ID)
        .expect("readable")
        .expect("the answer rides the trigger input");
    assert_eq!(carried.verdict, BlockerVerdict::Skip);
}

/// The one verdict that starts nothing.
#[tokio::test]
async fn cancel_starts_no_run() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id = park_node_blocker(&rt, json!({ "topic": "quarterly numbers" })).await;

    answer(&rt, &id, BlockerReplyIntent::Cancel, "cancel that").await;

    assert!(
        runner.started().is_empty(),
        "a cancel abandons the work rather than re-entering it"
    );
}

/// The ledgers ride the same trigger input the answer is threaded onto,
/// so a continuation still knows what the blocked run already sent
/// (issues #438 / #846 / #978).
#[tokio::test]
async fn a_skip_continuation_still_carries_what_the_run_already_sent() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let input = json!({
        "topic": "quarterly numbers",
        crate::runtime::workflow_resume::CONTINUATION_DELIVERED_KEY: [
            { "node": "report", "kind": "owner" }
        ],
        crate::runtime::workflow_resume::CONTINUATION_PERFORMED_KEY: [
            { "node": "post", "tool": "send", "result": { "ok": true } }
        ],
        crate::runtime::workflow_resume::CONTINUATION_DENIED_KEY: ["refused-gate"],
    });
    let id = park_node_blocker(&rt, input).await;

    answer(&rt, &id, BlockerReplyIntent::Skip, "skip it").await;

    let started = runner.started();
    assert_eq!(started.len(), 1);
    let carried = &started[0].input;
    assert_eq!(
        crate::runtime::workflow_resume::delivered_in_input(carried).len(),
        1,
        "the report the blocked run already delivered must not be sent twice"
    );
    assert_eq!(
        crate::runtime::workflow_resume::performed_in_input(carried).len(),
        1,
        "the call the blocked run already made must not be made twice"
    );
    assert_eq!(
        crate::runtime::workflow_resume::denied_in_input(carried),
        vec!["refused-gate".to_string()],
        "a gate the operator already refused must not be asked about again"
    );
}

/// Answering twice re-enters the node once: the second decision finds
/// the continuation already dispatched and launches nothing.
#[tokio::test]
async fn a_node_is_re_entered_once_however_many_answers_land() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let first = park_node_blocker(&rt, json!({ "topic": "quarterly numbers" })).await;
    answer(&rt, &first, BlockerReplyIntent::Retry, "retry").await;

    let second = park_node_blocker(&rt, json!({ "topic": "quarterly numbers" })).await;
    answer(&rt, &second, BlockerReplyIntent::Retry, "retry").await;

    assert_eq!(
        runner.started().len(),
        1,
        "the dispatch marker is what stops a second continuation for one node"
    );
}

/// Two blocker cards on one node share one stash: only the first park
/// arms it. Resolving the first dispatches and retires that shared
/// stash — the second card's answer must then find the dispatch
/// marker and be acknowledged, not read the now-missing stash as "this
/// host no longer holds the run".
#[tokio::test]
async fn a_second_card_on_the_same_node_is_acknowledged_once_the_first_dispatches() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let first = park_node_blocker(&rt, json!({ "topic": "quarterly numbers" })).await;
    let second =
        park_node_blocker_stashed(&rt, json!({ "topic": "quarterly numbers" }), false, None).await;

    answer(&rt, &first, BlockerReplyIntent::Retry, "retry").await;
    assert_eq!(
        runner.started().len(),
        1,
        "the first answer dispatches the node"
    );

    let ids = vec![crate::ports::types::ApprovalId::from(second)];
    let outcome = rt
        .apply_blocker_reply(&ids, BlockerReplyIntent::Retry, "retry", None)
        .await;

    assert!(
        outcome.is_ok(),
        "the second card's answer must be acknowledged once the dispatch marker is set, \
         not returned as an error: {outcome:?}"
    );
    assert_eq!(
        runner.started().len(),
        1,
        "the dispatch marker must still stop a second continuation for this node"
    );
}

/// A host that no longer holds the run's stash reports it rather than
/// acknowledging a resume that did not happen.
#[tokio::test]
async fn an_answer_with_no_run_to_re_enter_is_reported_not_swallowed() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let id =
        park_node_blocker_stashed(&rt, json!({ "topic": "quarterly numbers" }), false, None).await;

    let ids = vec![crate::ports::types::ApprovalId::from(id)];
    let outcome = rt
        .apply_blocker_reply(&ids, BlockerReplyIntent::Retry, "retry", None)
        .await;

    assert!(
        outcome.is_err(),
        "an answer that reached no run must not read as a resume"
    );
    assert!(runner.started().is_empty());
}

/// A batch's members are independent: one failing to resume must not
/// stop the rest from getting their own resume attempt.
#[tokio::test]
async fn a_batch_follow_up_continues_past_one_members_failure() {
    let home = seed_home();
    let (rt, runner) = runtime(home.path(), true).await;
    let failing_id =
        park_node_blocker_on_run(&rt, RUN_ID, json!({ "topic": "quarterly numbers" }), false).await;
    let ok_id =
        park_node_blocker_on_run(&rt, "run-ok", json!({ "topic": "quarterly numbers" }), true)
            .await;

    let ids = vec![
        crate::ports::types::ApprovalId::from(failing_id),
        crate::ports::types::ApprovalId::from(ok_id),
    ];
    let outcome = rt
        .apply_blocker_reply(&ids, BlockerReplyIntent::Retry, "retry", None)
        .await;

    assert!(
        outcome.is_err(),
        "the batch must still surface the failing member's error"
    );
    assert_eq!(
        runner.started().len(),
        1,
        "the member after the failing one must still get its resume, not be skipped \
         because an earlier member's follow-up errored"
    );
}
