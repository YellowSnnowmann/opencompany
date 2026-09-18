//! Translate a company [`WorkflowFile`] into a tinyflows
//! [`WorkflowGraph`](tinyflows::model::WorkflowGraph).
//!
//! OpenCompany's on-disk model is a validated node/edge graph whose accepted
//! node kinds are the
//! [`WORKFLOW_NODE_KINDS`](crate::company::workflow_file::WORKFLOW_NODE_KINDS)
//! authoring set (see [`crate::company::workflow_file`]); tinyflows' runnable
//! model carries the wider `NODE_KINDS` engine catalog. Every accepted kind
//! lowers into that catalog, but the parser deliberately refuses the
//! engine-only kinds — the authoring contract and the rejected set are spelled
//! out in `docs/spec/runtime/workflow-vocabulary.md`. The mapping is mostly
//! one-to-one, with two deliberate choices:
//!
//! * **`output` → [`Transform`](tinyflows::model::NodeKind::Transform)** —
//!   tinyflows has no `output` kind. A `transform` node with no `set` config is
//!   a pure pass-through, which is exactly the terminal "report back" semantics
//!   of an `output` node (its predecessors' items flow through unchanged).
//! * **condition edge labels → `true`/`false` ports** — tinyflows keys a
//!   `condition` node's branch EXCLUSIVELY on the edge `from_port`, which must be
//!   `"true"` or `"false"` (any other value is a hard validation error). The
//!   OpenCompany model carries the branch on an edge `label` (`"yes"`/`"no"`),
//!   so an edge leaving a condition node maps its label to a `true`/`false`
//!   port. Every other edge stays on the default `"main"` port.
//!
//! An `output` node's [`destination`](crate::company::WorkflowDestinationDef)
//! is deliberately **not** translated. Delivery runs host-side after the engine
//! returns (see [`super::delivery`]), so the engine has no use for it and a
//! `destination` key in node config would be inert cargo. A destination-bearing
//! `output` node therefore lowers to exactly the same bare pass-through
//! `Transform` as one without — pinned by the
//! `an_output_destination_never_reaches_the_engine_graph` test below.
//!
//! An **agent** node's roster teammate id becomes the tinyflows `agent_ref` in
//! node config, which the engine's `agent` node routes to the injected
//! `AgentRunner` — that is how a step lands on the harness pool (see
//! [`super::caps`]). It also carries an `input = "=items"` binding (issue #782)
//! so the engine resolves the full set of upstream node outputs into the node's
//! config at run time, giving the agent a channel to the previous step's result
//! (the runner folds it into the turn message; fan-in delivers every predecessor). `tool_call` and `http_request` nodes are mapped
//! structurally and both execute for real: a `tool_call` node runs a Cell A
//! toolbelt tool, fail-closed on the company's `[tools].allow` grants, and an
//! `http_request` node routes through the SSRF-guarded `GuardedHttpClient` —
//! both wired in [`super::caps`].

use std::collections::HashSet;

use serde_json::{Value, json};
use tinyflows::model::{Edge, Node, NodeKind, WorkflowGraph};

use crate::company::{WorkflowEdgeDef, WorkflowFile, WorkflowNodeDef, WorkflowNodeKind};

/// Translates a validated [`WorkflowFile`] into a tinyflows
/// [`WorkflowGraph`](tinyflows::model::WorkflowGraph) ready for
/// [`tinyflows::compiler::compile`].
///
/// The source file is assumed already validated by
/// [`parse_workflow`](crate::company::workflow_file::parse_workflow) (exactly
/// one trigger, unique node ids, edges reference real nodes), so this is a
/// total, side-effect-free mapping.
pub fn translate(file: &WorkflowFile) -> WorkflowGraph {
    // The ids of every `condition` node, so an edge leaving one can map its
    // label onto the required `true`/`false` branch port.
    let condition_ids: HashSet<&str> = file
        .nodes
        .iter()
        .filter(|n| n.kind == WorkflowNodeKind::Condition)
        .map(|n| n.id.as_str())
        .collect();
    // The ids of every `on_error = "route"` node, so an "error"-labeled edge
    // leaving one maps onto the engine's `error` port (the same mechanism as a
    // condition node's `true`/`false` ports).
    let route_ids: HashSet<&str> = file
        .nodes
        .iter()
        .filter(|n| n.on_error.as_deref() == Some("route"))
        .map(|n| n.id.as_str())
        .collect();
    // The ids of every `switch` node, so an edge leaving one carries its label
    // VERBATIM as the branch port — the engine's switch node routes to the port
    // whose name equals the computed case value (an unlabeled edge → `default`,
    // the same fallback the engine emits for a null/non-scalar discriminant).
    let switch_ids: HashSet<&str> = file
        .nodes
        .iter()
        .filter(|n| n.kind == WorkflowNodeKind::Switch)
        .map(|n| n.id.as_str())
        .collect();

    WorkflowGraph {
        id: Some(file.id.clone()),
        name: file.name.clone(),
        nodes: file.nodes.iter().map(translate_node).collect(),
        edges: file
            .edges
            .iter()
            .map(|edge| translate_edge(edge, &condition_ids, &route_ids, &switch_ids))
            .collect(),
        ..WorkflowGraph::default()
    }
}

/// The tinyflows [`NodeKind`] one OpenCompany node kind lowers to. `output` has
/// no tinyflows counterpart — a config-less `transform` is a pure pass-through
/// terminal, exactly the "report back" semantics of an `output` node.
fn tinyflows_kind(kind: WorkflowNodeKind) -> NodeKind {
    match kind {
        WorkflowNodeKind::Trigger => NodeKind::Trigger,
        WorkflowNodeKind::Agent => NodeKind::Agent,
        WorkflowNodeKind::ToolCall => NodeKind::ToolCall,
        WorkflowNodeKind::HttpRequest => NodeKind::HttpRequest,
        WorkflowNodeKind::Condition => NodeKind::Condition,
        WorkflowNodeKind::Output => NodeKind::Transform,
        // The P2 catalog maps one-to-one onto tinyflows' own kinds; all of their
        // contract rides in the node `config` overlay (P1), so `translate_node`
        // needs no per-kind handling.
        WorkflowNodeKind::Switch => NodeKind::Switch,
        WorkflowNodeKind::Merge => NodeKind::Merge,
        WorkflowNodeKind::SplitOut => NodeKind::SplitOut,
        WorkflowNodeKind::Transform => NodeKind::Transform,
        WorkflowNodeKind::OutputParser => NodeKind::OutputParser,
        WorkflowNodeKind::SubWorkflow => NodeKind::SubWorkflow,
    }
}

/// Maps one OpenCompany node to its tinyflows [`Node`], assembling the engine
/// config in three layers so a node's own config can specialize a step without
/// ever subverting the graph's identity:
///
/// 1. **Derived defaults** — the kind's built-in config (an `agent` node's
///    `prompt`).
/// 2. **User config overlay** — the node's free-form `config`, laid over the
///    defaults (so an author can override the derived `prompt`, add a
///    `tool_call` `slug`/`args`, or shape an `http_request` descriptor).
/// 3. **First-class fields LAST** — `agent_ref` (bound from `agent`, so config
///    can never rebind the node to another teammate) and the engine-read
///    `on_error` / `retry` / `requires_approval` keys. A `tool_call`'s `slug`
///    rides in the config overlay (layer 2); author-time validation guarantees
///    it is present, so translation adds no placeholder (issue #661).
///
/// A legacy node (no `config`, no typed fields) yields exactly the pre-P1
/// config, so translation of an unchanged file is byte-identical.
fn translate_node(def: &WorkflowNodeDef) -> Node {
    let mut config = serde_json::Map::new();

    // 1. Derived defaults.
    if def.kind == WorkflowNodeKind::Agent {
        config.insert("prompt".to_string(), json!(prompt_for(def)));
        // Issue #782: bind the FULL upstream node output so the agent's turn can
        // reference what the previous step produced. `=items` resolves (via the
        // engine's `resolve_config_traced`) to the `json` of every input item —
        // i.e. every direct-predecessor item, so a fan-in (`merge -> agent`, or
        // several edges into one agent) delivers ALL predecessors rather than
        // silently losing all but the first. The runner folds the resolved value
        // into the turn message (see `super::caps`); before this an agent node
        // lowered to only a static `prompt`, so an upstream node's output had no
        // channel to the next agent and was dropped. Kept in the derived-default
        // layer (like `prompt`), so an author can override the binding with their
        // own expression (e.g. `=nodes.<id>.item.text`) via node config.
        config.insert("input".to_string(), json!("=items"));
    }

    // 2. User config overlay.
    if let Some(Value::Object(user)) = &def.config {
        for (key, value) in user {
            config.insert(key.clone(), value.clone());
        }
    }

    // 3. First-class fields, written last so config cannot shadow them.
    match def.kind {
        WorkflowNodeKind::Agent => {
            if let Some(agent) = def.agent.as_deref().filter(|a| !a.is_empty()) {
                config.insert("agent_ref".to_string(), json!(agent));
            }
            // Issue #881: which node this is. The vendored `AgentRunner` trait
            // hands the capability only the resolved config and the trusted
            // `agent_ref` — the node's own id is lost at that boundary — so a
            // node that blocks on an approval could not say *which* node
            // blocked. Written in the first-class layer beside `agent_ref`, and
            // for the same reason: config must not be able to rebind a node's
            // identity to another node's name.
            config.insert("node_id".to_string(), json!(def.id));
        }
        // A `tool_call` needs a `slug` (the config overlay above carries it). The
        // masking node-id default was removed (issue #661): author-time
        // validation now rejects a slug-less `tool_call` on BOTH the on-disk seed
        // path (`workflow_file::validate`) and the console-draft path
        // (`validate_draft_against_record`), so a validated graph always binds a
        // real slug. Defaulting to the node id instead pointed the engine's
        // "missing tool" error at the wrong name and could collide with a real
        // tool, silently invoking one the author never chose; absent a slug the
        // engine's `tool_call` node now fails loudly on the missing key.
        WorkflowNodeKind::ToolCall => {}
        _ => {}
    }

    // Per-node error policy the tinyflows engine reads straight off config.
    if let Some(on_error) = def.on_error.as_deref() {
        config.insert("on_error".to_string(), json!(on_error));
    }
    if let Some(retry) = &def.retry {
        config.insert("retry".to_string(), retry_config(retry));
    }
    if let Some(requires_approval) = def.requires_approval {
        config.insert("requires_approval".to_string(), json!(requires_approval));
    }
    // Issue #1866: the deterministic postcondition, lowered the same way
    // `retry` is — a typed model field becomes the exact config key
    // `HarnessAgentRunner::run_turn` reads (`caps::postcondition`).
    if let Some(postcondition) = &def.postcondition {
        config.insert(
            "postcondition".to_string(),
            serde_json::to_value(postcondition)
                .expect("WorkflowPostconditionDef always serializes"),
        );
    }
    if let Some(verify) = &def.verify {
        config.insert(
            "verify".to_string(),
            serde_json::to_value(verify).expect("WorkflowJudgeDef always serializes"),
        );
    }

    Node {
        id: def.id.clone(),
        kind: tinyflows_kind(def.kind),
        type_version: 1,
        name: def.name.clone(),
        config: Value::Object(config),
        ports: Vec::new(),
        position: None,
    }
}

/// Builds the engine's `retry` config object from the typed policy, emitting the
/// exact keys the tinyflows engine reads (`max_attempts` / `backoff_ms` /
/// `backoff`) and omitting any the author left unset (the engine defaults them).
fn retry_config(retry: &crate::company::WorkflowRetryDef) -> Value {
    let mut map = serde_json::Map::new();
    if let Some(max_attempts) = retry.max_attempts {
        map.insert("max_attempts".to_string(), json!(max_attempts));
    }
    if let Some(backoff_ms) = retry.backoff_ms {
        map.insert("backoff_ms".to_string(), json!(backoff_ms));
    }
    if let Some(backoff) = &retry.backoff {
        map.insert("backoff".to_string(), json!(backoff));
    }
    Value::Object(map)
}

/// The instruction handed to an agent node: its summary when present, else its
/// human-readable name.
fn prompt_for(def: &WorkflowNodeDef) -> String {
    def.summary
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&def.name)
        .to_string()
}

/// Maps one OpenCompany edge to a tinyflows [`Edge`]. An "error"-labeled edge
/// leaving an `on_error = "route"` node carries the `error` port (the engine
/// emits the failure item there); edges leaving a `condition` node carry their
/// branch on `from_port` (`true`/`false`, mapped from the label); every other
/// edge stays on the default `main` port.
///
/// The error-port check runs **first** so it takes precedence for a node that is
/// both a `condition` and `on_error = "route"`: without it, the condition branch
/// would map the `"error"` label through [`condition_port`] onto the `true`
/// port, silently misrouting the failure item onto the truthy branch instead of
/// the error edge.
fn translate_edge(
    edge: &WorkflowEdgeDef,
    condition_ids: &HashSet<&str>,
    route_ids: &HashSet<&str>,
    switch_ids: &HashSet<&str>,
) -> Edge {
    let from_port =
        if route_ids.contains(edge.from.as_str()) && edge.label.as_deref() == Some("error") {
            // The error-port check stays FIRST so a node that is both a routing
            // node and a condition/switch routes its `"error"` edge to the error
            // port rather than through the branch mapping below.
            "error".to_string()
        } else if switch_ids.contains(edge.from.as_str()) {
            // A switch edge's label is the case name, carried verbatim; an
            // unlabeled edge falls to the engine's `default` fallback port.
            edge.label
                .as_deref()
                .map(str::to_string)
                .unwrap_or_else(|| "default".to_string())
        } else if condition_ids.contains(edge.from.as_str()) {
            condition_port(edge.label.as_deref())
        } else {
            "main".to_string()
        };
    Edge {
        from_node: edge.from.clone(),
        from_port,
        to_node: edge.to.clone(),
        to_port: "main".to_string(),
    }
}

/// Maps a condition edge's label onto the required `true`/`false` branch port.
/// Negative labels (`no`/`false`/`n`) map to `"false"`; everything else
/// (including an absent label) maps to `"true"`.
fn condition_port(label: Option<&str>) -> String {
    let negative = label
        .map(|l| l.trim().to_ascii_lowercase())
        .is_some_and(|l| matches!(l.as_str(), "no" | "false" | "n"));
    if negative { "false" } else { "true" }.to_string()
}

#[cfg(test)]
#[path = "translate_tests.rs"]
mod tests;
