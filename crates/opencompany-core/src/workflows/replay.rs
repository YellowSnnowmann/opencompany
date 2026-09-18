//! Issue #846: a continuation must not make an outward call its own lineage
//! already made.
//!
//! # What this is a second half of
//!
//! A paused workflow run is **settled**, not suspended — the engine finishes and
//! reports the gates it stopped at — so approving is a re-run from the trigger
//! with the gate listed in `approvals`. That primitive is deliberate, it is
//! restart-durable for free, and #438 accepted it: the requirement it wrote down
//! was not "stop re-executing" but **"an approval must never cause a message to
//! be sent to a person twice."**
//!
//! #496 met that requirement for the half the host performs with its own hands:
//! an `output` node's report, routed by [`deliver_outputs`](super::delivery)
//! after the engine settles. A ledger rides the approval card, and a reached
//! output node listed in it is skipped rather than dispatched.
//!
//! It does not reach the other half. A `send`, `publish` or `repo_publish`
//! wired as a **`tool_call` node** is performed by the engine, mid-run, through
//! a capability — so there is no post-hoc dispatch for the host to decline. Put
//! such a node upstream of a gate and today the first run sends, the operator
//! approves, and the continuation sends again. Same exposure, same lineage, the
//! other mechanism.
//!
//! # The seam, and why it is this one
//!
//! [`ToolInvoker::invoke`](tinyflows::caps::ToolInvoker) receives
//! `(slug, args, conn)` — no node id, no run state — which
//! [`gate`](super::gate) already records as the reason the *approval* gate could
//! not live there. The same fact rules out a node-keyed skip inside the invoker,
//! and node identity is not negotiable here: keying on `(tool, args)` instead
//! would miss every send whose body an upstream agent node re-generates, which
//! is most of them.
//!
//! What the host does own is the **translation** — it builds the graph per run,
//! which is where [`apply_policy_gates`](super::gate::apply_policy_gates)
//! already writes per-node decisions. So a node the lineage has already called
//! is rewritten *before the run starts* to invoke a host-private sentinel slug
//! carrying its recorded result, and the invoker answers that slug from its
//! arguments without touching the toolbelt. The engine is unchanged, the graph
//! shape is unchanged, and the node still produces output — the same output,
//! because the recorded value is the verbatim capability return and
//! `envelope::wrap` is a pure function of it.
//!
//! # Which nodes, and the line drawn
//!
//! Two kinds, on two different rules, because the two have different amounts of
//! declaration behind them.
//!
//! **`http_request` with a mutating method** — anything but `GET`/`HEAD`. This
//! is the live one. An `http_request` node reaches an arbitrary address today,
//! on every company, and a `POST` upstream of a gate is a second POST on every
//! approval. Idempotency by HTTP method is the standard the method names carry,
//! and it is the only thing the host can read about a call whose destination is
//! authored per node.
//!
//! **`tool_call` whose [`consequence_of`](crate::policy::consequence_of) group
//! is an outward one** — anything but
//! [`EffectGroup::Other`](crate::ports::types::EffectGroup::Other), less
//! [`Reach::Money`](crate::policy::Reach::Money) (below). That is a reader of the
//! one declaration both policy tiers read, not a second table to keep in step
//! with it.
//!
//! ## What that second rule does and does not reach today, stated plainly
//!
//! **Nothing, today.** A workflow `tool_call` can only invoke the four families
//! [`WORKFLOW_TOOL_NAMESPACES`](super::caps) wires — shell, code, web, search —
//! and every slug in them is declared `EffectGroup::Other` except `web_search`,
//! which this rule excludes. So there is no wired slug this arm currently
//! guards, and #846's own example — "put a `send`, `publish` or `repo_publish`
//! node after two gated steps" — describes agent-turn tool families the workflow
//! invoker does not wire at all.
//!
//! That is worth stating rather than quietly implying otherwise, and it is a
//! reason to write the rule now rather than later: it is the guard that has to
//! exist *before* a send-capable namespace is wired into workflows, or wiring
//! one silently re-opens #438 on the day it lands. It costs one arm of one
//! `match`, and it is exercised by this module's tests against the declared
//! table rather than against a wired tool — which is the most that can honestly
//! be claimed for a rule whose subject does not exist yet.
//!
//! ## Two gaps this does NOT close
//!
//! * **`shell`, `curl` and `git_operations`.** All three are `EffectGroup::Other`
//!   and all three can reach a counterparty — `curl` to an address, `shell` to
//!   anything at all, `git_operations` to a remote. The host cannot tell a
//!   `git log` from a `git push`, and replaying every shell node's recorded
//!   stdout on every continuation would change the behaviour of the most common
//!   effectful node there is on the strength of a guess. Left as it is,
//!   deliberately, and filed as issue #850 rather than folded in.
//! * **`web_fetch` and `web_search`.** Read-only, so the three re-executed
//!   fetches in #846's own reproduction still re-execute — which is the issue's
//!   own reading of them: idempotent reads whose cost is latency and tokens.
//!   `web_search` is excluded by the [`Reach::Money`] carve-out rather than by
//!   its group, because it is declared `EffectGroup::Spend` for billing and is
//!   a read: it reaches nobody and changes nothing, and #438 priced repeated
//!   spend as cost rather than as the harm being guarded here. Replaying it
//!   would also hand a later run a stale answer it never asked for, which is the
//!   argument against replaying a fetch, and the two should not differ.
//!
//! # Two limits, stated rather than hidden
//!
//! * **A truncated result is never replayed.** The recorded value rides the
//!   durable approval card, so it is bounded like everything else that does
//!   ([`bound_node_output`](crate::ports::bound_node_output)). If bounding would
//!   clip it, the node is not recorded and the continuation calls again — a
//!   duplicate send is bad, and feeding downstream a silently-clipped receipt as
//!   if it were real is worse. The operator is told, via a run notice.
//! * **A per-item fan-out is not replayed.** A `split_out` → `tool_call` node
//!   invokes once per item, and the invoker sees no item index, so one recorded
//!   result cannot answer N invocations without inventing which. Recording only
//!   the single-invocation shape keeps the guard exact where it applies instead
//!   of approximate everywhere; the fan-out case behaves as it does today and
//!   says so. #496 took the same side of the same trade for a partial delivery
//!   fan-out.
//!
//! Both limits are *visible*: [`replay_performed`] returns what it could not
//! guard so the runner can surface it as a notice (issue #638's mechanism), so
//! "this continuation called out again" is something an operator reads rather
//! than something they reconstruct.
//!
//! The complete answer to all of this is checkpointed resume, and it is still
//! not available: tinyflows' `run_resumable` installs a no-op observer and a
//! process-local in-memory checkpointer and takes no cancellation token, so it
//! composes with neither the per-node progress trail (#371) nor the stop signal
//! (#398) this runner is built on, and an approval that arrives after a restart
//! would find no checkpoint. That is engine work in a vendored crate, exactly as
//! #438 recorded. This guard is forward-compatible with it: it is keyed on the
//! node and it withdraws to nothing when the ledger is empty.

use serde_json::{Value, json};
use tinyflows::model::{NodeKind, WorkflowGraph};

use crate::company::{WorkflowFile, WorkflowNodeKind};
use crate::ports::bound_node_output;

use crate::runtime::workflow_resume::{PerformedCall, performed_in_input};

use crate::workflows::caps::resolver::{
    ChildGateRecord, ChildGateRegistry, GATE_NAMESPACE, child_id_of,
};

/// The host-private slug a replayed node invokes instead of its real tool.
///
/// Namespaced with a `__opencompany` prefix that no toolbelt tool carries and no
/// authoring surface accepts. Even so the invoker's arm is written to be safe
/// against an author who types it anyway: it reaches no capability, executes
/// nothing and returns only what its own arguments carry, so the worst an
/// authored occurrence can do is produce an inert node — strictly less than any
/// grant would already allow.
pub(crate) const REPLAY_SLUG: &str = "__opencompany.already_performed";

/// The argument key carrying the recorded result, **JSON-encoded as a string**.
///
/// Encoded rather than embedded, and this is load-bearing. Node config is walked
/// by [`tinyflows::expr::resolve`] before the node runs, and every leaf string
/// beginning with `=` is evaluated as an expression against the run scope. A
/// recorded result is arbitrary provider data that may contain such a string, so
/// embedding it verbatim would hand a counterparty's response to the expression
/// engine. `serde_json::to_string` of any value yields a document starting with
/// `{`, `[`, `"`, a digit, `t`, `f` or `n` — never `=` — so a single encoded
/// string is inert by construction rather than by escaping rules.
pub(crate) const REPLAY_RESULT_KEY: &str = "result_json";

/// The recorded result behind a [`REPLAY_SLUG`] invocation, or `None` for every
/// other slug (issue #846).
///
/// The whole of what a tool invoker has to know about this module: one
/// comparison and one decode, so both invokers can answer the sentinel
/// identically without either growing a copy of the rules.
///
/// A sentinel invocation whose argument is missing or is not the JSON the host
/// wrote yields [`Value::Null`] rather than `None`. Falling through to the real
/// toolbelt would be the one outcome this module exists to prevent — the node
/// was rewritten precisely *because* calling out again is the bug — and no live
/// tool answers to this slug anyway, so the fall-through would fail the node
/// rather than send. A null return is a node that produced nothing, which is
/// visible in the run's own output.
pub(crate) fn replayed_result(slug: &str, args: &Value) -> Option<Value> {
    if slug != REPLAY_SLUG {
        return None;
    }
    let decoded = args
        .get(REPLAY_RESULT_KEY)
        .and_then(Value::as_str)
        .and_then(|encoded| serde_json::from_str(encoded).ok())
        .unwrap_or(Value::Null);
    tracing::info!(
        recovered = !decoded.is_null(),
        "workflow: replaying a call this run's lineage already made; not calling out again \
         (issue #846)"
    );
    Some(decoded)
}

/// A node the continuation could not replay, and why — surfaced to the operator
/// as a run notice rather than left in the host's logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnreplayableCall {
    /// The node that will call out a second time.
    pub node_id: String,
    /// The tool it will call.
    pub slug: String,
    /// Operator-facing prose for why the guard did not apply.
    pub why: &'static str,
}

impl UnreplayableCall {
    /// The sentence an operator reads, on the run that parked — before they
    /// approve, which is the only moment they can still act on it.
    pub fn notice(&self) -> String {
        format!(
            "“{}” ({}) reached outside the company on this run, and approving the gate below it \
             will call it again: {}.",
            self.node_id, self.slug, self.why
        )
    }
}

/// Every `tool_call` node in `graph` whose call left the building on this run,
/// paired with the verbatim result it returned (issue #846).
///
/// Read off the settled run's own `output["nodes"]`, which already holds every
/// completed node's items — so this costs no engine change and no second source
/// of truth. A node that did not complete has no entry and is not recorded; a
/// node the run never reached (everything past the gate) has none either, which
/// is what makes "recorded" mean "actually happened".
///
/// Returns `(performed, unreplayable)`: what a continuation may replay, and what
/// it may not, so the caller can carry the second half to the operator instead
/// of silently guarding less than it appears to.
pub(crate) fn outward_calls_performed(
    graph: &WorkflowGraph,
    output: &Value,
    authored: &WorkflowFile,
) -> (Vec<PerformedCall>, Vec<UnreplayableCall>) {
    let nodes = output.get("nodes");
    let mut performed = Vec::new();
    let mut unreplayable = Vec::new();
    let declared = declared_unrepeatable(authored);
    let declared_names = declared_call_names(authored);

    for node in &graph.nodes {
        let is_declared = declared.contains(node.id.as_str());
        // `replay_performed` overwrites a replayed node's own config with the
        // `REPLAY_SLUG` sentinel *before* this run's engine ever sees it, so by
        // the time the settled output reaches here the node's own `slug` no
        // longer names the call it made — it names the sentinel (issue #850 +
        // #846 interaction). Recording that sentinel verbatim would put
        // `__opencompany.already_performed` on the operator's approval card in
        // place of the real tool name. A declared node has to keep tracking
        // under its own name instead — dropping it (the naive fix) would stop
        // guarding it after this hop, so a third run downstream of a second
        // gate would find an empty ledger entry and call it for real, which is
        // exactly what the declaration exists to prevent. An undeclared node's
        // replay is dropped here exactly as it already, if accidentally, was:
        // it falls to `outward_call_of`'s classifier below, which cannot
        // classify the sentinel either — this just makes that explicit instead
        // of leaning on the policy table never having an opinion about it.
        let is_replay_sentinel = matches!(node.kind, NodeKind::ToolCall)
            && node.config.get("slug").and_then(Value::as_str) == Some(REPLAY_SLUG);
        let Some(slug) = (if is_replay_sentinel {
            is_declared
                .then(|| declared_names.get(node.id.as_str()).cloned())
                .flatten()
        } else {
            outward_call_of(node, is_declared)
        }) else {
            continue;
        };
        // Not reached, or reached and produced nothing: there is nothing to
        // guard and nothing to replay.
        let Some(items) = nodes
            .and_then(|map| map.get(&node.id))
            .and_then(|node_output| node_output.get("items"))
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
        else {
            continue;
        };

        if items.len() > 1 {
            unreplayable.push(UnreplayableCall {
                node_id: node.id.clone(),
                slug,
                why: "it runs once per input item, and a single recorded result cannot stand in \
                      for several",
            });
            continue;
        }
        // The engine wraps a capability return as `{ json, text, raw }`, so
        // `raw` is the verbatim value — replaying it reconstructs this exact
        // envelope rather than approximating it. An item without `raw` is not a
        // capability envelope and is not something this can faithfully replay.
        let Some(raw) = items[0].get("raw") else {
            unreplayable.push(UnreplayableCall {
                node_id: node.id.clone(),
                slug,
                why: "its output is not in the engine's capability envelope, so there is no \
                      verbatim result to replay",
            });
            continue;
        };

        let (bounded, truncated) = bound_node_output(raw);
        if truncated {
            unreplayable.push(UnreplayableCall {
                node_id: node.id.clone(),
                slug,
                why: "its result is too large to carry on the approval card, and a clipped \
                      result must not be replayed as if it were whole",
            });
            continue;
        }
        performed.push(PerformedCall {
            node: node.id.clone(),
            tool: slug,
            result: bounded,
        });
    }

    (performed, unreplayable)
}

/// The ungated outward calls a paused child will repeat when its gate is
/// approved (issue #617).
///
/// A `sub_workflow` child that stops at an approval gate pauses the *parent*:
/// tinyflows reports the child's gates namespaced (`<node>::<gate>`, nested
/// one level per child — `sub::nested::work` for a gate two levels down), and
/// the continuation re-runs the parent, which re-runs every child from the
/// top. Any outward call the child made before the gate therefore fires again —
/// but unlike a top-level call, its result does not travel up when the child
/// pauses (tinyflows drops the child's partial output on
/// `ChildOutcome::Paused`), so there is nothing to replay. Report it the same
/// way [`outward_calls_performed`] reports a top-level call it cannot replay,
/// so the operator is warned that approving restarts the child from the top.
///
/// Each namespaced pending id is resolved through `registry` to the child graph
/// the resolver actually gated — descending through nested namespaces, and
/// resolving an expression-bound `workflow_id` against `trigger_input` where
/// the engine's `once` scope allows — then every call **upstream-reachable**
/// from the paused gate is examined. "Upstream-reachable" is read off the
/// child's edges by walking backward from the gate; a call on an un-taken
/// branch of a fan-out is over-reported, the same conservative direction the
/// top-level [`outward_calls_performed`] takes when it reads "completed" from
/// the run state rather than per-branch.
///
/// A `requires_approval` node this run's list has already approved is *not*
/// treated as still-blocked: the engine executes an approved gate (it skips the
/// interrupt only when the id is listed), so the call fires on this
/// continuation and will fire again on the next — exactly what the operator
/// must be warned about.
pub(crate) fn child_calls_to_repeat(
    parent: &WorkflowGraph,
    pending: &[String],
    registry: &ChildGateRegistry,
    trigger_input: &Value,
) -> Vec<UnreplayableCall> {
    let approved = approved_ids(trigger_input);
    let mut out = Vec::new();
    for node_id in pending {
        let mut segments: Vec<&str> = node_id.split(GATE_NAMESPACE).collect();
        let Some(gate) = segments.pop() else {
            continue;
        };
        let mut graph = parent.clone();
        let mut record: Option<ChildGateRecord> = None;
        let mut prefix = String::new();

        // A nested child is restarted from its own root, so calls in every
        // ancestor child before the next `sub_workflow` node are repeated too.
        // Walk each intermediate graph before descending to the record below it.
        for segment in segments {
            let Some(child_id) = child_id_of(&graph, segment, Some(trigger_input)) else {
                record = None;
                break;
            };
            if let Some(ancestor) = record.as_ref() {
                for call in
                    child_calls_preceding(&ancestor.graph, segment, &approved, &prefix, &prefix)
                {
                    if !out.contains(&call) {
                        out.push(call);
                    }
                }
            }
            let Some(next) = registry.get(&child_id) else {
                record = None;
                break;
            };
            prefix.push_str(segment);
            prefix.push_str(GATE_NAMESPACE);
            graph = next.graph.clone();
            record = Some(next);
        }

        let Some(record) = record else {
            continue;
        };
        // Keep the deepest-child node ids in their established local form;
        // ancestor calls carry the namespace so equal local ids remain distinct.
        for call in child_calls_preceding(&record.graph, gate, &approved, &prefix, "") {
            if !out.contains(&call) {
                out.push(call);
            }
        }
    }
    out
}

/// The ids this lineage has already approved, read off the continuation input
/// the same way the engine's `approvals_for_child` reads them
/// (`run.trigger.approvals`).
fn approved_ids(trigger_input: &Value) -> std::collections::HashSet<String> {
    trigger_input
        .get("approvals")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The outward calls in `child` that run before `gate` and are not themselves
/// gated.
///
/// Every node from which `gate` is reachable by walking `edges` backward must
/// have run (or be on the branch that ran) before the pause; everything past
/// the gate is unreachable and excluded. A `requires_approval` node the gate
/// pass marked is excluded too — the child restarts and *pauses at it again*
/// rather than executing it — **unless its namespaced id (`namespace_prefix` +
/// the node's own id) is already in `approved`**: an approved gate does
/// execute, on this continuation, and will execute again on the next, so it is
/// exactly the call the operator must be warned about (issue #617).
///
/// Classified with the same [`outward_call_of`] the top-level guard uses, so
/// the two reports agree about what an "outward call" is. The child's authored
/// `repeatable` declarations are not consulted (the resolver keeps the
/// translated graph, not the authored file); an undeclared classification is
/// the conservative direction for a warning.
fn child_calls_preceding(
    child: &WorkflowGraph,
    gate: &str,
    approved: &std::collections::HashSet<String>,
    namespace_prefix: &str,
    output_prefix: &str,
) -> Vec<UnreplayableCall> {
    let mut reached = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from([gate.to_string()]);
    while let Some(id) = queue.pop_front() {
        if !reached.insert(id.clone()) {
            continue;
        }
        for edge in &child.edges {
            if edge.to_node == id {
                queue.push_back(edge.from_node.clone());
            }
        }
    }
    let mut out = Vec::new();
    for node in &child.nodes {
        if !reached.contains(&node.id) {
            continue;
        }
        let is_gate = node
            .config
            .get("requires_approval")
            .and_then(Value::as_bool)
            == Some(true);
        if is_gate && !approved.contains(&format!("{namespace_prefix}{}", node.id)) {
            // The child restarts and pauses at this node again; it does not
            // execute it.
            continue;
        }
        let Some(slug) = outward_call_of(node, false) else {
            continue;
        };
        out.push(UnreplayableCall {
            node_id: format!("{output_prefix}{}", node.id),
            slug,
            why: "it runs inside a child workflow whose ungated calls are not carried up when \
                  the child pauses, so approving restarts the child and calls it again",
        });
    }
    out
}

/// Rewrites every node this lineage has already called so the continuation
/// replays its recorded result instead of calling again (issue #846).
///
/// Mutates `graph` in place, beside [`apply_policy_gates`](super::gate::apply_policy_gates)
/// and before compilation, which is the one moment the host holds both the node
/// ids and the trigger input. A first run — anything with no ledger on its input
/// — leaves the graph byte-identical, so every existing run and every existing
/// test is untouched.
///
/// Returns the node ids it rewrote, for the log line and for the tests that pin
/// this: an empty return is the honest answer for a graph whose ledger names
/// nodes it does not contain (a graph edited between the pause and the
/// approval), where re-calling is the only thing left to do.
pub(crate) fn replay_performed(graph: &mut WorkflowGraph, trigger_input: &Value) -> Vec<String> {
    let ledger = performed_in_input(trigger_input);
    if ledger.is_empty() {
        return Vec::new();
    }

    let mut replayed = Vec::new();
    for node in &mut graph.nodes {
        if !matches!(node.kind, NodeKind::ToolCall | NodeKind::HttpRequest) {
            continue;
        }
        let Some(call) = ledger.iter().find(|call| call.node == node.id) else {
            continue;
        };
        let Value::Object(config) = &mut node.config else {
            continue;
        };
        let Ok(encoded) = serde_json::to_string(&call.result) else {
            // Unserializable is unreachable for a value that arrived by
            // deserialization, and re-calling is the safe direction if it ever
            // is not: a node that runs twice is the bug being fixed, a node that
            // silently returns nothing is a new one.
            continue;
        };

        // An `http_request` node becomes a `tool_call` invoking the sentinel.
        //
        // The kind change is what lets one seam serve both, and it is safe
        // because the two node kinds already agree about the only thing that
        // leaves the node: every capability node wraps its result in the same
        // `{ json, text, raw }` envelope, so a downstream `=item.json.<field>`
        // binding reads the same value either way. The alternative was a second
        // replay mechanism inside `GuardedHttpClient` — which, like the invoker,
        // sees no node id and would have needed its own identity scheme.
        node.kind = NodeKind::ToolCall;
        // The request descriptor, removed rather than left to rot. Leaving it
        // would have the engine resolve a URL and headers for a call that is
        // not made, and would leave a reader of the compiled graph unable to
        // tell a replayed node from a live one.
        for key in ["url", "method", "headers", "body"] {
            config.remove(key);
        }

        config.insert("slug".to_string(), json!(REPLAY_SLUG));
        config.insert("args".to_string(), json!({ REPLAY_RESULT_KEY: encoded }));
        // Nothing to connect to, and leaving a stale ref would have the engine
        // resolve a connection for a call that is not made.
        config.remove("connection_ref");
        // One invocation, one recorded result. A node left in `per_item` mode
        // would replay the same result once per input item; `outward_calls_performed`
        // refuses to record a fan-out for exactly that reason, and pinning the
        // mode here means a graph edited between the pause and the approval
        // cannot re-open the hole.
        config.insert("execution".to_string(), json!("once"));
        replayed.push(node.id.clone());
    }

    replayed
}

/// The node ids whose author declared `repeatable = false` (issue #850).
///
/// Read off the **authored** file rather than the compiled graph, the same way
/// [`deliver_outputs`](super::delivery::deliver_outputs) reads `destination`:
/// this is host-side policy the engine never sees, so putting it in engine
/// config would be an inert key riding into the graph.
///
/// Restricted to the two kinds that make a call, mirroring the validation that
/// rejects the field anywhere else — so a graph loaded from an older or looser
/// source cannot widen the guarded set through a kind the rewrite would not
/// touch anyway.
fn declared_unrepeatable(authored: &WorkflowFile) -> std::collections::HashSet<&str> {
    authored
        .nodes
        .iter()
        .filter(|node| {
            node.repeatable == Some(false)
                && matches!(
                    node.kind,
                    WorkflowNodeKind::ToolCall | WorkflowNodeKind::HttpRequest
                )
        })
        .map(|node| node.id.as_str())
        .collect()
}

/// The outward-call identity a declared-unrepeatable node's own authored
/// config names, keyed by node id.
///
/// Consulted only when [`outward_calls_performed`] finds a node whose compiled
/// config has already been overwritten by [`replay_performed`] with the
/// [`REPLAY_SLUG`] sentinel: at that point the graph itself has nothing left
/// to classify, so the name has to be read back off `authored` — the same
/// source [`declared_unrepeatable`] reads, for the same reason.
fn declared_call_names(authored: &WorkflowFile) -> std::collections::HashMap<&str, String> {
    authored
        .nodes
        .iter()
        .filter(|node| node.repeatable == Some(false))
        .filter_map(|node| {
            let name = match node.kind {
                WorkflowNodeKind::ToolCall => node
                    .config
                    .as_ref()
                    .and_then(|c| c.get("slug"))
                    .and_then(Value::as_str)?
                    .to_string(),
                WorkflowNodeKind::HttpRequest => {
                    let method = node
                        .config
                        .as_ref()
                        .and_then(|c| c.get("method"))
                        .and_then(Value::as_str)
                        .unwrap_or("GET")
                        .to_uppercase();
                    format!("http_request {method}")
                }
                _ => return None,
            };
            Some((node.id.as_str(), name))
        })
        .collect()
}

/// The name of the outward call a node makes — or `None` when the node makes no
/// call, or makes one that reaches nobody outside the company.
///
/// The name is for the operator and the log line; the *identity* a ledger entry
/// is matched on is always the node id.
///
/// An `agent` node is deliberately absent, and it is a different question rather
/// than an oversight: its own tool calls park through #395's drain and are
/// decided one at a time, and its re-execution is the token cost #438 already
/// priced and declined to fix here.
fn outward_call_of(node: &tinyflows::model::Node, declared_unrepeatable: bool) -> Option<String> {
    match node.kind {
        NodeKind::ToolCall => {
            let slug = node.config.get("slug").and_then(Value::as_str)?;
            let args = node
                .config
                .get("args")
                .cloned()
                .unwrap_or_else(|| json!({}));
            // The author said this call must not be made twice (issue #850).
            // Read BEFORE the classifier, because the whole point is the calls
            // the classifier cannot see: `shell` runs an arbitrary command and
            // the host does not parse it, so no amount of inspection here will
            // ever reach the right answer. Only the guarding direction is taken
            // from the author — `repeatable = true` falls through to the
            // classifier below rather than overriding it, so a declaration can
            // never switch off a guard #846 already applies.
            if declared_unrepeatable {
                return Some(slug.to_string());
            }
            let consequence = crate::policy::consequence_of(slug, &args);
            // The residual bucket — "no particular consequence to name on the
            // card" — is everything that stays inside the company plus the three
            // this module's docs name as an open gap.
            if consequence.group.is_unclassified() {
                return None;
            }
            // Billed, but it reaches nobody and changes nothing. See the module
            // docs: repeated spend is cost, not the harm being guarded.
            if consequence.reach.costs_money() {
                return None;
            }
            Some(slug.to_string())
        }
        // Reaches an arbitrary address on every company today, so this is the
        // arm that guards a *live* duplicate. Read by method, which is the only
        // thing the host knows about a destination the author supplies per node
        // — and which is exactly what the method names are for.
        NodeKind::HttpRequest => {
            let method = node
                .config
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .to_uppercase();
            // A `GET` the author knows has a side effect — the method promises
            // to have done nothing, and an endpoint is free to break that
            // promise. Same one-way rule as the `tool_call` arm: the
            // declaration adds this node to the guarded set and can never take
            // a non-safe method out of it.
            if SAFE_METHODS.contains(&method.as_str()) && !declared_unrepeatable {
                return None;
            }
            Some(format!("http_request {method}"))
        }
        _ => None,
    }
}

/// HTTP methods a continuation may repeat: the **safe** ones, not the
/// idempotent ones.
///
/// `PUT` and `DELETE` are idempotent in the RFC's sense — the server state after
/// N identical requests equals the state after one — and that is deliberately
/// not the property being asked for. A duplicate `DELETE` still fires whatever
/// the endpoint does on receipt, and "the row is still gone" is no comfort if
/// the second request also sent someone a notification. Only `GET` and `HEAD`
/// promise to have done nothing.
const SAFE_METHODS: [&str; 2] = ["GET", "HEAD"];

#[cfg(test)]
#[path = "replay_child_gate_tests.rs"]
mod tests_child_gate;
#[cfg(test)]
#[path = "replay_recording_tests.rs"]
mod tests_recording;
