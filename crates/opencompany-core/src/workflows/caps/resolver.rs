//! The [`WorkflowResolver`] that backs `sub_workflow`-by-id for a company run.
//!
//! tinyflows is persistence-free: when a `sub_workflow` node references a child
//! by `workflow_id`, the engine asks the host to resolve that id to a runnable
//! [`WorkflowGraph`](tinyflows::model::WorkflowGraph). [`StoreWorkflowResolver`]
//! serves that from the union of the company's two graph sources — the seed
//! files (`companies/<name>/workflows/<id>.toml`) and the runtime-authored
//! bodies on the [`CompanyRecord`](crate::ports::types::CompanyRecord) overlay —
//! running the child through the SAME full
//! [`parse_workflow`](crate::company::parse_workflow) validation a
//! hand-authored or console-created workflow gets, then translating it.
//!
//! Reading the overlay matters for more than availability: a hosted tenant's
//! children live *only* there, so a resolver that saw the seed side alone would
//! both fail to resolve them and — worse — miss them in the cycle scan below.
//!
//! ## Cycle safety
//!
//! Two independent guards stop a `sub_workflow` chain from looping forever:
//!
//! * **This resolver's static guard** — before a child is loaded/translated, a
//!   bounded breadth-first scan walks the *static* `workflow_id` references in the
//!   store starting from the requested id. If the requested id itself, or the
//!   run's `root_id`, appears in that transitive closure, the chain would loop and
//!   the resolve is refused with a named error. This catches the common
//!   one-and-two-level cycles (A→B→A) eagerly, before any child runs.
//! * **The engine's depth backstop** — a *dynamic* id (an `=expr` resolved at run
//!   time) can't be scanned statically, so the engine's
//!   [`MAX_SUB_WORKFLOW_DEPTH`](tinyflows::engine::MAX_SUB_WORKFLOW_DEPTH) bound
//!   still terminates any cycle formed through expression-computed ids.
//!
//! The resolver is stateless per call — each `resolve` re-loads the record and
//! re-reads the source directory, so a workflow edited between steps is picked
//! up.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use async_trait::async_trait;
use tinyflows::caps::WorkflowResolver;
use tinyflows::error::{EngineError, Result as TfResult};
use tinyflows::model::{NodeKind, WorkflowGraph};

use crate::company::{WorkflowFile, WorkflowNodeKind, load_workflow_with_globals};
use crate::ports::CompanyStore;
use crate::ports::types::{CompanyId, OverlayWorkflow};

/// Hard bound on how many workflows the static cycle scan visits before giving
/// up. A store this deep is either pathological or adversarial; refusing to run
/// is safer than an unbounded walk.
const MAX_STATIC_RESOLVE_NODES: usize = 64;

/// A [`WorkflowResolver`] serving `sub_workflow`-by-id from a company's on-disk
/// `workflows/` directory, with a static transitive-closure cycle guard.
pub struct StoreWorkflowResolver {
    /// The company source directory (`companies/<name>`); seed children live
    /// under its `workflows/<id>.toml`. `None` on a hosted tenant, whose
    /// children are all overlay bodies.
    source_dir: Option<PathBuf>,
    /// The company store, read per resolve for the runtime-authored graph
    /// bodies (the overlay half of the union).
    store: Arc<dyn CompanyStore>,
    /// The company whose record carries those bodies.
    company: CompanyId,
    /// The id of the top-level workflow the current run started from — a child
    /// whose closure reaches back to it would loop the whole run.
    root_id: String,
    /// The live per-run policy inputs used to gate child graphs (issue #617).
    /// `None` for a dry run, whose effect slots are all inert.
    gates: Option<ChildPolicyGates>,
}

/// The policy inputs the top-level runner selected for this run.
///
/// Grouped so [`StoreWorkflowResolver::new`] does not grow more parameters for
/// one concern. A dry run passes `None`, preserving its no-gate semantics.
pub struct ChildPolicyGates {
    /// Production sets this false while policy-generated HITL is disabled.
    pub policy_hitl_enabled: bool,
    /// The effective company policy, including any console override.
    pub policy: crate::company::Policy,
    /// The parent run resolving this child.
    pub run_id: String,
    /// The company's live standing permissions.
    pub grants: crate::runtime::grants::GrantSet,
    /// Per-run record of the gates the resolver marked, keyed by child id.
    ///
    /// The parent's parking path reads this when a child pauses, so the card
    /// can name the child's tool and reason the way a top-level gate's card
    /// does (issue #617).
    pub registry: Arc<ChildGateRegistry>,
}

/// The namespace that separates a child gate's id from the `sub_workflow` node
/// that ran it, as tinyflows builds it (`namespaced_gate` in
/// `vendor/openhuman/vendor/tinyflows/src/nodes/integration/sub_workflow.rs`).
///
/// A parent gate `approve` and a child's gate `approve` are different gates in
/// different id spaces, so tinyflows reports the child's as `<node>::<gate>`.
/// Both consumers of that shape live on this side of the seam: the resolver's
/// [`child_gate_call`] and the runner's unreplayable-call report.
pub(crate) const GATE_NAMESPACE: &str = "::";

/// What the resolver recorded about one resolved child: the gated graph as
/// tinyflows runs it, and the calls the policy raised on it.
#[derive(Clone)]
pub(crate) struct ChildGateRecord {
    /// The child graph the engine is running, post-gate-pass.
    pub graph: WorkflowGraph,
    /// The calls the policy stopped on it (the ones marked `requires_approval`).
    pub gated: Vec<crate::workflows::gate::GatedCall>,
}

/// A per-run record of every child graph the resolver gated, keyed by child id.
///
/// The resolver is invoked by the engine mid-run; the parent's parking path
/// runs after the engine returns. This registry is the one channel between the
/// two, so a child that pauses can be described from what the gate pass
/// actually classified instead of being re-read from the store (which may have
/// moved on, and the graph the engine ran is what the card must describe).
#[derive(Default)]
pub(crate) struct ChildGateRegistry {
    inner: std::sync::Mutex<HashMap<String, ChildGateRecord>>,
}

impl ChildGateRegistry {
    /// Records the gate pass for one resolved child.
    pub(crate) fn record(&self, child_id: &str, record: ChildGateRecord) {
        self.inner
            .lock()
            .unwrap()
            .insert(child_id.to_string(), record);
    }

    /// The record for `child_id`, cloned out so the caller need not hold the
    /// lock (the graphs are small and lookups happen at pause time, not per
    /// node).
    pub(crate) fn get(&self, child_id: &str) -> Option<ChildGateRecord> {
        self.inner.lock().unwrap().get(child_id).cloned()
    }
}

impl StoreWorkflowResolver {
    /// Builds a resolver serving children from `source_dir` ∪ `company`'s
    /// overlay bodies, for a run rooted at `root_id`.
    pub fn new(
        source_dir: Option<PathBuf>,
        store: Arc<dyn CompanyStore>,
        company: CompanyId,
        root_id: String,
        gates: Option<ChildPolicyGates>,
    ) -> Self {
        Self {
            source_dir,
            store,
            company,
            root_id,
            gates,
        }
    }

    /// Applies the same policy pass the top-level runner uses to a child graph.
    ///
    /// tinyflows now surfaces a child pause at the parent's `sub_workflow` node
    /// using a namespaced id and forwards that approval when the parent re-runs.
    /// Marking the child here therefore reaches the ordinary card and resume
    /// path instead of silently executing an effect beneath the parent graph.
    ///
    /// The gate pass runs against the run's **root** workflow id, not the
    /// child's own: `policy_gates` binds its standing-permission subject to the
    /// workflow it is gating, and the card the parent parks is minted with the
    /// root's id (issue #617). Binding the child to its own id would make a
    /// permission the operator granted the top-level workflow invisible to the
    /// child's checks, so a child call would park again under a grant that
    /// should have admitted it.
    ///
    /// The resulting gates — and the gated graph itself — are recorded per child
    /// id so the parent's parking path can name them after the run pauses (see
    /// [`ChildGateRegistry`]).
    async fn apply_policy_gates(&self, child_id: &str, graph: &mut WorkflowGraph) {
        let Some(gates) = self.gates.as_ref() else {
            return;
        };
        let gated = if gates.policy_hitl_enabled {
            crate::workflows::gate::apply_policy_gates_with_policy(
                graph,
                &gates.policy,
                &self.company,
                &self.root_id,
                &gates.run_id,
                &gates.grants,
            )
            .await
        } else {
            crate::workflows::gate::policy_hitl_disabled(graph)
        };
        gates.registry.record(
            child_id,
            ChildGateRecord {
                graph: graph.clone(),
                gated,
            },
        );
    }

    /// The **static** `workflow_id` references a graph makes — literal ids only.
    /// A dynamic `=expr` id is resolved by the engine at run time (and cannot be
    /// scanned statically), so it is skipped here and left to the engine's depth
    /// backstop.
    fn static_refs(file: &WorkflowFile) -> Vec<String> {
        file.nodes
            .iter()
            .filter(|n| n.kind == WorkflowNodeKind::SubWorkflow)
            .filter_map(|n| {
                n.config
                    .as_ref()
                    .and_then(|config| config.get("workflow_id"))
                    .and_then(|value| value.as_str())
                    .filter(|id| !id.starts_with('='))
                    .map(str::to_string)
            })
            .collect()
    }

    /// Rejects a cycle reachable by static references from `start_id`: if the
    /// requested id or the run's `root_id` appears in `start_id`'s transitive
    /// closure, the `sub_workflow` chain would loop. Bounded by a visited set and
    /// [`MAX_STATIC_RESOLVE_NODES`]; an unresolvable child is not a cycle (it
    /// fails loudly at its own resolve) so it is skipped here.
    fn guard_cycle(
        source_dir: Option<PathBuf>,
        overlays: Vec<OverlayWorkflow>,
        globals_disable: Vec<String>,
        root_id: String,
        start_id: String,
        start_file: WorkflowFile,
    ) -> TfResult<()> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        visited.insert(start_id.clone());
        let mut budget = 1usize;
        for referenced in Self::static_refs(&start_file) {
            if referenced == start_id || referenced == root_id {
                return Err(EngineError::Capability(format!(
                    "sub_workflow cycle detected: '{start_id}' references '{referenced}', which loops back into the running chain (root '{root_id}', resolving '{start_id}')"
                )));
            }
            queue.push_back(referenced);
        }

        while let Some(current) = queue.pop_front() {
            if !visited.insert(current.clone()) {
                continue;
            }
            budget += 1;
            if budget > MAX_STATIC_RESOLVE_NODES {
                return Err(EngineError::Capability(format!(
                    "sub_workflow chain from '{start_id}' spans more than {MAX_STATIC_RESOLVE_NODES} workflows; refusing to run"
                )));
            }
            // An unresolvable / invalid child is not itself a cycle — it will
            // fail loudly when the engine resolves it. Skip it in the scan.
            let Ok(Some(file)) = load_workflow_with_globals(
                source_dir.as_deref(),
                &overlays,
                &globals_disable,
                &current,
            ) else {
                continue;
            };
            for referenced in Self::static_refs(&file) {
                if referenced == start_id || referenced == root_id {
                    return Err(EngineError::Capability(format!(
                        "sub_workflow cycle detected: '{current}' references '{referenced}', which loops back into the running chain (root '{}', resolving '{start_id}')",
                        root_id
                    )));
                }
                queue.push_back(referenced);
            }
        }
        Ok(())
    }
}

#[async_trait]
impl WorkflowResolver for StoreWorkflowResolver {
    async fn resolve(&self, workflow_id: &str) -> TfResult<WorkflowGraph> {
        // (a) The id becomes a path segment — reject anything that could escape
        // the `workflows/` directory before it touches the filesystem.
        if !is_safe_workflow_id(workflow_id) {
            return Err(EngineError::Capability(format!(
                "sub_workflow id '{workflow_id}' is not a valid workflow id"
            )));
        }

        // (b) The company's runtime-authored bodies, read once and reused by the
        // load and the cycle scan below so both see the same snapshot.
        let overlays = self
            .store
            .load(&self.company)
            .await
            .map_err(|err| {
                EngineError::Capability(format!(
                    "sub_workflow '{workflow_id}': could not read saved workflows: {err}"
                ))
            })?
            .map(|record| (record.overlay_workflows, record.manifest.globals.disable))
            .unwrap_or_default();
        let (overlays, globals_disable) = overlays;

        // (c) Load the child from the seed ∪ overlay union, re-running full
        // OpenCompany parse + validation on it (the same rules a hand-authored
        // or console-created graph passes).
        let file = load_workflow_with_globals(
            self.source_dir.as_deref(),
            &overlays,
            &globals_disable,
            workflow_id,
        )
        .map_err(|err| EngineError::Capability(format!("sub_workflow '{workflow_id}': {err}")))?
        .ok_or_else(|| {
            EngineError::Capability(format!(
                "sub_workflow '{workflow_id}' is not a saved workflow on this company"
            ))
        })?;

        // (d) Static cycle guard over the same union, before the child is handed
        // back to the engine to compile + run.
        let source_dir = self.source_dir.clone();
        let root_id = self.root_id.clone();
        let start_id = workflow_id.to_string();
        let start_file = file.clone();
        tokio::task::spawn_blocking(move || {
            Self::guard_cycle(
                source_dir,
                overlays,
                globals_disable,
                root_id,
                start_id,
                start_file,
            )
        })
        .await
        .map_err(|err| {
            EngineError::Capability(format!(
                "sub_workflow '{workflow_id}' cycle scan failed: {err}"
            ))
        })??;

        // (e) Translate to a runnable tinyflows graph.
        let mut graph = crate::workflows::translate::translate(&file);
        // (f) A child is translated inside tinyflows, after the top-level
        // runner's gate pass. Apply that same pass before giving it back.
        self.apply_policy_gates(workflow_id, &mut graph).await;
        Ok(graph)
    }
}

/// The child workflow id a `sub_workflow` node resolves to — the key the
/// registry records the gate pass under.
///
/// A static `workflow_id` names it directly. A `=`-expression names it through
/// the engine, which resolves it against the node's run scope at run time; for
/// a `once` node — the only shape whose id this lookup can reconstruct — that
/// scope's `item` is the whole trigger input, so the same resolution is
/// repeated here. Best-effort: an expression touching a key a parked run no
/// longer carries (`run`, `nodes`), or a `per_item` fan-out whose per-element
/// scope needs the item index the paused id does not carry, yields `None` and
/// the caller falls back rather than failing the pause.
pub(crate) fn child_id_of(
    graph: &WorkflowGraph,
    node: &str,
    trigger_input: Option<&Value>,
) -> Option<String> {
    let node = graph.nodes.iter().find(|n| n.id == node)?;
    if !matches!(node.kind, NodeKind::SubWorkflow) {
        return None;
    }
    let config = node.config.get("workflow_id")?;
    let id = config.as_str()?;
    if !id.starts_with('=') {
        return Some(id.to_string());
    }
    // A `per_item` fan-out resolves `workflow_id` against each element's own
    // scope, and the paused id carries no item index to say which element that
    // was. Resolving against the whole-input scope could describe the wrong
    // child, so fall back rather than guess.
    if node.config.get("execution").and_then(Value::as_str) == Some("per_item") {
        return None;
    }
    let input = trigger_input?;
    tinyflows::expr::resolve(
        config,
        &serde_json::json!({ "item": input, "items": [input] }),
    )
    .as_str()
    .map(str::to_string)
}

/// Walks a namespaced pending id (`sub::work`, `sub::nested::work`) down through
/// the registry's per-child records, to the gate the deepest child paused on.
///
/// Each segment before the last names a `sub_workflow` node in the graph one
/// level up; [`child_id_of`] resolves the child it runs, and the registry entry
/// for that child carries the next graph down. The last segment names the gate
/// *inside* the deepest child. Returns that deepest record and the gate id, so
/// the caller can read the gate's own classification ([`ChildGateRecord::gated`])
/// or walk its upstream calls.
///
/// A segment that names no `sub_workflow` node, a child id the registry did not
/// record (its gate pass never ran — a child the policy let through), or an
/// expression-bound id this side cannot reconstruct yields `None`, and the
/// caller falls back to its own default rather than failing the pause.
pub(crate) fn descend(
    registry: &ChildGateRegistry,
    parent: &WorkflowGraph,
    node_id: &str,
    trigger_input: Option<&Value>,
) -> Option<(ChildGateRecord, String)> {
    let mut segments: Vec<&str> = node_id.split(GATE_NAMESPACE).collect();
    let gate = segments.pop()?;
    let mut record: Option<ChildGateRecord> = None;
    for node in segments {
        let graph = match &record {
            None => parent,
            Some(record) => &record.graph,
        };
        let child_id = child_id_of(graph, node, trigger_input)?;
        record = Some(registry.get(&child_id)?);
    }
    Some((record?, gate.to_string()))
}

/// The policy classification behind a namespaced child gate (`sub::work`,
/// `sub::nested::work`), if the resolver recorded it this run.
///
/// The parent's parking path resolves a namespaced id by descending the
/// registry's per-child records — resolving each intermediate `sub_workflow`
/// node's `workflow_id` (static, or a `=expr` best-effort against
/// `trigger_input`) to the child the engine actually ran — and reading the
/// deepest child's own classification. This is the only route by which the
/// child's policy gate (tool, reason, arguments) reaches the card: the parent
/// graph does not contain the child's nodes, and the child's partial output
/// does not travel up when it pauses (tinyflows drops it on
/// `ChildOutcome::Paused`).
pub(crate) fn child_gate_call(
    registry: &ChildGateRegistry,
    parent: &WorkflowGraph,
    node_id: &str,
    trigger_input: Option<&Value>,
) -> Option<crate::workflows::gate::GatedCall> {
    let (record, gate) = descend(registry, parent, node_id, trigger_input)?;
    record.gated.iter().find(|g| g.node_id == gate).cloned()
}

/// Whether `id` is a single safe on-disk filename stem — no path separators, no
/// `..`, not empty — so it cannot escape the `workflows/` directory. Mirrors the
/// `safe_wid` check the REST workflow routes use.
fn is_safe_workflow_id(id: &str) -> bool {
    use std::path::{Component, Path};
    let mut comps = Path::new(id).components();
    matches!(comps.next(), Some(Component::Normal(_))) && comps.next().is_none()
}

#[cfg(test)]
#[path = "resolver_gating_tests.rs"]
mod tests_gating;
#[cfg(test)]
#[path = "resolver_resolution_tests.rs"]
mod tests_resolution;
