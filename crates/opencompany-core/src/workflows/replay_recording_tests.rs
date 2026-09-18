use super::*;
use crate::ports::run_output::RUN_OUTPUT_MAX_BYTES;
use crate::runtime::workflow_resume::CONTINUATION_PERFORMED_KEY;
use tinyflows::model::Node;

use crate::company::{WorkflowEdgeDef, WorkflowNodeDef};

/// The authored file behind a test graph: one `WorkflowNodeDef` per
/// `(id, kind, repeatable)`, and nothing else set.
///
/// Declared kinds matter — [`declared_unrepeatable`] filters on them, so a
/// test that passed the wrong kind would assert nothing.
fn authored(nodes: &[(&str, WorkflowNodeKind, Option<bool>)]) -> WorkflowFile {
    WorkflowFile {
        id: "wf".into(),
        name: "wf".into(),
        description: None,
        owner_desk: None,
        nodes: nodes
            .iter()
            .map(|(id, kind, repeatable)| WorkflowNodeDef {
                id: (*id).to_string(),
                kind: *kind,
                name: String::new(),
                summary: None,
                agent: None,
                schedule: None,
                config: None,
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: *repeatable,
                destination: None,
                postcondition: None,
                verify: None,
            })
            .collect(),
        edges: Vec::<WorkflowEdgeDef>::new(),
        global: false,
    }
}

/// An authored file that declares nothing — the pre-#850 world, and the
/// shape every test written before this issue implicitly assumed.
fn undeclared() -> WorkflowFile {
    authored(&[])
}

pub(super) fn node(id: &str, kind: NodeKind, config: Value) -> Node {
    Node {
        id: id.to_string(),
        kind,
        type_version: 1,
        name: String::new(),
        config,
        ports: Vec::new(),
        position: None,
    }
}

pub(super) fn graph(nodes: Vec<Node>) -> WorkflowGraph {
    WorkflowGraph {
        id: Some("wf".to_string()),
        nodes,
        ..WorkflowGraph::default()
    }
}

/// A settled run's output: one completed node, one capability envelope.
fn settled(node_id: &str, raw: Value) -> Value {
    json!({ "nodes": { node_id: { "items": [{ "json": raw, "text": null, "raw": raw }] } } })
}

/// A `POST` that already fired is recorded, so a continuation can replay it.
///
/// The live half of issue #846: an `http_request` node reaches an arbitrary
/// address on every company today, so this is the duplicate that is
/// reachable now rather than the one that becomes reachable when a
/// send-capable namespace is wired.
#[test]
fn a_post_that_already_fired_is_recorded() {
    let g = graph(vec![node(
        "notify",
        NodeKind::HttpRequest,
        json!({ "method": "POST", "url": "https://api.test/hooks" }),
    )]);
    let (performed, unreplayable) = outward_calls_performed(
        &g,
        &settled("notify", json!({ "status": 201 })),
        &undeclared(),
    );

    assert_eq!(performed.len(), 1, "{performed:?}");
    assert_eq!(performed[0].node, "notify");
    assert_eq!(performed[0].tool, "http_request POST");
    assert_eq!(performed[0].result, json!({ "status": 201 }));
    assert!(unreplayable.is_empty(), "{unreplayable:?}");
}

/// A `GET` is not recorded, and neither is a read-only tool.
///
/// The negative control for the classification, and the issue's own reading
/// of its reproduction: the three re-executed `web_fetch` nodes still
/// re-execute, because repeating a read costs latency rather than reaching
/// anybody. `web_search` is the `Reach::Money` carve-out — declared
/// `EffectGroup::Spend` for billing, but still a read.
#[test]
fn reads_are_not_recorded_whatever_they_cost() {
    let g = graph(vec![
        node(
            "fetch",
            NodeKind::HttpRequest,
            json!({ "method": "GET", "url": "https://api.test/x" }),
        ),
        node(
            "implicit_get",
            NodeKind::HttpRequest,
            json!({ "url": "https://api.test/x" }),
        ),
        node(
            "page",
            NodeKind::ToolCall,
            json!({ "slug": "web_fetch", "args": { "url": "https://www.bbc.com/sport" } }),
        ),
        node(
            "search",
            NodeKind::ToolCall,
            json!({ "slug": "web_search", "args": { "query": "scores" } }),
        ),
    ]);
    let mut output = json!({ "nodes": {} });
    for id in ["fetch", "implicit_get", "page", "search"] {
        output["nodes"][id] = settled(id, json!({ "ok": true }))["nodes"][id].clone();
    }

    let (performed, unreplayable) = outward_calls_performed(&g, &output, &undeclared());
    assert!(performed.is_empty(), "{performed:?}");
    assert!(unreplayable.is_empty(), "{unreplayable:?}");
}

/// `shell` is NOT recorded on its own (issue #850).
///
/// The load-bearing negative. `shell` runs an arbitrary command the host
/// does not parse, so it is `EffectGroup::Other` and falls in the residual
/// bucket — and it must stay there. Replaying it by default would stop
/// every `shell` node that builds, lints or reads from re-running, to guard
/// the rare one that reached a counterparty. #846 declined that trade on
/// purpose; this test is what stops a later change making it silently.
#[test]
fn shell_is_not_recorded_without_a_declaration() {
    let g = graph(vec![node(
        "build",
        NodeKind::ToolCall,
        json!({ "slug": "shell", "args": { "command": "curl -X POST https://api.test/x" } }),
    )]);
    let (performed, unreplayable) = outward_calls_performed(
        &g,
        &settled("build", json!({ "stdout": "" })),
        &undeclared(),
    );
    assert!(performed.is_empty(), "{performed:?}");
    assert!(unreplayable.is_empty(), "{unreplayable:?}");
}

/// `repeatable = false` puts a `shell` node in the guarded set (issue #850).
///
/// The whole feature: the author states what the host cannot infer, and the
/// same recording path every classified call already travels picks it up.
#[test]
fn a_declared_shell_node_is_recorded() {
    let g = graph(vec![node(
        "publish",
        NodeKind::ToolCall,
        json!({ "slug": "shell", "args": { "command": "./bin/announce" } }),
    )]);
    let (performed, unreplayable) = outward_calls_performed(
        &g,
        &settled("publish", json!({ "stdout": "sent" })),
        &authored(&[("publish", WorkflowNodeKind::ToolCall, Some(false))]),
    );
    assert!(unreplayable.is_empty(), "{unreplayable:?}");
    assert_eq!(performed.len(), 1, "{performed:?}");
    assert_eq!(performed[0].node, "publish");
    assert_eq!(performed[0].tool, "shell");
}

/// A replayed declared node still records its own tool name, not the
/// replay sentinel (issue #850 + #846 interaction).
///
/// `replay_performed` overwrites a declared node's own config with the
/// `REPLAY_SLUG` sentinel before this run's engine ever executes it, so by
/// the time `outward_calls_performed` looks at the graph, the node's
/// `slug` reads `__opencompany.already_performed`, not `shell`. Recording
/// that sentinel verbatim would put it on the operator's approval card in
/// place of the tool name — and dropping the node instead (the naive fix)
/// would stop tracking it after this hop: a third run downstream of a
/// second gate would find an empty ledger entry for it and call `shell`
/// for real, which is exactly the violation issue #850 exists to prevent.
/// This pins both: the name stays correct, and the node stays guarded.
#[test]
fn a_replayed_declared_node_keeps_its_own_name() {
    let authored = {
        let mut file = authored(&[("publish", WorkflowNodeKind::ToolCall, Some(false))]);
        file.nodes[0].config = Some(json!({
            "slug": "shell",
            "args": { "command": "./bin/announce" }
        }));
        file
    };
    // What `replay_performed` leaves behind on the second hop: the node's
    // own config is gone, replaced by the sentinel.
    let g = graph(vec![node(
        "publish",
        NodeKind::ToolCall,
        json!({ "slug": REPLAY_SLUG, "args": { "__replayed_result": "\"sent\"" } }),
    )]);
    let (performed, unreplayable) = outward_calls_performed(
        &g,
        &settled("publish", json!({ "stdout": "sent" })),
        &authored,
    );
    assert!(unreplayable.is_empty(), "{unreplayable:?}");
    assert_eq!(performed.len(), 1, "{performed:?}");
    assert_eq!(performed[0].node, "publish");
    assert_eq!(
        performed[0].tool, "shell",
        "must record the authored tool name, not the replay sentinel — recording the \
         sentinel is a display bug on the operator's card, and dropping the node instead \
         would silently let it run for real on the hop after next"
    );
}

/// The same recovery, for an `http_request` node (issue #850 + #846
/// interaction).
///
/// `declared_call_names` has a separate branch for `HttpRequest` that
/// builds the name from the authored method rather than a `slug` — this
/// pins that it is actually reached through the sentinel-recovery path,
/// not just present in the source. `replay_performed` converts a replayed
/// `HttpRequest` node into `NodeKind::ToolCall` with `REPLAY_SLUG` — the
/// same shape a replayed `ToolCall` node ends up in — so this is the
/// fixture that exercises the `HttpRequest` arm of `declared_call_names`
/// rather than its `ToolCall` arm.
#[test]
fn a_replayed_declared_http_node_keeps_its_own_name() {
    let authored = {
        let mut file = authored(&[("notify", WorkflowNodeKind::HttpRequest, Some(false))]);
        file.nodes[0].config = Some(json!({
            "method": "POST",
            "url": "https://api.test/hooks"
        }));
        file
    };
    // What `replay_performed` leaves behind on the second hop: kind
    // rewritten to `ToolCall`, config replaced by the sentinel — an
    // `HttpRequest` node is indistinguishable in shape from a replayed
    // `ToolCall` node by the time this function sees it.
    let g = graph(vec![node(
        "notify",
        NodeKind::ToolCall,
        json!({ "slug": REPLAY_SLUG, "args": { "__replayed_result": "{\"status\":201}" } }),
    )]);
    let (performed, unreplayable) =
        outward_calls_performed(&g, &settled("notify", json!({ "status": 201 })), &authored);
    assert!(unreplayable.is_empty(), "{unreplayable:?}");
    assert_eq!(performed.len(), 1, "{performed:?}");
    assert_eq!(performed[0].node, "notify");
    assert_eq!(
        performed[0].tool, "http_request POST",
        "must recover the authored method-based name through the HttpRequest arm of \
         declared_call_names, not the replay sentinel"
    );
}

/// A declaration only ever ADDS a guard.
///
/// `repeatable = true` on a node the host already classifies as outward is
/// not a way to switch #846 off. The author is not more authoritative than
/// the consequence table about a call the table can see; the field exists
/// for the calls it cannot.
#[test]
fn repeatable_true_cannot_remove_a_guard() {
    let g = graph(vec![node(
        "notify",
        NodeKind::HttpRequest,
        json!({ "method": "POST", "url": "https://api.test/hooks" }),
    )]);
    let (performed, _) = outward_calls_performed(
        &g,
        &settled("notify", json!({ "status": 201 })),
        &authored(&[("notify", WorkflowNodeKind::HttpRequest, Some(true))]),
    );
    assert_eq!(
        performed.len(),
        1,
        "a declared-repeatable POST is still guarded"
    );
}

/// A declaration on a `GET` guards it, because an endpoint is free to break
/// the promise the method makes.
#[test]
fn a_declared_get_is_recorded() {
    let g = graph(vec![node(
        "trip",
        NodeKind::HttpRequest,
        json!({ "method": "GET", "url": "https://api.test/fire" }),
    )]);
    let (performed, _) = outward_calls_performed(
        &g,
        &settled("trip", json!({ "status": 200 })),
        &authored(&[("trip", WorkflowNodeKind::HttpRequest, Some(false))]),
    );
    assert_eq!(performed.len(), 1, "{performed:?}");
}

/// A declaration names a node id, and a stale one guards nothing.
///
/// A graph edited between the pause and the approval can leave a
/// declaration naming a node that is gone. Silently guarding the wrong node
/// would be worse than guarding none, so the lookup is by id and misses.
#[test]
fn a_declaration_for_a_node_not_in_the_graph_guards_nothing() {
    let g = graph(vec![node(
        "build",
        NodeKind::ToolCall,
        json!({ "slug": "shell", "args": { "command": "make" } }),
    )]);
    let (performed, unreplayable) = outward_calls_performed(
        &g,
        &settled("build", json!({ "stdout": "" })),
        &authored(&[("gone", WorkflowNodeKind::ToolCall, Some(false))]),
    );
    assert!(performed.is_empty(), "{performed:?}");
    assert!(unreplayable.is_empty(), "{unreplayable:?}");
}

/// A declared node still obeys every limit `outward_calls_performed`
/// already enforces — here, the fan-out refusal.
///
/// The declaration decides *whether the node is guarded*, never *how*. A
/// declared fan-out is still refused and surfaced, because one recorded
/// result cannot answer N invocations whoever asked for the guard.
#[test]
fn a_declared_fan_out_is_still_refused() {
    let g = graph(vec![node(
        "notify_each",
        NodeKind::ToolCall,
        json!({ "slug": "shell", "args": { "command": "./bin/announce" } }),
    )]);
    let output = json!({
        "nodes": { "notify_each": { "items": [
            { "raw": { "status": 201 } },
            { "raw": { "status": 201 } }
        ] } }
    });
    let (performed, unreplayable) = outward_calls_performed(
        &g,
        &output,
        &authored(&[("notify_each", WorkflowNodeKind::ToolCall, Some(false))]),
    );
    assert!(performed.is_empty(), "{performed:?}");
    assert_eq!(unreplayable.len(), 1, "{unreplayable:?}");
    assert_eq!(unreplayable[0].node_id, "notify_each");
}

/// A declaration on a kind that makes no call is ignored here as well as
/// rejected at validation.
///
/// Validation is the place an author hears about it; this is the belt to
/// that braces, so a graph loaded from an older or looser source cannot
/// widen the guarded set through a kind the rewrite would not touch.
#[test]
fn a_declaration_on_a_non_calling_kind_is_ignored() {
    let file = authored(&[("report", WorkflowNodeKind::Output, Some(false))]);
    assert!(declared_unrepeatable(&file).is_empty());
}

/// A node the run never reached is not recorded — which is what makes
/// "recorded" mean "actually happened" rather than "is in the graph".
#[test]
fn a_node_the_run_never_reached_is_not_recorded() {
    let g = graph(vec![node(
        "notify",
        NodeKind::HttpRequest,
        json!({ "method": "POST", "url": "https://api.test/hooks" }),
    )]);
    let (performed, unreplayable) = outward_calls_performed(
        &g,
        &json!({ "nodes": { "notify": { "items": [] } } }),
        &undeclared(),
    );
    assert!(performed.is_empty(), "{performed:?}");
    assert!(unreplayable.is_empty(), "{unreplayable:?}");
}

/// A per-item fan-out is NOT recorded, and says so.
///
/// The invoker sees no item index, so one recorded result cannot answer N
/// invocations without inventing which. Guarding it approximately would be
/// worse than not guarding it, so this refuses — and surfaces a notice, so
/// the operator learns before they approve rather than afterwards.
#[test]
fn a_fan_out_is_refused_and_surfaced() {
    let g = graph(vec![node(
        "notify_each",
        NodeKind::HttpRequest,
        json!({ "method": "POST", "url": "https://api.test/hooks" }),
    )]);
    let output = json!({
        "nodes": { "notify_each": { "items": [
            { "raw": { "status": 201 } },
            { "raw": { "status": 201 } },
        ] } }
    });

    let (performed, unreplayable) = outward_calls_performed(&g, &output, &undeclared());
    assert!(performed.is_empty(), "{performed:?}");
    assert_eq!(unreplayable.len(), 1, "{unreplayable:?}");
    assert_eq!(unreplayable[0].node_id, "notify_each");
    assert!(
        unreplayable[0].notice().contains("will call it again"),
        "the notice must say what approving does: {}",
        unreplayable[0].notice()
    );
}

/// A result too large for the card is NOT recorded.
///
/// A duplicate send is bad; feeding a downstream node a silently-clipped
/// receipt as though it were whole is worse, so the guard withdraws rather
/// than degrading — and says so.
#[test]
fn an_oversized_result_is_refused_rather_than_clipped() {
    let g = graph(vec![node(
        "notify",
        NodeKind::HttpRequest,
        json!({ "method": "POST", "url": "https://api.test/hooks" }),
    )]);
    let huge = json!({ "body": "x".repeat(RUN_OUTPUT_MAX_BYTES + 1) });
    let (performed, unreplayable) =
        outward_calls_performed(&g, &settled("notify", huge), &undeclared());

    assert!(performed.is_empty(), "{performed:?}");
    assert_eq!(unreplayable.len(), 1, "{unreplayable:?}");
    assert!(
        unreplayable[0].why.contains("too large"),
        "{}",
        unreplayable[0].why
    );
}

/// The rewrite: a recorded node invokes the sentinel instead of its tool.
#[test]
fn a_recorded_node_is_rewritten_to_replay() {
    let mut g = graph(vec![
        node(
            "notify",
            NodeKind::HttpRequest,
            json!({ "method": "POST", "url": "https://api.test/hooks", "body": "hi" }),
        ),
        node(
            "page",
            NodeKind::ToolCall,
            json!({ "slug": "web_fetch", "args": { "url": "https://www.bbc.com" } }),
        ),
    ]);
    let input = json!({
        CONTINUATION_PERFORMED_KEY: [
            { "node": "notify", "tool": "http_request POST", "result": { "status": 201 } }
        ]
    });

    assert_eq!(replay_performed(&mut g, &input), vec!["notify".to_string()]);

    let notify = &g.nodes[0];
    assert_eq!(notify.kind, NodeKind::ToolCall, "the seam is the invoker's");
    assert_eq!(notify.config["slug"], json!(REPLAY_SLUG));
    assert_eq!(notify.config["execution"], json!("once"));
    // The request descriptor is gone, so nothing resolves a URL for a call
    // that is not made.
    for key in ["url", "method", "body"] {
        assert!(
            notify.config.get(key).is_none(),
            "{key} survived the rewrite"
        );
    }
    // An unrecorded node is untouched — this promotes, it never rewrites
    // what it was not told about.
    assert_eq!(g.nodes[1].config["slug"], json!("web_fetch"));
}

/// A first run — no ledger — leaves the graph byte-identical.
///
/// The claim every existing run and every existing test depends on.
#[test]
fn a_first_run_rewrites_nothing() {
    let original = graph(vec![node(
        "notify",
        NodeKind::HttpRequest,
        json!({ "method": "POST", "url": "https://api.test/hooks" }),
    )]);
    let mut g = original.clone();

    assert!(replay_performed(&mut g, &json!({ "topic": "q3" })).is_empty());
    assert_eq!(
        serde_json::to_value(&g).unwrap(),
        serde_json::to_value(&original).unwrap(),
    );
}

/// The invoker answers the sentinel with the verbatim recorded value.
#[test]
fn the_sentinel_returns_the_recorded_result() {
    let encoded = serde_json::to_string(&json!({ "status": 201, "id": "abc" })).unwrap();
    let replayed = replayed_result(REPLAY_SLUG, &json!({ REPLAY_RESULT_KEY: encoded }));
    assert_eq!(replayed, Some(json!({ "status": 201, "id": "abc" })));

    // Every other slug is none of this module's business.
    assert_eq!(replayed_result("web_fetch", &json!({ "url": "x" })), None);
    // A sentinel with nothing to replay yields null rather than falling
    // through to the real toolbelt — falling through is the one outcome
    // this exists to prevent.
    assert_eq!(replayed_result(REPLAY_SLUG, &json!({})), Some(Value::Null));
}

/// A recorded result containing an `=`-prefixed string is replayed
/// **verbatim**, not evaluated as an engine expression.
///
/// This is why the result is JSON-encoded into a single string rather than
/// embedded in the node config: `tinyflows::expr::resolve` walks a node's
/// config before it runs and evaluates every leaf beginning with `=`, and a
/// recorded result is arbitrary data from a counterparty. The encoding makes
/// that inert by construction — `serde_json::to_string` never yields a
/// document starting with `=`.
#[test]
fn a_recorded_expression_string_is_not_evaluated() {
    let hostile = json!({ "note": "=run.trigger.secret" });
    let mut g = graph(vec![node(
        "notify",
        NodeKind::HttpRequest,
        json!({ "method": "POST", "url": "https://api.test/hooks" }),
    )]);
    let (performed, _) =
        outward_calls_performed(&g, &settled("notify", hostile.clone()), &undeclared());
    let input = json!({ CONTINUATION_PERFORMED_KEY: performed });

    replay_performed(&mut g, &input);
    let args = &g.nodes[0].config["args"];

    // Nothing in the rewritten config is an `=`-expression: the whole
    // recorded value is one JSON string.
    let encoded = args[REPLAY_RESULT_KEY]
        .as_str()
        .expect("encoded as a string");
    assert!(!encoded.starts_with('='), "{encoded}");
    assert_eq!(replayed_result(REPLAY_SLUG, args), Some(hostile));
}
