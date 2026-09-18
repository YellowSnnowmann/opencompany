use super::tests_budget_postcondition::ScriptedTurn;
use super::tests_multi_call_notices::overflowing_runner_notices;
use super::*;

/// CodeRabbit review on #1937 (issue #1866) — confirms the fix covers
/// `field_present` with the documented `json.items` dotted path, not just
/// `non_empty_list`'s no-field form (the two are fixed by the same
/// envelope change: the reply is best-effort JSON-parsed into a `json`
/// key, and `field_present`'s existing dotted-path resolution reaches it
/// like any other nested object). On the code as it stood before the fix,
/// this assertion fails: the envelope carried no `json` key at all, so
/// `json.items` could never resolve.
#[tokio::test]
async fn a_reply_that_is_json_satisfies_field_present_on_a_json_dotted_path() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1937-postcondition-field-present-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "{\"items\": [1, 2, 3]}".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1937c"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1937c"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1937c".to_string(),
        "run-1937c".to_string(),
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

    let (value, outcome) = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "lister",
                "prompt": "reply with a JSON object naming items",
                "postcondition": { "require": "field_present", "field": "json.items" }
            }),
        )
        .await
        .expect(
            "a reply that IS a JSON object carrying `items` must satisfy \
             `field_present` on the documented `json.items` path",
        );
    assert_eq!(outcome.reply, "{\"items\": [1, 2, 3]}");
    assert_eq!(value["text"], "{\"items\": [1, 2, 3]}");
}

/// Codex review on #1937 (issue #1866) — the emitted-value companion to
/// the test above: `value` (the tuple's first element) is exactly what
/// `run`'s `AgentRunOutcome.json` becomes (`json: value.clone()`, a few
/// lines below this call site), which tinyflows' `finish_agent_run`
/// (`nodes/integration/agent.rs`) then lands unchanged at the item
/// envelope's `json` whenever it is an `Object`/`Array` — i.e. `value`
/// literally IS what a downstream `=item.json.<field>` binding reads.
/// Before merging `parsed_reply`'s fields into `value` (Codex
/// #3893330383), this was `{"text": ..., "agent_ref": ...}` regardless of
/// what the reply parsed to, so the gate above could pass while
/// `value["items"]` (and therefore `item.json.items` downstream) stayed
/// absent. See `a_structured_agent_reply_is_readable_by_a_downstream_json_binding`
/// in `workflows::runner` for the same claim proven through a real
/// two-node graph with an actual `=item.json.items` expression, not just
/// this unit-level inspection of the returned tuple.
#[tokio::test]
async fn the_parsed_reply_lands_in_the_emitted_value_a_downstream_binding_reads() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1937-postcondition-emitted-value-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "{\"items\": [1, 2, 3]}".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1937d"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1937d"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1937d".to_string(),
        "run-1937d".to_string(),
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
                "node_id": "lister",
                "prompt": "reply with a JSON object naming items",
                "postcondition": { "require": "field_present", "field": "json.items" }
            }),
        )
        .await
        .expect("the postcondition is satisfied, so the turn must succeed");

    // `text`/`agent_ref` must survive the merge unchanged — delivery.rs's
    // report_text reads a delivered report's body via `item.json.text`
    // and must keep finding the raw reply string here, not the parsed
    // object's own (absent, in this reply) `text` key.
    assert_eq!(value["text"], "{\"items\": [1, 2, 3]}");
    assert_eq!(value["agent_ref"], "researcher");
    // The actual finding: the SAME value `field = "json.items"` certified
    // above must also be readable off the emitted value a downstream
    // binding sees.
    assert_eq!(value["items"], json!([1, 2, 3]));
}

/// CodeRabbit #3893565788 on #1937 — the "blast radius" proof. A node
/// with NO declared postcondition, whose reply happens to be valid JSON,
/// must emit the exact `{text, agent_ref}` shape it always has — the
/// merge must never run for a node that did not opt into structured
/// output evaluation. On the code as it stood right after the
/// #3893330383 fix (before this scoping), this assertion fails:
/// `revenue` would appear as a top-level key in `value`, changing the
/// output contract for every agent node in every existing workflow that
/// happens to reply with a JSON object, whether or not it ever declared
/// a postcondition.
#[tokio::test]
async fn a_reply_that_parses_as_json_is_not_merged_without_a_declared_postcondition() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1937-no-postcondition-json-reply-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "{\"revenue\": 12000, \"text\": \"ignored\"}".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1937e"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1937e"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1937e".to_string(),
        "run-1937e".to_string(),
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

    // No `postcondition` key at all — the ordinary, overwhelmingly common
    // case: an agent node nobody ever asked to declare a run-safety gate.
    let (value, outcome) = runner
        .run_turn(
            "researcher",
            json!({ "node_id": "analyst", "prompt": "give me the numbers" }),
        )
        .await
        .expect("a node with no postcondition must not be gated at all");

    assert_eq!(outcome.reply, "{\"revenue\": 12000, \"text\": \"ignored\"}");
    assert_eq!(
        value,
        json!({
            "text": "{\"revenue\": 12000, \"text\": \"ignored\"}",
            "agent_ref": "researcher",
        }),
        "a node with no declared postcondition must emit exactly {{text, agent_ref}} \
         regardless of what the reply parses as — no `revenue` key, and `text` must \
         stay the raw reply string, not the parsed object's own `text` value: {value}"
    );
}

/// Codex #3893541856 on #1937 — the bare-array companion to the object
/// merge above. A node whose declared `non_empty_list` (no `field`)
/// passes against a bare JSON-array reply must emit that array itself as
/// `value`, not the `{text, agent_ref}` wrapper the gate never validated
/// — otherwise a downstream `=item.json` binding (reading the whole
/// value, not a dotted field into it) resolves to the wrapper instead of
/// the array the gate certified, reproducing the exact defect
/// #3893330383 fixed for the object case.
#[tokio::test]
async fn a_bare_array_reply_replaces_the_emitted_value_wholesale() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1937-bare-array-emission-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "[\"x\", \"y\"]".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1937f"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1937f"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1937f".to_string(),
        "run-1937f".to_string(),
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

    let (value, outcome) = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "lister",
                "prompt": "list two things",
                "postcondition": { "require": "non_empty_list" }
            }),
        )
        .await
        .expect("a reply that IS a non-empty JSON array must satisfy non_empty_list");

    // The gate certified the array; the emitted value must literally BE
    // that array — not an object wrapping it, and not the old
    // `{text, agent_ref}` shape.
    assert_eq!(value, json!(["x", "y"]));
    // The raw reply string is still available independently: `outcome`
    // (a distinct field from `value`) and `AgentRunOutcome.text` (built
    // from `outcome.reply` directly, not from `value`) both still carry
    // it — nothing that reads the prose loses it.
    assert_eq!(outcome.reply, "[\"x\", \"y\"]");
}

/// Codex #3894162757 on #1937 — supersedes a prior round's
/// `a_bare_scalar_reply_replaces_the_emitted_value_wholesale`, which
/// asserted `run_turn`'s OWN return value and never noticed that
/// tinyflows nulls a bare scalar one layer further out (see the doc
/// comment on the removed `Value::Bool(_) | Value::Number(_) |
/// Value::String(_)` emission arm, and
/// `workflows::runner::tests_node_output::a_scalar_reply_cannot_satisfy_field_present_on_the_bare_json_root`
/// for the full-graph proof of the delivery gap that test missed).
/// `field_present` on the bare `field = "json"` root can now never
/// pass for a scalar reply — the gate refuses to certify a shape it
/// knows cannot reach a downstream `=item.json` binding.
#[tokio::test]
async fn a_bare_scalar_reply_fails_field_present_on_the_bare_json_root() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1937-bare-scalar-rejected-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "42".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1937g"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1937g"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1937g".to_string(),
        "run-1937g".to_string(),
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
                "node_id": "scorer",
                "prompt": "reply with a single confidence score",
                "postcondition": { "require": "field_present", "field": "json" }
            }),
        )
        .await;

    let err = result.expect_err(
        "a bare scalar reply (`42`) must NOT satisfy field_present on the bare \
         `json` root — tinyflows can never deliver a scalar through \
         `=item.json` (it normalizes anything but Object/Array to null), so \
         certifying it would pass a gate whose value the workflow can never \
         actually read",
    );
    let EngineError::Capability(message) = err else {
        panic!("expected a capability error");
    };
    assert!(
        message.contains("json") && message.contains("scalar"),
        "the halting message should say why a scalar under `json` cannot \
         satisfy this gate: {message}"
    );
}

/// Codex #3894038816 on #1937 — the silent-disable finding, traced
/// end-to-end rather than inferred. `postcondition` rides inside the
/// engine-resolved node config (`translate_node` writes it as an
/// ordinary config key, same as `on_error`/`retry` — see the module doc
/// above the function), so `tinyflows::expr::resolve` — the SAME
/// resolution `nodes::execution::resolve_config_traced` runs on the
/// whole node config before an agent node's turn — walks straight into
/// it. An authored `field = "=item.missing"` is an ordinary
/// `=`-expression as far as that resolver is concerned; it does not know
/// or care that this particular leaf is a safety policy rather than
/// ordinary data.
///
/// Step 1 below proves `translate()` carries the expression through
/// UNRESOLVED (translation is not where resolution happens). Step 2
/// proves the mechanism concretely: running the real
/// `tinyflows::expr::resolve` against a scope whose `item` genuinely
/// lacks `missing` (the ordinary case the author meant to catch) turns
/// `postcondition.field` into a plain `Value::Null` — indistinguishable,
/// at that point, from no `field` having been authored at all. Step 3
/// feeds exactly that resolved shape to `run_turn`.
///
/// `field = "=item.missing"` cannot reach this point through
/// `parse_workflow` today — `workflow_file::validate`'s bare-structured-
/// root check (`postcondition_field_with_a_bare_structured_root_is_rejected`)
/// rejects it as a byproduct, since no `=`-expression's first dotted
/// segment can ever equal `json`/`text`/`agent_ref`. This test builds the
/// node directly instead (the same technique
/// `agent_ref_survives_a_spoofing_config` in `workflows::translate` uses)
/// to isolate the SECOND, independent layer: `evaluate_postcondition`
/// must not silently pass just because *something upstream* — this
/// resolution step today, a future one tomorrow — turned a validated
/// `field` into null before `run_turn` ever saw it.
///
/// RED on the code as it stood before the `evaluate_postcondition` fix:
/// `run_turn` returned `Ok`, for a reply ("just prose, no items here")
/// that plainly satisfies nothing — the gate silently did not run.
#[tokio::test]
async fn a_field_resolved_away_by_an_authored_expression_fails_closed_at_run_turn() {
    use crate::company::{
        WorkflowFile, WorkflowNodeDef, WorkflowNodeKind, WorkflowPostconditionDef,
    };
    use crate::workflows::translate::translate;

    // Step 1 — author `field = "=item.missing"` directly on the model
    // (bypassing `parse_workflow`/`validate`, per the doc comment above),
    // and confirm `translate()` carries it through as the literal
    // expression string — translation does not resolve expressions.
    let file = WorkflowFile {
        global: false,
        id: "wf".into(),
        name: "WF".into(),
        description: None,
        owner_desk: None,
        nodes: vec![WorkflowNodeDef {
            id: "worker".into(),
            kind: WorkflowNodeKind::Agent,
            name: "Worker".into(),
            summary: None,
            agent: Some("researcher".into()),
            schedule: None,
            config: None,
            on_error: None,
            retry: None,
            requires_approval: None,
            repeatable: None,
            destination: None,
            postcondition: Some(WorkflowPostconditionDef {
                require: "field_present".to_string(),
                field: Some("=item.missing".to_string()),
            }),
            verify: None,
        }],
        edges: Vec::new(),
    };
    let graph = translate(&file);
    let node_config = graph.nodes[0].config.clone();
    assert_eq!(
        node_config["postcondition"]["field"], "=item.missing",
        "translate() must carry the authored expression through UNRESOLVED —              it is config resolution, not translate(), that evaluates it"
    );

    // Step 2 — run the SAME resolution the engine runs
    // (`tinyflows::nodes::execution::resolve_config_traced` calls
    // `tinyflows::expr::resolve` on the whole config tree) against a
    // scope whose `item` genuinely has no `missing` key — the ordinary
    // case `=item.missing` exists to catch.
    let scope = json!({ "item": { "other_field": "present, but not the missing key" } });
    let resolved_config = tinyflows::expr::resolve(&node_config, &scope);
    assert_eq!(
        resolved_config["postcondition"]["field"],
        Value::Null,
        "traced: config resolution turns the authored `=item.missing` into a              plain JSON null before run_turn ever sees it"
    );

    // Step 3 — feed exactly that resolved postcondition to `run_turn`,
    // with a reply that plainly does not satisfy any real check.
    let dir = tempfile::Builder::new()
        .prefix("oc-1937-expression-field-resolved-away-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "just prose, no items here".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1937h"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1937h"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1937h".to_string(),
        "run-1937h".to_string(),
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

    let request = json!({
        "node_id": "worker",
        "prompt": "say something",
        "postcondition": resolved_config["postcondition"].clone(),
    });

    let result = runner.run_turn("researcher", request).await;

    let err = result.expect_err(
        "a postcondition whose `field` resolved away to null must halt the node —              the gate silently not running is worse than the gate certifying the wrong              value",
    );
    let EngineError::Capability(message) = err else {
        panic!("expected a capability error");
    };
    assert!(
        message.contains("field_present"),
        "the halting message should name the predicate that could not be              evaluated: {message}"
    );
}

/// Codex #3893619015 on #1937 — traces the underlying mechanism this
/// finding names, at the layer `evaluate_postcondition`/`run_turn`
/// operates on. `postcondition_field_into_reserved_json_key_is_rejected`
/// in `company::workflow_file::tests` is the actual fix: `validate()`
/// refuses `field: "json.text"`/`"json.agent_ref"` at author time, so no
/// graph that ever reaches `run_turn` in production can carry one. This
/// test constructs the request `run_turn` would see if that guarantee
/// were ever bypassed, to pin — and make visible — exactly why the
/// validation-time rejection is the right layer for the fix rather than
/// something patchable here: `text`/`agent_ref` are inserted into `value`
/// FIRST and merged with `or_insert` (base wins), on purpose, so
/// `delivery.rs::report_text` keeps finding the raw reply string for the
/// overwhelming majority of nodes whose reply is plain prose — the same
/// base-wins rule that protects that majority is exactly what makes a
/// `field` colliding with one of those two reserved keys validate a
/// value the emitted output can never actually hold.
#[tokio::test]
async fn a_colliding_field_would_diverge_between_gate_and_emitted_value() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1937-colliding-field-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let record = crate::workflows::gated_tool_turn_tests::record();
    let turn = Arc::new(ScriptedTurn(crate::harness::TurnOutcome {
        reply: "{\"text\": [\"a\", \"b\"], \"agent_ref\": 123}".to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    }));
    let board_claim = Arc::new(deps.delegations.claim_board("run-1937g"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1937g"));
    let runner = HarnessAgentRunner::new(
        turn,
        deps,
        record,
        CompanyId::new("acme"),
        "wf-1937g".to_string(),
        "run-1937g".to_string(),
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

    // `field = "json.text"`: the parsed reply's OWN `text` key is an
    // array. `field_present` only asks "is this present and non-null" —
    // it passes, having validated an ARRAY.
    let (value, _outcome) = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "lister",
                "prompt": "reply with structured data",
                "postcondition": { "require": "field_present", "field": "json.text" }
            }),
        )
        .await
        .expect("field_present on json.text finds the parsed reply's own text key, an array");

    // But the emitted `value["text"]` — what a downstream `=item.json.text`
    // binding actually reads — is the RAW REPLY STRING (`or_insert`, base
    // wins), a completely different type from the array the gate just
    // validated. Gate green; downstream gets a string where the author
    // was told to expect (and validated) a non-empty array.
    assert!(
        value["text"].is_string(),
        "value[\"text\"] must still be the raw reply string (the report_text              guarantee), not the array the gate validated: {value}"
    );
    assert_ne!(
        value["text"],
        json!(["a", "b"]),
        "the gate validated json.text as an array, but the emitted value's              text key is a different value entirely: {value}"
    );

    // Same divergence on `agent_ref`: the parsed reply's own `agent_ref`
    // is the number 123; the gate's `field_present` on `json.agent_ref`
    // passes on that number, but the emitted `value["agent_ref"]` is the
    // real roster id string, not 123.
    let (value2, _outcome2) = runner
        .run_turn(
            "researcher",
            json!({
                "node_id": "lister",
                "prompt": "reply with structured data",
                "postcondition": { "require": "field_present", "field": "json.agent_ref" }
            }),
        )
        .await
        .expect("field_present on json.agent_ref finds the parsed reply's own agent_ref key");
    assert_eq!(
        value2["agent_ref"], "researcher",
        "the emitted agent_ref must stay the real roster id (not the model-supplied              123 the gate validated): {value2}"
    );
}

/// Issue #638: a node that gates more calls than the cap allows leaves the
/// operator a **notice**, not only a log line.
///
/// Asserted on `RunNotices` — the value that becomes `WorkflowRun::notices`
/// and then the journaled outcome the history panel reads — rather than on
/// a log, which is what the issue asks for and what the chat path already
/// had via #561.
#[tokio::test]
async fn an_overflowing_node_leaves_the_operator_a_notice() {
    let over = MAX_APPROVAL_REQUESTS_PER_TURN + 3;
    let (notices, queue) = overflowing_runner_notices(over, true).await;

    assert_eq!(notices.len(), 1, "one notice for one overflow: {notices:?}");
    let notice = &notices[0];
    assert!(
        notice.contains(&format!("at most {MAX_APPROVAL_REQUESTS_PER_TURN}")),
        "it must quote the cap that did the discarding: {notice}"
    );
    assert!(notice.contains('3'), "…and how many went past it: {notice}");
    assert_eq!(
        queue.drain(MAX_APPROVAL_REQUESTS_PER_TURN).requests.len(),
        0,
        "the drain already emptied this run's scope"
    );
}
