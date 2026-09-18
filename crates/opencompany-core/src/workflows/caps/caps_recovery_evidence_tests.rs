use super::tests_budget_postcondition::ScriptedTurn;
use super::tests_turn_dispatch::RefusalWorkflowTurn;
use super::*;

#[async_trait]
impl RunTurn for RefusalWorkflowTurn {
    async fn run(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        unreachable!("workflow agent nodes route through run_background_workflow")
    }

    async fn run_steered(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat_id: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        unreachable!("workflow agent nodes route through run_background_workflow")
    }

    async fn run_steered_background(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _control: &crate::company::steer::SteerControl,
        _chat: crate::runtime::delegation::ChatTarget<'_>,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        unreachable!("workflow agent nodes route through run_background_workflow")
    }

    async fn run_background_workflow(
        &self,
        _company: &CompanyId,
        _agent_id: &str,
        _message: &str,
        _run_sink: Option<Arc<crate::harness::run_trace::RunTraceSink>>,
        _workflow_run_id: &str,
        _node_id: &str,
    ) -> crate::Result<crate::harness::TurnOutcome> {
        Ok(crate::harness::TurnOutcome {
            reply: "I cannot draft the email without the customer's name.".to_string(),
            steps: Vec::new(),
            hit_iteration_cap: false,
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        })
    }
}

/// A [`FactStore`] that always answers `list` with one fixed fact,
/// regardless of the query — standing in for a real match so `ask_around`
/// always has evidence to offer.
struct OneFactStore;

#[async_trait]
impl crate::ports::FactStore for OneFactStore {
    async fn list(
        &self,
        _company: &CompanyId,
        _query: Option<&str>,
        _kind: Option<crate::ports::FactKind>,
    ) -> crate::Result<Vec<crate::ports::FactRecord>> {
        Ok(vec![crate::ports::FactRecord {
            id: "f1".to_string(),
            kind: crate::ports::FactKind::Fact,
            title: "Company context".to_string(),
            body: "irrelevant background, not the customer's name".to_string(),
            source: "test".to_string(),
            updated_at_millis: 0,
        }])
    }

    async fn upsert(
        &self,
        _company: &CompanyId,
        _fact: &crate::ports::FactRecord,
    ) -> crate::Result<()> {
        unreachable!("not exercised by this test")
    }

    async fn delete(&self, _company: &CompanyId, _id: &str) -> crate::Result<bool> {
        unreachable!("not exercised by this test")
    }
}

/// Codex review on #1990 (issue #1866, #3903874673): found evidence is not
/// itself proof the gap closed. Before the fix, ANY evidence — however
/// unrelated — was appended to a refusal's own reply and the node settled
/// `Succeeded` without ever re-checking whether the augmented text now
/// actually answers the ask. Here the "recovered" fact is deliberately
/// irrelevant to the missing customer name, so a correct re-verify must
/// still refuse to accept the node.
#[tokio::test]
async fn recovered_evidence_that_does_not_close_the_gap_is_not_accepted() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-recover-reverify-")
        .tempdir()
        .expect("tempdir");
    let (base_url, script) = crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"retry\"}"),
    ])
    .await;
    let (mut deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    deps.facts = Some(Arc::new(OneFactStore));
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(RefusalWorkflowTurn);
    let board_claim = Arc::new(deps.delegations.claim_board("run-1990g"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1990g"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1990g".to_string(),
        "run-1990g".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let result = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "draft",
                "prompt": "Draft the customer email.",
                "verify": { "criteria": "must include the customer's name" }
            }),
        )
        .await;

    assert!(
        result.is_err(),
        "irrelevant recovered evidence must not turn a refusal into a success"
    );
    let seen = script.seen.lock().expect("seen");
    assert_eq!(
        seen.len(),
        2,
        "the judge must be asked again about the augmented output, not just once up front"
    );
    let reverify_prompt = seen[1].to_string();
    assert!(
        reverify_prompt.contains("Recovered company context"),
        "the second judge call must see the augmented output, not the original refusal alone: \
         {reverify_prompt}"
    );
}

/// The text a node ships after a successful recovery must be the exact
/// text the re-verification judge was shown. `augment_with_recovery`
/// bounds the reply so the evidence survives the judge's output window;
/// a separately composed, unbounded string would let an oversized reply
/// ship content the judge never read.
#[tokio::test]
async fn a_recovered_reply_ships_the_exact_text_the_judge_certified() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-recover-certified-text-")
        .tempdir()
        .expect("tempdir");
    let (base_url, script) = crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
        crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"continue\"}"),
    ])
    .await;
    let (mut deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    deps.facts = Some(Arc::new(OneFactStore));
    let record = crate::workflows::gated_tool_turn_tests::record();
    let oversized = "R".repeat(25_000);
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: oversized.clone(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1990h"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1990h"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1990h".to_string(),
        "run-1990h".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let (_value, outcome) = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "draft",
                "prompt": "Draft the customer email.",
                "verify": { "criteria": "must include the customer's name" }
            }),
        )
        .await
        .expect("a `continue` re-verdict accepts the recovered reply");

    let judged = script.seen.lock().expect("seen")[1].to_string();
    let escaped = serde_json::to_string(&outcome.reply).expect("reply serializes");
    assert!(
        judged.contains(escaped.trim_matches('"')),
        "the reply stored on the outcome must be the same text the re-verification judge \
         read, but the judge never saw it ({} stored chars)",
        outcome.reply.chars().count()
    );
    assert!(
        outcome.reply.chars().count() < oversized.chars().count(),
        "the fixture must be large enough that recovery augmentation has to bound it, \
         otherwise this test cannot observe the divergence"
    );
}

/// A node declaring both a postcondition and a verify criteria emits one
/// output with two views of it: `value["text"]` and the JSON fields
/// merged into `value`. Recovery rewrites the reply, so both views must
/// describe the rewritten reply — a merge carrying the pre-recovery parse
/// would let a downstream `=item.json.<field>` binding read fields that
/// the shipped text no longer backs.
#[tokio::test]
async fn a_recovered_reply_does_not_emit_its_pre_recovery_json_parse() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-recover-stale-parse-")
        .tempdir()
        .expect("tempdir");
    let (base_url, _script) =
        crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"continue\"}"),
        ])
        .await;
    let (mut deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    deps.facts = Some(Arc::new(OneFactStore));
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "{\"draft\": \"no customer name yet\"}".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1990i"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1990i"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1990i".to_string(),
        "run-1990i".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let (value, _outcome) = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "draft",
                "prompt": "Draft the customer email.",
                "postcondition": { "require": "non_empty" },
                "verify": { "criteria": "must include the customer's name" }
            }),
        )
        .await
        .expect("a `continue` re-verdict accepts the recovered reply");

    assert!(
        value["text"]
            .as_str()
            .expect("text is a string")
            .contains("Recovered company context"),
        "the emitted text must be the recovered reply: {}",
        value["text"]
    );
    assert!(
        value.get("draft").is_none(),
        "the pre-recovery parse must not ship alongside a reply that no longer carries it: \
         {value}"
    );
}

/// `augment_with_recovery` always appends a `Recovered company context:`
/// prose block to the reply, so a reply that satisfied a declared
/// `field_present` postcondition before recovery (its JSON parsed and
/// carried the field) stops satisfying it after (the augmented text no
/// longer parses as JSON at all). A node must not settle `Succeeded`
/// carrying an output that no longer satisfies the postcondition its own
/// gate certified — the recovered reply is re-checked against the same
/// postcondition, and a node whose recovery breaks it fails instead of
/// silently shipping the field as absent.
#[tokio::test]
async fn a_recovered_reply_that_fails_its_postcondition_does_not_settle_succeeded() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-recover-postcondition-")
        .tempdir()
        .expect("tempdir");
    let (base_url, _script) =
        crate::workflows::gated_tool_turn_tests::spawn_script_recording(vec![
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"recover\"}"),
            crate::workflows::gated_tool_turn_tests::Turn::Say("{\"verdict\":\"continue\"}"),
        ])
        .await;
    let (mut deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(base_url, dir.path());
    deps.facts = Some(Arc::new(OneFactStore));
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "{\"draft\": \"no customer name yet\"}".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1990j"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1990j"));
    let runs: Arc<dyn crate::ports::RunStore> =
        Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1990j".to_string(),
        "run-1990j".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    )
    .with_runs(Some(runs.clone()), None, RunAttempts::default());

    let result = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "draft",
                "prompt": "Draft the customer email.",
                "postcondition": { "require": "field_present", "field": "json.draft" },
                "verify": { "criteria": "must include the customer's name" }
            }),
        )
        .await;

    assert!(
        result.is_err(),
        "a recovered reply that no longer satisfies its declared postcondition must not \
         settle Succeeded"
    );

    let attempts = runs
        .list_runs(
            &CompanyId::new("acme"),
            &crate::ports::RunFilter::for_workflow_run("run-1990j".to_string()),
        )
        .await
        .expect("list attempts");
    let statuses: Vec<_> = attempts.iter().map(|a| a.status).collect();
    assert_eq!(
        statuses,
        vec![crate::ports::RunStatus::Failed],
        "the recovered-but-noncompliant node must settle Failed, not Succeeded: {statuses:?}"
    );
}

/// A provider that always returns the `recover` verdict, driving the
/// judge's recovery-then-park branch.
#[derive(Default)]
struct RecoverJudgeProvider {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl tinyinference::model::ChatModel<()> for RecoverJudgeProvider {
    async fn invoke(
        &self,
        _state: &(),
        _request: tinyinference::model::ModelRequest,
    ) -> tinyinference::Result<tinyinference::model::ModelResponse> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(tinyinference::model::ModelResponse::assistant(
            "{\"verdict\":\"recover\"}".to_string(),
        ))
    }
}

impl crate::harness::provider::HarnessModel for RecoverJudgeProvider {
    fn telemetry_provider_id(&self) -> String {
        "recover-judge".to_string()
    }
}

/// tinysweeper on #1990 (#3905096415) read the recover branch as settling
/// `Blocked` and then having an outer handler overwrite it with `Failed`.
/// `run_turn` has no such handler — every settle is followed by an
/// immediate `return Err`, and the trailing settle is the fall-through
/// success path — so the parked row stays `Blocked`. Pinned here so a
/// future outer error handler cannot silently introduce the overwrite.
#[tokio::test]
async fn a_recovery_park_leaves_the_attempt_row_blocked() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-recover-park-")
        .tempdir()
        .expect("tempdir");
    let (mut deps, _journal) =
        crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    deps.provider = Arc::new(RecoverJudgeProvider::default());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "I cannot draft this without the customer's renewal date".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-recover"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-recover"));
    let runs: Arc<dyn crate::ports::RunStore> =
        Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-recover".to_string(),
        "run-recover".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    )
    .with_runs(Some(runs.clone()), None, RunAttempts::default());

    let result = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "draft_step",
                "prompt": "draft the renewal email",
                "verify": { "criteria": "the email must name the renewal date" },
            }),
        )
        .await;
    assert!(result.is_err(), "a parked node halts its branch");

    let attempts = runs
        .list_runs(
            &CompanyId::new("acme"),
            &crate::ports::RunFilter::for_workflow_run("run-recover".to_string()),
        )
        .await
        .expect("list attempts");
    let statuses: Vec<_> = attempts.iter().map(|a| a.status).collect();
    assert_eq!(
        statuses,
        vec![crate::ports::RunStatus::Blocked],
        "the parked node must be recorded Blocked, not overwritten with Failed"
    );
}

/// A provider that counts every `invoke` and always escalates, so a judge
/// call is both detectable and destructive to the caller's diagnosis.
#[derive(Default)]
pub(super) struct EscalatingJudgeProvider {
    pub(super) calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl tinyinference::model::ChatModel<()> for EscalatingJudgeProvider {
    async fn invoke(
        &self,
        _state: &(),
        _request: tinyinference::model::ModelRequest,
    ) -> tinyinference::Result<tinyinference::model::ModelResponse> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(tinyinference::model::ModelResponse::assistant(
            "{\"verdict\":\"escalate\",\"gap\":\"information\"}".to_string(),
        ))
    }
}

impl crate::harness::provider::HarnessModel for EscalatingJudgeProvider {
    fn telemetry_provider_id(&self) -> String {
        "escalating-judge".to_string()
    }
}

/// Codex review on #1990 (#3905537805): a turn refused by its per-agent
/// spend cap is rejected by the `LimitStop` path regardless, so paying for
/// a judge on the pause notice buys nothing — and an `escalate` verdict
/// returns early with a generic blocker, replacing the budget-pause
/// diagnosis the operator needs with "needs information intervention".
#[tokio::test]
async fn a_budget_paused_turn_skips_the_sufficiency_judge() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-budget-paused-judge-")
        .tempdir()
        .expect("tempdir");
    let (mut deps, _journal) =
        crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let provider = Arc::new(EscalatingJudgeProvider::default());
    deps.provider = provider.clone();
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "paused — out of budget".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: Some(crate::harness::BudgetPause {
            agent: "researcher".to_string(),
            summary: "acme is out of inference credits".to_string(),
        }),
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1990"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1990"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1990".to_string(),
        "run-1990".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let result = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "spend_step",
                "prompt": "keep going",
                "verify": { "criteria": "the report must be sent" },
            }),
        )
        .await;

    assert_eq!(
        provider.calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a budget-paused turn must not pay for a sufficiency judge"
    );
    let (_, outcome) = result.expect(
        "the budget-pause diagnosis must survive: the judge must not turn this into a \
         generic information blocker",
    );
    assert!(outcome.budget_paused.is_some());
}
