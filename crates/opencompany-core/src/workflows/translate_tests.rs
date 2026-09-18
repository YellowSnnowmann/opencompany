use super::*;
use crate::company::parse_workflow;

const CAMPAIGN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../companies/marketing_agency/workflows/campaign_pipeline.toml"
));

/// The shipped campaign pipeline translates into a graph tinyflows accepts,
/// exercising every one of the six node kinds.
#[test]
fn translates_the_shipped_campaign_pipeline() {
    let file = parse_workflow(CAMPAIGN).expect("campaign parses");
    let graph = translate(&file);

    assert_eq!(graph.id.as_deref(), Some("campaign_pipeline"));
    assert_eq!(graph.name, "Campaign pipeline");
    assert_eq!(graph.nodes.len(), file.nodes.len());
    assert_eq!(graph.edges.len(), file.edges.len());

    // The translated graph is structurally valid for the engine.
    tinyflows::compiler::compile(&graph).expect("translated graph compiles");
}

/// Node kinds map across, and an `output` node becomes a pass-through
/// `transform`.
#[test]
fn maps_every_node_kind() {
    let file = parse_workflow(CAMPAIGN).expect("campaign parses");
    let graph = translate(&file);
    let kind = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.kind.clone())
    };

    assert_eq!(kind("brief"), Some(NodeKind::Trigger));
    assert_eq!(kind("strategist"), Some(NodeKind::Agent));
    assert_eq!(kind("gate"), Some(NodeKind::Condition));
    assert_eq!(kind("research"), Some(NodeKind::ToolCall));
    // `publish` is an agent assembly step (#530) — there is no CMS to POST to.
    assert_eq!(kind("publish"), Some(NodeKind::Agent));
    // `output` lowers to a pass-through `transform`.
    assert_eq!(kind("done"), Some(NodeKind::Transform));
}

/// An agent node carries its roster teammate id as `agent_ref` plus a prompt.
#[test]
fn agent_node_carries_agent_ref_and_prompt() {
    let file = parse_workflow(CAMPAIGN).expect("campaign parses");
    let graph = translate(&file);
    let strategist = graph.nodes.iter().find(|n| n.id == "strategist").unwrap();
    assert_eq!(strategist.config["agent_ref"], "brand_strategist");
    assert_eq!(
        strategist.config["prompt"],
        "Turns the brief into an angle + outline."
    );
}

/// **Issue #782.** Every agent node carries an `input = "=items"` binding so
/// the engine resolves the full set of upstream node outputs into its config
/// at run time — the only channel an upstream step's output has to the next
/// agent's turn. `=items` (not `=item`) is deliberate: it is the whole
/// predecessor set, so a fan-in (`merge -> agent`) delivers every predecessor
/// rather than only the first.
#[test]
fn agent_node_binds_the_full_upstream_output() {
    let file = parse_workflow(CAMPAIGN).expect("campaign parses");
    let graph = translate(&file);
    // Every translated agent node carries the binding…
    for node in graph.nodes.iter().filter(|n| n.kind == NodeKind::Agent) {
        assert_eq!(
            node.config["input"], "=items",
            "agent node {} must bind the full upstream output set",
            node.id
        );
    }
    // …and a non-agent node does not (the binding is agent-specific).
    let research = graph.nodes.iter().find(|n| n.id == "research").unwrap();
    assert!(
        research.config.get("input").is_none(),
        "a tool_call node carries no upstream-output binding"
    );
}

/// A condition node's `yes`/`no` labels become `true`/`false` branch ports.
#[test]
fn condition_labels_map_to_true_false_ports() {
    let file = parse_workflow(CAMPAIGN).expect("campaign parses");
    let graph = translate(&file);
    let port = |to: &str| {
        graph
            .edges
            .iter()
            .find(|e| e.from_node == "gate" && e.to_node == to)
            .map(|e| e.from_port.clone())
    };
    assert_eq!(port("research").as_deref(), Some("true")); // label "yes"
    assert_eq!(port("copy").as_deref(), Some("false")); // label "no"
}

/// Non-condition edges keep the default `main` port.
#[test]
fn plain_edges_stay_on_main() {
    let file = parse_workflow(CAMPAIGN).expect("campaign parses");
    let graph = translate(&file);
    let edge = graph
        .edges
        .iter()
        .find(|e| e.from_node == "brief" && e.to_node == "strategist")
        .unwrap();
    assert_eq!(edge.from_port, "main");
    assert_eq!(edge.to_port, "main");
}

/// The label mapping is total: negatives → false, everything else → true.
#[test]
fn condition_port_mapping() {
    assert_eq!(condition_port(Some("yes")), "true");
    assert_eq!(condition_port(Some("no")), "false");
    assert_eq!(condition_port(Some("TRUE")), "true");
    assert_eq!(condition_port(Some("False")), "false");
    assert_eq!(condition_port(None), "true");
}

// --- P1: config overlay + error/retry/approval + error routing ---------

/// A snapshot pinning the shipped campaign pipeline's translated config. Most
/// nodes carry only kind-derived config; the `research` `tool_call` binds the
/// metered `web_search` slug + args and an `on_error = "continue"` policy
/// (#530), and `publish` is an `agent` assembly step (there is no CMS to POST
/// to), so each carries exactly the config those choices imply.
#[test]
fn campaign_translation_lowers_to_the_expected_engine_config() {
    let file = parse_workflow(CAMPAIGN).expect("campaign parses");
    let graph = translate(&file);
    let config = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.config.clone())
            .unwrap()
    };
    assert_eq!(config("brief"), json!({}));
    assert_eq!(
        config("strategist"),
        json!({
            "agent_ref": "brand_strategist",
            "prompt": "Turns the brief into an angle + outline.",
            // Issue #782: the upstream-output binding every agent node carries.
            "input": "=items",
            // Issue #881: the node's own id, so the agent capability can say
            // WHICH node blocked when its turn parks an approval.
            "node_id": "strategist"
        })
    );
    // The gate carries its boolean discriminant (issue #661): a condition
    // node must name the `field` it branches on.
    assert_eq!(config("gate"), json!({ "field": "=item.needs_research" }));
    assert_eq!(
        config("research"),
        json!({
            "slug": "web_search",
            "args": { "query": "=item.text", "max_results": 5 },
            "on_error": "continue"
        })
    );
    assert_eq!(
        config("publish"),
        json!({
            "agent_ref": "copywriter",
            "prompt": "Assemble the publish-ready post and hero-image reference, then hand off for operator sign-off.",
            // Issue #782: the upstream-output binding every agent node carries.
            "input": "=items",
            // Issue #881: as above.
            "node_id": "publish"
        })
    );
    assert_eq!(config("done"), json!({}));
}

/// A `tool_call` node's config `slug` is carried into the engine config
/// verbatim (issue #661 removed the node-id placeholder fallback; author-time
/// validation now guarantees a real slug is always present).
#[test]
fn config_slug_is_carried_into_engine_config() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "call"
        kind = "tool_call"
        name = "Call"
        [node.config]
        slug = "csv_export"
        [node.config.args]
        filename = "out.csv"
        [[edge]]
        from = "start"
        to = "call"
    "#;
    let graph = translate(&parse_workflow(src).expect("parses"));
    let call = graph.nodes.iter().find(|n| n.id == "call").unwrap();
    assert_eq!(call.config["slug"], "csv_export");
    assert_eq!(call.config["args"]["filename"], "out.csv");
}

/// A config that tries to spoof `agent_ref` cannot win: the first-class
/// `agent` field is written last, so the roster binding is authoritative.
/// (The model would reject this config at parse time; here we build the node
/// directly to prove the layering order in `translate` itself.)
#[test]
fn agent_ref_survives_a_spoofing_config() {
    use crate::company::{WorkflowFile, WorkflowNodeDef, WorkflowNodeKind};
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
            agent: Some("real".into()),
            schedule: None,
            config: Some(json!({ "agent_ref": "impostor" })),
            on_error: None,
            retry: None,
            requires_approval: None,
            repeatable: None,
            destination: None,
            postcondition: None,
            verify: None,
        }],
        edges: Vec::new(),
    };
    let graph = translate(&file);
    assert_eq!(graph.nodes[0].config["agent_ref"], "real");
}

/// `on_error` / `retry` / `requires_approval` land as the exact config keys
/// the tinyflows engine reads.
#[test]
fn error_retry_approval_land_as_engine_config_keys() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        requires_approval = true
        [[node]]
        id = "call"
        kind = "tool_call"
        name = "Call"
        on_error = "continue"
        [node.config]
        slug = "csv_export"
        [node.retry]
        max_attempts = 3
        backoff_ms = 250
        backoff = "exponential"
        [[edge]]
        from = "start"
        to = "call"
    "#;
    let graph = translate(&parse_workflow(src).expect("parses"));
    let start = graph.nodes.iter().find(|n| n.id == "start").unwrap();
    assert_eq!(start.config["requires_approval"], true);
    let call = graph.nodes.iter().find(|n| n.id == "call").unwrap();
    assert_eq!(call.config["on_error"], "continue");
    assert_eq!(call.config["retry"]["max_attempts"], 3);
    assert_eq!(call.config["retry"]["backoff_ms"], 250);
    assert_eq!(call.config["retry"]["backoff"], "exponential");
}

/// Issue #1866: an agent node's declared `postcondition` lands as the
/// exact config key `HarnessAgentRunner::run_turn` reads it back from —
/// the same first-class-field-becomes-config-key contract `retry` and
/// `requires_approval` already have, pinned above.
#[test]
fn postcondition_lands_as_an_engine_config_key() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "worker"
        kind = "agent"
        name = "Worker"
        agent = "researcher"
        [node.postcondition]
        require = "field_present"
        field = "json.items"
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let graph = translate(&parse_workflow(src).expect("parses"));
    let worker = graph.nodes.iter().find(|n| n.id == "worker").unwrap();
    assert_eq!(worker.config["postcondition"]["require"], "field_present");
    assert_eq!(worker.config["postcondition"]["field"], "json.items");
}

/// The other half of the same contract: a node with no `postcondition`
/// declared carries no `postcondition` key at all — translation of an
/// unchanged (pre-#1866) file stays byte-identical, matching every other
/// first-class field's `Option<T>` -> `if let Some` -> config-key
/// pattern above.
#[test]
fn a_node_without_a_postcondition_carries_no_postcondition_key() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "worker"
        kind = "agent"
        name = "Worker"
        agent = "researcher"
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let graph = translate(&parse_workflow(src).expect("parses"));
    let worker = graph.nodes.iter().find(|n| n.id == "worker").unwrap();
    assert!(
        worker.config.get("postcondition").is_none(),
        "an undeclared postcondition must not appear in the lowered config at all: {:?}",
        worker.config
    );
}

#[test]
fn verify_lowers_to_the_exact_agent_config_key() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "worker"
        kind = "agent"
        name = "Worker"
        agent = "researcher"
        [node.verify]
        criteria = "Cite the evidence."
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let graph = translate(&parse_workflow(src).expect("parses"));
    let worker = graph.nodes.iter().find(|node| node.id == "worker").unwrap();
    assert_eq!(worker.config["verify"]["criteria"], "Cite the evidence.");
}

/// An "error"-labeled edge leaving a routing node maps onto the engine's
/// `error` port; a non-error edge from the same node stays on `main`.
#[test]
fn error_label_maps_to_error_port() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "call"
        kind = "tool_call"
        name = "Call"
        on_error = "route"
        [node.config]
        slug = "csv_export"
        [[node]]
        id = "ok"
        kind = "output"
        name = "OK"
        [[node]]
        id = "recover"
        kind = "output"
        name = "Recover"
        [[edge]]
        from = "start"
        to = "call"
        [[edge]]
        from = "call"
        to = "ok"
        [[edge]]
        from = "call"
        to = "recover"
        label = "error"
    "#;
    let graph = translate(&parse_workflow(src).expect("parses"));
    let port = |to: &str| {
        graph
            .edges
            .iter()
            .find(|e| e.from_node == "call" && e.to_node == to)
            .map(|e| e.from_port.clone())
    };
    assert_eq!(port("recover").as_deref(), Some("error"));
    assert_eq!(port("ok").as_deref(), Some("main"));
}

/// A node that is BOTH a `condition` and `on_error = "route"` must route its
/// `"error"`-labeled edge onto the `error` port — not the `true` port. The
/// error-port check runs before the condition branch precisely so the
/// `"error"` label is never funneled through `condition_port` (which maps any
/// non-negative label, including `"error"`, to `true`). Its `yes`/`no` branch
/// edges must still map to `true`/`false`.
#[test]
fn condition_node_with_route_sends_error_edge_to_error_port() {
    use crate::company::{WorkflowFile, WorkflowNodeDef, WorkflowNodeKind};
    let file = WorkflowFile {
        global: false,
        id: "wf".into(),
        name: "WF".into(),
        description: None,
        owner_desk: None,
        nodes: vec![
            WorkflowNodeDef {
                id: "gate".into(),
                kind: WorkflowNodeKind::Condition,
                name: "Gate".into(),
                summary: None,
                agent: None,
                schedule: None,
                config: None,
                on_error: Some("route".into()),
                retry: None,
                requires_approval: None,
                repeatable: None,
                destination: None,
                postcondition: None,
                verify: None,
            },
            node_stub("yes_path"),
            node_stub("no_path"),
            node_stub("recover"),
        ],
        edges: vec![
            edge_stub("gate", "yes_path", Some("yes")),
            edge_stub("gate", "no_path", Some("no")),
            edge_stub("gate", "recover", Some("error")),
        ],
    };
    let graph = translate(&file);
    let port = |to: &str| {
        graph
            .edges
            .iter()
            .find(|e| e.from_node == "gate" && e.to_node == to)
            .map(|e| e.from_port.clone())
    };
    // The error edge wins the error port; the branch edges keep true/false.
    assert_eq!(port("recover").as_deref(), Some("error"));
    assert_eq!(port("yes_path").as_deref(), Some("true"));
    assert_eq!(port("no_path").as_deref(), Some("false"));
}

// --- P2: the twelve-kind map + switch ports ----------------------------

/// Every OpenCompany kind lowers to its tinyflows counterpart; the P2 kinds
/// map one-to-one and `output` still lowers to a pass-through `transform`.
#[test]
fn twelve_kind_map_is_total() {
    use crate::company::{WorkflowFile, WorkflowNodeDef, WorkflowNodeKind};
    let kinds = [
        (WorkflowNodeKind::Trigger, NodeKind::Trigger),
        (WorkflowNodeKind::Agent, NodeKind::Agent),
        (WorkflowNodeKind::ToolCall, NodeKind::ToolCall),
        (WorkflowNodeKind::HttpRequest, NodeKind::HttpRequest),
        (WorkflowNodeKind::Condition, NodeKind::Condition),
        (WorkflowNodeKind::Output, NodeKind::Transform),
        (WorkflowNodeKind::Switch, NodeKind::Switch),
        (WorkflowNodeKind::Merge, NodeKind::Merge),
        (WorkflowNodeKind::SplitOut, NodeKind::SplitOut),
        (WorkflowNodeKind::Transform, NodeKind::Transform),
        (WorkflowNodeKind::OutputParser, NodeKind::OutputParser),
        (WorkflowNodeKind::SubWorkflow, NodeKind::SubWorkflow),
    ];
    for (oc, tf) in kinds {
        let file = WorkflowFile {
            global: false,
            id: "wf".into(),
            name: "WF".into(),
            description: None,
            owner_desk: None,
            nodes: vec![WorkflowNodeDef {
                id: "n".into(),
                kind: oc,
                name: "N".into(),
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
            }],
            edges: Vec::new(),
        };
        assert_eq!(translate(&file).nodes[0].kind, tf, "{oc:?} → {tf:?}");
    }
}

/// An edge leaving a `switch` carries its label VERBATIM as the branch port;
/// an unlabeled switch edge falls to the engine's `default` fallback port.
#[test]
fn switch_labels_map_to_verbatim_ports() {
    use crate::company::{WorkflowFile, WorkflowNodeDef, WorkflowNodeKind};
    let file = WorkflowFile {
        global: false,
        id: "wf".into(),
        name: "WF".into(),
        description: None,
        owner_desk: None,
        nodes: vec![
            WorkflowNodeDef {
                id: "sw".into(),
                kind: WorkflowNodeKind::Switch,
                name: "Switch".into(),
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
            node_stub("paid"),
            node_stub("error_case"),
            node_stub("fallthrough"),
        ],
        edges: vec![
            edge_stub("sw", "paid", Some("paid")),
            // `error` is a legitimate case name on a switch — carried verbatim,
            // NOT mapped onto the engine's `error` port.
            edge_stub("sw", "error_case", Some("error")),
            edge_stub("sw", "fallthrough", None),
        ],
    };
    let graph = translate(&file);
    let port = |to: &str| {
        graph
            .edges
            .iter()
            .find(|e| e.from_node == "sw" && e.to_node == to)
            .map(|e| e.from_port.clone())
    };
    assert_eq!(port("paid").as_deref(), Some("paid"));
    assert_eq!(port("error_case").as_deref(), Some("error"));
    assert_eq!(port("fallthrough").as_deref(), Some("default"));
}

/// **The delivery invariant (issue #170).** An `output` node's
/// `destination` is host-side routing, not engine config: it must NOT reach
/// the compiled graph. A destination-bearing output node lowers to exactly
/// the same bare pass-through `Transform` it lowered to before the field
/// existed — so translation of a graph is unaffected by where its report
/// goes, and the engine never gains an inert key it does not understand.
#[test]
fn an_output_destination_never_reaches_the_engine_graph() {
    let with_destination = crate::company::parse_workflow(
        r#"
id = "wf"
name = "WF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "done"
kind = "output"
name = "Report"
[node.destination]
kind = "email"
target = "ada@example.com"
[[edge]]
from = "start"
to = "done"
"#,
    )
    .expect("parses");
    let without = crate::company::parse_workflow(
        r#"
id = "wf"
name = "WF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "done"
kind = "output"
name = "Report"
[[edge]]
from = "start"
to = "done"
"#,
    )
    .expect("parses");

    let done = |file: &crate::company::WorkflowFile| {
        translate(file)
            .nodes
            .into_iter()
            .find(|n| n.id == "done")
            .expect("output node lowered")
    };
    let with = done(&with_destination);
    let plain = done(&without);

    assert_eq!(with.kind, NodeKind::Transform, "output lowers to transform");
    // A bare pass-through: no `set` bindings, and above all no destination.
    assert_eq!(with.config, json!({}));
    assert_eq!(
        with.config, plain.config,
        "a destination must not change the engine config"
    );
}

fn node_stub(id: &str) -> crate::company::WorkflowNodeDef {
    crate::company::WorkflowNodeDef {
        id: id.into(),
        kind: crate::company::WorkflowNodeKind::Output,
        name: id.into(),
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
    }
}

fn edge_stub(from: &str, to: &str, label: Option<&str>) -> crate::company::WorkflowEdgeDef {
    crate::company::WorkflowEdgeDef {
        from: from.into(),
        to: to.into(),
        label: label.map(Into::into),
    }
}
