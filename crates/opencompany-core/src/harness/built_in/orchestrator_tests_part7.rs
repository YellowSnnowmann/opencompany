use super::*;

/// Issue #661 (H1): a `tool_call` with no `slug` is still rejected — the
/// inherited author-time gate, now reachable with a useful message instead of
/// the tool being unable to author a `tool_call` at all.
#[tokio::test]
async fn create_workflow_tool_rejects_tool_call_without_slug() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let tool = CreateWorkflowTool::new(company, None, store, None, WorkflowRefQueue::default());
    let result = tool
        .execute(json!({
            "id": "bad",
            "name": "Bad",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                { "id": "grab", "kind": "tool_call", "name": "Grab" },
                { "id": "done", "kind": "output", "name": "Report" }
            ],
            "edges": [
                { "from": "start", "to": "grab" },
                { "from": "grab", "to": "done" }
            ]
        }))
        .await
        .expect("execute");
    assert!(result.is_error, "{result:?}");
    assert!(
        result.output_for_llm(false).contains("slug"),
        "the refusal names the missing slug: {result:?}"
    );
}

/// Issue #661 (H1): the exact GitHub/Composio failure mode — a `tool_call`
/// naming an agent-turn tool family (`composio_execute`) can never run on a
/// workflow `tool_call` node, so it is refused at save. Gated on `openhuman`
/// because the namespace resolution (`namespace_of`) lives behind it.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn create_workflow_tool_rejects_agent_turn_tool_call() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let tool = CreateWorkflowTool::new(company, None, store, None, WorkflowRefQueue::default());
    let result = tool
        .execute(json!({
            "id": "gh",
            "name": "GitHub",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                {
                    "id": "call",
                    "kind": "tool_call",
                    "name": "Call",
                    "config": { "slug": "composio_execute" }
                },
                { "id": "done", "kind": "output", "name": "Report" }
            ],
            "edges": [
                { "from": "start", "to": "call" },
                { "from": "call", "to": "done" }
            ]
        }))
        .await
        .expect("execute");
    assert!(result.is_error, "{result:?}");
    assert!(
        result.output_for_llm(false).contains("agent-turn"),
        "the refusal explains it is an agent-turn family, not a workflow tool: {result:?}"
    );
}

/// Issue #661 (H1): a JSON `null` inside a node's `config` can't be stored —
/// TOML has no null — so the fallible `TryFrom` conversion refuses it as an
/// agent-actionable error, never a panic or a silently-dropped key.
#[tokio::test]
async fn create_workflow_tool_rejects_null_config_value() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let tool = CreateWorkflowTool::new(company, None, store, None, WorkflowRefQueue::default());
    let result = tool
        .execute(json!({
            "id": "nullish",
            "name": "Nullish",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                {
                    "id": "grab",
                    "kind": "tool_call",
                    "name": "Grab",
                    "config": { "slug": "web_fetch", "args": { "url": null } }
                },
                { "id": "done", "kind": "output", "name": "Report" }
            ],
            "edges": [
                { "from": "start", "to": "grab" },
                { "from": "grab", "to": "done" }
            ]
        }))
        .await
        .expect("execute");
    assert!(result.is_error, "{result:?}");
    assert!(
        result.output_for_llm(false).contains("TOML has no null"),
        "the refusal explains why the config can't be stored: {result:?}"
    );
}

/// Issue #674 boundary: an agent-authored `tool_call` whose `config.args`
/// carries a templated `=`-expression is rejected — that node would take
/// saved-node runtime position with model-chosen templated args, collapsing
/// the two-operator-gate model. The refusal names the node and points at the
/// console for templated wiring. Feature-independent: the check runs in the
/// `TryFrom`, before any namespace/grant gate.
#[tokio::test]
async fn create_workflow_tool_rejects_tool_call_expression_args() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_granting_web(&company)));
    let tool = CreateWorkflowTool::new(
        company.clone(),
        None,
        store.clone(),
        None,
        WorkflowRefQueue::default(),
    );
    let result = tool
        .execute(json!({
            "id": "templated",
            "name": "Templated",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                {
                    "id": "grab",
                    "kind": "tool_call",
                    "name": "Grab",
                    "config": { "slug": "web_fetch", "args": { "url": "=item.url" } }
                },
                { "id": "done", "kind": "output", "name": "Report" }
            ],
            "edges": [
                { "from": "start", "to": "grab" },
                { "from": "grab", "to": "done" }
            ]
        }))
        .await
        .expect("execute");
    assert!(result.is_error, "{result:?}");
    let msg = result.output_for_llm(false);
    assert!(
        msg.contains("=`-expression") && msg.contains("config.args.url"),
        "the refusal names the templated expression and its location: {result:?}"
    );
    // Nothing was persisted — the reject happens before the store write.
    let record = store.load(&company).await.unwrap().unwrap();
    assert!(
        record.overlay_workflows.is_empty(),
        "a rejected draft persists nothing"
    );
}

/// Issue #674 boundary, positive half: the same `tool_call` with a LITERAL
/// arg (no `=` prefix) persists — the restriction is on templated
/// `=`-expressions, not on args as such.
#[tokio::test]
async fn create_workflow_tool_persists_tool_call_literal_args() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_granting_web(&company)));
    let tool = CreateWorkflowTool::new(
        company.clone(),
        None,
        store.clone(),
        None,
        WorkflowRefQueue::default(),
    );
    let result = tool
        .execute(json!({
            "id": "literal",
            "name": "Literal",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                {
                    "id": "grab",
                    "kind": "tool_call",
                    "name": "Grab",
                    "config": { "slug": "web_fetch", "args": { "url": "https://example.com" } }
                },
                { "id": "done", "kind": "output", "name": "Report" }
            ],
            "edges": [
                { "from": "start", "to": "grab" },
                { "from": "grab", "to": "done" }
            ]
        }))
        .await
        .expect("execute");
    assert!(!result.is_error, "{result:?}");
    let record = store.load(&company).await.unwrap().unwrap();
    assert_eq!(record.overlay_workflows.len(), 1);
    assert!(
        record.overlay_workflows[0]
            .toml
            .contains("url = \"https://example.com\""),
        "the persisted graph carries the literal arg: {}",
        record.overlay_workflows[0].toml
    );
}

/// Issue #661 (H1): the `=`-expression restriction is scoped to `tool_call`.
/// A `condition` node legitimately branches on a `config.field` expression,
/// so a `=`-prefixed field must NOT be rejected — proving the guard doesn't
/// over-reach into the kinds that resolve expressions by design.
#[tokio::test]
async fn create_workflow_tool_allows_condition_expression_field() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let tool = CreateWorkflowTool::new(
        company.clone(),
        None,
        store.clone(),
        None,
        WorkflowRefQueue::default(),
    );
    let result = tool
        .execute(json!({
            "id": "brancher",
            "name": "Brancher",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                {
                    "id": "check",
                    "kind": "condition",
                    "name": "Check",
                    "config": { "field": "=item.ok" }
                },
                { "id": "yes", "kind": "output", "name": "Yes" },
                { "id": "no", "kind": "output", "name": "No" }
            ],
            "edges": [
                { "from": "start", "to": "check" },
                { "from": "check", "to": "yes", "label": "yes" },
                { "from": "check", "to": "no", "label": "no" }
            ]
        }))
        .await
        .expect("execute");
    assert!(
        !result.is_error,
        "a condition's `=`-expression field is allowed: {result:?}"
    );
}

/// Issue #661 (H1): a non-object `config` (here a bare string on an
/// `http_request` node — the path that would otherwise persist silently) is
/// refused with an agent-actionable message, not saved as an inert TOML
/// scalar.
#[tokio::test]
async fn create_workflow_tool_rejects_non_object_config() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let tool = CreateWorkflowTool::new(company, None, store, None, WorkflowRefQueue::default());
    let result = tool
        .execute(json!({
            "id": "scalar",
            "name": "Scalar",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                {
                    "id": "call",
                    "kind": "http_request",
                    "name": "Call",
                    "config": "GET https://example.com"
                },
                { "id": "done", "kind": "output", "name": "Report" }
            ],
            "edges": [
                { "from": "start", "to": "call" },
                { "from": "call", "to": "done" }
            ]
        }))
        .await
        .expect("execute");
    assert!(result.is_error, "{result:?}");
    assert!(
        result.output_for_llm(false).contains("non-object `config`"),
        "the refusal explains config must be a JSON object: {result:?}"
    );
}

/// Issue #661 (H1) — item #2: a `destination` on a non-`output` node is
/// already rejected end-to-end by the shared `validate` (`render_workflow` →
/// `parse_workflow` inside `create_company_workflow`), so the create_workflow
/// tool inherits the catch with no duplicated validation of its own. This
/// pins that end-to-end behaviour; the shared-validator hardening is #682's.
#[tokio::test]
async fn create_workflow_tool_rejects_destination_on_non_output() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let tool = CreateWorkflowTool::new(company, None, store, None, WorkflowRefQueue::default());
    let result = tool
        .execute(json!({
            "id": "misrouted",
            "name": "Misrouted",
            "nodes": [
                {
                    "id": "start",
                    "kind": "trigger",
                    "name": "Start",
                    "destination": { "kind": "owner" }
                },
                { "id": "done", "kind": "output", "name": "Report" }
            ],
            "edges": [ { "from": "start", "to": "done" } ]
        }))
        .await
        .expect("execute");
    assert!(result.is_error, "{result:?}");
    assert!(
        result
            .output_for_llm(false)
            .contains("only `output` nodes route a report"),
        "the shared validator's destination-placement message surfaces: {result:?}"
    );
}

/// Issue #661 (H1) — item #3: the JSON→TOML conversion remedy is conditional.
/// A failure that is NOT about a null (here a `u64` beyond `i64` range) must
/// get the converter's own message WITHOUT the misleading "TOML has no null"
/// hint — the null case keeps that hint (`create_workflow_tool_rejects_null_config_value`).
#[tokio::test]
async fn create_workflow_tool_non_null_conversion_error_omits_null_hint() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let tool = CreateWorkflowTool::new(company, None, store, None, WorkflowRefQueue::default());
    let result = tool
        .execute(json!({
            "id": "toobig",
            "name": "TooBig",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                {
                    "id": "grab",
                    "kind": "tool_call",
                    "name": "Grab",
                    "config": { "slug": "web_fetch", "args": { "n": 18446744073709551615u64 } }
                },
                { "id": "done", "kind": "output", "name": "Report" }
            ],
            "edges": [
                { "from": "start", "to": "grab" },
                { "from": "grab", "to": "done" }
            ]
        }))
        .await
        .expect("execute");
    assert!(result.is_error, "{result:?}");
    let msg = result.output_for_llm(false);
    assert!(
        msg.contains("can't be stored"),
        "names the failure: {result:?}"
    );
    assert!(
        !msg.contains("TOML has no null"),
        "a non-null conversion failure must not misdirect to the null remedy: {result:?}"
    );
}

#[test]
fn first_expression_location_walks_nested_config() {
    // Matches tinyflows' `is_expression`: a leading `=` (no trim).
    assert_eq!(
        first_expression_location(&json!({ "args": { "command": "=item.x" } }), ""),
        Some("args.command".to_string())
    );
    // Array elements become numeric segments.
    assert_eq!(
        first_expression_location(&json!({ "args": { "cc": ["a", "=item.y"] } }), ""),
        Some("args.cc.1".to_string())
    );
    // Literals — including a `=` in the MIDDLE — are not expressions.
    assert_eq!(
        first_expression_location(&json!({ "args": { "q": "a=b", "s": "ls -la" } }), ""),
        None
    );
}

#[test]
fn json_contains_null_is_recursive() {
    assert!(json_contains_null(&json!({ "args": { "url": null } })));
    assert!(json_contains_null(&json!(["ok", [null]])));
    assert!(!json_contains_null(
        &json!({ "args": { "url": "https://x" } })
    ));
}

/// T1: a clipped preview names exactly how many characters it dropped, and
/// counts them in `chars()` — so a multibyte string past the boundary
/// reports codepoints dropped, never bytes, and never panics on a byte
/// index that lands mid-character.
#[test]
fn preview_marks_the_exact_dropped_char_count_including_multibyte() {
    // 130 ASCII chars → 120 kept, 10 dropped.
    let ascii = "a".repeat(130);
    let preview = preview_item(&json!(ascii));
    assert!(preview.ends_with("… (+10 chars)"), "{preview}");
    assert!(preview.starts_with(&"a".repeat(120)), "{preview}");
    // 120 kept chars, then the '…' and the marker — the kept body is exactly
    // the cap, not one over.
    assert_eq!(preview.chars().take_while(|c| *c == 'a').count(), 120);

    // A multibyte fill: 130 'é' (2 bytes each). The marker must count the 10
    // dropped *characters*, not their 20 bytes, and the boundary must not
    // split a codepoint.
    let multibyte = "é".repeat(130);
    let preview = preview_item(&json!(multibyte));
    assert!(preview.ends_with("… (+10 chars)"), "{preview}");
    assert_eq!(preview.chars().take_while(|c| *c == 'é').count(), 120);

    // At or below the cap there is no marker at all.
    let short = "x".repeat(ITEM_PREVIEW_CHARS);
    assert_eq!(preview_item(&json!(short)), short);
}

/// T2: a node with more than one item is labelled `last of N items`, and the
/// summary footer names the companion tool via the `READ_RUN_OUTPUT_TOOL`
/// const (so wording can't drift) and embeds the run id.
#[test]
fn summary_labels_multi_item_nodes_and_footers_the_companion() {
    let file = crate::company::parse_workflow(DEMO_WF).unwrap();
    let run = WorkflowRun {
        output: json!({ "nodes": { "worker": { "items": ["first", "second", "third"] } } }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        cancelled: false,
        nodes: Vec::new(),
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    let md = summarize_run(&file, &run, "run-xyz", RunOutputStored::Stored);
    assert!(md.contains("last of 3 items — third"), "{md}");
    assert!(md.contains(READ_RUN_OUTPUT_TOOL), "{md}");
    assert!(md.contains("run-xyz"), "{md}");

    // A single-item node keeps the plain "1 item(s)" phrasing.
    let run_one = WorkflowRun {
        output: json!({ "nodes": { "worker": { "items": ["only"] } } }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        cancelled: false,
        nodes: Vec::new(),
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    let md = summarize_run(&file, &run_one, "run-1", RunOutputStored::Stored);
    assert!(md.contains("1 item(s) — only"), "{md}");
    assert!(!md.contains("last of"), "{md}");

    // The oversized footer sends the agent to the console run drawer instead
    // of to a `read_run_output` call that would find nothing cached.
    let md = summarize_run(
        &file,
        &run,
        "run-big",
        RunOutputStored::Oversized { bytes: 999 },
    );
    assert!(md.contains("console"), "{md}");
    assert!(md.contains("run drawer"), "{md}");
    assert!(md.contains("999 bytes"), "{md}");
    assert!(!md.contains("Read any node's full output"), "{md}");
}

/// Issue #981 (part 2): the summary says a report did not go out.
///
/// Before this, `summarize_run` never read `deliveries`, so a run whose
/// report was refused closed with "The run reached its terminal node(s)
/// without pausing for approval" and nothing else — a true sentence about a
/// run that had just dropped its only output, which the model then reported
/// upward as a clean run.
#[test]
fn the_summary_says_when_a_report_did_not_go_out() {
    let file = crate::company::parse_workflow(DEMO_WF).unwrap();
    let dropped = WorkflowRun {
        output: json!({ "nodes": { "worker": { "items": ["the report"] } } }),
        pending_approvals: Vec::new(),
        deliveries: vec![crate::ports::DeliveryReport {
            node: "worker".into(),
            kind: "channel".into(),
            target: Some("operator".into()),
            status: crate::ports::DeliveryStatus::Failed,
            detail: "`operator` is not an automation delivery channel — this runtime has:                          engineering"
                .into(),
            reason: crate::ports::DeliveryReason::ChannelNotWired,
        }],
        cancelled: false,
        nodes: Vec::new(),
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    let md = summarize_run(&file, &dropped, "run-drop", RunOutputStored::Stored);
    assert!(
        md.contains("1 report(s) did NOT reach a destination"),
        "{md}"
    );
    assert!(md.contains("`worker` (channel)"), "{md}");
    // The reason, from the closed set — never `detail`, which quotes what a
    // transport said and is for the operator's own surfaces (issue #248).
    assert!(
        md.contains(&crate::ports::DeliveryReason::ChannelNotWired.to_string()),
        "{md}"
    );
    assert!(
        !md.contains("this runtime has: engineering"),
        "the operator-only `detail` must not ride the summary: {md}"
    );
    // And it does not claim the graph broke: the per-node line still reports
    // what the node produced.
    assert!(md.contains("1 item(s) — the report"), "{md}");

    // A run that delivered fine says nothing about delivery at all, so an
    // ordinary summary is unchanged.
    let clean = WorkflowRun {
        deliveries: vec![crate::ports::DeliveryReport {
            node: "worker".into(),
            kind: "owner".into(),
            target: Some("ada@example.com".into()),
            status: crate::ports::DeliveryStatus::Sent,
            detail: "emailed the company's admin".into(),
            reason: crate::ports::DeliveryReason::OwnerEmailed,
        }],
        ..dropped.clone()
    };
    let md = summarize_run(&file, &clean, "run-ok", RunOutputStored::Stored);
    assert!(!md.contains("did NOT reach a destination"), "{md}");

    // A report parked for an operator's approval is waiting on a person,
    // not lost — counting it here would tell the model to go fix a queue
    // that is working.
    let parked = WorkflowRun {
        deliveries: vec![crate::ports::DeliveryReport {
            node: "worker".into(),
            kind: "email".into(),
            target: Some("new@example.com".into()),
            status: crate::ports::DeliveryStatus::Pending,
            detail: "waiting in Approvals".into(),
            reason: crate::ports::DeliveryReason::ParkedForApproval,
        }],
        ..dropped.clone()
    };
    let md = summarize_run(&file, &parked, "run-parked", RunOutputStored::Stored);
    assert!(!md.contains("did NOT reach a destination"), "{md}");

    // Issue #981, the second half. This paragraph's own prose is the
    // argument: it says the report "did not go out, and it will not without
    // a change". Neither is true of a test run, which attempted nothing on
    // purpose, nor of a continuation whose report an earlier run in the
    // lineage already sent — so telling the model to "fix the destination"
    // for either would send it at a graph that is behaving as designed.
    for reason in [
        crate::ports::DeliveryReason::DryRun,
        crate::ports::DeliveryReason::AlreadyDelivered,
    ] {
        let accounted = WorkflowRun {
            deliveries: vec![crate::ports::DeliveryReport {
                node: "worker".into(),
                kind: "channel".into(),
                target: Some("engineering".into()),
                status: crate::ports::DeliveryStatus::Skipped,
                detail: "nothing was sent".into(),
                reason,
            }],
            ..dropped.clone()
        };
        let md = summarize_run(&file, &accounted, "run-skip", RunOutputStored::Stored);
        assert!(
            !md.contains("did NOT reach a destination"),
            "{reason:?}: {md}"
        );
    }

    // The deliberate non-move: an `output` node with nowhere to send DID
    // lose its report, and the model is exactly the reader that should be
    // told (issues #925 / #947 / #963).
    let nowhere = WorkflowRun {
        deliveries: vec![crate::ports::DeliveryReport {
            node: "worker".into(),
            kind: "none".into(),
            target: None,
            status: crate::ports::DeliveryStatus::Skipped,
            detail: "this output node has no destination".into(),
            reason: crate::ports::DeliveryReason::NoDestinationConfigured,
        }],
        ..dropped.clone()
    };
    let md = summarize_run(&file, &nowhere, "run-nowhere", RunOutputStored::Stored);
    assert!(
        md.contains("1 report(s) did NOT reach a destination"),
        "{md}"
    );
}

/// Codex review on #1990 (#3905407434): a `halt_benign` judge verdict
/// scrubs the declined node from `run.output`, so this summary — which
/// derives its per-node lines from that map and separately inspects only
/// `Error` rows — called the node "not reached" and still claimed the run
/// reached its terminal nodes. The intentional stop was invisible to the
/// agent that started the run.
#[test]
fn the_summary_reports_a_declined_node_as_not_needed() {
    let file = crate::company::parse_workflow(DEMO_WF).unwrap();
    let declined = WorkflowRun {
        output: json!({ "nodes": { "start": { "items": ["go"] } } }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        cancelled: false,
        nodes: vec![crate::ports::WorkflowRunNodeRow {
            node_id: "worker".into(),
            status: WorkflowNodeStatus::Declined,
            elapsed_ms: 12,
            diagnostics: Vec::new(),
        }],
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    let md = summarize_run(&file, &declined, "run-declined", RunOutputStored::Stored);
    assert!(
        md.contains("not needed"),
        "a declined node must read as an intentional stop: {md}"
    );
    assert!(
        !md.contains("**Worker** (`worker`, agent): not reached"),
        "a declined node is not an unreached one: {md}"
    );
    assert!(
        !md.contains("reached its terminal node(s) without pausing"),
        "the run stopped on purpose; the happy-path sentence is false: {md}"
    );
    assert!(
        !md.contains("NOT a clean run"),
        "a declined node is not an error: {md}"
    );
}
