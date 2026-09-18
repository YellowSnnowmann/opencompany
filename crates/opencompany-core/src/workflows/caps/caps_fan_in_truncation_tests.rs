use super::tests_multi_call_notices::{MARKER_SLACK, source_envelope};
use super::tests_turn_dispatch::single_turn;
use super::*;

/// The "is it only a fan-in?" question, answered: it is not. A **single**
/// enormous `web_fetch` into one agent runs the same unbounded path, and the
/// bound at the join covers it with no second rule.
#[test]
fn a_single_enormous_source_is_bounded_too() {
    let budget = upstream::DEFAULT_UPSTREAM_BUDGET_CHARS;
    let request = json!({
        "prompt": "Summarise this page.",
        "input": [source_envelope("ONLY_SOURCE", 500_000)],
    });
    let (message, report) =
        append_upstream_input(&message_from_request(&request), &request, budget);

    assert!(
        message.chars().count() <= budget + MARKER_SLACK,
        "a single 500k-character page must not reach a turn whole: {} characters",
        message.chars().count()
    );
    assert!(message.contains("ONLY_SOURCE"), "the source still arrives");
    assert!(message.contains("source 1 of 1"), "{message}");
    assert_eq!(report.sources.len(), 1);
    assert!(report.truncated_any());
}

/// A large sibling must not starve a small one — the fan-in failure mode a
/// flat per-source cap would not fix and a running total would make
/// order-dependent.
#[test]
fn a_short_source_survives_whole_beside_an_enormous_one() {
    let budget = upstream::DEFAULT_UPSTREAM_BUDGET_CHARS;
    let short = "SHORT_SOURCE: the wire service filed three lines today.";
    let request = json!({
        "prompt": "Rank today's stories.",
        "input": [
            source_envelope("HUGE_SOURCE", 400_000),
            json!({ "json": {}, "text": short, "raw": {} }),
        ],
    });
    let (message, report) =
        append_upstream_input(&message_from_request(&request), &request, budget);

    assert!(
        message.contains(short),
        "the short source must arrive intact, not be crowded out: {message}"
    );
    assert_eq!(
        message.matches("TRUNCATED BY OPENCOMPANY").count(),
        1,
        "only the enormous source is cut: {message}"
    );
    assert_eq!(report.sources[1].produced, report.sources[1].kept);
    assert!(report.sources[0].kept < report.sources[0].produced);
}

/// The overwhelmingly common run: everything fits, so the fold is exactly
/// what #782 produced and the operator is told nothing new.
#[test]
fn an_ordinary_fan_in_is_untouched_and_says_nothing() {
    let budget = upstream::DEFAULT_UPSTREAM_BUDGET_CHARS;
    let request = json!({
        "prompt": "Combine the research.",
        "input": [
            { "json": {}, "text": "Predecessor A: market is up.", "raw": {} },
            { "json": {}, "text": "Predecessor B: sentiment is positive.", "raw": {} },
        ],
    });
    let (message, report) =
        append_upstream_input(&message_from_request(&request), &request, budget);

    assert!(!message.contains("TRUNCATED"), "{message}");
    assert!(!report.truncated_any());
    assert_eq!(report.notice(), None);
    assert!(
        message.contains("Predecessor A: market is up."),
        "{message}"
    );
    assert!(
        message.contains("Predecessor B: sentiment is positive."),
        "{message}"
    );
}

/// The bound survives composition: the marker is still in the message the
/// teammate is actually sent, alongside the node's instruction and the #154
/// run topic.
#[test]
fn the_truncation_marker_survives_into_the_composed_turn() {
    let request = json!({
        "prompt": "Rank today's stories.",
        "input": [source_envelope("BIG_SOURCE", 200_000)],
    });
    let (instruction, _) = append_upstream_input(
        &message_from_request(&request),
        &request,
        upstream::DEFAULT_UPSTREAM_BUDGET_CHARS,
    );
    let message = compose_turn_message(&instruction, Some("today's sport"));
    assert!(message.starts_with("Rank today's stories."), "{message}");
    assert!(message.contains("TRUNCATED BY OPENCOMPANY"), "{message}");
    assert!(message.contains("Request for this run:"), "{message}");
    assert!(message.contains("today's sport"), "{message}");
}

/// A thousand-way fan-in — a `split_out` over a large array is all it takes —
/// must not smuggle a thousand truncation markers past the budget. This is
/// the fold-level twin of `upstream`'s
/// `a_thousand_oversized_sources_stay_inside_the_budget`, driven through the
/// real envelope shape rather than pre-rendered strings, because that is the
/// path a graph actually takes.
#[test]
fn a_thousand_way_fan_in_cannot_smuggle_its_markers_past_the_budget() {
    let budget = upstream::DEFAULT_UPSTREAM_BUDGET_CHARS;
    let inputs: Vec<Value> = (0..1_000)
        .map(|n| source_envelope(&format!("SOURCE_{n}"), 5_000))
        .collect();
    let request = json!({ "prompt": "Rank today's stories.", "input": inputs });
    let (message, report) =
        append_upstream_input(&message_from_request(&request), &request, budget);

    // The section itself is bounded by `budget`; the message adds only the
    // node's own instruction and the heading, which are not upstream text.
    assert!(
        message.chars().count() <= budget + MARKER_SLACK,
        "5,000,000 characters of upstream input across 1,000 sources produced a {}-character \
         turn",
        message.chars().count()
    );
    assert_eq!(report.sources.len(), 1_000, "every input is accounted for");
    let notice = report.notice().expect("the operator is told");
    assert!(notice.contains("1000 sources"), "{notice}");
}

/// A source rendered as JSON (a `transform` / structured `tool_call` output,
/// which has no prose `text`) is bounded on the same path — the bound is on
/// what the turn carries, not on which node kind produced it.
#[test]
fn a_structured_source_is_bounded_on_the_same_path() {
    let budget = upstream::DEFAULT_UPSTREAM_BUDGET_CHARS;
    let rows: Vec<Value> = (0..20_000)
        .map(|n| json!({ "headline": format!("story {n}"), "score": n }))
        .collect();
    let request = json!({
        "prompt": "Rank these.",
        "input": [{ "json": { "rows": rows }, "text": null, "raw": {} }],
    });
    let (message, report) =
        append_upstream_input(&message_from_request(&request), &request, budget);

    assert!(
        message.chars().count() <= budget + MARKER_SLACK,
        "a structured payload is bounded too: {} characters",
        message.chars().count()
    );
    assert!(message.contains("TRUNCATED BY OPENCOMPANY"), "{message}");
    assert!(report.truncated_any());
}

#[test]
fn workflow_workspace_is_unique_per_run_and_traversal_safe() {
    let root = std::path::Path::new("/tmp/workspaces");
    let company = CompanyId::new("acme");
    let first = workflow_workspace(root, &company, "../billing", "run:1");
    let second = workflow_workspace(root, &company, "../billing", "run:2");

    assert_ne!(first, second);
    assert!(first.starts_with(root.join("acme").join("_workflow")));
    assert!(!first.to_string_lossy().contains("../billing"));
    assert_eq!(
        first.file_name().and_then(|part| part.to_str()),
        Some("workspace")
    );
}

/// Issue #499. tinyflows 0.6 added `Capabilities::memory`, and this pins the
/// answer we gave it.
///
/// `None` is a decision, not an omission — see the comment at the field. A
/// `MemoryProvider` here would let a workflow read and *write* agent memory
/// (`remember`/`forget` are on the trait), and which scopes a workflow may
/// touch is a policy question this repo has not answered. Until it is,
/// unwired is the honest state: a `memory` node fails with a capability
/// error rather than quietly writing somewhere nobody authorised.
///
/// So this test is here to make wiring it a *deliberate* act. Whoever
/// changes it has to change this line too, which is where they will find the
/// question they need to answer first.
#[tokio::test]
async fn the_memory_capability_is_left_unwired_on_purpose() {
    let dir = tempfile::tempdir().expect("tempdir");
    // No endpoint is spawned: `build_capabilities` assembles a struct of
    // handles and never calls the provider, so a base URL that answers
    // nothing is sufficient and keeps this off the network.
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(
        "http://127.0.0.1:1/unused".to_string(),
        dir.path(),
    );
    let record = crate::workflows::gated_tool_turn_tests::record();

    let caps = build_capabilities(
        single_turn(&deps),
        deps,
        &record,
        RunContext {
            workflow_id: "wf",
            run_id: "run:1",
            checkpoint_thread_id: "run:1",
            workflow_fingerprint: "fp:1",
            run_request: None,
            trigger_input: &Value::Null,
            started_by: crate::ports::types::StartedBy::Operator,
            dry_run: false,
            notices: RunNotices::default(),
            board: RunBoard::default(),
            blocks: Default::default(),
            capped: Default::default(),
            halted: Default::default(),
            approvals: Default::default(),
            artifacts: Default::default(),
            runs: None,
            deep: None,
            attempts: Default::default(),
            child_gates: Default::default(),
        },
    )
    .await
    .expect("build_capabilities");

    assert!(
        caps.memory.is_none(),
        "wiring `Capabilities::memory` gives workflows read AND write access \
         to agent memory — settle which scopes a workflow may touch before \
         changing this, and say so at the field"
    );
    // The neighbouring optional capability IS wired, so this is a statement
    // about `memory` specifically rather than about the bundle being empty.
    assert!(
        caps.agent.is_some(),
        "agent capability should still be wired"
    );
}

/// Issue #542 — T9: a dry bundle wires the effect STUBS (agent / tools / http
/// all echo with the `dry_run` marker) and the inert `NoopState`, while the
/// read-only resolver stays real. Pinned behaviourally through the marker, so
/// a future refactor that quietly wired a real effect into a dry bundle fails
/// here.
#[tokio::test]
async fn a_dry_bundle_wires_stubs_and_noop_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(
        "http://127.0.0.1:1/unused".to_string(),
        dir.path(),
    );
    let record = crate::workflows::gated_tool_turn_tests::record();

    let caps = build_capabilities(
        single_turn(&deps),
        deps,
        &record,
        RunContext {
            workflow_id: "wf",
            run_id: "run:1",
            checkpoint_thread_id: "run:1",
            workflow_fingerprint: "fp:1",
            run_request: None,
            trigger_input: &Value::Null,
            started_by: crate::ports::types::StartedBy::Operator,
            dry_run: true,
            notices: RunNotices::default(),
            board: RunBoard::default(),
            blocks: Default::default(),
            capped: Default::default(),
            halted: Default::default(),
            approvals: Default::default(),
            artifacts: Default::default(),
            runs: None,
            deep: None,
            attempts: Default::default(),
            child_gates: Default::default(),
        },
    )
    .await
    .expect("build_capabilities");

    // http: the stub reports without sending, carrying the marker.
    //
    // A *public* URL, deliberately. This case used to use `127.0.0.1`, which
    // the real guard refuses — so it asserted that the dry slot answers `ok`
    // for a target no real run can reach, pinning issue #1048's false green
    // in place. The slot being the stub is what this test is about; whether a
    // given target is refused is `dry_run`'s own suite.
    let http_out = caps
        .http
        .request(json!({ "url": "https://example.com/hook" }), None)
        .await
        .expect("an allowed target is not refused by the dry stub");
    assert_eq!(
        http_out["dry_run"],
        json!(true),
        "http slot should be the dry stub"
    );

    // agent: the stub echoes with no pool routing.
    let agent = caps.agent.as_ref().expect("agent stub is wired");
    let agent_out = agent
        .run_agent("ceo", json!({ "prompt": "hi" }), None)
        .await
        .expect("dry agent never fails");
    assert_eq!(
        agent_out["dry_run"],
        json!(true),
        "agent slot should be the dry stub"
    );

    // state: NoopState — a load reads None and a store is dropped.
    assert_eq!(caps.state.load("k").await.expect("noop load"), None);
    caps.state.store("k", json!(1)).await.expect("noop store");
    assert_eq!(
        caps.state.load("k").await.expect("noop load"),
        None,
        "dry state must be the inert NoopState, never durable"
    );
}

// ── Issue #661 (M4): the unwired `llm` stub reports the RIGHT failure ──

/// T1 — the engine's output_parser auto-fix request (it calls `llm` to repair
/// a schema mismatch) surfaces the SCHEMA errors, not the generic bare-LLM
/// lead that used to mask them.
#[tokio::test]
async fn unwired_llm_surfaces_schema_errors_on_auto_fix_request() {
    let request = json!({
        "task": "coerce_to_schema",
        "schema": { "type": "object", "required": ["name", "age"] },
        "value": { "other": 1 },
        "errors": [
            "$: missing required property `name`",
            "$: missing required property `age`",
        ],
    });
    let EngineError::Capability(msg) = UnwiredLlm
        .complete(request, None)
        .await
        .expect_err("an unwired llm must error")
    else {
        panic!("expected a capability error");
    };
    // The real cause is present…
    assert!(
        msg.contains("failed schema validation"),
        "should carry the schema-validation lead: {msg}"
    );
    assert!(
        msg.contains("missing required property `name`")
            && msg.contains("missing required property `age`"),
        "should carry the specific schema failures: {msg}"
    );
    // …and it does NOT lead with the generic bare-LLM message that hid them.
    assert!(
        !msg.starts_with("workflow agent node has no roster agent"),
        "the schema failure must not be masked by the generic lead: {msg}"
    );
}

/// T2 — any other request (here an agent node with no `agent_ref`, whose
/// request is the node config) keeps the generic message byte-identical.
#[tokio::test]
async fn unwired_llm_keeps_generic_message_for_non_auto_fix_request() {
    let EngineError::Capability(msg) = UnwiredLlm
        .complete(json!({ "prompt": "hi" }), None)
        .await
        .expect_err("an unwired llm must error")
    else {
        panic!("expected a capability error");
    };
    assert_eq!(
        msg, BARE_LLM_UNWIRED_MESSAGE,
        "a non-auto-fix request must get the byte-identical generic message"
    );
}

/// T4 — a `coerce_to_schema` request whose `errors` is empty or missing (or
/// not an array of strings) falls back to the generic message rather than
/// emitting an empty schema-error string or panicking.
#[tokio::test]
async fn unwired_llm_falls_back_when_auto_fix_carries_no_errors() {
    for request in [
        json!({ "task": "coerce_to_schema" }),
        json!({ "task": "coerce_to_schema", "errors": [] }),
        json!({ "task": "coerce_to_schema", "errors": "oops" }),
        json!({ "task": "coerce_to_schema", "errors": [1, 2] }),
    ] {
        let EngineError::Capability(msg) = UnwiredLlm
            .complete(request.clone(), None)
            .await
            .expect_err("an unwired llm must error")
        else {
            panic!("expected a capability error for {request}");
        };
        assert_eq!(
            msg, BARE_LLM_UNWIRED_MESSAGE,
            "a coerce_to_schema request with no usable errors must fall back \
             to the generic message: {request}"
        );
    }
}

// ── Issue #661 (L2): a workspace mkdir failure aborts the live build ──

/// T5 — live mode with an impossible `workspace_root` (a path rooted under a
/// regular file) fails the build with a `Harness` error naming the path and
/// the underlying I/O cause, instead of warning past it and handing back a
/// bundle whose effects are rooted at a directory that does not exist.
#[tokio::test]
async fn build_capabilities_live_errors_when_workspace_cannot_be_created() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A regular file where a directory would need to be: `create_dir_all`
    // under it fails with ENOTDIR.
    let not_a_dir = dir.path().join("not-a-dir");
    std::fs::write(&not_a_dir, b"x").expect("write file");

    let (mut deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(
        "http://127.0.0.1:1/unused".to_string(),
        dir.path(),
    );
    deps.workspace_root = not_a_dir.clone();
    let record = crate::workflows::gated_tool_turn_tests::record();

    // `Capabilities` is not `Debug`, so match rather than `expect_err`.
    let err = match build_capabilities(
        single_turn(&deps),
        deps,
        &record,
        RunContext {
            workflow_id: "wf",
            run_id: "run:1",
            checkpoint_thread_id: "run:1",
            workflow_fingerprint: "fp:1",
            run_request: None,
            trigger_input: &Value::Null,
            started_by: crate::ports::types::StartedBy::Operator,
            dry_run: false, // live: the workspace mkdir runs
            notices: RunNotices::default(),
            board: RunBoard::default(),
            blocks: Default::default(),
            capped: Default::default(),
            halted: Default::default(),
            approvals: Default::default(),
            artifacts: Default::default(),
            runs: None,
            deep: None,
            attempts: Default::default(),
            child_gates: Default::default(),
        },
    )
    .await
    {
        Ok(_) => panic!("an uncreatable workspace must fail the build"),
        Err(err) => err,
    };

    let crate::error::OpenCompanyError::Harness(msg) = &err else {
        panic!("expected a Harness error, got {err:?}");
    };
    assert!(
        msg.contains("could not create its workspace directory"),
        "message should name the failure: {msg}"
    );
    assert!(
        msg.contains("not-a-dir"),
        "message should name the offending path: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("not a directory"),
        "message should carry the underlying I/O cause: {msg}"
    );
}

/// T6 — the same impossible root is harmless for a dry run: it builds no
/// workspace, so the bundle assembles fine.
#[tokio::test]
async fn build_capabilities_dry_ignores_an_impossible_workspace_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let not_a_dir = dir.path().join("not-a-dir");
    std::fs::write(&not_a_dir, b"x").expect("write file");

    let (mut deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(
        "http://127.0.0.1:1/unused".to_string(),
        dir.path(),
    );
    deps.workspace_root = not_a_dir;
    let record = crate::workflows::gated_tool_turn_tests::record();

    build_capabilities(
        single_turn(&deps),
        deps,
        &record,
        RunContext {
            workflow_id: "wf",
            run_id: "run:1",
            checkpoint_thread_id: "run:1",
            workflow_fingerprint: "fp:1",
            run_request: None,
            trigger_input: &Value::Null,
            started_by: crate::ports::types::StartedBy::Operator,
            dry_run: true, // dry: no workspace mkdir at all
            notices: RunNotices::default(),
            board: RunBoard::default(),
            blocks: Default::default(),
            capped: Default::default(),
            halted: Default::default(),
            approvals: Default::default(),
            artifacts: Default::default(),
            runs: None,
            deep: None,
            attempts: Default::default(),
            child_gates: Default::default(),
        },
    )
    .await
    .expect("a dry build never touches the workspace");
}
