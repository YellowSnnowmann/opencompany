use super::tests_turn_dispatch::single_turn;
use super::*;

/// Issue #1825 (P1, fourth follow-up — found by chatgpt-codex-connector):
/// approving the first card a multi-call node parks must not complete its
/// continuation batch before the rest of the node's calls have even been
/// attempted.
///
/// # The race this closes
///
/// `park_gated_calls` parks a node's gated calls one at a time in a loop,
/// and each successful `park_and_journal` arms `ContinuationQueue` for the
/// node's turn — issue #469/#978's original per-call mechanism, unchanged.
/// With no hold, `outstanding` right after the FIRST call parks is exactly
/// 1: a decision on that lone card zeroes it out and
/// `ContinuationQueue::decide` hands back a "complete" batch, even though
/// the loop has not attempted the node's second call yet.
/// `blocked_nodes.arm` (the P1 first follow-up, above) already makes the
/// workflow id and trigger input available the instant the first card
/// exists, so a premature zero here finds a real stash rather than an
/// empty one — pre-fix, that reaches `resume_blocked_agent_node` and
/// re-dispatches the run while this node is still parking its remaining
/// calls.
///
/// # How this is reproduced deterministically
///
/// A real timing race needs two concurrent tasks; this test gets the same
/// interleaving without one. `RaceGate` wraps the approval gate
/// `park_gated_calls` parks through, and its second `park` call — the
/// second gated call's — first decides the FIRST card via the SAME
/// `ContinuationQueue` handle `park_and_journal` arms, synchronously,
/// before that second park even returns. That is exactly where a fast
/// operator's decision would land relative to the loop below, reproduced
/// on ordering rather than wall-clock luck.
#[tokio::test]
async fn approving_the_first_card_of_a_multi_call_node_does_not_complete_the_batch_early() {
    use crate::harness::policy::{ApprovalRequest, ApprovalScope};
    use crate::ports::approvals::ApprovalGate;
    use crate::ports::types::{
        Actor, ActorKind, ApprovalId, CompanyEvent, Effect, EffectGroup, PolicyDecision, Verdict,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex as AsyncMutex;

    /// Delegates every call to `inner`, except that the SECOND `park` it
    /// sees first decides the FIRST approval it minted, via the same
    /// `ContinuationQueue` the real park path arms — simulating an
    /// operator racing ahead of `park_gated_calls`'s own loop.
    struct RaceGate {
        inner: Arc<dyn ApprovalGate>,
        continuations: crate::runtime::continuation::ContinuationQueue,
        node_turn: String,
        calls: AtomicUsize,
        first_approval: AsyncMutex<Option<ApprovalId>>,
        /// What `ContinuationQueue::decide` returned for the interleaved
        /// decision on the first card — the assertion this test exists
        /// for. Outer `Option`: whether the interleave actually ran.
        early_decide_result: AsyncMutex<Option<Option<Vec<CompanyEvent>>>>,
    }

    #[async_trait::async_trait]
    impl ApprovalGate for RaceGate {
        async fn evaluate(
            &self,
            company: &CompanyId,
            effect: &Effect,
        ) -> crate::Result<PolicyDecision> {
            self.inner.evaluate(company, effect).await
        }

        async fn park(&self, company: &CompanyId, effect: Effect) -> crate::Result<ApprovalId> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let id = self.inner.park(company, effect).await?;
            if call == 0 {
                *self.first_approval.lock().await = Some(id.clone());
            } else if call == 1 {
                let first = self
                    .first_approval
                    .lock()
                    .await
                    .clone()
                    .expect("the first card must have parked before the second");
                let event = CompanyEvent::ApprovalResolved {
                    approval_id: first,
                    verdict: Verdict::Approve,
                    by: Actor {
                        kind: ActorKind::Operator,
                        id: "operator".to_string(),
                    },
                };
                let result = self.continuations.decide(&self.node_turn, Some(event));
                *self.early_decide_result.lock().await = Some(result);
            }
            Ok(id)
        }

        async fn resolve(
            &self,
            id: &ApprovalId,
            verdict: Verdict,
            by: Actor,
        ) -> crate::Result<Option<Effect>> {
            self.inner.resolve(id, verdict, by).await
        }
    }

    let dir = tempfile::Builder::new()
        .prefix("oc-1825-p1-4-")
        .tempdir()
        .expect("tempdir");
    let (mut deps, _journal) =
        crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());

    let node_turn =
        crate::runtime::workflow_resume::workflow_node_turn_key("run-1825-p1-4", "work");

    let delivery = deps
        .delivery
        .as_mut()
        .expect("gated_tool_turn_tests::deps wires delivery");
    let parking = delivery
        .parking
        .as_mut()
        .expect("gated_tool_turn_tests::deps wires parking");
    let race_gate = Arc::new(RaceGate {
        inner: parking.approvals.clone(),
        continuations: parking.continuations.clone(),
        node_turn: node_turn.clone(),
        calls: AtomicUsize::new(0),
        first_approval: AsyncMutex::new(None),
        early_decide_result: AsyncMutex::new(None),
    });
    parking.approvals = race_gate.clone();

    let queue = deps.approval_requests.clone();
    let trigger_input = json!({ "request": "quarterly numbers" });
    let board_claim = Arc::new(deps.delegations.claim_board("run-1825-p1-4"));
    let publish_refusal_claim = Arc::new(
        deps.pending_publishes
            .claim_refusals_for_run("run-1825-p1-4"),
    );
    let runner = HarnessAgentRunner::new(
        single_turn(&deps),
        deps,
        crate::workflows::gated_tool_turn_tests::record(),
        CompanyId::new("acme"),
        "wf-1825-p1-4".to_string(),
        "run-1825-p1-4".to_string(),
        None,
        trigger_input.clone(),
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

    let claim = queue.claim(ApprovalScope::Run("run-1825-p1-4".to_string()));
    claim
        .scoped(async {
            for tool in ["shell", "http"] {
                queue.push(ApprovalRequest {
                    tool: tool.to_string(),
                    reason: "gated".to_string(),
                    effect: Effect {
                        kind: tool.to_string(),
                        group: EffectGroup::Other,
                        amount_usd: None,
                        established_thread: false,
                        first_time_counterparty: false,
                        payload: json!({ "call": tool }),
                        agent: Some("ceo".to_string()),
                        run_id: None,
                    },
                });
            }
        })
        .await;

    let summary = claim
        .scoped(runner.park_gated_calls(Some("work"), "work", &node_turn))
        .await;

    assert_eq!(summary.approval_ids.len(), 2, "both calls must have parked");

    let early_result = race_gate.early_decide_result.lock().await.clone();
    assert_eq!(
        early_result,
        Some(None),
        "deciding the first card while the loop was still parking the second must NOT \
         complete the batch — ContinuationQueue::decide must report 'still waiting' \
         (None), not hand back a batch the run has not finished parking yet"
    );
}

/// Queues `count` gated calls in a run's scope, drains them through
/// `park_gated_calls`, and returns whatever the run was told.
///
/// `with_gate` selects whether a `parking` sink is wired, which is the axis
/// the guard-order test needs.
pub(super) async fn overflowing_runner_notices(
    count: usize,
    with_gate: bool,
) -> (Vec<String>, crate::harness::policy::ApprovalRequestQueue) {
    use crate::harness::policy::{ApprovalRequest, ApprovalScope};
    use crate::ports::types::{Effect, EffectGroup};

    let dir = tempfile::Builder::new()
        .prefix("oc-638-")
        .tempdir()
        .expect("tempdir");
    let (mut deps, _journal) =
        crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    if !with_gate {
        deps.delivery = None;
    }
    let queue = deps.approval_requests.clone();
    let notices = RunNotices::default();
    let board_claim = Arc::new(deps.delegations.claim_board("run-1"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1"));
    let runner = HarnessAgentRunner::new(
        single_turn(&deps),
        deps,
        crate::workflows::gated_tool_turn_tests::record(),
        CompanyId::new("acme"),
        "wf-1".to_string(),
        "run-1".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        notices.clone(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    // Pushed inside the run's own scope, exactly as its turn would.
    let claim = queue.claim(ApprovalScope::Run("run-1".to_string()));
    claim
        .scoped(async {
            for i in 0..count {
                queue.push(ApprovalRequest {
                    tool: "shell".to_string(),
                    reason: "gated".to_string(),
                    effect: Effect {
                        kind: "shell".to_string(),
                        group: EffectGroup::Other,
                        amount_usd: None,
                        established_thread: false,
                        first_time_counterparty: false,
                        payload: json!({ "n": i }),
                        agent: Some("ceo".to_string()),
                        run_id: None,
                    },
                });
            }
        })
        .await;
    let node_turn = crate::runtime::workflow_resume::workflow_node_turn_key(&runner.run_id, "work");
    claim
        .scoped(runner.park_gated_calls(Some("work"), "work", &node_turn))
        .await;
    (notices.take(), queue)
}

/// PR #1775 review: a publish the tool refused mid-turn, but which the
/// post-turn workspace capture materialized anyway, must not be silently
/// dropped from the run's notices. The node's own turn reply already told
/// the operator delivery failed (the tool's response, at call time); going
/// silent here would leave that unreconciled against a run inspector that
/// shows the file delivered.
#[tokio::test]
async fn a_captured_publish_reconciles_its_earlier_refusal_notice() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1775-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let pending_publishes = deps.pending_publishes.clone();
    let notices = RunNotices::default();
    let board_claim = Arc::new(deps.delegations.claim_board("run-1775"));
    let publish_refusal_claim = Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1775"));
    let runner = HarnessAgentRunner::new(
        single_turn(&deps),
        deps,
        crate::workflows::gated_tool_turn_tests::record(),
        CompanyId::new("acme"),
        "wf-1".to_string(),
        "run-1775".to_string(),
        None,
        Value::Null,
        crate::ports::types::StartedBy::Operator,
        notices.clone(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim.clone(),
    );

    // The tool's refusal, staged inside this run's scope exactly as the
    // live `publish_artifact` call would have.
    publish_refusal_claim
        .scoped(async { pending_publishes.push_refusal("specs/plan.md".to_string()) })
        .await;

    // The post-turn drain, told that the workspace scan captured that
    // same file anyway.
    publish_refusal_claim
        .scoped(async {
            runner.drain_publish_refusals(&["specs/plan.md".to_string()]);
        })
        .await;

    let recorded = notices.take();
    assert_eq!(
        recorded.len(),
        1,
        "the refusal must be reconciled with a notice, not silenced: {recorded:?}"
    );
    assert!(
        recorded[0].contains("specs/plan.md"),
        "the notice must name the file: {recorded:?}"
    );
    assert!(
        recorded[0].contains("captured"),
        "the notice must say the file landed anyway, not just that it was refused: \
         {recorded:?}"
    );
}

#[test]
fn message_prefers_prompt_then_input_then_message() {
    assert_eq!(
        message_from_request(&json!({ "prompt": "P", "input": "I" })),
        "P"
    );
    assert_eq!(message_from_request(&json!({ "input": "I" })), "I");
    assert_eq!(message_from_request(&json!({ "message": "M" })), "M");
}

// ── Issue #154: the operator's run request reaches the agent ──

#[test]
fn run_request_is_appended_under_a_labelled_heading() {
    let out = compose_turn_message("Draft the launch post.", Some("dark mode for iOS"));
    // The node's standing instruction still leads.
    assert!(out.starts_with("Draft the launch post."), "{out}");
    // …and this run's subject is distinguishable from it.
    assert!(out.contains("Request for this run:"), "{out}");
    assert!(out.contains("dark mode for iOS"), "{out}");
}

#[test]
fn a_run_with_no_request_is_byte_identical_to_the_old_message() {
    // The guarantee that makes this safe to land: runs that supply no topic
    // must behave exactly as they did before.
    for empty in [None, Some(""), Some("   "), Some("\n\t ")] {
        assert_eq!(
            compose_turn_message("Draft the launch post.", empty),
            "Draft the launch post.",
            "empty request {empty:?} must not alter the message"
        );
    }
}

#[test]
fn a_request_with_no_instruction_stands_on_its_own() {
    // No dangling heading when the node carries no usable instruction.
    assert_eq!(
        compose_turn_message("", Some("ship dark mode")),
        "ship dark mode"
    );
    assert_eq!(
        compose_turn_message("   ", Some("ship dark mode")),
        "ship dark mode"
    );
}

#[test]
fn run_request_text_reads_the_console_payload_and_a_bare_string() {
    assert_eq!(
        run_request_text(&json!({ "request": "dark mode" })).as_deref(),
        Some("dark mode")
    );
    assert_eq!(
        run_request_text(&json!("dark mode")).as_deref(),
        Some("dark mode")
    );
    // Tolerated spellings from a hand-written call or an older client.
    for key in ["input", "topic", "message", "text"] {
        let mut payload = serde_json::Map::new();
        payload.insert(key.to_string(), json!("dark mode"));
        assert_eq!(
            run_request_text(&Value::Object(payload)).as_deref(),
            Some("dark mode"),
            "key {key} should be accepted"
        );
    }
    // Trimmed.
    assert_eq!(
        run_request_text(&json!({ "request": "  dark mode  " })).as_deref(),
        Some("dark mode")
    );
}

#[test]
fn run_request_text_is_none_for_payloads_that_carry_no_topic() {
    // These are the shapes an existing caller already sends — none may start
    // injecting a topic into agent messages.
    for payload in [
        json!({}),
        json!(null),
        json!(42),
        json!({ "request": "" }),
        json!({ "request": "   " }),
        json!({ "unrelated": "value" }),
        json!({ "request": 7 }),
        json!(["dark mode"]),
    ] {
        assert_eq!(
            run_request_text(&payload),
            None,
            "payload {payload} must carry no topic"
        );
    }
}

#[test]
fn message_falls_back_to_serialized_request() {
    // No known string key: fall back to the serialized object.
    let out = message_from_request(&json!({ "agent_ref": "x" }));
    assert!(out.contains("agent_ref"));
}

// ── Issue #782: the upstream node's output reaches the next agent's turn ──

/// [`append_upstream_input`] under the shipped budget, keeping the #782
/// tests reading about *what reaches the turn* rather than about the #849
/// budget they are all far below. The truncation report those calls discard
/// has its own tests below.
fn folded(request: &Value) -> String {
    append_upstream_input(
        &message_from_request(request),
        request,
        upstream::DEFAULT_UPSTREAM_BUDGET_CHARS,
    )
    .0
}

/// The headline. An `agent -> agent` pipeline's second teammate must receive
/// the first's output. `translate` binds `input = "=items"`, the engine
/// resolves it to the predecessor envelope, and this proves the runner folds
/// that envelope's prose into the turn — under a heading, AFTER the node's
/// own instruction, so both survive.
#[test]
fn upstream_output_is_folded_into_the_turn() {
    // The shape the engine hands us: `prompt` (the node's static job) plus
    // `input` (the resolved `=items`) — one predecessor agent envelope.
    let request = json!({
        "prompt": "Write the launch post.",
        "input": [{ "json": {}, "text": "The analyst found a 20% MoM jump.", "raw": {} }],
    });
    let message = folded(&request);
    // The node's own instruction still leads.
    assert!(message.starts_with("Write the launch post."), "{message}");
    // …the upstream output is present, under its heading…
    assert!(message.contains(UPSTREAM_INPUT_HEADING), "{message}");
    assert!(
        message.contains("The analyst found a 20% MoM jump."),
        "the previous step's output must reach the turn: {message}"
    );
}

/// Fan-in: a `merge -> agent` (or several edges into one agent) resolves
/// `=items` to EVERY predecessor, and all of them must be delivered — the
/// "loses all but the first" failure is exactly what `=items` (not `=item`)
/// guards against.
#[test]
fn fan_in_delivers_every_predecessor() {
    let request = json!({
        "prompt": "Combine the research.",
        "input": [
            { "json": {}, "text": "Predecessor A: market is up.", "raw": {} },
            { "json": {}, "text": "Predecessor B: sentiment is positive.", "raw": {} },
        ],
    });
    let message = folded(&request);
    assert!(
        message.contains("Predecessor A: market is up."),
        "first predecessor missing: {message}"
    );
    assert!(
        message.contains("Predecessor B: sentiment is positive."),
        "second predecessor missing — a fan-in must not lose all but the first: {message}"
    );
}

/// A non-agent predecessor (a `tool_call` / `transform` / `output` node) has
/// no prose `text`, so its structured output is rendered as JSON rather than
/// dropped.
#[test]
fn a_structured_predecessor_is_rendered_as_json() {
    let request = json!({
        "prompt": "Summarise the fetch.",
        "input": [{ "json": { "rows": 3 }, "text": null, "raw": { "rows": 3 } }],
    });
    let message = folded(&request);
    assert!(message.contains(UPSTREAM_INPUT_HEADING), "{message}");
    assert!(
        message.contains("\"rows\""),
        "structured output rendered: {message}"
    );
}

/// The byte-identical guarantee. A single-agent workflow with no predecessor
/// (no `input`, or an empty / all-null / empty-container `input`) composes
/// exactly the pre-#782 message — never a dangling empty heading.
#[test]
fn no_upstream_output_is_byte_identical() {
    let base = "Draft the launch post.";
    for input in [
        None,
        Some(json!(null)),
        Some(json!([])),
        Some(json!([null])),
        Some(json!([{}])),
        Some(json!([{ "json": {}, "text": "   ", "raw": {} }])),
    ] {
        let mut request = serde_json::Map::new();
        request.insert("prompt".to_string(), json!(base));
        if let Some(input) = input.clone() {
            request.insert("input".to_string(), input);
        }
        let request = Value::Object(request);
        assert_eq!(
            folded(&request),
            base,
            "input {input:?} must not alter the message or add an empty heading"
        );
        // And the whole composition (including the #154 run topic) is
        // unchanged from what `compose_turn_message` alone would produce.
        let instruction = folded(&request);
        assert_eq!(
            compose_turn_message(&instruction, Some("ship dark mode")),
            compose_turn_message(base, Some("ship dark mode")),
            "the no-upstream path must leave the run-topic composition untouched"
        );
    }
}

/// Upstream output and the #154 run topic coexist: the node's instruction
/// leads, the previous step's output follows under its heading, and the run's
/// subject follows under its own — all three reach the teammate.
#[test]
fn upstream_output_and_run_topic_coexist() {
    let request = json!({
        "prompt": "Write the post.",
        "input": [{ "json": {}, "text": "ANALYST_SAID_THIS", "raw": {} }],
    });
    let instruction = folded(&request);
    let message = compose_turn_message(&instruction, Some("dark mode launch"));
    assert!(message.starts_with("Write the post."), "{message}");
    assert!(message.contains(UPSTREAM_INPUT_HEADING), "{message}");
    assert!(message.contains("ANALYST_SAID_THIS"), "{message}");
    assert!(message.contains("Request for this run:"), "{message}");
    assert!(message.contains("dark mode launch"), "{message}");
}

// ── Issue #849: nothing may hand an agent node an unbounded payload ──
//
// Driven by synthetic oversized payloads, never by a live page: the reported
// failure is intermittent *because* it depends on how much text a sports
// section happened to return that minute, so a test that reproduced it that
// way would be a coin flip too.

/// One predecessor envelope carrying `chars` characters of page-like text —
/// the shape a `web_fetch` `tool_call` node emits (its non-JSON output is
/// wrapped as `{"text": …}`, which the tinyflows envelope lifts to `text`).
pub(super) fn source_envelope(marker: &str, chars: usize) -> Value {
    let body = format!("{marker}{}", "x".repeat(chars.saturating_sub(marker.len())));
    json!({ "json": { "text": body.clone() }, "text": body, "raw": { "text": body } })
}

/// How much slack above the budget the markers, the heading and the section
/// rules are allowed to add. They sit **outside** the budget deliberately —
/// the budget exists to bound upstream *text*, and letting our own accounting
/// compete for room would mean the truncation marker could itself be the
/// thing squeezed out (the reasoning `memory_loop`'s skipped-hit marker
/// arrived at first).
pub(super) const MARKER_SLACK: usize = 2_000;

/// The reported shape: three fetched sources fan in to one ranking agent.
/// Every source must still be represented, the turn must be bounded, and
/// every cut must be visible.
#[test]
fn a_three_way_fan_in_is_bounded_and_no_source_is_lost() {
    let budget = upstream::DEFAULT_UPSTREAM_BUDGET_CHARS;
    let request = json!({
        "prompt": "Rank today's stories.",
        "input": [
            source_envelope("SOURCE_ONE", 200_000),
            source_envelope("SOURCE_TWO", 200_000),
            source_envelope("SOURCE_THREE", 200_000),
        ],
    });
    let (message, report) =
        append_upstream_input(&message_from_request(&request), &request, budget);

    assert!(
        message.chars().count() <= budget + MARKER_SLACK,
        "600k characters of upstream input must not reach a turn: {} characters",
        message.chars().count()
    );
    // …and every source is still *there*, which is what separates a bound
    // from "drop everything after the first".
    for marker in ["SOURCE_ONE", "SOURCE_TWO", "SOURCE_THREE"] {
        assert!(message.contains(marker), "{marker} was lost entirely");
    }
    // Each cut is visible to the agent.
    assert_eq!(
        message.matches("TRUNCATED BY OPENCOMPANY").count(),
        3,
        "every truncated source carries its own marker: {message}"
    );
    assert!(message.contains("source 3 of 3"), "{message}");

    // …and to the operator.
    assert_eq!(report.sources.len(), 3);
    assert!(report.truncated_any());
    let notice = report.notice().expect("the operator is told");
    assert!(notice.contains("3 sources"), "{notice}");
    assert!(notice.contains("3 of them were truncated"), "{notice}");
}
