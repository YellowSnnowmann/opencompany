use super::*;
// The globals-unaware readers: these tests assert the company's own two
// sources, so they call the form that resolves no baseline.
use super::workflows_test_support::*;
use crate::company::{list_workflows_union, load_workflow_union};

/// **The `editable` predicate, including the case the route harness can't
/// reach** (its runtimes are hosted, so they have no source directory).
///
/// A seed file wins the union read, so an overlay body sitting behind one is
/// not editable even though a body exists — persisting an edit there would
/// store a graph nothing serves. This is the same predicate the write core
/// enforces with a 409; if the two ever disagree the console offers a button
/// that cannot work.
#[test]
fn editable_is_overlay_backed_and_not_seed_shadowed() {
    let dir = seed_demo(); // writes workflows/demo.toml
    let source = Some(dir.path());
    let overlay = |id: &str| OverlayWorkflow {
        id: id.to_string(),
        toml: DEMO.to_string(),
    };

    // Overlay body, no seed file → editable.
    assert!(is_editable(source, &[overlay("mine")], "mine"));
    // Overlay body shadowed by a seed file of the same id → NOT editable.
    assert!(!is_editable(source, &[overlay("demo")], "demo"));
    // Seed file only → not editable.
    assert!(!is_editable(source, &[], "demo"));
    // Nothing at all → not editable.
    assert!(!is_editable(source, &[], "ghost"));
    // No source tree (the hosted shape): an overlay body is editable, and a
    // seed id that no longer has a tree behind it simply isn't there.
    assert!(is_editable(None, &[overlay("mine")], "mine"));
    assert!(!is_editable(None, &[], "demo"));
}

#[test]
fn list_returns_a_summary_per_saved_workflow() {
    let dir = seed_demo();
    let files = list_workflows_union(Some(dir.path()), &[]);
    let summaries: Vec<WorkflowSummary> = files
        .into_iter()
        .map(|f| WorkflowSummary::new(f, false, true))
        .collect();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, "demo");
    assert_eq!(summaries[0].name, "Demo flow");
    assert_eq!(
        summaries[0].description.as_deref(),
        Some("A tiny trigger → agent → output graph.")
    );
    assert_eq!(summaries[0].schedule, None);
    assert_eq!(summaries[0].node_count, 3);

    let json = serde_json::to_value(&summaries[0]).unwrap();
    assert_eq!(json["schedule"], serde_json::Value::Null);
    assert_eq!(json["nodeCount"], 3);
}

#[test]
fn scheduled_summary_carries_the_trigger_cron_and_node_count() {
    let scheduled = DEMO.replace(
        "name = \"Start\"\n        summary",
        "name = \"Start\"\n        schedule = \"0 9 * * MON\"\n        summary",
    );
    let file = crate::company::parse_workflow(&scheduled).expect("scheduled fixture parses");
    let json = serde_json::to_value(WorkflowSummary::new(file, false, true)).unwrap();

    assert_eq!(json["schedule"], "0 9 * * MON");
    assert_eq!(json["nodeCount"], 3);
}

#[test]
fn get_returns_the_full_graph_with_nodes_and_edges() {
    let dir = seed_demo();
    let file = load_workflow_union(Some(dir.path()), &[], "demo")
        .expect("loads")
        .expect("one file");
    let graph = WorkflowGraph::new(file, false, None, true);

    assert_eq!(graph.id, "demo");
    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.edges.len(), 2);

    // The `kind` field is the on-disk string via `as_str()`.
    let worker = graph.nodes.iter().find(|n| n.id == "worker").unwrap();
    assert_eq!(worker.kind, "agent");
    assert_eq!(worker.agent.as_deref(), Some("assistant"));

    let trigger = graph.nodes.iter().find(|n| n.id == "start").unwrap();
    assert_eq!(trigger.kind, "trigger");
    assert!(trigger.agent.is_none());

    let labeled = graph.edges.iter().find(|e| e.to == "done").unwrap();
    assert_eq!(labeled.from, "worker");
    assert_eq!(labeled.label.as_deref(), Some("ok"));
}

#[test]
fn no_source_dir_and_no_overlay_lists_empty() {
    assert!(list_workflows_union(None, &[]).is_empty());
}

#[test]
fn no_workflows_dir_lists_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(list_workflows_union(Some(dir.path()), &[]).is_empty());
}

#[test]
fn json_serializes_camelcase_and_omits_empty_options() {
    let dir = seed_demo();
    let file = load_workflow_union(Some(dir.path()), &[], "demo")
        .unwrap()
        .unwrap();
    let json = serde_json::to_value(WorkflowGraph::new(file, false, None, true)).unwrap();
    // A node with no summary/agent omits those keys entirely.
    let done = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "done")
        .unwrap();
    assert!(done.get("agent").is_none());
    assert!(done.get("summary").is_none());
    assert_eq!(done["kind"], "output");
}

/// A non-editable graph serializes `version` as an explicit `null` rather
/// than omitting the key (issue #1013). Omitting it made a client read
/// `version` as `undefined` and send nothing, silently overwriting a
/// concurrent save; an explicit `null` is the honest "no token here".
#[test]
fn a_non_editable_graph_serializes_version_as_null() {
    let dir = seed_demo();
    let file = load_workflow_union(Some(dir.path()), &[], "demo")
        .unwrap()
        .unwrap();
    let json = serde_json::to_value(WorkflowGraph::new(file, false, None, false)).unwrap();
    assert!(
        json.get("version").is_some(),
        "version key must be present, not omitted: {json}"
    );
    assert!(
        json["version"].is_null(),
        "no token serializes as null: {json}"
    );
}

#[test]
fn json_serializes_p1_node_fields_in_camelcase() {
    use crate::company::{WorkflowNodeDef, WorkflowNodeKind, WorkflowRetryDef};

    let file = WorkflowFile {
        global: false,
        id: "wf".into(),
        name: "WF".into(),
        description: None,
        owner_desk: None,
        nodes: vec![WorkflowNodeDef {
            id: "call".into(),
            kind: WorkflowNodeKind::ToolCall,
            name: "Call".into(),
            summary: None,
            agent: None,
            schedule: None,
            config: Some(serde_json::json!({ "slug": "csv_export" })),
            on_error: Some("continue".into()),
            retry: Some(WorkflowRetryDef {
                max_attempts: Some(3),
                backoff_ms: Some(250),
                backoff: Some("exponential".into()),
            }),
            requires_approval: Some(true),
            repeatable: None,
            destination: None,
            postcondition: None,
            verify: None,
        }],
        edges: Vec::new(),
    };
    let json = serde_json::to_value(WorkflowGraph::new(file, false, None, true)).unwrap();
    let node = &json["nodes"][0];
    assert_eq!(node["config"]["slug"], "csv_export");
    assert_eq!(node["onError"], "continue");
    assert_eq!(node["retry"]["maxAttempts"], 3);
    assert_eq!(node["retry"]["backoffMs"], 250);
    assert_eq!(node["retry"]["backoff"], "exponential");
    assert_eq!(node["requiresApproval"], true);
}

/// `repeatable: false` round-trips through the read model (issue #850).
///
/// `WorkflowNode` previously omitted the field entirely, so a console edit
/// that read a node back from `GET`/create/update and resubmitted it would
/// silently drop the author's `repeatable = false` declaration on the next
/// save — the exact safety declaration issue #850 exists to protect.
#[test]
fn json_serializes_repeatable_field() {
    use crate::company::{WorkflowNodeDef, WorkflowNodeKind};

    let file = WorkflowFile {
        global: false,
        id: "wf".into(),
        name: "WF".into(),
        description: None,
        owner_desk: None,
        nodes: vec![
            WorkflowNodeDef {
                id: "publish".into(),
                kind: WorkflowNodeKind::ToolCall,
                name: "Publish".into(),
                summary: None,
                agent: None,
                schedule: None,
                config: Some(serde_json::json!({ "slug": "shell" })),
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: Some(false),
                destination: None,
                postcondition: None,
                verify: None,
            },
            WorkflowNodeDef {
                id: "read".into(),
                kind: WorkflowNodeKind::ToolCall,
                name: "Read".into(),
                summary: None,
                agent: None,
                schedule: None,
                config: Some(serde_json::json!({ "slug": "web_fetch" })),
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: None,
                destination: None,
                postcondition: None,
                verify: None,
            },
        ],
        edges: Vec::new(),
    };
    let json = serde_json::to_value(WorkflowGraph::new(file, false, None, true)).unwrap();
    let nodes = json["nodes"].as_array().unwrap();
    let publish = nodes.iter().find(|n| n["id"] == "publish").unwrap();
    assert_eq!(
        publish["repeatable"], false,
        "declared repeatable:false must survive the read model: {publish}"
    );
    let read = nodes.iter().find(|n| n["id"] == "read").unwrap();
    assert!(
        read.get("repeatable").is_none(),
        "an undeclared node omits the key rather than serializing null: {read}"
    );
}

// --- P2: create body maps the new node fields (config/error/retry/approval)

/// A create body carrying P2 node fields round-trips them through the
/// render → parse pipeline the endpoint uses before writing to disk: config
/// (with an `=expr` binding), `onError`, `retry` (camelCase → snake), and
/// `requiresApproval` all survive.
#[test]
fn create_body_round_trips_p2_node_fields() {
    use crate::company::WorkflowNodeKind;

    let body: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
        "id": "wf",
        "name": "WF",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start" },
            {
                "id": "tf", "kind": "transform", "name": "Transform",
                "config": { "set": { "count": "=items | length" } },
                "onError": "continue",
                "retry": { "maxAttempts": 3, "backoffMs": 250, "backoff": "exponential" },
                "requiresApproval": true
            }
        ],
        "edges": [ { "from": "start", "to": "tf" } ]
    }))
    .expect("body deserializes");

    let raw = RawWorkflow::try_from(body).expect("converts");
    let toml_src = crate::company::render_workflow(&raw).expect("renders");
    let file = crate::company::parse_workflow(&toml_src).expect("re-parses");

    let tf = file.nodes.iter().find(|n| n.id == "tf").unwrap();
    assert_eq!(tf.kind, WorkflowNodeKind::Transform);
    assert_eq!(tf.on_error.as_deref(), Some("continue"));
    assert_eq!(tf.requires_approval, Some(true));
    let retry = tf.retry.as_ref().expect("retry present");
    assert_eq!(retry.max_attempts, Some(3));
    assert_eq!(retry.backoff_ms, Some(250));
    assert_eq!(retry.backoff.as_deref(), Some("exponential"));
    // The `=expr` binding is preserved verbatim for the engine to evaluate.
    assert_eq!(
        tf.config.as_ref().unwrap()["set"]["count"],
        "=items | length"
    );
}

/// An old create body (no P2 fields) still produces a bare node — every new
/// field unset — so nothing changes for existing callers.
#[test]
fn create_body_without_new_fields_is_unchanged() {
    let body: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
        "id": "wf",
        "name": "WF",
        "nodes": [ { "id": "start", "kind": "trigger", "name": "Start" } ],
        "edges": []
    }))
    .unwrap();
    let raw = RawWorkflow::try_from(body).expect("converts");
    let node = &raw.nodes[0];
    assert!(node.config.is_none());
    assert!(node.on_error.is_none());
    assert!(node.retry.is_none());
    assert!(node.requires_approval.is_none());
}

/// A JSON `null` inside node config is a 400 — TOML has no null to store it,
/// so it is rejected before anything touches disk.
#[test]
fn create_body_with_null_config_is_a_bad_request() {
    use axum::http::StatusCode;

    let body: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
        "id": "wf",
        "name": "WF",
        "nodes": [
            { "id": "call", "kind": "tool_call", "name": "Call", "config": { "slug": null } }
        ],
        "edges": []
    }))
    .unwrap();
    // `RawWorkflow` is not `Debug`, so unwrap the error by hand rather than
    // via `expect_err`.
    let err = match RawWorkflow::try_from(body) {
        Ok(_) => panic!("a null config value must be rejected"),
        Err(err) => err,
    };
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    assert!(matches!(err.0, OpenCompanyError::InvalidRequest(_)));
}

// --- trigger schedule (issue #169) --------------------------------------

/// A create body carrying a trigger `schedule` round-trips it through the
/// render → parse pipeline the endpoint runs before persisting.
#[test]
fn create_body_round_trips_a_trigger_schedule() {
    let body: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
        "id": "wf",
        "name": "WF",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start", "schedule": "0 9 * * MON" },
            { "id": "done", "kind": "output", "name": "Done" }
        ],
        "edges": [ { "from": "start", "to": "done" } ]
    }))
    .expect("body deserializes");

    let raw = RawWorkflow::try_from(body).expect("converts");
    let toml_src = crate::company::render_workflow(&raw).expect("renders");
    let file = crate::company::parse_workflow(&toml_src).expect("re-parses");

    let start = file.nodes.iter().find(|n| n.id == "start").unwrap();
    assert_eq!(start.schedule.as_deref(), Some("0 9 * * MON"));
    let done = file.nodes.iter().find(|n| n.id == "done").unwrap();
    assert!(done.schedule.is_none());
}

/// The create route needs no schedule-specific validation code: the same
/// render → parse round trip surfaces a bad cron (and a schedule on the
/// wrong node kind) as the model's prosumer error, which the handler maps
/// to a `400`.
#[test]
fn create_body_with_a_bad_schedule_fails_reparse() {
    let bad_cron: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
        "id": "wf",
        "name": "WF",
        "nodes": [ { "id": "start", "kind": "trigger", "name": "Start", "schedule": "hourly" } ],
        "edges": []
    }))
    .unwrap();
    let raw = RawWorkflow::try_from(bad_cron).expect("converts");
    let toml_src = crate::company::render_workflow(&raw).expect("renders");
    let err = crate::company::parse_workflow(&toml_src).unwrap_err();
    assert!(err.to_string().contains("not a valid cron"), "{err}");

    let wrong_kind: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
        "id": "wf",
        "name": "WF",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start" },
            { "id": "done", "kind": "output", "name": "Done", "schedule": "0 * * * *" }
        ],
        "edges": [ { "from": "start", "to": "done" } ]
    }))
    .unwrap();
    let raw = RawWorkflow::try_from(wrong_kind).expect("converts");
    let toml_src = crate::company::render_workflow(&raw).expect("renders");
    let err = crate::company::parse_workflow(&toml_src).unwrap_err();
    assert!(err.to_string().contains("only `trigger` nodes"), "{err}");
}

/// `GET …/workflows/{wid}` serializes the schedule in camelCase (it is a
/// single word, so the key is `schedule`) and omits it when unset.
#[test]
fn json_serializes_the_trigger_schedule() {
    use crate::company::{WorkflowNodeDef, WorkflowNodeKind};

    let file = WorkflowFile {
        global: false,
        id: "wf".into(),
        name: "WF".into(),
        description: None,
        owner_desk: None,
        nodes: vec![
            WorkflowNodeDef {
                id: "start".into(),
                kind: WorkflowNodeKind::Trigger,
                name: "Start".into(),
                summary: None,
                agent: None,
                schedule: Some("0 * * * *".into()),
                config: None,
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: None,
                destination: None,
                postcondition: None,
                verify: None,
            },
            WorkflowNodeDef {
                id: "done".into(),
                kind: WorkflowNodeKind::Output,
                name: "Done".into(),
                summary: None,
                agent: None,
                schedule: None,
                config: None,
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: None,
                destination: None,
                postcondition: None,
                verify: None,
            },
        ],
        edges: Vec::new(),
    };
    let json = serde_json::to_value(WorkflowGraph::new(file, false, None, true)).unwrap();
    assert_eq!(json["nodes"][0]["schedule"], "0 * * * *");
    assert!(json["nodes"][1].get("schedule").is_none());
}

/// A legacy create body with no `schedule` key still converts, with the
/// field unset — nothing changes for existing callers.
#[test]
fn create_body_without_a_schedule_is_unchanged() {
    let body: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
        "id": "wf",
        "name": "WF",
        "nodes": [ { "id": "start", "kind": "trigger", "name": "Start" } ],
        "edges": []
    }))
    .unwrap();
    let raw = RawWorkflow::try_from(body).expect("converts");
    assert!(raw.nodes[0].schedule.is_none());
}

// --- Output destination on the wire (issue #170) ------------------------

/// A create body's `destination` survives the render → parse pipeline the
/// endpoint runs before persisting, and comes back out on the GET shape
/// under the same key — so the console can post exactly what it reads.
#[test]
fn create_body_round_trips_an_output_destination() {
    let body: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
        "id": "wf",
        "name": "WF",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start" },
            {
                "id": "done", "kind": "output", "name": "Report",
                "destination": { "kind": "email", "target": "ada@example.com" }
            }
        ],
        "edges": [ { "from": "start", "to": "done" } ]
    }))
    .expect("body deserializes");

    let raw = RawWorkflow::try_from(body).expect("converts");
    let toml_src = crate::company::render_workflow(&raw).expect("renders");
    let file = crate::company::parse_workflow(&toml_src).expect("re-parses");

    let done = file.nodes.iter().find(|n| n.id == "done").unwrap();
    let dest = done.destination.as_ref().expect("destination survived");
    assert_eq!(dest.kind, "email");
    assert_eq!(dest.target.as_deref(), Some("ada@example.com"));

    // …and back out on the read shape, under the same key.
    let json = serde_json::to_value(WorkflowGraph::new(file, false, None, true)).unwrap();
    let node = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "done")
        .unwrap()
        .clone();
    assert_eq!(node["destination"]["kind"], "email");
    assert_eq!(node["destination"]["target"], "ada@example.com");
}

/// An `owner` destination carries no target, and the key is omitted rather
/// than serialized as `null`.
#[test]
fn an_owner_destination_omits_the_target_key() {
    let body: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
        "id": "wf",
        "name": "WF",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start" },
            {
                "id": "done", "kind": "output", "name": "Report",
                "destination": { "kind": "owner" }
            }
        ],
        "edges": [ { "from": "start", "to": "done" } ]
    }))
    .unwrap();
    let raw = RawWorkflow::try_from(body).expect("converts");
    let file =
        crate::company::parse_workflow(&crate::company::render_workflow(&raw).expect("renders"))
            .expect("re-parses");
    let json = serde_json::to_value(WorkflowGraph::new(file, false, None, true)).unwrap();
    let node = &json["nodes"][1];
    assert_eq!(node["destination"]["kind"], "owner");
    assert!(node["destination"].get("target").is_none());
}

/// A node with no destination omits the key entirely — the pre-#170 read
/// shape is byte-identical for every existing graph.
#[test]
fn a_node_without_a_destination_omits_the_key() {
    let dir = seed_demo();
    let file = load_workflow_union(Some(dir.path()), &[], "demo")
        .unwrap()
        .unwrap();
    let json = serde_json::to_value(WorkflowGraph::new(file, false, None, true)).unwrap();
    for node in json["nodes"].as_array().unwrap() {
        assert!(node.get("destination").is_none(), "{node}");
    }
}

/// A destination the host cannot honour is rejected before anything is
/// persisted — the create route surfaces the same prosumer-language problem
/// a hand-authored file gets.
#[test]
fn create_body_with_a_bad_destination_is_rejected_at_validation() {
    for (dest, expected) in [
        (
            serde_json::json!({ "kind": "email", "target": "ada" }),
            "not an email address",
        ),
        (
            serde_json::json!({ "kind": "carrier_pigeon" }),
            "unknown `destination.kind`",
        ),
        (serde_json::json!({ "kind": "channel" }), "no `target`"),
    ] {
        let body: CreateWorkflowBody = serde_json::from_value(serde_json::json!({
            "id": "wf",
            "name": "WF",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                { "id": "done", "kind": "output", "name": "Report", "destination": dest }
            ],
            "edges": [ { "from": "start", "to": "done" } ]
        }))
        .unwrap();
        let raw = RawWorkflow::try_from(body).expect("converts");
        let toml_src = crate::company::render_workflow(&raw).expect("renders");
        let err = crate::company::parse_workflow(&toml_src)
            .expect_err("an unhonourable destination must not persist");
        assert!(err.to_string().contains(expected), "{err}");
    }
}

/// The HTTP run DTO must expose a settled soft node error as the explicit
/// `degraded` verdict, rather than allowing clients to infer success from
/// otherwise-green node rows.
#[test]
fn run_response_serializes_a_degraded_verdict() {
    let json = serde_json::to_value(RunWorkflowResponse {
        output: serde_json::json!({"nodes": {"worker": {"items": ["partial"]}}}),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        run_id: "run-degraded".into(),
        cancelled: false,
        verdict: WorkflowRunVerdict::Degraded,
        nodes: vec![WorkflowRunNode {
            node_id: "worker".into(),
            status: WorkflowNodeStatus::Error,
            elapsed_ms: 17,
            diagnostics: Vec::new(),
        }],
        dry_run: false,
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    })
    .expect("serialize");

    assert_eq!(json["verdict"], "degraded", "the HTTP contract is explicit");
    assert_eq!(json["nodes"][0]["status"], "error");
}
