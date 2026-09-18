//! Workflow surfaces: create a graph (`POST /workflows`), read the company's
//! saved graphs (`GET /workflows`, `GET /workflows/{wid}`), run one
//! (`POST /workflows/{wid}/run`), read back what past runs did
//! (`GET /workflows/runs`), and preview what a trigger's cron means
//! (`POST /workflows/cron/preview`) — under both scope forms.
//!
//! ## Cron preview (issue #262)
//!
//! `POST /workflows/cron/preview` answers what a 5-field expression means and
//! when it next fires. A schedule's dangerous failure is the one that
//! *validates*: `0 9 * * *` and `9 0 * * *` are both valid and nine hours
//! apart, and the dialect is always UTC, so nothing in the authoring flow
//! contradicts an author who meant something else. Reading the parsed schedule
//! back is the only defence, since neither expression is invalid.
//!
//! It is the only route here that answers **200 on bad input**, carrying the
//! parser's message in the body — the console calls it while the author is
//! still typing, so a half-written expression is the normal state rather than
//! an error. See [`preview_cron`] for the full reasoning; `POST /workflows`
//! still refuses to save a graph whose schedule does not parse.
//!
//! ## Run history (issue #228)
//!
//! A run's outcome used to exist only in the moment: a manual run's
//! [`DeliveryReport`](crate::ports::DeliveryReport) rows lived in the console
//! drawer until it was dismissed, and a scheduled run's reached only host
//! stdout. The run route now journals every finished run through
//! [`record_run_finished`](crate::runtime::record_run_finished) — the same helper the cron
//! [`WorkflowScheduler`](crate::runtime::WorkflowScheduler) calls, so history is
//! uniform whatever started the run — and `GET /workflows/runs` folds those
//! events back out, newest first.
//!
//! A company's graphs come from two places, unioned by
//! [`load_workflow_union`](crate::company::load_workflow_union) /
//! [`list_workflows_union`](crate::company::list_workflows_union):
//!
//! * the **seed** files committed to the company source directory
//!   (`companies/<name>/workflows/<wid>.toml`), and
//! * the **runtime-authored** graph bodies persisted on the [`CompanyRecord`]
//!   overlay.
//!
//! The seed wins on an id collision. A hosted tenant has no source directory at
//! all (or a read-only one), so every graph it owns is an overlay body — which
//! is why these routes never require a source directory to answer.
//!
//! ## Pre-flight validation (issue #1074)
//!
//! `POST /workflows/validate` answers whether Create would accept a draft, and
//! persists nothing. It exists because two of Create's rules — node reachability
//! and the condition branch-label rule — cannot be pre-empted by a client
//! without re-implementing them, and a client-side copy of a host rule drifts.
//! It runs [`courtesy_validate_draft`](crate::company::courtesy_validate_draft),
//! the same sequence Create runs before its save, so the two cannot disagree.
//! Unlike the cron preview above it answers `400` on a bad draft — the identical
//! body Create would answer with, `problems` array and all.
//!
//! Creation (issues #69, #112, #168) delegates to
//! [`create_company_workflow`](crate::company::create_company_workflow), the
//! single validated-persist core the orchestrator's `create_workflow` tool also
//! runs, so the two surfaces cannot drift. It persists the graph body **and**
//! the enabled id on the operator's live record in one save; the
//! version-controlled `company.toml` and the source tree are never written
//! (see `crate::server::ops::team`) — the source tree is read-only in hosted
//! mode, which is what issue #168 reports.
//!
//! `list_workflows` additionally unions in the manifest's `[workflows].enabled`
//! ids that have no body in either source, falling back to the id as the display
//! name — the same fallback the GraphQL `Company.workflows` resolver uses — so a
//! provisioned tenant's picker isn't empty.
//!
//! Execution is dependency-inverted behind the [`WorkflowRunner`] port. When no
//! runner is wired the run route classifies *why* (see
//! [`runner_gap_for`](crate::server::ops::inference::runner_gap_for)) and answers
//! with one of three responses, because the operator's next step differs: the
//! default build or a runtime built without a harness reports `not_wired` (the
//! same 404 seam the DNS/SMTP surfaces use) so it stays inert; a company holding
//! a saved config a restart would pick up reports `restart_required` (409, issue
//! #266); and a company that never configured inference reports
//! `inference_required` (409, issue #514) so the console points the operator at
//! Settings instead of degrading to read-only. The read routes need no runner:
//! they only parse the saved graphs, so the console can list and render
//! workflows even on a build that cannot execute them.

use std::collections::HashSet;
use std::path::Path as FsPath;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;
use crate::company::{
    RawEdge, RawNode, RawWorkflow, WorkflowDestinationDef, WorkflowEdgeDef, WorkflowFile,
    WorkflowJudgeDef, WorkflowNodeDef, WorkflowPostconditionDef, WorkflowRetryDef,
    courtesy_validate_draft, create_company_workflow, delete_company_workflow,
    list_workflows_with_globals, load_workflow_with_globals, rollback_company_workflow,
    seed_file_exists, set_company_workflow_enabled, update_company_workflow, workflow_version,
};
use crate::error::OpenCompanyError;
use crate::ports::types::{
    CompanyEvent, CompanyRecord, EventSeq, OverlayWorkflow, StoredEvent, WorkflowNodeStatus,
};
use crate::ports::workflow_verdict::{RunVerdictFacts, WorkflowRunVerdict};
use crate::runtime::cron::{CivilTime, CronExpr};
use crate::server::error::ApiError;
use crate::server::ops::{ScopedCompany, scoped};

/// Builds the workflow route fragment: create + list, one graph read, and the
/// run write.
pub fn router() -> Router<AppState> {
    scoped("/workflows", post(create_workflow).get(list_workflows))
        // The static `/workflows/runs` GET is registered BEFORE the dynamic
        // `/workflows/{wid}`, mirroring `GET /tasks/inflight` in
        // [`super::tasks`]. Axum prefers a static segment over a parameter, so
        // the run-history read wins even though `runs` is a syntactically valid
        // `wid`; `run_history_is_not_shadowed_by_the_graph_read` pins it,
        // because a regression here would silently 404 the history panel.
        //
        // The cost is that a workflow whose id is literally `runs` cannot be
        // read through this route. That is the same trade `tasks/inflight`
        // takes, and the history surface is worth more than the one reserved id.
        .merge(scoped("/workflows/runs", get(list_runs)))
        // Same static-before-dynamic ordering as `/workflows/runs` above, for
        // the same reason: `cron` is a syntactically valid `wid`.
        .merge(scoped("/workflows/cron/preview", post(preview_cron)))
        // Issue #1074: the author-time verdict, without the save. Runs the SAME
        // validation `POST …/workflows` runs and persists nothing, so the create
        // dialog can pre-empt a refusal instead of discovering it on submit. A
        // static prefix registered here with the others and BEFORE the dynamic
        // `/workflows/{wid}`: `validate` is a syntactically valid `wid`.
        .merge(scoped("/workflows/validate", post(validate_workflow)))
        // Issue #753: the create-time copilot. Drafts a graph from a free-text
        // description and hands it back for the New-workflow dialog to hydrate —
        // it never persists (Create still does). A static prefix registered here
        // with the others and BEFORE the dynamic `/workflows/{wid}`, for the
        // reason the comment above gives: `draft-from-description` is a
        // syntactically valid `wid`.
        .merge(scoped(
            "/workflows/draft-from-description",
            post(draft_from_description),
        ))
        // Issue #783: the `tool_call` slugs this company can reach from a
        // workflow, so the per-workflow copilot can ground a proposal on real
        // tools instead of guessing (`github_integration` and the like). Reads
        // the SAME `workflow_effective_tool_slugs` the create-time copilot
        // grounds on (issues #753, #874), so the two cannot drift — and, since
        // #874, so neither offers a tool this deployment has not wired. A static
        // prefix registered here with the others and BEFORE the dynamic
        // `/workflows/{wid}` below — `tool-slugs` is a syntactically valid `wid`.
        .merge(scoped("/workflows/tool-slugs", get(workflow_tool_slugs)))
        // Issue #813: the chat channels actually wired for this running company,
        // so the output-node destination editor can offer a picker of real
        // targets instead of a free-text box that only fails at delivery time
        // with `ChannelNotWired`. A static prefix registered here BEFORE the
        // dynamic `/workflows/{wid}` — `wired-channels` is a valid `wid`.
        .merge(scoped(
            "/workflows/wired-channels",
            get(workflow_wired_channels),
        ))
        // Issue #383: stop a run that is still walking its graph. Registered
        // here, with the other static `/workflows/...` prefixes and BEFORE the
        // dynamic `/workflows/{wid}` below, for the reason the comment above
        // gives — `runs` is a syntactically valid `wid`. This particular path is
        // four segments deep so it could not actually collide with the two- and
        // three-segment dynamic routes, but keeping the registration order
        // uniform is what stops the next four-segment static route from being
        // the one that silently loses.
        .merge(scoped(
            "/workflows/runs/{rid}/cancel",
            post(cancel_workflow_run),
        ))
        // Issue #596: read one past run's per-node output for the run inspector.
        // Same static-before-dynamic family as the cancel route above and, like
        // it, four segments deep so it cannot collide with the two/three-segment
        // dynamic routes — but registered here to keep the ordering uniform.
        // Deliberately a lazy per-run fetch, NOT folded into `list_runs`: that
        // fold is already expensive, and an inspector only ever opens one run at
        // a time (the make.com pattern).
        .merge(scoped("/workflows/runs/{rid}/output", get(get_run_output)))
        // Issue #1684: read the files ONE past run produced, for the run
        // inspector's "Files associated" section. Same static-before-dynamic
        // family as the output route above and, like it, four segments deep so
        // it cannot collide with the two/three-segment dynamic routes — but
        // registered here to keep the ordering uniform. Deliberately a lazy
        // per-run fetch, NOT folded into `list_runs`: that fold is already
        // expensive, and an inspector only ever opens one run at a time. The
        // join it runs — `run_id → cards where origin_run_id == run_id → each
        // card's artifacts` — is the authoritative provenance path (see
        // `run_artifacts`).
        .merge(scoped(
            "/workflows/runs/{rid}/artifacts",
            get(run_artifacts),
        ))
        // Issue #259: read, replace, remove — all on the same id. `PUT` is a
        // full replace rather than a `PATCH` merge because a workflow *is* its
        // graph: a partial node/edge merge has no well-defined meaning (which
        // half of a rewired edge set wins?), and the console always holds the
        // whole graph anyway.
        //
        // Registered LAST of the `/workflows/...` reads, after both static
        // segments above: this is the dynamic route they have to outrank.
        .merge(scoped(
            "/workflows/{wid}",
            get(get_workflow)
                .put(update_workflow)
                .delete(delete_workflow),
        ))
        .merge(scoped("/workflows/{wid}/run", post(run_workflow)))
        // Issue #840 (PR-3): correct a saved workflow whose run failed, with the
        // create-time copilot. Drafts a corrected graph from the failing graph +
        // the run's journaled failure and hands it back for the edit dialog to
        // hydrate — it never persists (Save still does). A sub-resource of
        // `{wid}`, strictly more specific than the dynamic `{wid}` reads above, so
        // registration order cannot make it lose.
        .merge(scoped("/workflows/{wid}/fix-from-run", post(fix_from_run)))
        // Issue #276: the pause switch. A sub-resource `PUT` rather than a field
        // on the graph `PUT` above, because the two are different decisions with
        // different permissions to grow into and different bodies: replacing a
        // graph requires holding the whole graph and a version token, and an
        // operator who only wants to stop a schedule should not have to send
        // one — nor risk a 409 from a stale editor tab while doing it.
        .merge(scoped(
            "/workflows/{wid}/enabled",
            put(set_workflow_enabled),
        ))
        // Issue #274: the edit history of one workflow, and the restore of one of
        // its snapshots. Both hang off `{wid}` after a further static segment
        // (`revisions`), so they cannot collide with the dynamic `{wid}` reads
        // above — they are strictly more specific — and they carry zero overlap
        // with the `…/runs*` family, which lives on a different static prefix.
        .merge(scoped(
            "/workflows/{wid}/revisions",
            get(list_workflow_revisions),
        ))
        .merge(scoped(
            "/workflows/{wid}/revisions/{rev}/restore",
            post(restore_workflow_revision),
        ))
}

/// A one-line workflow entry as the console's picker renders it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowSummary {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// The trigger node's 5-field UTC cron, or explicit `null` for a manual
    /// workflow. Kept present when `None` so a current host can distinguish
    /// "manual" from an older host that predates this field.
    schedule: Option<String>,
    /// Number of nodes in the graph. A body-less manifest entry reports zero.
    node_count: usize,
    /// Whether `PUT`/`DELETE` on this id will be accepted (issue #259) — see
    /// [`is_editable`]. The console disables its Edit/Delete affordances on a
    /// `false`, so an operator is told *before* clicking rather than by a 409
    /// after.
    editable: bool,
    /// Whether this workflow's schedule is armed (issue #276). `false` means the
    /// graph is saved and still runnable by hand, but
    /// [`WorkflowScheduler::tick`](crate::runtime::WorkflowScheduler) skips it.
    ///
    /// Always serialized, including `true`, so the console can render the toggle
    /// from the list read alone. Unlike `editable` this is a property of the
    /// *company record*, not of where the graph lives — a seed-defined workflow
    /// is `editable: false` and still toggleable.
    enabled: bool,
}

impl WorkflowSummary {
    fn new(f: WorkflowFile, editable: bool, enabled: bool) -> Self {
        let schedule = f.trigger_schedule().map(str::to_owned);
        let node_count = f.nodes.len();
        Self {
            id: f.id,
            name: f.name,
            description: f.description,
            schedule,
            node_count,
            editable,
            enabled,
        }
    }
}

/// The full graph the canvas renders — nodes and directed edges, camelCase.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowGraph {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// The owning desk (issue #1862 prerequisite) — see
    /// [`WorkflowFile::owner_desk`]. Round-tripped verbatim, for the same
    /// reason `repeatable` on [`WorkflowNode`] is: a `PUT` replaces the whole
    /// graph, so an editor that builds its save from what this route just
    /// returned has to have the field in order to carry it forward. Omitting
    /// it here was the actual bug (issue #1882 review) — `ownerDesk` was
    /// accepted on the create/update body but never appeared on a read, so
    /// the very next edit that round-tripped a read into a write silently
    /// cleared whatever desk an operator had set.
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_desk: Option<String>,
    nodes: Vec<WorkflowNode>,
    edges: Vec<WorkflowEdge>,
    /// Whether this graph can be replaced or removed through the API — see
    /// [`is_editable`].
    editable: bool,
    /// The opaque optimistic-concurrency token for this graph (issue #259).
    /// Always serialized: a string when the graph is `editable`, and explicit
    /// `null` when it is not (a source-defined or body-less graph has nothing to
    /// version). It is deliberately NOT omitted — a client that read `version`
    /// off a graph whose key was absent got `undefined` and sent nothing,
    /// silently overwriting a concurrent save (issue #1013). An explicit `null`
    /// says "no token here" instead of hiding the field.
    ///
    /// The contract is **echo it back**: hand it to `PUT` in the body or to
    /// `DELETE` as `?expectedVersion=`, and the write is refused with a `409` if
    /// the graph moved in between — and refused with a `400` if you omit it
    /// entirely (issue #1013). Never parse or derive it.
    version: Option<String>,
    /// Whether this workflow's schedule is armed (issue #276) — see
    /// [`WorkflowSummary::enabled`]. Carried on the graph read as well as the
    /// list so the editor can show the state it is about to change, and so a
    /// `PUT` response reports the disarm the edit may have just triggered
    /// without a follow-up read.
    enabled: bool,
}

impl WorkflowGraph {
    fn new(f: WorkflowFile, editable: bool, version: Option<String>, enabled: bool) -> Self {
        Self {
            id: f.id,
            name: f.name,
            description: f.description,
            owner_desk: f.owner_desk,
            nodes: f.nodes.into_iter().map(WorkflowNode::from).collect(),
            edges: f.edges.into_iter().map(WorkflowEdge::from).collect(),
            editable,
            version,
            enabled,
        }
    }
}

/// A single graph node. `kind` is the on-disk string
/// (`trigger`/`agent`/`tool_call`/`http_request`/`condition`/`output`); `agent`
/// is only meaningful on `agent` nodes; `schedule` (a 5-field UTC cron) only on
/// `trigger` nodes. The P1 fields (`config` / `onError` / `retry` /
/// `requiresApproval`) are serialized so `GET …/workflows/{wid}` does not drop
/// model data (they are omitted entirely when unset).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowNode {
    id: String,
    kind: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schedule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    on_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry: Option<WorkflowRetryOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requires_approval: Option<bool>,
    /// When `false`, a continuation must not repeat this node's call — see
    /// [`WorkflowNodeDef::repeatable`] (issue #850). Round-tripped verbatim so
    /// the console's edit form does not lose the declaration on an unrelated
    /// save: the create/update body reads this same field back, so omitting it
    /// here would have `repeatable: false` silently disappear on the first
    /// re-submit.
    #[serde(skip_serializing_if = "Option::is_none")]
    repeatable: Option<bool>,
    /// Where an `output` node's report is routed once the run finishes (issue
    /// #170). The model shape is reused verbatim in both directions: its two
    /// fields (`kind` / `target`) are single words, so there is no snake_case →
    /// camelCase gap to bridge and no second shape to drift from.
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<WorkflowDestinationDef>,
    /// A node's declared deterministic postcondition (issue #1866), reused
    /// verbatim like `destination` above. Round-tripped so a `GET` → edit →
    /// `PUT` cycle in the console does not silently clear an existing gate —
    /// see [`WorkflowPostconditionDef`].
    #[serde(skip_serializing_if = "Option::is_none")]
    postcondition: Option<WorkflowPostconditionDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verify: Option<WorkflowJudgeDef>,
}

/// The camelCase retry policy shape the console reads back (`maxAttempts` /
/// `backoffMs` / `backoff`). Distinct from the snake_case
/// [`WorkflowRetryDef`] the model/TOML use.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRetryOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backoff_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backoff: Option<String>,
}

impl From<WorkflowRetryDef> for WorkflowRetryOut {
    fn from(r: WorkflowRetryDef) -> Self {
        Self {
            max_attempts: r.max_attempts,
            backoff_ms: r.backoff_ms,
            backoff: r.backoff,
        }
    }
}

impl From<WorkflowNodeDef> for WorkflowNode {
    fn from(n: WorkflowNodeDef) -> Self {
        Self {
            id: n.id,
            kind: n.kind.as_str().to_string(),
            name: n.name,
            summary: n.summary,
            agent: n.agent,
            schedule: n.schedule,
            config: n.config,
            on_error: n.on_error,
            retry: n.retry.map(WorkflowRetryOut::from),
            requires_approval: n.requires_approval,
            repeatable: n.repeatable,
            destination: n.destination,
            postcondition: n.postcondition,
            verify: n.verify,
        }
    }
}

/// A directed edge between two node ids, with an optional branch label.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowEdge {
    from: String,
    to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

impl From<WorkflowEdgeDef> for WorkflowEdge {
    fn from(e: WorkflowEdgeDef) -> Self {
        Self {
            from: e.from,
            to: e.to,
            label: e.label,
        }
    }
}

/// The company's runtime-authored graph bodies, read once per request from the
/// live record. A company with no persisted record contributes none — the read
/// routes stay tolerant (they still serve the seed files), and the create route
/// re-loads the record under its write lock and 404s properly there.
/// Whether `wid` can be replaced or removed through `PUT`/`DELETE` (issue #259):
/// it is backed by a **record overlay body** and is **not shadowed by a seed
/// file**.
///
/// Both halves are the reader's rules, restated. `load_workflow_union` gives a
/// seed file precedence on an id collision, so an edit to a seed-shadowed id
/// would persist a graph nothing serves; and `merge_enabled_workflows` (#208)
/// re-derives `[workflows].enabled` from seed ids at boot, so a "delete" of a
/// seed-backed workflow would undo itself on restart. The write core enforces
/// exactly this and answers `409` — this flag is the same predicate, projected
/// so the console can grey the button instead of surfacing that 409.
///
/// The seed probe is [`seed_file_exists`], shared with the create path's
/// id-uniqueness check, so the flag and the host's actual answer cannot drift.
fn is_editable(source_dir: Option<&FsPath>, overlays: &[OverlayWorkflow], wid: &str) -> bool {
    overlays.iter().any(|w| w.id == wid) && !seed_file_exists(source_dir, wid)
}

/// The stored overlay TOML for `wid`, when the record has one.
fn overlay_toml<'a>(overlays: &'a [OverlayWorkflow], wid: &str) -> Option<&'a str> {
    overlays
        .iter()
        .find(|w| w.id == wid)
        .map(|w| w.toml.as_str())
}

/// This company's overlay graph bodies and its `[globals].disable`, read
/// together — the pair every union read needs.
async fn overlay_workflows_and_globals(
    company: &ScopedCompany,
) -> Result<(Vec<OverlayWorkflow>, Vec<String>), ApiError> {
    let (overlays, _, globals_disable) = workflow_state(company).await?;
    Ok((overlays, globals_disable))
}

/// The company's runtime-authored graph bodies **and** the ids the operator has
/// switched off (issue #276), from one record read.
///
/// One read rather than two because the two answers have to agree: a route that
/// loaded the bodies and the switches separately could list a workflow from one
/// record and its armed state from another, and the window is exactly the moment
/// an operator has just toggled it.
async fn workflow_state(
    company: &ScopedCompany,
) -> Result<(Vec<OverlayWorkflow>, Vec<String>, Vec<String>), ApiError> {
    let record: Option<CompanyRecord> = company
        .runtime
        .store()
        .load(company.id())
        .await
        .map_err(ApiError)?;
    // The company's `[globals].disable` rides along for the same reason the
    // other two do: a route that resolved a global graph without it would serve
    // one this company opted out of.
    Ok(record
        .map(|r| {
            (
                r.overlay_workflows,
                r.disabled_workflows,
                r.manifest.globals.disable,
            )
        })
        .unwrap_or_default())
}

/// `GET …/workflows` — the company's saved workflows as picker summaries.
///
/// Summaries come from the union of the company's two graph sources — the seed
/// `workflows/*.toml` files and the record's runtime-authored bodies — so a
/// hosted tenant with no source directory still lists everything it created.
/// The manifest's `[workflows].enabled` ids are then unioned in (deduped by id),
/// falling back to the id as the name for an id with no body in either source —
/// the same fallback the GraphQL resolver uses. Only when all three are empty
/// does this return `200 []`, so the console renders "no workflows yet" rather
/// than a failure.
async fn list_workflows(company: ScopedCompany) -> Result<Json<Vec<WorkflowSummary>>, ApiError> {
    let (overlays, disabled, globals_disable) = workflow_state(&company).await?;
    let source_dir = company.runtime.source_dir();
    let files = list_workflows_with_globals(source_dir, &overlays, &globals_disable);
    let mut seen: HashSet<String> = files.iter().map(|f| f.id.clone()).collect();
    let mut summaries: Vec<WorkflowSummary> = files
        .into_iter()
        .map(|f| {
            let editable = is_editable(source_dir, &overlays, &f.id);
            let enabled = !disabled.iter().any(|id| id == &f.id);
            WorkflowSummary::new(f, editable, enabled)
        })
        .collect();

    let enabled_ids = company
        .runtime
        .enabled_workflow_ids()
        .await
        .map_err(ApiError)?;
    for id in enabled_ids {
        // Already summarized from a real body — skip so an id that is both
        // enabled and saved doesn't double-list.
        if !seen.insert(id.clone()) {
            continue;
        }
        summaries.push(WorkflowSummary {
            id: id.clone(),
            name: id,
            description: None,
            schedule: None,
            node_count: 0,
            // A manifest-`enabled` id with no body in either source: there is
            // nothing to replace or remove, and the write core says so with a
            // 409. Never editable.
            editable: false,
            // Nor toggleable, for the same reason — there is no graph, so no
            // schedule to switch off. Reported as `true` because that is what it
            // is: nothing is holding it back, there is simply nothing there.
            enabled: true,
        });
    }

    Ok(Json(summaries))
}

/// `GET …/workflows/{wid}` — the full graph for one workflow.
///
/// Reads through the seed ∪ overlay union, so a graph created on a hosted
/// tenant (no source directory) resolves here too. An unknown `wid` is a `404`,
/// mirroring the sub-resource-not-found shape the task routes use.
async fn get_workflow(
    company: ScopedCompany,
    Path(WorkflowPath { wid }): Path<WorkflowPath>,
) -> Result<Json<WorkflowGraph>, ApiError> {
    // `wid` may become a filename on the seed side — reject anything that could
    // escape `workflows/`.
    if !safe_wid(&wid) {
        return Err(ApiError(OpenCompanyError::CompanyNotFound(format!(
            "workflow {wid}"
        ))));
    }
    let source_dir = company.runtime.source_dir();
    let (overlays, disabled, globals_disable) = workflow_state(&company).await?;
    let file = load_workflow_with_globals(source_dir, &overlays, &globals_disable, &wid)
        .map_err(ApiError)?
        .ok_or_else(|| ApiError(OpenCompanyError::NotFound(format!("workflow {wid}"))))?;
    // Issue #259: the version token rides out with the graph, so the console
    // gets it for free on the same read it renders from — there is no second
    // round trip for a caller to skip and thereby lose the concurrency guard.
    let editable = is_editable(source_dir, &overlays, &wid);
    let version = editable
        .then(|| overlay_toml(&overlays, &wid).map(workflow_version))
        .flatten();
    let enabled = !disabled.iter().any(|id| id == &wid);
    Ok(Json(WorkflowGraph::new(file, editable, version, enabled)))
}

/// The create-workflow body — the same camelCase graph shape the GET routes
/// return (`id`/`name`/`description?`/`ownerDesk?`/`nodes`/`edges`), so the
/// console's creator can post exactly what it would otherwise read back.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorkflowBody {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// The owning desk (issue #1862 prerequisite), so the console's creator
    /// can post it exactly like every other field — see
    /// [`WorkflowFile::owner_desk`] and [`WorkflowGraph::owner_desk`] (the
    /// symmetric read-side field an edit round-trips it through).
    #[serde(default)]
    owner_desk: Option<String>,
    #[serde(default)]
    nodes: Vec<CreateNode>,
    #[serde(default)]
    edges: Vec<CreateEdge>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateNode {
    id: String,
    kind: String,
    name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    /// A 5-field UTC cron saying when the workflow starts on its own — only
    /// valid on a `trigger` node (issue #169). Validated by the render → parse
    /// round trip below, so a bad expression or a schedule on the wrong node
    /// kind is a `400`, not a persisted graph that never fires.
    #[serde(default)]
    schedule: Option<String>,
    /// Free-form, kind-specific node config (P2): `switch`/`transform`
    /// `=expr` bindings, a `sub_workflow` `workflow_id`, a `tool_call` slug/args,
    /// … . Carried as JSON on the wire and converted to a `toml::Value` on the
    /// way to disk; a JSON `null` anywhere inside is rejected as a 4xx (TOML has
    /// no null to represent it).
    #[serde(default)]
    config: Option<serde_json::Value>,
    /// Per-node error policy: `stop` (default) / `continue` / `route`.
    #[serde(default)]
    on_error: Option<String>,
    /// Per-node retry policy the engine honors.
    #[serde(default)]
    retry: Option<CreateRetry>,
    /// When `true`, the node pauses awaiting operator approval before it runs.
    #[serde(default)]
    requires_approval: Option<bool>,
    /// When `false`, a continuation must not repeat this node's call — it
    /// replays the result the earlier run recorded (issue #850). Only valid on
    /// `tool_call` and `http_request`, the two kinds that make a call.
    #[serde(default)]
    repeatable: Option<bool>,
    /// Where an `output` node's report goes once the run finishes:
    /// `{"kind": "owner"|"email"|"channel", "target"?: "…"}`. Rejected on any
    /// other node kind, and each kind's target contract is enforced by
    /// `parse_workflow` before anything is persisted.
    #[serde(default)]
    destination: Option<WorkflowDestinationDef>,
    /// A node's declared deterministic postcondition (issue #1866): `{require:
    /// "non_empty"|"field_present"|"non_empty_list", field?: "…"}`, checked
    /// against the node's output before it is allowed to flow downstream.
    /// Carried through create/validate/update so a caller-declared gate is not
    /// silently discarded — see [`WorkflowPostconditionDef`].
    #[serde(default)]
    postcondition: Option<WorkflowPostconditionDef>,
    /// Optional semantic sufficiency policy for an agent node.
    #[serde(default)]
    verify: Option<WorkflowJudgeDef>,
}

/// The camelCase retry policy the create body carries (`maxAttempts` /
/// `backoffMs` / `backoff`), mapped to the snake_case [`WorkflowRetryDef`] the
/// model + TOML use — the inverse of the [`WorkflowRetryOut`] read shape.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRetry {
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    backoff_ms: Option<u64>,
    #[serde(default)]
    backoff: Option<String>,
}

impl From<CreateRetry> for WorkflowRetryDef {
    fn from(r: CreateRetry) -> Self {
        Self {
            max_attempts: r.max_attempts,
            backoff_ms: r.backoff_ms,
            backoff: r.backoff,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateEdge {
    from: String,
    to: String,
    #[serde(default)]
    label: Option<String>,
}

impl TryFrom<CreateWorkflowBody> for RawWorkflow {
    type Error = ApiError;

    fn try_from(body: CreateWorkflowBody) -> Result<Self, ApiError> {
        let mut nodes = Vec::with_capacity(body.nodes.len());
        for n in body.nodes {
            // JSON config → TOML value. TOML has no `null`, so a `null` anywhere
            // in the config is a caller error (400), not a 500 on write.
            let config = match n.config {
                Some(json) => Some(toml::Value::try_from(json).map_err(|err| {
                    ApiError(OpenCompanyError::InvalidRequest(format!(
                        "node `{}` has config that can't be stored ({err}) — TOML has no null; drop null-valued keys.",
                        n.id
                    )))
                })?),
                None => None,
            };
            nodes.push(RawNode {
                id: n.id,
                kind: n.kind,
                name: n.name,
                summary: n.summary,
                agent: n.agent,
                schedule: n.schedule,
                config,
                on_error: n.on_error,
                retry: n.retry.map(WorkflowRetryDef::from),
                requires_approval: n.requires_approval,
                repeatable: n.repeatable,
                destination: n.destination,
                postcondition: n.postcondition,
                verify: n.verify,
            });
        }
        Ok(Self {
            id: body.id,
            name: body.name,
            description: body.description,
            // Blank/whitespace is treated as absent, not as a real desk name
            // — the same rule `validate_draft_against_record` already applies
            // at author time, applied here too so the persisted TOML never
            // stores a blank string in the first place.
            owner_desk: RawWorkflow::normalize_owner_desk(body.owner_desk),
            nodes,
            edges: body
                .edges
                .into_iter()
                .map(|e| RawEdge {
                    from: e.from,
                    to: e.to,
                    label: e.label,
                })
                .collect(),
        })
    }
}

/// `POST …/workflows` — authors a new workflow graph (issues #69, #112, #168):
/// the console's form creator, or any direct API caller, posts the graph shape
/// and it is persisted on the company record.
///
/// The whole validated-persist sequence lives in
/// [`create_company_workflow`] — the same core the orchestrator's
/// `create_workflow` tool runs — so the two surfaces cannot drift: safe id,
/// size caps, exactly one trigger, every `agent` node on the roster, unique id
/// (409) and unique display name (409), full [`parse_workflow`] structural
/// validation, one atomic record save, and an audit event.
///
/// No source directory is required: the body lands on the record, so a hosted
/// tenant whose source tree is a read-only mount can create workflows (issue
/// #168 — this used to fail with `EROFS`).
async fn create_workflow(
    company: ScopedCompany,
    Json(body): Json<CreateWorkflowBody>,
) -> Result<Json<WorkflowGraph>, ApiError> {
    let draft = RawWorkflow::try_from(body)?;
    let file = create_company_workflow(
        company.id(),
        company.runtime.source_dir(),
        company.runtime.store(),
        Some(company.runtime.events()),
        draft,
        Some(&company.runtime.deliverable_channel_ids()),
        // Issue #1843: the signed-in human behind this write, when there is
        // one — the REST create path is real user activation, unlike the
        // orchestrator's own `create_workflow` tool.
        company.actor.clone(),
    )
    .await
    .map_err(ApiError)?;
    // A freshly created graph is always an overlay body and, since create
    // refuses a seed-colliding id, never seed-shadowed — so it is editable, and
    // its version token goes back on the create response. That lets the console
    // hold a valid token from the moment it creates a workflow, without a
    // follow-up GET.
    Ok(Json(graph_with_version(&company, file).await?))
}

/// The `POST …/workflows/validate` response for a draft the host accepts.
///
/// A bare `{"valid": true}` rather than an echo of the graph. The caller already
/// holds the draft — it just sent it — and echoing a *normalized* copy back
/// would invite a console to adopt it, which is a persist-shaped side effect on
/// a route whose whole contract is that nothing happens.
#[derive(Debug, Serialize)]
struct ValidateWorkflowResponse {
    valid: bool,
}

/// `POST …/workflows/validate` — the host's author-time verdict on a draft
/// graph, with nothing persisted (issue #1074).
///
/// Two of the rules `POST …/workflows` enforces cannot be pre-empted by a client
/// without re-implementing them: **node reachability** (every node must be
/// reachable from the trigger — `crate::company::parse_workflow`) and the
/// **condition branch-label** rule (an edge leaving a `condition` node must read
/// `yes`/`no`, or `error` when that node is also `on_error = "route"`). A console
/// that mirrored either would own a second copy of a rule it does not define,
/// and the copy would drift — the failure mode issue #168 already cost this
/// codebase once. So the console asks the host instead.
///
/// The verdict comes from [`courtesy_validate_draft`], the same
/// shape → render → byte-cap → [`parse_workflow`] → roster/tool/label sequence
/// `create_company_workflow` runs before its save. **This route reimplements no
/// rule**, which is the point: the two surfaces cannot disagree because there is
/// only one of them. A refusal is returned as the error itself, so the body is
/// byte-for-byte the `400` Create would have answered with — including the
/// structured `problems` array from issue #1016 — and a console can render one
/// code path for both.
///
/// # What it deliberately does not answer
///
/// **Id and display-name uniqueness.** `courtesy_validate_draft` omits it on
/// purpose: uniqueness is a function of the live record at *save* time, and
/// `create_company_workflow` decides it under the per-company write lock. An
/// unlocked pre-flight could only answer for an instant, and answering "the id
/// is free" a moment before it is taken is worse than not answering — so a `200`
/// here still permits the `409` there. The console already holds the workflow
/// list and can pre-empt that one itself.
///
/// Statuses: `200` (the draft would be accepted), `400` (it would not).
async fn validate_workflow(
    company: ScopedCompany,
    Json(body): Json<CreateWorkflowBody>,
) -> Result<Json<ValidateWorkflowResponse>, ApiError> {
    let draft = RawWorkflow::try_from(body)?;
    let record = company
        .runtime
        .store()
        .load(company.id())
        .await
        .map_err(ApiError)?
        .ok_or_else(|| ApiError(OpenCompanyError::CompanyNotFound(company.id().to_string())))?;
    // The SAME source directory create passes (`create_company_workflow` at the
    // top of this module). It feeds only the `sub_workflow` existence probe, and
    // withholding it here made this refuse a child workflow that lives as a seed
    // file — which create accepts. Review of #1074.
    courtesy_validate_draft(
        &draft,
        &record,
        company.runtime.source_dir(),
        Some(&company.runtime.deliverable_channel_ids()),
        // This route pre-flights a body, not a specific saved workflow: it is
        // reached for a create and for an edit alike and has no stored owner to
        // grandfather against (issue #1882 review). It keeps the documented
        // false-negative direction described above `validate_workflow`.
        None,
    )
    .map_err(ApiError)?;
    Ok(Json(ValidateWorkflowResponse { valid: true }))
}

/// Re-reads the just-written overlay body to attach the current `editable` flag
/// and version token to a write response.
///
/// The re-read is deliberate rather than derived from the value we just
/// rendered: the token must be the hash of what is *stored*, so that echoing it
/// back is guaranteed to match. Computing it from an in-memory copy would work
/// right up until the persist path ever normalizes the TOML, and then fail as a
/// mysterious permanent 409.
async fn graph_with_version(
    company: &ScopedCompany,
    file: WorkflowFile,
) -> Result<WorkflowGraph, ApiError> {
    let source_dir = company.runtime.source_dir();
    let (overlays, disabled, _) = workflow_state(company).await?;
    let editable = is_editable(source_dir, &overlays, &file.id);
    let version = editable
        .then(|| overlay_toml(&overlays, &file.id).map(workflow_version))
        .flatten();
    // Read back rather than assumed, so a create or an edit that the disarm rule
    // just switched off reports `enabled: false` on its own response — the
    // console learns about the disarm from the write it made, not from a later
    // refresh it might not do.
    let enabled = !disabled.iter().any(|id| id == &file.id);
    Ok(WorkflowGraph::new(file, editable, version, enabled))
}

/// The `PUT …/workflows/{wid}` body: the same camelCase graph shape the read and
/// create routes speak, plus the optional concurrency token.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateWorkflowBody {
    #[serde(flatten)]
    graph: CreateWorkflowBody,
    /// The token from the `GET`/`PUT` this edit was based on. **Required** (issue
    /// #1013): a missing token is a `400`, not an unconditional write, so a stale
    /// editor can't silently clobber a concurrent save. Kept `Option` +
    /// `serde(default)` so an omitted field is a clean handler-level `400` with a
    /// recovery message, rather than an opaque serde `422`.
    #[serde(default)]
    expected_version: Option<String>,
}

/// `PUT …/workflows/{wid}` — replaces a saved workflow graph wholesale (issue
/// #259).
///
/// Before this, a workflow was write-once: a typo in a cron expression or a
/// node pointed at the wrong teammate was permanent, and the only recovery was
/// to author a second workflow and leave the broken one firing forever.
///
/// The body's `id` **must equal** `wid`. Renaming an id through `PUT` is
/// deliberately rejected rather than quietly supported: the id keys the union
/// read path, the scheduler's per-workflow fire bookkeeping, and every
/// journalled run in the history — a rename would silently orphan all three. A
/// rename is a create plus a delete, and the operator should say so.
///
/// `expectedVersion` is **required** (issue #1013): omitting it used to mean an
/// unconditional write, so a console holding a stale graph — or one that read
/// `version` as `undefined` and sent nothing — silently clobbered a concurrent
/// save. A missing token is now a `400`, matching the agent `update_workflow`
/// tool, which has always demanded it. A caller re-reads the workflow and echoes
/// back its `version`; the conditional write then refuses with a `409` if the
/// graph moved in between.
///
/// Statuses: `400` (bad graph, `id` ≠ `wid`, or a missing `expectedVersion`),
/// `404` (unknown id), `409` (source-defined, body-less, name taken, or a stale
/// `expectedVersion`).
async fn update_workflow(
    company: ScopedCompany,
    Path(WorkflowPath { wid }): Path<WorkflowPath>,
    Json(body): Json<UpdateWorkflowBody>,
) -> Result<Json<WorkflowGraph>, ApiError> {
    if !safe_wid(&wid) {
        return Err(ApiError(OpenCompanyError::CompanyNotFound(format!(
            "workflow {wid}"
        ))));
    }
    if body.graph.id != wid {
        return Err(ApiError(OpenCompanyError::InvalidRequest(format!(
            "this request would change the workflow's id from `{wid}` to `{}`. A workflow's id \
             can't change — it keys the saved graph, its schedule and its run history. Create a \
             new workflow under the new id and delete this one instead.",
            body.graph.id
        ))));
    }

    // `expectedVersion` is required (issue #1013). An absent token used to mean
    // an unconditional write; that let a stale editor overwrite a concurrent save
    // without ever seeing a 409. Refuse the write with a 400 instead, mirroring
    // the agent `update_workflow` tool, and tell the caller how to recover.
    let Some(expected) = body.expected_version.clone() else {
        return Err(ApiError(OpenCompanyError::InvalidRequest(
            "`expectedVersion` is required: re-read this workflow and send back the `version` it \
             returns. A `PUT` replaces the whole graph, so saving without the version you read \
             from could silently overwrite a change made since."
                .to_string(),
        )));
    };
    let draft = RawWorkflow::try_from(body.graph)?;
    let file = update_company_workflow(
        company.id(),
        company.runtime.source_dir(),
        company.runtime.store(),
        company.runtime.workflow_revisions(),
        Some(company.runtime.events()),
        draft,
        Some(expected.as_str()),
        Some(&company.runtime.deliverable_channel_ids()),
    )
    .await
    .map_err(ApiError)?;
    // The response carries the NEW token, so a console can save twice in a row
    // without a re-read in between.
    Ok(Json(graph_with_version(&company, file).await?))
}

/// The optional `?expectedVersion=` query on `DELETE …/workflows/{wid}`.
///
/// A query param rather than a body because a `DELETE` with a body is poorly
/// supported by intermediaries and by `fetch`; the token is 64 hex characters,
/// so it is URL-safe without escaping.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteWorkflowQuery {
    /// The token of the graph being removed. **Required** (issue #1013): an
    /// absent `?expectedVersion=` is a `400`, not an unconditional delete, so a
    /// stale editor can't drop a workflow that changed since they last looked.
    #[serde(default)]
    expected_version: Option<String>,
}

/// `DELETE …/workflows/{wid}` — removes a saved workflow (issue #259).
///
/// Drops the graph body **and** the id from `[workflows].enabled` in one save,
/// so the workflow stops appearing in the picker and stops firing on its
/// schedule, and stays gone across a restart.
///
/// **Past runs are deliberately kept.** They are journal entries recording what
/// the workflow did, and that stays true after it is gone — `GET
/// …/workflows/runs` keeps serving them. See the module doc.
///
/// **A run still in flight is stopped** (B-121). Delete tore down the schedule
/// and the revisions and left the run executing — and left it *uncontrollable*,
/// because the only Stop button in the product lives on the workflow detail page
/// this request removes. So the run went on calling models and spending with
/// nothing anywhere able to reach it, while `GET …/workflows/runs` kept
/// reporting it `running: true` under a workflow that no longer existed. See
/// [`stop_runs_of_workflow`](crate::company::runtime::CompanyRuntime::stop_runs_of_workflow).
///
/// `expectedVersion` is **required** (issue #1013), for the same reason it is on
/// `PUT`: an absent token used to mean an unconditional delete, so a console
/// holding a stale graph could remove a workflow that changed underneath it. A
/// missing `?expectedVersion=` is now a `400`.
///
/// `200` with [`DeleteWorkflowResponse`] on success — **not** `204` (CodeRabbit
/// review, PR #2053): the sweep's own count is the only truthful source for
/// "was a run actually stopped", and a `204` has nowhere to carry it. See that
/// type's doc for why the console cannot derive the same answer itself. `400`
/// for a missing `expectedVersion`; `404` for an unknown id; `409` for a
/// source-defined or body-less id, or a stale `expectedVersion`.
async fn delete_workflow(
    company: ScopedCompany,
    Path(WorkflowPath { wid }): Path<WorkflowPath>,
    Query(query): Query<DeleteWorkflowQuery>,
) -> Result<Json<DeleteWorkflowResponse>, ApiError> {
    if !safe_wid(&wid) {
        return Err(ApiError(OpenCompanyError::CompanyNotFound(format!(
            "workflow {wid}"
        ))));
    }
    // `expectedVersion` is required (issue #1013) — a tokenless delete is refused
    // rather than run unconditionally, so a stale editor can't drop a workflow
    // that moved since they loaded it.
    let Some(expected) = query.expected_version.as_deref() else {
        return Err(ApiError(OpenCompanyError::InvalidRequest(
            "`expectedVersion` is required: read this workflow and pass its `version` as \
             `?expectedVersion=`. Deleting without the version you read from could remove a \
             workflow that changed since you last looked."
                .to_string(),
        )));
    };
    delete_company_workflow(
        company.id(),
        company.runtime.source_dir(),
        company.runtime.store(),
        company.runtime.workflow_revisions(),
        Some(company.runtime.schedule_fires()),
        Some(company.runtime.events()),
        &wid,
        Some(expected),
    )
    .await
    .map_err(ApiError)?;
    // B-121: after the durable delete, never before. The workflow has to be gone
    // first, or a run cancelled here could be replaced by one racing in behind
    // it through a route that still resolves the graph — the same ordering
    // Pause's sweep takes for the same reason.
    let stopped_runs = company.runtime.stop_runs_of_workflow(&wid);
    Ok(Json(DeleteWorkflowResponse { stopped_runs }))
}

/// The `DELETE …/workflows/{wid}` response body (CodeRabbit review, PR #2053).
///
/// The console used to guess "did this delete stop a run" from its own
/// pre-request state (whether it was watching a run when the operator clicked
/// Delete) and print that guess in the confirmation toast. That guess and this
/// count can disagree in the most ordinary way possible, no race required: a
/// long run the console was watching can finish **on its own**, normally, in
/// the seconds between the operator clicking the confirm button and this
/// request reaching the sweep — at which point [`stop_runs_of_workflow`]
/// truthfully stops nothing, while the console's pre-request guess still says
/// it did. `stopped_runs` is the sweep's own count, the only place that
/// answer actually lives.
///
/// [`stop_runs_of_workflow`]: crate::company::runtime::CompanyRuntime::stop_runs_of_workflow
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteWorkflowResponse {
    /// How many in-flight runs of this workflow the sweep fired a stop at.
    /// Zero is a completely ordinary answer — see the type doc.
    stopped_runs: usize,
}

/// The `PUT …/workflows/{wid}/enabled` body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetEnabledBody {
    /// The state to move to: `true` arms the schedule, `false` pauses it.
    enabled: bool,
}

/// `PUT …/workflows/{wid}/enabled` — arms or pauses a workflow's schedule
/// (issue #276).
///
/// Before this, the only way to stop a schedule firing was to delete the
/// workflow, which threw the graph away to silence it for an afternoon.
///
/// **Pausing stops the schedule, not the workflow.** A paused workflow keeps its
/// graph, stays in the picker, and still runs from the console's Run button —
/// [`WorkflowScheduler::tick`](crate::runtime::WorkflowScheduler) is the only
/// reader of the flag. That split is the point: "don't fire this on its own" and
/// "I can't run this" are different asks, and an operator debugging a workflow
/// needs the first without the second.
///
/// Idempotent: setting the state a workflow already holds is a `200` that writes
/// nothing and journals nothing, so a double-click costs one no-op rather than a
/// second audit entry.
///
/// Statuses: `200` (armed state is now what was asked, changed or not), `404`
/// (unknown id), `409` (a manifest-`enabled` id with no saved graph — no
/// schedule to switch off). No `expectedVersion`: see
/// [`set_company_workflow_enabled`].
async fn set_workflow_enabled(
    company: ScopedCompany,
    Path(WorkflowPath { wid }): Path<WorkflowPath>,
    Json(body): Json<SetEnabledBody>,
) -> Result<Json<WorkflowGraph>, ApiError> {
    if !safe_wid(&wid) {
        return Err(ApiError(OpenCompanyError::CompanyNotFound(format!(
            "workflow {wid}"
        ))));
    }
    // Issue #1046: the arm-time delivery check needs the deployment's delivery
    // capability, which lives on the runtime and the arm path cannot otherwise
    // see: whether a mailbox is wired (so `owner`/`email` outputs can land) and
    // which channels are deliverable (`deliverable_channel_ids` already excludes
    // the operator channel, and is the console destination picker's own source
    // of truth, #813).
    let mail_configured = company.runtime.mail().is_some();
    let wired_channels = company.runtime.deliverable_channel_ids();
    set_company_workflow_enabled(
        company.id(),
        company.runtime.source_dir(),
        company.runtime.store(),
        Some(company.runtime.events()),
        &wid,
        body.enabled,
        mail_configured,
        &wired_channels,
    )
    .await
    .map_err(ApiError)?;

    // Answer with the graph, re-read, rather than a bare 204: the console
    // renders the row from this shape, and reading it back means the `enabled`
    // it shows is what the store holds rather than what the request asked for.
    let (overlays, disabled, globals_disable) = workflow_state(&company).await?;
    let source_dir = company.runtime.source_dir();
    let file = load_workflow_with_globals(source_dir, &overlays, &globals_disable, &wid)
        .map_err(ApiError)?
        .ok_or_else(|| ApiError(OpenCompanyError::NotFound(format!("workflow {wid}"))))?;
    let editable = is_editable(source_dir, &overlays, &wid);
    let version = editable
        .then(|| overlay_toml(&overlays, &wid).map(workflow_version))
        .flatten();
    let enabled = !disabled.iter().any(|id| id == &wid);
    Ok(Json(WorkflowGraph::new(file, editable, version, enabled)))
}

// ---------------------------------------------------------------------------
// Revision history + rollback (issue #274)
// ---------------------------------------------------------------------------

/// One revision as the console's history panel renders it — **metadata only**.
///
/// The graph body is deliberately absent: the list is a chooser, and shipping a
/// full graph per row would make the history read as heavy as N graph reads for
/// no benefit. The restore route fetches (and returns) the body when an operator
/// actually picks one. `version` is the same opaque token
/// `GET …/workflows/{wid}` hands out for the current body, so a console can tell
/// which revision matches what it is looking at.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RevisionSummary {
    id: String,
    name: String,
    version: String,
    created_at_millis: u64,
}

/// The `GET …/workflows/{wid}/revisions` response: the snapshots, newest first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRevisionsResponse {
    revisions: Vec<RevisionSummary>,
}

/// `GET …/workflows/{wid}/revisions` — one workflow's edit history (issue #274),
/// newest first, **metadata only** (no graph bodies — see [`RevisionSummary`]).
///
/// A workflow with no history (never edited, or seed-backed) answers `200` with
/// an empty list rather than a `404`: "no revisions" is a normal state the
/// console renders as an empty panel, not an error. A malformed `wid` is a `404`
/// like every other read here.
async fn list_workflow_revisions(
    company: ScopedCompany,
    Path(WorkflowPath { wid }): Path<WorkflowPath>,
) -> Result<Json<WorkflowRevisionsResponse>, ApiError> {
    if !safe_wid(&wid) {
        return Err(ApiError(OpenCompanyError::CompanyNotFound(format!(
            "workflow {wid}"
        ))));
    }
    let rows = company
        .runtime
        .workflow_revisions()
        .list_revisions(company.id(), &wid)
        .await
        .map_err(ApiError)?;
    let revisions = rows
        .into_iter()
        .map(|r| RevisionSummary {
            id: r.id,
            name: r.name,
            // The token the current-graph read would hand out for this body, so
            // the console can correlate a row with what it currently holds.
            version: workflow_version(&r.toml),
            created_at_millis: r.created_at_millis,
        })
        .collect();
    Ok(Json(WorkflowRevisionsResponse { revisions }))
}

/// The sub-resource path on the restore route: the workflow id and the revision
/// id. The scope `id` is consumed by the extractor.
#[derive(Debug, Deserialize)]
struct RevisionPath {
    wid: String,
    rev: String,
}

/// The `POST …/workflows/{wid}/revisions/{rev}/restore` body: the optional
/// concurrency token of the graph being replaced.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreRevisionBody {
    /// The token from the `GET`/`PUT` the operator was looking at when they hit
    /// Restore. **Required** (issue #1013): an absent token — or an absent body —
    /// is a `400`, not an unconditional restore, so a stale editor can't overwrite
    /// a concurrent save. On a `409` reload rather than retry — the graph moved
    /// under it.
    #[serde(default)]
    expected_version: Option<String>,
}

/// `POST …/workflows/{wid}/revisions/{rev}/restore` — roll a workflow back to a
/// captured revision (issue #274), returning the restored [`WorkflowGraph`] with
/// a fresh version token.
///
/// This is an ordinary edit whose new body is an old one, so it routes through
/// the same [`rollback_company_workflow`] → [`update_company_workflow`] path a
/// `PUT` does and inherits every one of its guarantees: re-validation against
/// the *current* record, a snapshot of the body it replaces (so the restore is
/// itself undoable), the optimistic-concurrency token, and the #276 disarm of a
/// restored schedule.
///
/// `expectedVersion` is **required** (issue #1013), aligning restore with `PUT`:
/// an absent token — or an omitted body — used to mean an unconditional restore,
/// so a stale editor could overwrite a concurrent save. A missing token is now a
/// `400`.
///
/// Statuses: `200` (restored), `400` (a missing `expectedVersion`, or the
/// revision is invalid against the current record — e.g. it names a since-removed
/// teammate), `404` (unknown `wid` or unknown `rev`), `409` (seed-backed /
/// body-less `wid`, a stale `expectedVersion`, or a name collision).
async fn restore_workflow_revision(
    company: ScopedCompany,
    Path(RevisionPath { wid, rev }): Path<RevisionPath>,
    body: Option<Json<RestoreRevisionBody>>,
) -> Result<Json<WorkflowGraph>, ApiError> {
    if !safe_wid(&wid) {
        return Err(ApiError(OpenCompanyError::CompanyNotFound(format!(
            "workflow {wid}"
        ))));
    }
    // `expectedVersion` is required (issue #1013). Resolve it from the optional
    // body; an absent token or an absent body alike is a 400, not an
    // unconditional restore, so a stale editor can't clobber a concurrent save.
    let Some(expected) = body.and_then(|Json(b)| b.expected_version) else {
        return Err(ApiError(OpenCompanyError::InvalidRequest(
            "`expectedVersion` is required: read this workflow and send back its `version`. A \
             restore replaces the current graph, so doing it without the version you read from \
             could silently overwrite a change made since."
                .to_string(),
        )));
    };
    let file = rollback_company_workflow(
        company.id(),
        company.runtime.source_dir(),
        company.runtime.store(),
        company.runtime.workflow_revisions(),
        Some(company.runtime.events()),
        &wid,
        &rev,
        Some(expected.as_str()),
    )
    .await
    .map_err(ApiError)?;
    // The response carries the restored graph plus its NEW token — a restore is a
    // write, so the console holds a valid token for the next edit without a
    // follow-up read (and can see the #276 disarm on `enabled` if the restored
    // graph re-introduced a schedule).
    Ok(Json(graph_with_version(&company, file).await?))
}

/// Whether `wid` is a single safe on-disk filename stem — no path separators,
/// no `..`, not empty — so it can't escape the `workflows/` directory.
fn safe_wid(wid: &str) -> bool {
    use std::path::Component;
    let mut comps = FsPath::new(wid).components();
    matches!(comps.next(), Some(Component::Normal(_))) && comps.next().is_none()
}

/// The sub-resource path (`wid`); the scope `id` is consumed by the extractor.
#[derive(Debug, Deserialize)]
struct WorkflowPath {
    wid: String,
}

/// The run body: an optional trigger `input` payload seeded as the trigger
/// node's item. An empty object (`{}`) runs with a null input.
#[derive(Debug, Default, Deserialize)]
struct RunWorkflowBody {
    #[serde(default)]
    input: Value,
    /// Return as soon as the run has an id, instead of holding the request open
    /// for the whole run (issue #383).
    ///
    /// **Opt-in, and compatible in both directions.** A caller that omits it
    /// gets today's synchronous response byte-for-byte. A newer console talking
    /// to an *older* host sends it and the old host ignores the unknown field
    /// (this struct has no `deny_unknown_fields`) and answers the full
    /// synchronous 200 — which is exactly why the console must decide what
    /// happened from the response's **shape**, not from what it asked for.
    #[serde(default)]
    detach: bool,
    /// Run as a **dry run / test run** (issue #542): walk the real graph with
    /// real branch selection over stubbed effectful capabilities, so the run
    /// proves routing and output shape without any real effect — no agent
    /// inference, no tool/http execution, no delivery, no journaling, no gate
    /// parked in Approvals.
    ///
    /// **Opt-in and compatible in both directions, exactly like `detach`.** A
    /// caller that omits it gets today's behaviour byte-for-byte. A newer
    /// console asking an *older* host for a dry run sends it and the old host
    /// ignores the unknown field (no `deny_unknown_fields`) and runs it **FOR
    /// REAL** — which is why the response carries a `dryRun` presence
    /// discriminator the console must read, never trusting what it asked for.
    #[serde(default)]
    dry_run: bool,
}

/// The run response: the engine's final state plus any nodes left pending
/// approval.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunWorkflowResponse {
    output: Value,
    pending_approvals: Vec<String>,
    /// One row per attempt to route a reached `output` node's report to its
    /// destination (issue #170). Empty for a graph that routes nothing. This is
    /// where an operator learns a report was NOT delivered — a delivery failure
    /// never fails the run, so it has nowhere else to surface.
    deliveries: Vec<crate::ports::DeliveryReport>,
    /// The run's correlation id (issue #371).
    ///
    /// Additive, and the console needs it for one specific reason: the run's
    /// progress events arrive over SSE *while this request is still in flight*,
    /// so without an id handed back the console cannot be certain the frames it
    /// has been painting belong to the run it just awaited rather than a cron
    /// fire that overlapped it.
    run_id: String,
    /// Whether an operator stopped this run while the request was still open
    /// (issue #383).
    ///
    /// **A synchronous run is cancellable too**, which is easy to miss: the run
    /// id is registered the moment `spawn_workflow_run` returns, and the console
    /// learns it from the `workflow_run_started` SSE frame — so
    /// `POST …/runs/{rid}/cancel` is reachable long before this response is
    /// written. When that happens the runner resolves to a cancelled run whose
    /// `output` is `null` with no approvals and no deliveries, which without
    /// this flag is indistinguishable from a run that legitimately produced null
    /// output and routed nothing.
    ///
    /// Omitted when false, exactly like
    /// [`WorkflowRunOutcome::cancelled`](WorkflowRunOutcome), so an existing
    /// caller's body is byte-unchanged.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    cancelled: bool,
    /// What this run adds up to, in one word (issue #981).
    ///
    /// **Always serialized**, unlike every optional field around it, because
    /// its whole purpose is to be the field a client reads instead of
    /// re-deriving the reading from the rows. An omitted verdict would push
    /// every reader straight back into the six-field ladder this replaces.
    ///
    /// It is the *only* place this response says a report did not go out. A
    /// delivery failure never fails the run and never touches `nodes[].status`
    /// (see [`WorkflowRunVerdict`]), so a console or an API client folding node
    /// statuses scores a dropped report green — which is exactly what issue
    /// #981 caught in the field.
    verdict: WorkflowRunVerdict,
    /// Per-node progress for this run, in the order the nodes finished (issue
    /// #542). Carried for **every** synchronous run, not only a dry one — it is
    /// the same structural per-node timeline `GET …/workflows/runs` returns, so
    /// the run-result panel can render it without a second read. For a dry run
    /// it is the *only* record of what ran, since a test run journals nothing.
    ///
    /// Empty for a run whose nodes all failed to report (or a build with no
    /// progress observer), so an empty list means "no per-node trail", never
    /// "the run did nothing".
    nodes: Vec<WorkflowRunNode>,
    /// Whether this was a **dry run** (issue #542) — the presence discriminator.
    ///
    /// **A constant `true` when set, and absent otherwise, on purpose** — the
    /// exact shape `detached` takes for #383. A newer console asking an older
    /// host for a dry run gets a *real* run back (the old host ignored the
    /// unknown request field), and the body then carries no `dryRun` key. So the
    /// console cannot tell a dry run from a real one by what it asked for — only
    /// by what came back. A field that is only ever `true` makes that a presence
    /// check, and its absence a loud signal that the run was REAL.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
    /// The board writes this run's agent nodes performed (issue #661 / M5).
    ///
    /// The same rows `GET …/workflows/runs` returns and the same rows the
    /// `WorkflowRunFinished` event carries — one shape across all three, so the
    /// console reads a run's board effects identically whether it awaited the run
    /// or found it in the history.
    ///
    /// Omitted when empty, so an existing caller's body is byte-unchanged for every
    /// run that touched no card.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    board: Vec<crate::ports::WorkflowRunBoardRow>,
    /// The nodes this run blocked on a human (issue #881).
    ///
    /// The same rows `GET …/workflows/runs` returns and the same rows the
    /// `WorkflowRunFinished` event carries — one shape across all three, so a
    /// console reads a blocked run identically whether it awaited it or found
    /// it in the history. Omitted when empty, so a run that blocked on nobody
    /// is byte-unchanged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blocked_nodes: Vec<crate::ports::WorkflowBlockedNode>,
    /// The approvals this run parked (issue #880) — what it opened, not what
    /// is still outstanding.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    approvals: Vec<crate::ports::WorkflowRunApprovalRow>,
}

/// The `detach: true` response (issue #383): the run's id, handed back before
/// the engine has walked a single node.
///
/// **`detached` is the discriminator, and it is a constant `true` on purpose.**
/// A newer console pointed at an older host sends `detach` and gets the *full
/// synchronous* body back, because the old host ignores the unknown field. So
/// the console cannot tell the two apart by what it asked for — only by what
/// came back. `output` present means the run already settled; `detached`
/// present means watch the stream. A field that is only ever `true` is what
/// makes that a presence check rather than a guess.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DetachedRunResponse {
    run_id: String,
    detached: bool,
}

/// The two shapes `POST …/workflows/{wid}/run` can answer with.
///
/// An enum rather than a bare [`Response`] so the two bodies stay typed and the
/// status codes live in one place: `200` for the settled run the route has
/// always returned, `202 Accepted` for a run that has been accepted and started
/// but has not finished — which is precisely what `202` means.
enum RunWorkflowOk {
    /// Boxed: the settled body is far wider than the detached one (it carries
    /// the run's output, node trail, deliveries, board rows and — since #881 /
    /// #880 — its blocked nodes and parked-approval receipts), and holding it
    /// inline made the 202 path pay that width too.
    Settled(Box<RunWorkflowResponse>),
    Detached(DetachedRunResponse),
}

impl IntoResponse for RunWorkflowOk {
    fn into_response(self) -> Response {
        match self {
            Self::Settled(body) => Json(body).into_response(),
            Self::Detached(body) => (StatusCode::ACCEPTED, Json(body)).into_response(),
        }
    }
}

/// Registers a run with the company's [`RunSupervisor`] and drives it on its own
/// task (issue #383).
///
/// # Why both modes go through here
///
/// The detached mode obviously needs a spawned task — there is no request left
/// to hold it. The *synchronous* mode does not, and routes it through anyway,
/// which buys something the old inline `await` did not have: the run no longer
/// dies with the connection. Axum drops a handler future when the client goes
/// away, so before this, a `curl` killed mid-run took the run's remaining nodes
/// with it — leaving a `WorkflowRunStarted` with no finish, which the boot sweep
/// then stamped "interrupted by a host restart" even though no restart happened.
/// A spawned task outlives the handler, so the sync path now journals its
/// outcome whether or not anyone is still listening.
///
/// The [`RunGuard`](crate::runtime::RunGuard) is moved into the task and held
/// across the `record_run_finished`, so a run stays cancellable right up to the
/// moment it settles and not one moment after.
///
/// A **fresh task is correct here** for the same reason the cron scheduler's is
/// (see `workflow_scheduler`): the `WORKFLOW_DEPTH` re-entry guard counts one
/// causal chain, and an operator pressing Run is a new root at depth 0. What
/// would break the guard is spawning *inside* an existing run's chain — which is
/// why the orchestrator's `run_workflow` tool deliberately does NOT use this.
fn spawn_workflow_run(
    runtime: &crate::company::runtime::CompanyRuntime,
    runner: std::sync::Arc<dyn crate::ports::WorkflowRunner>,
    workflow: WorkflowFile,
    input: Value,
    dry_run: bool,
) -> crate::Result<(
    String,
    tokio::task::JoinHandle<crate::Result<crate::ports::WorkflowRun>>,
)> {
    // Issue #395: the supervisor registration and the both-arms outcome
    // journalling that used to live here now live in `WorkflowSpawn`, because
    // approving a paused workflow gate starts a run too and owes exactly the
    // same two things. One copy of the discipline, two entry points.
    //
    // Issue #401: `spawn` is fallible — a company at its in-flight run ceiling
    // is refused here, before any task is spawned, and the caller maps that to
    // a 429.
    //
    // Issue #542: `dry_run` rides through to the spawn task, which stamps it on
    // the run context and skips the outcome journal write when set.
    crate::runtime::WorkflowSpawn::new(runtime, runner).spawn(workflow, input, false, dry_run)
}

/// `POST …/workflows/{wid}/run` (both scope forms).
async fn run_workflow(
    company: ScopedCompany,
    Path(WorkflowPath { wid }): Path<WorkflowPath>,
    body: Option<Json<RunWorkflowBody>>,
) -> Result<RunWorkflowOk, crate::server::Rejection> {
    // **A paused company does not take new work, and pressing Run is new work.**
    //
    // Every other path already refuses on this: the workflow scheduler
    // (`workflow_scheduler.rs`), the task scheduler, the mailbox poller, A2A,
    // chat, and — verified, because the first draft of this comment claimed
    // otherwise — the approve/resume route, which gates in `run_resolve`
    // (`operator.rs`) above both `resolve_approval_spawned` and the workflow
    // resume it forks into. This POST was the single unguarded door, which is
    // exactly why the report reads "chat correctly refuses with 409, and
    // pressing Run on a workflow starts a real billed run anyway": the console
    // promises "Pause stops this company taking new work" on the same screen.
    //
    // Pause deliberately does **not** stop a run already executing — `pause`
    // only writes the lifecycle (`provision.rs` `transition`), and the promise
    // is about *taking new work*. That is a separate question from this one.
    //
    // **Before the runner lookup, not after.** Whether this company is paused
    // does not depend on whether workflow execution is wired, and a host without
    // a runner would otherwise answer `not_wired` to a paused company — a true
    // sentence about the deployment that hides the one the operator needs. The
    // cheap, always-correct refusal goes first so nothing can mask it.
    //
    // Refused with the same `LifecycleConflict` chat answers, so one pause reads
    // identically wherever it is met, rather than a second rule with a second
    // error shape.
    //
    // Emergency-stop checked first, ahead of the ordinary pause: it is a
    // separate switch from `lifecycle` (a stopped company still reports
    // `running`), so `ensure_running` alone would miss it — this was the one
    // manual workflow-run door `CompanyRuntime::ensure_not_emergency_stopped`'s
    // three doorways did not cover, because it never reaches `run_cycle`,
    // `spawn_follow_up`, or the boot reconciler at all.
    company.runtime.ensure_not_emergency_stopped()?;
    company.runtime.ensure_running().await?;

    // No runner wired. THREE very different causes look identical from here —
    // `workflow_runner() == None` — and each points the operator at a different
    // next step (issues #266, #514):
    //   1. this build/deployment has no workflow execution at all — nothing the
    //      operator can do, so "not wired in this deployment" is the truth;
    //   2. this *boot* has none, because the company started with no inference
    //      source but one is saved now. The runner is populated from the harness
    //      arm at build time, so configuring inference afterwards leaves it
    //      `None` until a restart — reported as `restart_required` (#266);
    //   3. nothing was ever configured, but this host can run the harness. The
    //      fix is to configure an inference source (which rebuilds in place,
    //      #290) — reported as `inference_required`, not the `not_wired` 404 that
    //      would send the operator hunting a deployment problem that does not
    //      exist (#514).
    let Some(runner) = company.runtime.workflow_runner() else {
        use super::inference::RunnerGap;
        return Err(
            match super::inference::runner_gap_for(company.runtime.as_ref()).await {
                RunnerGap::RestartPending => super::restart_required("workflow execution").into(),
                RunnerGap::InferenceRequired => {
                    super::inference_required("workflow execution").into()
                }
                RunnerGap::NotWired => super::not_wired("workflow execution").into(),
            },
        );
    };

    // `wid` becomes a filename — reject anything that could escape `workflows/`.
    if !safe_wid(&wid) {
        return Err(OpenCompanyError::NotFound(format!("workflow {wid}")).into());
    }

    // Load the saved graph from the seed ∪ overlay union, so a graph created on
    // a hosted tenant (no source directory) runs the same as a committed one.
    let (overlays, globals_disable) = overlay_workflows_and_globals(&company).await?;
    let file = load_workflow_with_globals(
        company.runtime.source_dir(),
        &overlays,
        &globals_disable,
        &wid,
    )?
    .ok_or_else(|| OpenCompanyError::NotFound(format!("workflow {wid}")))?;

    let body = body.map(|Json(b)| b).unwrap_or_default();
    let detach = body.detach;
    // Issue #542: captured before `body.input` moves, and threaded into both the
    // spawn (so the run runs dry) and the settled response's discriminator (so
    // the console can confirm the host honoured the request rather than running
    // for real).
    let dry_run = body.dry_run;

    // Issue #383: registered and spawned before either mode branches, so the two
    // modes cannot drift in what they start. Issue #228's journalling now lives
    // inside the task rather than around this await.
    //
    // Issue #401: the concurrency ceiling is enforced HERE, before the
    // detach/sync branch below, so both modes refuse identically — a 429 with
    // the actionable `{error, code: "workflow_run_limit"}` envelope and no run
    // id, because nothing started. The rejection precedes any task or any
    // `WorkflowRunStarted`, so there is nothing to unwind.
    let (run_id, handle) = spawn_workflow_run(
        company.runtime.as_ref(),
        runner.clone(),
        file,
        body.input,
        dry_run,
    )?;

    if detach {
        // Returned before the engine has walked a node. From here the client
        // follows the run through the SSE frames issue #371 already keys by this
        // id, and reads its outcome back from `GET …/workflows/runs`, whose fold
        // already reports `running: true` for a run in flight.
        //
        // The task is deliberately NOT joined and its handle is dropped: it
        // settles itself, journals its own outcome, and its guard deregisters
        // it. Detaching is the entire point.
        return Ok(RunWorkflowOk::Detached(DetachedRunResponse {
            run_id,
            detached: true,
        }));
    }

    // The synchronous mode, whose response is unchanged: await the task rather
    // than the runner. A `JoinError` here means the run task panicked or was
    // aborted — the run's outcome was never journaled, so there is nothing
    // truthful to hand back and this is a genuine 500 rather than a run result.
    match handle.await {
        Ok(Ok(run)) => {
            // Issue #981: read the verdict off the settled run BEFORE its fields
            // are moved onto the wire shape below.
            //
            // `running` and `error` are constants rather than fields, and both
            // are facts about this arm: a body is written only for a run that
            // settled, and a run that failed leaves through `Ok(Err(err))` one
            // arm down as an `ApiError` with no body at all. So the readings
            // this response can carry are `stopped`, `blocked`, `undelivered`,
            // `awaiting-approval`, `degraded` (issue #1865) and `ok` — never
            // `running`, never `failed`.
            let verdict = WorkflowRunVerdict::of(RunVerdictFacts {
                running: false,
                error: None,
                cancelled: run.cancelled,
                blocked_nodes: run.blocked_nodes.len(),
                deliveries: &run.deliveries,
                pending_approvals: run.pending_approvals.len(),
                // Issue #1189: the live-approvals JOIN is deliberately not run
                // here — this body is written microseconds after
                // `park_pending_gates` minted the cards, so joining against the
                // queue would be a guaranteed-zero query on the hot path; that
                // reconciliation is a fact about a run somebody comes back to,
                // which is what the history route is for.
                //
                // Issue #1865: but a card this run tried to park and never
                // reached the queue at all — `ParkFailed`/`Discarded` — is known
                // the instant the run settles, with no query. Reading it off
                // `run.approvals` here (rather than hardcoding zero) is what
                // stops a run whose approvals queue is unwired, or whose turn
                // gated more calls than the per-batch cap, from reporting
                // `awaiting-approval` on a card nobody will ever see.
                // Codex review (#1865): counted per pending *node*, not per
                // gated call — `run.pending_approvals` and `run.approvals` are
                // different units, and a call-level count made a node with one
                // live parked call alongside one failed one read as fully
                // stranded. See `workflow_runner::stranded_approvals`.
                stranded_approvals: crate::ports::workflow_runner::stranded_approvals(
                    &run.pending_approvals,
                    &run.approvals,
                ),
                // Issue #1865: `run.nodes` already carries the runner's own
                // reclassification (`reclassify_blocked`, `reclassify_capped_nodes`)
                // by the time it reaches this response, so a row still `Error`
                // here is a genuine one — either a capability error under
                // `on_error: continue|route`, or a turn that truncated at the
                // iteration cap. Read before `run.nodes` moves onto the wire
                // shape below.
                errored_nodes: run
                    .nodes
                    .iter()
                    .filter(|n| n.status == WorkflowNodeStatus::Error)
                    .count(),
            });
            Ok(RunWorkflowOk::Settled(Box::new(RunWorkflowResponse {
                output: run.output,
                pending_approvals: run.pending_approvals,
                deliveries: run.deliveries,
                run_id,
                cancelled: run.cancelled,
                verdict,
                // Issue #542: the runner collects this per-node trail on every
                // run; map the port rows onto the wire shape the history route
                // already uses. `dry_run` is the request's, echoed back as the
                // presence discriminator a console pointed at an old host would
                // never see.
                nodes: run.nodes.into_iter().map(WorkflowRunNode::from).collect(),
                dry_run,
                // Issue #661 (M5): carried on the synchronous path too, so a
                // console that pressed Run learns what the run did to the board
                // without a second read of the history.
                board: run.board,
                // Issues #881 / #880: likewise. An operator who pressed Run and
                // watched eight green nodes come back is exactly the reader
                // these two exist for — the run drawer is where they first
                // learn the pipeline delivered nothing and why.
                blocked_nodes: run.blocked_nodes,
                approvals: run.approvals,
            })))
        }
        Ok(Err(err)) => Err(ApiError(err).into_response().into()),
        Err(join) => {
            tracing::error!(
                company = %company.id(),
                workflow = %wid,
                %run_id,
                %join,
                "workflow run task did not complete; no outcome was journaled for it"
            );
            // `BackgroundTask`, not a harness error: the distinction it draws —
            // "the work's outcome is unknown", as opposed to "the work failed" —
            // is exactly right here. The run may even have done most of its
            // nodes; what is missing is an answer.
            Err(ApiError(OpenCompanyError::BackgroundTask(
                "the workflow run task did not complete".to_string(),
            ))
            .into_response()
            .into())
        }
    }
}

/// The sub-resource path on the cancel route: the run id.
#[derive(Debug, Deserialize)]
struct RunPath {
    rid: String,
}

/// The cancel acknowledgement (issue #383).
///
/// `cancelling`, not `cancelled`: this route fires a signal and returns. The run
/// is stopped at the engine future's next suspension point and settles itself a
/// moment later with a `WorkflowRunFinished{cancelled: true}` — which is the
/// event the console should believe, not this body.
#[derive(Debug, Serialize)]
struct CancelRunResponse {
    cancelling: bool,
}

/// `POST …/workflows/runs/{rid}/cancel` (both scope forms) — stop a run that is
/// still walking its graph (issue #383).
///
/// # Who may cancel
///
/// Anyone who passes this route's [`ScopedCompany`] guard, i.e. any operator of
/// the company. There is deliberately no "only the operator who started it"
/// rule: a run is a company-level activity, the console shows it to every
/// operator, and the case this exists for — a run wedged on a slow agent node —
/// is exactly the one where whoever started it may have gone home.
///
/// # 404 covers two cases, and means one thing
///
/// An unknown run id and an already-settled run both answer `404`. They are the
/// same answer to the operator: there is nothing here to stop. Keeping a
/// tombstone to tell them apart would mean picking an expiry for it, and the run
/// history already says what became of a settled run.
///
/// # What a cancelled run leaves behind
///
/// Journaled node rows for the nodes that completed, a `WorkflowRunFinished`
/// carrying `cancelled: true` and no error, and **any approvals earlier nodes
/// parked stay valid in the queue** — those are journal-backed and independent
/// of the run, so an operator may still approve or deny them afterwards. No
/// grant minted during the run is revoked.
async fn cancel_workflow_run(
    company: ScopedCompany,
    Path(RunPath { rid }): Path<RunPath>,
) -> Result<Json<CancelRunResponse>, ApiError> {
    if company.runtime.run_supervisor().cancel(&rid) {
        return Ok(Json(CancelRunResponse { cancelling: true }));
    }
    Err(ApiError(OpenCompanyError::CompanyNotFound(format!(
        "workflow run {rid}"
    ))))
}

/// `GET …/workflows/runs/{rid}/output` (both scope forms) — the durable per-node
/// output snapshot of one past (or live-and-settled) run (issue #596).
///
/// This is the data the run inspector renders: `{ runId, workflowId, atMillis,
/// nodes, truncated }`, where `nodes` is the engine's `{ "<node id>": { "items":
/// [ … ] } }` map, bounded for storage. The console opens one node in a past run
/// and shows what it produced — the make.com per-node output view.
///
/// # 404 means "no output snapshot", and that covers three honest cases
///
/// A `404` here is not an error the console should surface loudly: it means this
/// run has no stored output, which is true for **every run that predates this
/// feature**, for a **dry run** (writes nothing durable), and for a
/// **hard-aborted** run (dropped mid-flight, no outcome to persist). The console
/// renders an explicit empty state ("this run predates output capture / produced
/// none") rather than a failure. An unknown run id lands here too, and means the
/// same thing to the operator: there is nothing to show.
///
/// Deliberately a lazy per-run fetch rather than a field folded into
/// [`list_runs`]: that fold is already expensive, and the inspector only ever
/// needs the one run an operator clicked into.
async fn get_run_output(
    company: ScopedCompany,
    Path(RunPath { rid }): Path<RunPath>,
) -> Result<Json<crate::ports::WorkflowRunOutputRecord>, ApiError> {
    match company
        .runtime
        .workflow_run_outputs()
        .get_run_output(company.id(), &rid)
        .await
        .map_err(ApiError)?
    {
        Some(record) => Ok(Json(record)),
        None => Err(ApiError(OpenCompanyError::NotFound(format!(
            "no output captured for workflow run {rid}"
        )))),
    }
}

/// The row cap on [`run_artifacts`]: a defensive ceiling on how many files one
/// run's response carries, in the spirit of [`MAX_RUN_LIMIT`] on the history
/// read. A run that opened an unusually large number of cards — or a card with
/// a long publish history — cannot turn one lazy expand into an unbounded
/// payload; the newest rows survive the truncation, matching the newest-first
/// sort the handler applies just before it.
const MAX_RUN_ARTIFACTS: usize = 500;

/// One file a workflow run produced, projected for the run inspector's "Files
/// associated" section (issue #1684).
///
/// **Metadata only** — never the artifact body, the same discipline
/// [`WorkflowRunNode`] keeps. The console deep-links each row into the
/// producing card's Artifacts tab (`artifactHref` → `taskId` + `artifactId` +
/// `latestVersion`) and, when the file was mirrored into the workspace tree,
/// offers a second link to that node. Both are addresses; neither needs bytes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunArtifactRow {
    /// The card that produced the file — scopes both `artifactHref` and the
    /// Artifacts tab the link opens.
    task_id: String,
    /// The artifact's stable id → the tab's `openArtifactId`.
    artifact_id: String,
    /// The artifact's display title.
    title: String,
    /// What the file holds; drives the console's icon/renderer choice.
    kind: crate::ports::ArtifactKind,
    /// The workspace-relative path the agent published (e.g. `specs/launch.md`).
    /// `None` for a legacy record captured before issue #244 — the console
    /// labels it, it does not drop it, so the history stays honest.
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    /// The newest revision number → the tab's `openVersion` pin. `0` only for a
    /// hand-written or truncated record with no versions, which the accessors
    /// tolerate rather than panic on.
    latest_version: u32,
    /// Epoch-millis of the newest revision — the sort key and the display time.
    updated_at_millis: u64,
    /// The workspace node the newest revision was mirrored into, when one was
    /// (issue #552) → an optional `#/workspace/<id>` link. `None` when nothing
    /// mirrored it.
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_node_id: Option<String>,
    /// The producing card's title, for grouping rows by card in the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    task_title: Option<String>,
}

/// `GET …/workflows/runs/{rid}/artifacts` response. A wrapper rather than a
/// bare array so a run whose file count exceeds [`MAX_RUN_ARTIFACTS`] can say
/// so — the same `truncated` contract [`get_run_output`] uses for its per-node
/// snapshot — instead of silently dropping the older rows and letting the
/// console present the list as exhaustive.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunArtifactsResponse {
    /// The run's files, newest first (the sort `run_artifacts` applies).
    files: Vec<RunArtifactRow>,
    /// Whether older rows were cut by [`MAX_RUN_ARTIFACTS`]. `false` for every
    /// run in practice — the cap is a defensive ceiling, not a page size — but
    /// a caller that sees `true` knows the list is not the whole story.
    truncated: bool,
}

/// `GET …/workflows/runs/{rid}/artifacts` (both scope forms) — the files one
/// past run produced, for the run inspector's "Files associated" section
/// (issue #1684).
///
/// # The join, and why it reads two ports in memory
///
/// There is no direct "artifacts by run" index: [`ArtifactVersion::run_id`] is
/// the task **attempt** id, not the workflow run id. The authoritative link is
/// the card's [`origin_run_id`](crate::ports::TaskRecord::origin_run_id) — the
/// run that OPENED the card — so the read is `run_id → cards where
/// origin_run_id == run_id → each card's artifacts`. Both halves reuse the
/// broad list primitives ([`TaskStore::list`] once, then
/// [`ArtifactStore::list`] narrowed to each matched card) and filter in memory
/// rather than adding an origin-run query to the three storage backends — the
/// same choice `list_runs` makes for the same reason, and correct here because
/// this is a lazy per-run read, not a per-history-GET cost.
///
/// # `200 { files, truncated }`, never `404`
///
/// The one contract difference from [`get_run_output`]: a run that opened no
/// cards, or whose cards published no files, is the common case, not an error —
/// it answers `200` with `files: []`, mirroring [`ArtifactStore::list`]
/// returning `[]` for a card with no artifacts. An unknown run id is
/// indistinguishable from a run that produced nothing, and means the same thing
/// to the operator: no files.
///
/// `truncated` is `true` only when [`MAX_RUN_ARTIFACTS`] cut older rows — a
/// defensive ceiling for a run that opened an unusually large number of files,
/// not a page size. The console reads it and labels the list "newest 500
/// shown" rather than presenting an incomplete list as exhaustive.
///
/// # Provenance is the OPENING run
///
/// `origin_run_id` is stamped once, when the card is created; a later run that
/// re-owns the card does not overwrite it, so a card's files list under the run
/// that opened it, not a re-owner. A `sub_workflow` child stamps its parent
/// run, so sub-workflow cards roll up to the parent. This is the intended,
/// defensible reading of "the files this run produced".
async fn run_artifacts(
    company: ScopedCompany,
    Path(RunPath { rid }): Path<RunPath>,
) -> Result<Json<RunArtifactsResponse>, ApiError> {
    // One broad read of the board; the run's cards are the ones this run
    // opened. `TaskStore::list` also carries each card's title, so grouping the
    // files by card below needs no second read.
    let cards = company
        .runtime
        .tasks()
        .list(company.id())
        .await
        .map_err(ApiError)?;

    let mut rows: Vec<RunArtifactRow> = Vec::new();
    for card in cards {
        if card.origin_run_id.as_deref() != Some(rid.as_str()) {
            continue;
        }
        let artifacts = company
            .runtime
            .artifacts()
            .list(company.id(), Some(&card.id))
            .await
            .map_err(ApiError)?;
        for record in artifacts {
            // The two latest-derived fields, read before the record's own
            // fields move into the row below (the accessor borrows `record`).
            let latest_version = record.latest().map(|v| v.version).unwrap_or(0);
            let workspace_node_id = record.latest().and_then(|v| v.workspace_node_id.clone());
            rows.push(RunArtifactRow {
                task_id: record.task_id,
                artifact_id: record.id,
                title: record.title,
                kind: record.kind,
                source: record.source,
                latest_version,
                updated_at_millis: record.updated_at_millis,
                workspace_node_id,
                task_title: Some(card.title.to_string()),
            });
        }
    }

    // Newest first, then cap defensively — the newest rows are the ones an
    // operator opening a run reaches for, so they are the ones the cap keeps.
    rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at_millis));
    let truncated = rows.len() > MAX_RUN_ARTIFACTS;
    rows.truncate(MAX_RUN_ARTIFACTS);
    Ok(Json(RunArtifactsResponse {
        files: rows,
        truncated,
    }))
}

// ---------------------------------------------------------------------------
// Cron preview (issue #262)
// ---------------------------------------------------------------------------

/// How many upcoming fire times the preview returns. Three is enough to show
/// the *interval* — one time tells you when, three tell you how often — without
/// turning a one-line hint into a table.
const CRON_PREVIEW_FIRES: usize = 3;

/// The preview request: the expression as typed, plus an optional instant to
/// compute the next fires from.
#[derive(Debug, Deserialize)]
struct PreviewCronBody {
    expr: String,
    /// Epoch millis to search forward from. Defaults to now; present so tests
    /// can pin the answer instead of asserting against a moving clock.
    #[serde(default)]
    after: Option<u64>,
}

/// The preview response. Untagged, so the two outcomes are two shapes rather
/// than one shape with half its fields null.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum PreviewCronResponse {
    /// A valid expression: what it means (`null` when the shape is one
    /// [`CronExpr::describe`] declines to paraphrase) and when it next fires.
    Parsed {
        description: Option<String>,
        /// Epoch millis, ascending. The console renders each one twice — UTC
        /// and the viewer's local zone — from this single number, which is what
        /// makes the two readings incapable of disagreeing.
        next: Vec<u64>,
    },
    /// A malformed expression, carrying the parser's own message.
    Invalid { error: String },
}

/// `POST …/workflows/cron/preview` (both scope forms).
///
/// Issue #262: a trigger's schedule is a bare cron field, and the failure that
/// is NOT handled is the *successful* one — `0 9 * * *` saves cleanly whether or
/// not the author meant 9am, and it is UTC whether or not they read the hint.
/// Echoing the parsed meaning and the next fire times is the only thing that
/// turns a silently-wrong schedule into an obviously-wrong one.
///
/// **Always answers 200**, including for a malformed expression. The console
/// calls this while the author is still typing, so half-written garbage is the
/// normal state, not an exception — and its HTTP client throws on any non-2xx,
/// so a 400 per keystroke would make an ordinary parse failure arrive as a
/// thrown error and force try/catch as control flow. The rejection that matters
/// still happens: the create route validates the schedule and refuses to save a
/// bad one.
///
/// Scoped (and so authenticated) like every other route in this module even
/// though the computation touches no company state — an unauthenticated compute
/// endpoint would be a new kind of surface here for no gain.
async fn preview_cron(
    _company: ScopedCompany,
    Json(body): Json<PreviewCronBody>,
) -> Json<PreviewCronResponse> {
    let expr = match CronExpr::parse(&body.expr) {
        Ok(expr) => expr,
        Err(err) => {
            return Json(PreviewCronResponse::Invalid {
                error: err.to_string(),
            });
        }
    };

    let after = body.after.unwrap_or_else(now_millis);
    let mut cursor = CivilTime::from_unix_millis(after);
    let mut next = Vec::with_capacity(CRON_PREVIEW_FIRES);
    for _ in 0..CRON_PREVIEW_FIRES {
        // `next_after` is bounded and returns `None` only for an expression
        // that can never fire, which a parsed one cannot be — but stopping
        // early is still the right answer if that ever changes.
        let Some(fire) = expr.next_after(&cursor) else {
            break;
        };
        next.push(fire.unix_millis());
        cursor = fire;
    }

    Json(PreviewCronResponse::Parsed {
        description: expr.describe(),
        next,
    })
}

/// Wall-clock epoch millis. Saturates at the epoch rather than panicking on a
/// clock set before 1970 — a preview is not worth a 500.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Create-time copilot (issue #753)
// ---------------------------------------------------------------------------

/// The cap on a create-time copilot description (issue #753), in codepoints —
/// applied to the request body before it reaches the metered draft path. Gated
/// with the handler arm that reads it: the default build's `not_wired` arm never
/// drafts, so it never caps.
#[cfg(feature = "openhuman")]
const MAX_DRAFT_DESCRIPTION_CHARS: usize = 4_000;

/// The `POST …/workflows/draft-from-description` body: a free-text description of
/// the workflow the operator wants built.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftFromDescriptionBody {
    description: String,
}

/// The draft-from-description answer (issue #753).
///
/// Like the cron preview, it answers **200 in both model-answer cases** — a
/// drafted graph, or an honest "this is better done once" — because neither is
/// an error the operator fixes by retrying differently; the console renders
/// whichever came back and keys on `automatable`. Only a request problem (an
/// empty description → 400) or a capability gap (no brain wired → 404/409) is a
/// non-2xx.
#[derive(Debug, Serialize)]
#[serde(untagged)]
// The default build's `not_wired` arm returns this type but constructs neither
// variant — only the `openhuman` arm answers 200. The variants are live under
// the feature CI actually builds and tests, so this is a cfg artefact, not a
// dead type.
#[cfg_attr(not(feature = "openhuman"), allow(dead_code))]
enum DraftFromDescriptionResponse {
    /// A drafted graph for the New-workflow dialog to hydrate its form from.
    /// `workflow` is a `WorkflowGraphSpec` — the same camelCase node/edge shape
    /// the read routes return, so the console loads it with no adapter.
    Drafted {
        automatable: bool,
        summary: String,
        workflow: Value,
        /// Host corrections the operator should see (issue #813) — e.g. a
        /// name/role→id rewrite the resolver made. Empty when the draft needed
        /// none; `#[serde(default)]` on the reader keeps the older shape valid.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        notes: Vec<String>,
    },
    /// The described work is not worth a reusable workflow — or could not be
    /// drafted into one that would survive Create; `reason` says why.
    NotAutomatable { automatable: bool, reason: String },
}

/// `POST …/workflows/draft-from-description` (both scope forms) — the New-workflow
/// dialog's copilot (issue #753). Drafts a graph from the operator's description
/// and hands it back for review; it never persists, so the ordinary Create path
/// (`POST …/workflows`) stays the only way a graph reaches the workflow list.
#[cfg(feature = "openhuman")]
async fn draft_from_description(
    company: ScopedCompany,
    Json(body): Json<DraftFromDescriptionBody>,
) -> Result<Json<DraftFromDescriptionResponse>, crate::server::Rejection> {
    let description = body.description.trim();
    if description.is_empty() {
        return Err(ApiError(OpenCompanyError::InvalidRequest(
            "describe the workflow you want in a sentence or two.".to_string(),
        ))
        .into_response()
        .into());
    }
    // Char-safe cap on the request body before it reaches the metered path.
    let description: String = description
        .chars()
        .take(MAX_DRAFT_DESCRIPTION_CHARS)
        .collect();

    // No builder wired: classify WHY exactly as the run route does (issues #266,
    // #514), so the console points the operator at the same next step — restart,
    // configure inference, or "not in this deployment" — instead of a bare fail.
    if company.runtime.builder().is_none() {
        use super::inference::RunnerGap;
        return Err(
            match super::inference::runner_gap_for(company.runtime.as_ref()).await {
                RunnerGap::RestartPending => super::restart_required("the workflow copilot").into(),
                RunnerGap::InferenceRequired => {
                    super::inference_required("the workflow copilot").into()
                }
                RunnerGap::NotWired => super::not_wired("the workflow copilot").into(),
            },
        );
    }

    use crate::harness::workflow_build::{
        DescriptionDraftOutcome, draft_workflow_from_description,
    };
    match draft_workflow_from_description(&company.runtime, &description).await {
        Ok(DescriptionDraftOutcome::Graph {
            summary,
            spec,
            notes,
        }) => Ok(Json(DraftFromDescriptionResponse::Drafted {
            automatable: true,
            summary,
            workflow: serde_json::to_value(&spec).unwrap_or(Value::Null),
            notes,
        })),
        Ok(DescriptionDraftOutcome::NotAutomatable(reason)) => {
            Ok(Json(DraftFromDescriptionResponse::NotAutomatable {
                automatable: false,
                reason,
            }))
        }
        // A read the drafter could not proceed without (the company record) — a
        // genuine 500, not a model answer.
        Err(err) => Err(ApiError(err).into_response().into()),
    }
}

/// `POST …/workflows/draft-from-description` on a build with no harness. The
/// copilot needs the embedded brain, so it answers `not_wired` — the same 404
/// the run route's default-build arm gives. The empty-description 400 still runs
/// first, so the contract's shape is identical across builds.
#[cfg(not(feature = "openhuman"))]
async fn draft_from_description(
    company: ScopedCompany,
    Json(body): Json<DraftFromDescriptionBody>,
) -> Result<Json<DraftFromDescriptionResponse>, crate::server::Rejection> {
    let _ = &company;
    if body.description.trim().is_empty() {
        return Err(ApiError(OpenCompanyError::InvalidRequest(
            "describe the workflow you want in a sentence or two.".to_string(),
        ))
        .into_response()
        .into());
    }
    Err(super::not_wired("the workflow copilot").into())
}

// ---------------------------------------------------------------------------
// Fix a failed run with the copilot (issue #840, PR-3)
// ---------------------------------------------------------------------------

/// The `POST …/workflows/{wid}/fix-from-run` body (issue #840, PR-3): the failed
/// run to correct from, plus an optional caller-supplied error hint for a run the
/// journal never recorded a failure for (or that predates the failure journal).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixFromRunBody {
    /// The failed run's correlation id — the `runId` the run-history row carries.
    run_id: String,
    /// The run's error as the row already shows it, used only when the journal has
    /// no `WorkflowRunFinished{error}` for `run_id` to read.
    #[serde(default)]
    error_hint: Option<String>,
}

/// The static authoring readiness of a corrected graph (issue #840, PR-3) —
/// **advisory only**. `ok` is whether the always-compiled tinyflows authoring
/// gates found nothing; `advisories` names each remaining smell for the operator
/// to look at before saving. It NEVER blocks the save (Save is still the only
/// write), so a non-`ok` readiness rides a 200 alongside the corrected graph.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(feature = "openhuman"), allow(dead_code))]
struct ReadinessNote {
    ok: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    advisories: Vec<String>,
}

/// The fix-from-run answer (issue #840, PR-3), mirroring
/// [`DraftFromDescriptionResponse`]: **200 in both model-answer cases** — a
/// corrected graph to review, or an honest "this cannot be fixed by re-wiring".
/// Only a request problem (no error to fix from → 400) or a capability gap (no
/// brain wired → 404/409) is a non-2xx.
#[derive(Debug, Serialize)]
#[serde(untagged)]
// Only the `openhuman` arm constructs these variants; the default build's
// `not_wired` arm returns the type without building either. Live under the feature
// CI builds and tests, so this is a cfg artefact, not a dead type.
#[cfg_attr(not(feature = "openhuman"), allow(dead_code))]
enum FixFromRunResponse {
    /// A corrected graph for the edit dialog to hydrate, with the static readiness
    /// advisories over it. `workflow` is a `WorkflowGraphSpec` — the same camelCase
    /// node/edge shape the read routes return, and it keeps the SAME id as `wid` so
    /// the operator's Save is a new version, not an orphan.
    Fixed {
        automatable: bool,
        summary: String,
        workflow: Value,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        notes: Vec<String>,
        readiness: ReadinessNote,
    },
    /// The failure could not be fixed by re-wiring the graph with the teammates
    /// and tools available; `reason` says why.
    NotAutomatable { automatable: bool, reason: String },
}

/// The failure a past run recorded, scanned out of the company journal for a
/// `run_id` (issue #840, PR-3).
#[cfg(feature = "openhuman")]
struct JournaledFailure {
    /// The run's error, when it failed outright. `None` for a run that finished
    /// clean (fixing which makes no sense unless the caller passes a hint).
    error: Option<String>,
    /// The id of the node whose step errored, when the per-node trail named one.
    failed_node_id: Option<String>,
}

/// Scans the company journal for what run `run_id` recorded (issue #840, PR-3).
/// `None` means no `WorkflowRunFinished` for that id exists — the caller falls back
/// to a caller-supplied hint. Follows the same whole-log fold `list_runs` uses.
#[cfg(feature = "openhuman")]
async fn journaled_run_failure(
    company: &ScopedCompany,
    run_id: &str,
) -> Result<Option<JournaledFailure>, ApiError> {
    let stored = company
        .runtime
        .events()
        .read_from(company.id(), EventSeq::new(0), usize::MAX)
        .await
        .map_err(ApiError)?;

    let mut failed_node_id: Option<String> = None;
    // `Some(error)` once the run's finish is seen; the outer Option distinguishes
    // "the run finished (maybe cleanly)" from "no finish for this id at all".
    let mut finished: Option<Option<String>> = None;
    for stored in stored {
        match stored.event {
            CompanyEvent::WorkflowNodeFinished {
                run_id: rid,
                node_id,
                status,
                ..
            } if rid == run_id && status == WorkflowNodeStatus::Error => {
                failed_node_id = Some(node_id);
            }
            CompanyEvent::WorkflowRunFinished {
                run_id: Some(rid),
                error,
                ..
            } if rid == run_id => {
                finished = Some(error);
            }
            _ => {}
        }
    }
    Ok(finished.map(|error| JournaledFailure {
        error,
        failed_node_id,
    }))
}

/// Resolves the error + failing node a fix should be grounded on from what the
/// journal recorded and what the caller hinted (issue #840, PR-3). `None` means
/// there is nothing to fix from: neither a journaled error nor a usable hint.
///
/// A pure decision, factored out of [`fix_from_run`] so the fallback matrix — a
/// journaled error, a hint fallback, a clean run with no hint — is unit-testable
/// without a running host.
#[cfg(feature = "openhuman")]
fn resolve_fix_error(
    journaled: Option<JournaledFailure>,
    hint: Option<String>,
) -> Option<(String, Option<String>)> {
    let (error, failed_node_id) = match journaled {
        Some(j) => (j.error.or(hint), j.failed_node_id),
        // No finish for this run id in the journal — lean entirely on the hint.
        None => (hint, None),
    };
    let error = error.filter(|e| !e.trim().is_empty())?;
    Some((error, failed_node_id))
}

/// `POST …/workflows/{wid}/fix-from-run` (both scope forms) — correct a saved
/// workflow whose run failed, with the create-time copilot (issue #840, PR-3).
/// Drafts a corrected graph and hands it back for the edit dialog to hydrate; it
/// never persists, so Save (`PUT …/workflows/{wid}`) stays the only write, and the
/// corrected graph keeps the same id so Save is a new version of the workflow.
#[cfg(feature = "openhuman")]
async fn fix_from_run(
    company: ScopedCompany,
    Path(WorkflowPath { wid }): Path<WorkflowPath>,
    Json(body): Json<FixFromRunBody>,
) -> Result<Json<FixFromRunResponse>, crate::server::Rejection> {
    // `wid` becomes a filename on the read below — reject anything that could
    // escape `workflows/`.
    if !safe_wid(&wid) {
        return Err(OpenCompanyError::NotFound(format!("workflow {wid}")).into());
    }

    // No builder wired: classify WHY exactly as the draft + run routes do (issues
    // #266, #514), so the console points the operator at the same next step.
    if company.runtime.builder().is_none() {
        use super::inference::RunnerGap;
        return Err(
            match super::inference::runner_gap_for(company.runtime.as_ref()).await {
                RunnerGap::RestartPending => super::restart_required("the workflow copilot").into(),
                RunnerGap::InferenceRequired => {
                    super::inference_required("the workflow copilot").into()
                }
                RunnerGap::NotWired => super::not_wired("the workflow copilot").into(),
            },
        );
    }

    // Load the saved graph for `wid` (seed ∪ overlay) and convert it to the spec
    // the copilot corrects and pins its identity to.
    let (overlays, globals_disable) = overlay_workflows_and_globals(&company).await?;
    // A source-defined workflow (seed-backed, or seed-shadowed) can never take
    // the correction: `PUT …/workflows/{wid}` refuses it with the same 409 this
    // mirrors (`locate_editable_overlay`). Catching it here — before the copilot
    // turn — saves the tokens and the wait on a proposal the operator could never
    // save; without this a tinysweeper review flagged the route as misleading the
    // operator into drafting a fix it would then refuse.
    if !is_editable(company.runtime.source_dir(), &overlays, &wid) {
        return Err(ApiError(OpenCompanyError::Conflict(format!(
            "workflow `{wid}` is defined by a file in the company source tree, so a copilot fix \
             can't be saved for it. Edit `workflows/{wid}.toml` in the company repository instead."
        )))
        .into_response()
        .into());
    }
    let file = load_workflow_with_globals(
        company.runtime.source_dir(),
        &overlays,
        &globals_disable,
        &wid,
    )?
    .ok_or_else(|| OpenCompanyError::NotFound(format!("workflow {wid}")))?;
    // `workflow_spec_from_graph` below has no `on_error`/`retry`/`repeatable`/
    // `postcondition`/`verify` fields on `WorkflowNodeSpec` (the builder never
    // authors them), so a node that had any of
    // them set loses it silently once the operator saves the correction.
    // `repeatable` is the safety declaration issue #850 exists to protect;
    // `postcondition` (issue #1866) is the deterministic run-safety gate —
    // a correction that drops either with no warning can leave a
    // continuation free to replay a call its author explicitly marked
    // non-repeatable, or let an insufficient output flow downstream past a
    // gate the operator declared. Correlating this policy across a copilot
    // rewrite that may rename or drop nodes is the harder problem this PR
    // does not take on; naming it in a note at least makes the loss visible
    // instead of silent.
    let dropped_policy_nodes: Vec<String> = file
        .nodes
        .iter()
        .filter(|n| {
            n.on_error.is_some()
                || n.retry.is_some()
                || n.repeatable.is_some()
                || n.postcondition.is_some()
                || n.verify.is_some()
        })
        .map(|n| n.name.clone())
        .collect();
    let spec = crate::company::workflow_spec_from_graph(file);

    // The failure to correct from: prefer what the run journaled, fall back to the
    // caller's hint. Neither → there is nothing to fix from.
    let journaled = journaled_run_failure(&company, &body.run_id).await?;
    let hint = body
        .error_hint
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string);
    let Some((error, failed_node_id)) = resolve_fix_error(journaled, hint) else {
        return Err(ApiError(OpenCompanyError::InvalidRequest(
            "this run recorded no error to fix from — reopen the run, or pass its error as a hint."
                .to_string(),
        ))
        .into_response()
        .into());
    };
    // The journal names a node id; the human-readable name comes from the saved
    // graph the id belongs to.
    let failed_node_name = failed_node_id
        .as_deref()
        .and_then(|id| spec.nodes.iter().find(|n| n.id == id))
        .map(|n| n.name.clone());

    use crate::harness::workflow_build::{
        DescriptionDraftOutcome, RunFailureContext, fix_workflow_from_failure, workflow_readiness,
    };
    let failure = RunFailureContext {
        run_id: body.run_id.clone(),
        error,
        failed_node_id,
        failed_node_name,
    };
    match fix_workflow_from_failure(&company.runtime, &spec, &failure).await {
        Ok(DescriptionDraftOutcome::Graph {
            summary,
            spec,
            mut notes,
        }) => {
            let (ok, advisories) = workflow_readiness(&spec);
            if !dropped_policy_nodes.is_empty() {
                notes.push(format!(
                    "on_error/retry/repeatable/postcondition/verify on {} — this correction does not \
                     carry these per-node policies through; reapply them after reviewing if the \
                     node is still there.",
                    dropped_policy_nodes.join(", ")
                ));
            }
            Ok(Json(FixFromRunResponse::Fixed {
                automatable: true,
                summary,
                workflow: serde_json::to_value(&spec).unwrap_or(Value::Null),
                notes,
                readiness: ReadinessNote { ok, advisories },
            }))
        }
        Ok(DescriptionDraftOutcome::NotAutomatable(reason)) => {
            Ok(Json(FixFromRunResponse::NotAutomatable {
                automatable: false,
                reason,
            }))
        }
        // A read the drafter could not proceed without — a genuine 500.
        Err(err) => Err(ApiError(err).into_response().into()),
    }
}

/// `POST …/workflows/{wid}/fix-from-run` on a build with no harness (issue #840,
/// PR-3). The copilot needs the embedded brain, so it answers `not_wired` — the
/// same 404 the draft route's default-build arm gives.
#[cfg(not(feature = "openhuman"))]
async fn fix_from_run(
    company: ScopedCompany,
    Path(WorkflowPath { wid }): Path<WorkflowPath>,
    Json(body): Json<FixFromRunBody>,
) -> Result<Json<FixFromRunResponse>, crate::server::Rejection> {
    let _ = (&company, &wid, &body.run_id, &body.error_hint);
    Err(super::not_wired("the workflow copilot").into())
}

/// The `GET …/workflows/tool-slugs` answer (issues #783, #874): the tools the
/// per-workflow copilot may ground a proposal on, and — separately — the ones
/// this company holds a grant for that cannot run on this deployment.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowToolSlugsResponse {
    /// The **effective** slugs: granted by `[tools].allow` *and* wired here, so
    /// a proposed `tool_call` naming one has a chance of running. This is the
    /// only list a prompt should be grounded on.
    slugs: Vec<String>,
    /// Granted, but not wired on this deployment (issue #874). Reported rather
    /// than silently dropped so a reader can tell "this company is not allowed
    /// that tool" (absent from both lists) from "allowed, nobody has configured
    /// the provider yet" (here) — and so authoring ahead of wiring, which
    /// create validation still permits, remains a visible option.
    ///
    /// Empty when the wiring is not knowable (no harness deps attached): the
    /// honest answer to "which of these are unwired" is then "cannot say", and
    /// `slugs` degrades to the grant-only set rather than emptying out.
    unwired: Vec<UnwiredWorkflowTool>,
}

/// One granted-but-unwired tool, with the reason it cannot run here (issue
/// #874) — the same distinction
/// [`refusal_for`](crate::workflows::caps) draws at run time, moved forward to
/// the moment the console asks, instead of arriving as a failed run.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnwiredWorkflowTool {
    slug: String,
    /// A stable machine token for a client that wants to branch:
    /// `searchBackendNotConfigured`, `capabilityTierFiltered`, or the
    /// cause-less `unwired`. Treat it as open — a new deployment-wiring cause
    /// adds a token here, so match the ones you handle and fall back to
    /// [`detail`](Self::detail) rather than assuming the set is closed.
    reason: &'static str,
    /// The same sentence in prose, for a client that just wants to show it.
    detail: &'static str,
}

/// `GET …/workflows/tool-slugs` (both scope forms) — the per-workflow copilot's
/// tool grounding (issue #783), narrowed to the **effective** set by issue #874.
///
/// Answers what
/// [`workflow_effective_tool_slugs`](crate::company::workflow_effective_tool_slugs)
/// computes — catalogue, company grant and deployment wiring all agreeing — so
/// this route and the in-process create/fix copilot
/// (`crate::harness::workflow_build`) ground on one set and cannot drift.
///
/// It deliberately does **not** answer the wider *grant-only* set that
/// create/save validation accepts. That gate stays permissive on purpose so an
/// operator may author now and wire the provider later, and this route does not
/// change it. Serving the grant-only set here was issue #874 — a
/// granted-but-unwired `web_search` was offered to the copilot, which authored a
/// node that failed at the first run.
#[cfg(feature = "openhuman")]
async fn workflow_tool_slugs(
    company: ScopedCompany,
) -> Result<Json<WorkflowToolSlugsResponse>, crate::server::Rejection> {
    let record = company
        .runtime
        .store()
        .load(company.runtime.id())
        .await
        .map_err(|err| ApiError(err).into_response())?
        .ok_or_else(|| OpenCompanyError::CompanyNotFound(company.runtime.id().to_string()))?;
    // `None` — no harness deps on this runtime — means the wiring is unknowable,
    // not that nothing is wired. Both helpers below read it that way: `slugs`
    // falls back to the grant-only set and `unwired` stays empty, which is the
    // pre-#874 answer. That keeps a harness-less host honest instead of telling
    // the copilot every granted tool is broken.
    let wiring = company.runtime.workflow_tool_wiring(&record).await;
    let wired = wiring.as_ref().map(|w| &w.wired_namespaces);
    let unwired = crate::company::workflow_granted_but_unwired_tool_slugs(&record, wired)
        .into_iter()
        .map(|slug| {
            // Every slug here came out of `WORKFLOW_TOOL_CATALOG`, whose entries
            // are pinned to `namespace_of`, and it is unwired precisely because
            // its namespace is in `missing` — so both lookups resolve and `None`
            // is unreachable in practice.
            //
            // Matched EXHAUSTIVELY rather than with a catch-all: the whole point
            // of this field is letting an operator tell one cause from another,
            // so a third `MissingReason` must break the build here instead of
            // compiling into "raise your capability tier" — advice that would be
            // actively wrong for a cause that is not tier filtering. It also
            // keeps the defensive `None` from being conflated with the tier case.
            let missing = crate::workflows::caps::workflow_tool_info(&slug)
                .map(|info| info.namespace)
                .and_then(|ns| wiring.as_ref().and_then(|w| w.missing.get(ns)).copied());
            let (reason, detail) = match missing {
                Some(crate::workflows::caps::MissingReason::SearchBackendNotConfigured) => (
                    "searchBackendNotConfigured",
                    "granted, but no managed search backend is configured on this deployment; \
                     ask the platform operator to configure search",
                ),
                Some(crate::workflows::caps::MissingReason::CapabilityTierFiltered) => (
                    "capabilityTierFiltered",
                    "granted, but the deployment's capability tier filtered it; ask the platform \
                     operator to raise the capability tier",
                ),
                // Unreachable given the pairing above; answered honestly rather
                // than guessing a cause we do not have.
                None => (
                    "unwired",
                    "granted, but not wired on this deployment; ask the platform operator why",
                ),
            };
            UnwiredWorkflowTool {
                slug,
                reason,
                detail,
            }
        })
        .collect();
    Ok(Json(WorkflowToolSlugsResponse {
        slugs: crate::company::workflow_effective_tool_slugs(&record, wired),
        unwired,
    }))
}

/// `GET …/workflows/tool-slugs` on a build with no harness. The workflow tool
/// surface lives behind the `openhuman` feature, so a default build wires no
/// `tool_call` grants at all: the honest answer is an empty list, not a 404 —
/// the copilot then grounds on "no tools" rather than being unable to tell.
#[cfg(not(feature = "openhuman"))]
async fn workflow_tool_slugs(
    company: ScopedCompany,
) -> Result<Json<WorkflowToolSlugsResponse>, crate::server::Rejection> {
    let _ = &company;
    Ok(Json(WorkflowToolSlugsResponse {
        slugs: Vec::new(),
        unwired: Vec::new(),
    }))
}

/// The `GET …/workflows/wired-channels` answer (issue #813): the chat channel
/// ids this running company can actually deliver to, so the output-node
/// destination editor offers a picker of real targets. Not feature-gated — the
/// channel set exists on every build.
///
/// **`operator` is always among them** (issue #1757; previously excluded per
/// issue #981, when the in-memory `operator` adapter had no durable reader and
/// workflow delivery refused it by name). The built-in Operator channel is now
/// a durable, journal-backed delivery target present on every running company,
/// so it is a real entry in this picker like any other — never doubled, even
/// when a grandfathered manifest desk also claims the literal id `operator`
/// (`CompanyRuntime::deliverable_channel_ids` dedupes). An empty list is still
/// a truthful answer for everything else: a company with no desks and no
/// provider channels has nowhere else to deliver.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WiredChannelsResponse {
    /// The channel ids an `output` node's `channel` destination may target.
    /// Anything else is rejected when the workflow is saved, and — for a graph
    /// saved before the desk went away — fails at delivery with
    /// `ChannelNotWired`.
    channels: Vec<String>,
}

/// `GET …/workflows/wired-channels` (both scope forms). Reads the running
/// company's deliverable channels directly — infallible, no record load needed.
async fn workflow_wired_channels(company: ScopedCompany) -> Json<WiredChannelsResponse> {
    Json(WiredChannelsResponse {
        channels: company.runtime.deliverable_channel_ids(),
    })
}

// ---------------------------------------------------------------------------
// Run history (issue #228)
// ---------------------------------------------------------------------------

/// How many run outcomes `GET …/workflows/runs` returns when the caller names
/// no `?limit=`, and the ceiling it clamps a larger one to. The console's
/// history panel shows a short recent list; a bigger page would only make the
/// journal fold slower for no one's benefit.
const DEFAULT_RUN_LIMIT: usize = 20;
const MAX_RUN_LIMIT: usize = 200;

/// The journal page size [`list_runs`] walks backward in (issue #1012). Same
/// magnitude and rationale as `EVENT_PAGE` in
/// [`history_for_desk`](crate::server::chat_history::history_for_desk):
/// walking backward in fixed-size pages keeps the newest `limit` runs without
/// ever materialising the unrelated journal beyond them.
const RUN_EVENT_PAGE: usize = 512;

/// The `?workflow=` / `?limit=` / `?before_seq=` selectors on the run-history
/// read.
#[derive(Debug, Deserialize)]
struct RunsQuery {
    /// Return only runs of this workflow id. Absent = every workflow.
    workflow: Option<String>,
    /// Cap the page. Clamped to [`MAX_RUN_LIMIT`]; `0` falls back to the
    /// default rather than returning an empty page, which is never what a
    /// caller means.
    limit: Option<usize>,
    /// Opaque pagination cursor (issue #1012): only runs whose displayed
    /// `seq` is strictly less than this are considered. Absent reads the
    /// newest page. Same `before_seq` shape
    /// [`chat_history`](crate::server::chat_history)'s `?before=` and
    /// `TaskDetailQuery`'s `?discussionBefore=` already use for the same
    /// problem.
    ///
    /// **What to pass is the boundary the previous response issued** —
    /// [`WorkflowRunsResponse::next_before_seq`] — not "the `seq` of the oldest
    /// run you hold". Those two coincided while the page was cut in display
    /// order; they no longer do, because the cut is keyed on `seq` and the
    /// display is keyed on `(at_millis, seq)`. Deriving the cursor from the
    /// last displayed row re-opens the hole this parameter's own page cut
    /// closes: see [`select_run_page`].
    before_seq: Option<u64>,
}

/// `skip_serializing_if` predicate for a count that is almost always zero —
/// the same one `crate::ports::workflow_runner` uses for the per-node
/// `unparkable` / `stranded` pair, so the two counts are omitted on identical
/// terms.
fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// One finished run as the console's history panel renders it (camelCase).
///
/// `pub(crate)` since issue #1859: [`fold_run_events`] and the fields below are
/// the journal-fold surface the `read_run` orchestrator tool summarizes a
/// workflow run's verdict/nodes/pending-approvals from — see that tool's
/// doc comment for why it reads this rather than the full console history
/// route's shape.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowRunOutcome {
    /// The journal sequence position — a stable, monotonic row key.
    seq: u64,
    /// Epoch-millis the outcome was journaled.
    pub(crate) at_millis: u64,
    pub(crate) workflow_id: String,
    /// Whether a cron started this run rather than an operator. The console
    /// shows the distinction because a scheduled run is the one nobody watched.
    scheduled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume_semantic: Option<crate::ports::ResumeSemantic>,
    /// The same delivery rows a manual run's response carries — including
    /// `target`, which the run response already ships to this same console.
    deliveries: Vec<crate::ports::DeliveryReport>,
    pub(crate) pending_approvals: Vec<String>,
    /// Set when the run failed outright instead of finishing with rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// Per-node progress for this run, in the order the nodes finished (issue
    /// #371). Empty for a run journaled before #371, and for one whose nodes all
    /// failed to journal — so an empty list means "no per-node trail", never
    /// "the run did nothing".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) nodes: Vec<WorkflowRunNode>,
    /// The nodes this run has *begun* executing, in start order (issue #1010),
    /// folded from `WorkflowNodeStarted` (issue #382).
    ///
    /// The half of the trail the fold never carried. `nodes` is written by the
    /// *finish* bracket, so a run in flight came back listing only what was
    /// already over — and a console joining mid-run (a reload, a cron fire, an
    /// `EventSource` reconnect, or simply switching workflow and back) could
    /// render the graph's past but never the node executing right now. The
    /// engine has reported the opening bracket since #382; nothing read it.
    ///
    /// A **receipt of what started**, kept once the run settles rather than
    /// cleared: an id here with no matching `nodes` row on a settled run is the
    /// node the run was standing on when it was cancelled or lost, which is the
    /// one thing neither list says on its own. Consumers must therefore pair it
    /// with [`running`](Self::running) before painting anything as in-flight —
    /// see `statesFromRun` in the console.
    ///
    /// Omitted when empty, like `nodes` — which is every run journaled before
    /// #382 and every run whose nodes all failed to journal a start.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    started_nodes: Vec<String>,
    /// When the run *started*, from its `WorkflowRunStarted` row (issue #371).
    /// Absent on a pre-#371 row, whose only timestamp is the finish.
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at_millis: Option<u64>,
    /// `true` for a run that has started and not yet settled.
    ///
    /// Honest rather than optimistic, and only because of the boot sweep: a run
    /// whose host died is settled with an "interrupted" outcome at the next
    /// start, so nothing sits here spinning forever. Omitted when false, which
    /// keeps every settled row's wire shape as short as it was.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) running: bool,
    /// `true` for a run an operator stopped (issue #383).
    ///
    /// Separate from [`error`](Self::error) because it is a separate outcome: a
    /// cancelled run carries no error, so a console reading only `error` would
    /// render a deliberate stop as a clean success. Together with `error` these
    /// give the three terminal readings the history panel distinguishes —
    /// failed, interrupted by a host restart, stopped by an operator. Omitted
    /// when false, like `running`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) cancelled: bool,
    /// System notices raised about this run (issue #638) — today, that a node
    /// gated more tool calls than the per-batch cap allows and the excess was
    /// discarded.
    ///
    /// Not an `error`: the run succeeded. The history panel renders these as a
    /// warning rather than a failure, so a run that overflowed still reads as
    /// the success it was, with something the operator needs to know attached.
    /// Omitted when empty, like `nodes` — which is nearly every run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notices: Vec<String>,
    /// The board writes this run's agent nodes performed (issue #661 / M5) — one
    /// row per card opened or re-owned.
    ///
    /// The port row is projected **verbatim** rather than reshaped: it is already
    /// camelCase and already structural (see
    /// [`WorkflowRunBoardRow`](crate::ports::WorkflowRunBoardRow)), so a second
    /// transcription here would only be a place for the journal's shape and the
    /// console's to drift apart. Same choice `deliveries` makes one field up.
    ///
    /// Omitted when empty, like `notices` — which is nearly every run.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    board: Vec<crate::ports::WorkflowRunBoardRow>,
    /// The nodes this run blocked on a human (issue #881) — one row per node
    /// whose agent turn had a tool call parked, so it produced no deliverable
    /// and nothing after it ran.
    ///
    /// Projected **verbatim** from the port row, like `board` and `deliveries`
    /// above: it is already camelCase and already structural, and a second
    /// transcription is only a place for the two shapes to drift.
    ///
    /// This is what stops a blocked run reading as a clean one. Its nodes'
    /// rows arrive relabelled too — see the settle arm in [`list_runs`], which
    /// flips each blocked node's journaled `error` status to `blocked`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) blocked_nodes: Vec<crate::ports::WorkflowBlockedNode>,
    /// The approvals this run parked (issue #880) — a receipt of what it
    /// opened, the failed parks included.
    ///
    /// Named for what the run *parked*, never for what is still outstanding: a
    /// receipt cannot go stale, whereas a settle-time "still waiting on N"
    /// count becomes a fresh lie the moment somebody approves one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    approvals: Vec<crate::ports::WorkflowRunApprovalRow>,
    /// The run retained a degraded node fact even when the progress collector
    /// could not return its rows. This is a conservative read-side fact: a
    /// failed drain must never turn a known non-clean run into `ok`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    degraded: bool,
    /// How many of [`pending_approvals`](Self::pending_approvals) have **no
    /// live card left in the queue** (issue #1189).
    ///
    /// The gate-shaped sibling of `blockedNodes[].stranded`, and the number the
    /// console needs to stop telling an operator to go and decide something
    /// that is not there. #1143's per-node count is keyed on approval ids, and
    /// a gate parked by `park_pending_gates` records none — no approval-row
    /// receipt and no blocked-node row, only its node id here — so that count
    /// structurally cannot describe this shape. On the marketing tenant it is
    /// the shape of 34 of 60 runs.
    ///
    /// **Computed on the read, never journaled**, on exactly the terms
    /// `WorkflowBlockedNode::stranded` is: the journal records what happened and
    /// is not edited to reflect what is true now, and a stored "still waiting"
    /// count is a fresh lie the moment the queue moves. Skipped when zero, which
    /// is every healthy run, so an unaffected row's wire shape is unchanged.
    #[serde(skip_serializing_if = "is_zero")]
    stranded_approvals: usize,
    /// What this run adds up to, in one word (issue #981).
    ///
    /// **Always serialized**, and **derived in a single pass** once the fold
    /// and the issue-#1009 settle below have finished — see
    /// [`WorkflowRunOutcome::derive_verdict`]. The value the two construction
    /// sites give it is overwritten there, which is deliberate: the settle arm
    /// rewrites `running`, `error` and the blocked relabelling *after* a row is
    /// pushed, so a verdict computed at construction would be a stale reading
    /// of a row that has since changed underneath it.
    ///
    /// Derived, never journaled. Runs already in a company's history therefore
    /// re-score on deploy with no migration — and, as issue #981 notes, anyone
    /// counting successful runs off this endpoint will see their rate drop with
    /// no change in behaviour, because the dropped reports were always there.
    pub(crate) verdict: WorkflowRunVerdict,
}

impl WorkflowRunOutcome {
    /// Reads this row's verdict off the fields it already carries (issue #981).
    ///
    /// Called once per row, in a pass over the whole page, after everything
    /// that can still change its inputs has run. See [`Self::verdict`].
    ///
    /// `pub(crate)` since issue #1859: [`fold_run_events`] never calls this
    /// itself (the field it derives from the `#1189` stranded-approvals join
    /// runs *after* the fold, in [`list_runs`]), so `read_run` — which folds
    /// a single run straight out of the journal with no such join — calls
    /// this explicitly rather than trusting the placeholder [`Self::verdict`]
    /// the fold construction sites leave on the row (always
    /// [`WorkflowRunVerdict::Running`], since the fold never resolves it).
    /// `read_run` accepts the one gap this leaves: without the live-queue
    /// join, a run whose only remaining approval was orphaned reads as
    /// `AwaitingApproval` rather than `Stranded` — a lesser distinction for a
    /// chat answer than for the console's own history panel.
    pub(crate) fn derive_verdict(&self) -> WorkflowRunVerdict {
        WorkflowRunVerdict::of(RunVerdictFacts {
            running: self.running,
            error: self.error.as_deref(),
            cancelled: self.cancelled,
            blocked_nodes: self.blocked_nodes.len(),
            deliveries: &self.deliveries,
            pending_approvals: self.pending_approvals.len(),
            // Issue #1189: filled in by the join below, which is why the verdict
            // pass now runs AFTER it. See `list_runs`.
            stranded_approvals: self.stranded_approvals,
            // Issue #1865: `self.nodes` already carries `relabel_blocked`'s
            // reclassification by the time this runs — see `fold_run_events`'s
            // settle arm — so a row still `Error` here is a genuine one under
            // `on_error: continue|route`.
            //
            // A turn that truncated at the `max_tool_iterations` cap counts
            // here too, and that is newer than this comment's first draft
            // (CodeRabbit review on #1905). The engine reports such a node as
            // `Ok` and the host relabels it at settle, which used to reach only
            // the in-memory `WorkflowRun.nodes` — so a persisted, re-read run
            // undercounted by exactly that case and scored `ok` where the
            // synchronous response said `degraded`. The journal now carries the
            // relabelled status itself (see the progress collector in
            // `workflows::runner`), so both surfaces derive the same verdict
            // from the same fact.
            // Issue #1865: a degraded node fact may be carried separately when
            // progress draining failed, so history does not score the run green.
            errored_nodes: self
                .nodes
                .iter()
                .filter(|n| n.status == WorkflowNodeStatus::Error)
                .count()
                + usize::from(self.degraded),
        })
    }
}

/// One node's outcome inside a run (issue #371).
///
/// Structural only — id, status, duration. The node's own output and error text
/// are deliberately absent from the event this is folded from, so they cannot
/// appear here either.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowRunNode {
    pub(crate) node_id: String,
    pub(crate) status: WorkflowNodeStatus,
    elapsed_ms: u64,
    /// The node's null-resolved config paths (issue #1014) — the engine's own
    /// broken-wiring list, projected verbatim from the port row. Paths only, no
    /// payload: a null resolution has no value, so the console renders *where*
    /// the wiring came up empty and never *what* a node produced.
    ///
    /// `skip_serializing_if` keeps a clean node's row byte-identical to the
    /// pre-#1014 shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    diagnostics: Vec<String>,
}

impl From<crate::ports::WorkflowRunNodeRow> for WorkflowRunNode {
    /// The run-response path (issue #542): the runner hands its per-node trail
    /// back on [`WorkflowRun::nodes`](crate::ports::WorkflowRun) as
    /// [`WorkflowRunNodeRow`](crate::ports::WorkflowRunNodeRow)s, which carry the
    /// same three structural scalars this wire shape does — so the run response
    /// reuses the identical camelCase rows the history route already serves.
    fn from(row: crate::ports::WorkflowRunNodeRow) -> Self {
        Self {
            node_id: row.node_id,
            status: row.status,
            elapsed_ms: row.elapsed_ms,
            // Issue #1014: carry the null-resolved config paths through to the
            // wire shape, like the three structural scalars above.
            diagnostics: row.diagnostics,
        }
    }
}

/// Folds a chronologically-ordered slice of journal rows into per-run outcomes
/// (issue #371's group-by-run fold). Extracted out of [`list_runs`] by issue
/// #1012 so the caller can run it repeatedly over a growing, backward-paged
/// buffer instead of once over an unbounded forward read — see the read loop
/// there.
///
/// Issue #371 turned this from a filter into a **group-by-run fold**: a run
/// now contributes up to N+2 rows (a start, one per node, a finish) instead of
/// one, and they have to come back as a single history entry.
///
/// The invariant that keeps it simple: the journal is append-only and
/// single-writer, so a run's rows are ordered `Started < Node… < Finished` —
/// the runner drains and joins its progress collector before returning, which
/// is what makes the last part true rather than a race. Rows of *different*
/// runs may interleave (two workflows can run at once), so the grouping is
/// keyed on run id rather than on adjacency. **`rows` must be chronologically
/// ordered (ascending `seq`)** for this invariant to hold — a caller reading
/// backward via [`EventLog::read_before`](crate::ports::EventLog::read_before)
/// (which comes back newest-first) must reverse each page before it is folded
/// in.
///
/// A pre-#371 finished row has no run id and no start, so it simply folds to
/// itself — one row in, one entry out, exactly as before. The same shape
/// results for a post-#371 row whose start fell outside `rows` — whether
/// because the caller's window does not reach that far back yet, or because a
/// retention pass pruned the `WorkflowRunStarted` row while keeping its
/// `WorkflowRunFinished` (the two are independently prunable; see
/// `CompanyEvent::retention_class`). Both are "legitimate" in the sense the
/// original comment on this fold already drew: nothing here tells them apart,
/// and nothing needs to — a caller paging backward for more history is exactly
/// how the first case resolves itself into the second, or into a real match.
///
/// Returns the folded runs, in fold/push order (not sorted or truncated — the
/// caller does that), and the highest `seq` seen among EVERY row, matched or
/// not — the `read_through` high-water mark [`list_runs`]'s #1009 cross-check
/// resumes reading from.
///
/// `pub(crate)` since issue #1859: the orchestrator's `read_run` tool reuses
/// this same fold — reading the whole company journal, exactly like
/// [`QueryCompanyTool`](crate::harness::built_in::orchestrator::QueryCompanyTool)
/// already does for its recent-activity section — rather than re-deriving a
/// second, drifting notion of "what a workflow run adds up to". It does not
/// take on `list_runs`'s backward-paging or its #1009 live-run cross-check:
/// a chat tool answering "what happened on this run" tolerates the rare
/// eternal-spinner edge case that cross-check exists for, and paging the
/// whole journal once is the same cost `QueryCompanyTool` already pays.
pub(crate) fn fold_run_events(
    rows: Vec<StoredEvent>,
    wanted: Option<&str>,
) -> (Vec<WorkflowRunOutcome>, u64) {
    let mut runs: Vec<WorkflowRunOutcome> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let matches = |workflow_id: &str| wanted.is_none_or(|w| w == workflow_id);
    // The high-water mark over EVERY row rather than only the matched ones.
    let mut read_through = 0u64;

    for stored in rows {
        let seq = stored.seq.value();
        let at_millis = stored.at_millis;
        read_through = read_through.max(seq);
        match stored.event {
            CompanyEvent::WorkflowRunStarted {
                workflow_id,
                run_id,
                scheduled,
                // The run history fold does not surface who started a run
                // (issue #1862 prerequisite is data-only for now).
                started_by: _,
                resume_semantic,
            } => {
                if !matches(&workflow_id) {
                    continue;
                }
                index.insert(run_id.clone(), runs.len());
                runs.push(WorkflowRunOutcome {
                    // The start's own position and time key the entry. The
                    // finish overwrites `at_millis` below so a settled run still
                    // sorts and displays by when it *ended*, as it always has;
                    // `seq` likewise, so ordering is unchanged for settled runs.
                    seq,
                    at_millis,
                    workflow_id,
                    scheduled,
                    run_id: Some(run_id),
                    resume_semantic,
                    deliveries: Vec::new(),
                    pending_approvals: Vec::new(),
                    error: None,
                    nodes: Vec::new(),
                    // Issue #1010: filled by the `WorkflowNodeStarted` arm
                    // below, as the engine walks the graph.
                    started_nodes: Vec::new(),
                    started_at_millis: Some(at_millis),
                    // Flipped off by the finish. A start that never gets one is
                    // a run in flight — or one the boot sweep has yet to settle.
                    running: true,
                    // Only a finish can say this, so a run still in flight is
                    // never cancelled from the fold's point of view — even one
                    // whose signal has already been fired, because it has not
                    // wound down yet.
                    cancelled: false,
                    notices: Vec::new(),
                    // Only a finish carries these, so a run in flight lists none —
                    // even one whose nodes have already opened cards. The rows
                    // arrive with the settle below.
                    board: Vec::new(),
                    // Issues #881 / #880: same — only a finish carries these.
                    // A run in flight has blocked on nobody *yet*, and any
                    // approval it has already parked is listed once it settles.
                    blocked_nodes: Vec::new(),
                    approvals: Vec::new(),
                    // True of a row that has only a start — and re-derived
                    // anyway by the single pass below, which is what makes it
                    // right for a row the finish or the settle changes.
                    // Issue #1189: filled by the join in the tail, on the rows
                    // actually being returned. Zero here is the honest default —
                    // this row has not been reconciled against the queue yet.
                    stranded_approvals: 0,
                    degraded: false,
                    verdict: WorkflowRunVerdict::Running,
                });
            }
            // Issue #1010: the opening bracket, folded at last. The engine has
            // emitted this since #382 and this fold ignored it, so the only
            // per-node fact the history carried was "finished" — and a console
            // that had to read the history to learn about a run (every console
            // that joined mid-run) could not paint the node executing right
            // now, because nothing on the wire said which one it was.
            //
            // Recorded in start order, and deliberately NOT paired against the
            // finishes here: the subtraction belongs to the reader, which is
            // the only side that knows whether it is drawing a live canvas or a
            // settled run's overlay. See `started_nodes`.
            CompanyEvent::WorkflowNodeStarted {
                workflow_id,
                run_id,
                node_id,
            } => {
                if !matches(&workflow_id) {
                    continue;
                }
                // Same rule the finish arm follows one arm down: a node whose
                // run has no entry — a journal truncated below the start, or a
                // `?workflow=` filter that cannot match — is dropped rather
                // than synthesising a headless run.
                if let Some(entry) = index.get(&run_id).and_then(|i| runs.get_mut(*i)) {
                    entry.started_nodes.push(node_id);
                }
            }
            CompanyEvent::WorkflowNodeFinished {
                workflow_id,
                run_id,
                node_id,
                status,
                elapsed_ms,
                diagnostics,
                // Folded but not yet surfaced on the row: the run-history shape
                // is a stable console contract, so the join rides the SSE frame and
                // `GET {scope}/runs?workflow_run=` until a reader needs it here.
                agent_run_id: _,
            } => {
                if !matches(&workflow_id) {
                    continue;
                }
                // A node whose start is missing (a journal truncated below it,
                // or a `?workflow=` filter that cannot match) has no entry to
                // attach to. Dropped rather than synthesising a headless run.
                if let Some(entry) = index.get(&run_id).and_then(|i| runs.get_mut(*i)) {
                    entry.nodes.push(WorkflowRunNode {
                        node_id,
                        status,
                        elapsed_ms,
                        // Issue #1014: the broken-wiring paths, folded straight
                        // out of the journal so a re-read run shows the same
                        // diagnostics the live run response carried.
                        diagnostics,
                    });
                }
            }
            CompanyEvent::WorkflowRunFinished {
                workflow_id,
                scheduled,
                run_id,
                deliveries,
                pending_approvals,
                error,
                cancelled,
                notices,
                board,
                blocked_nodes,
                approvals,
            } => {
                if !matches(&workflow_id) {
                    continue;
                }
                // Settle the open entry when there is one…
                if let Some(entry) = run_id
                    .as_ref()
                    .and_then(|id| index.get(id))
                    .and_then(|i| runs.get_mut(*i))
                {
                    entry.seq = seq;
                    entry.at_millis = at_millis;
                    entry.deliveries = deliveries;
                    entry.pending_approvals = pending_approvals;
                    entry.error = error;
                    entry.running = false;
                    entry.cancelled = cancelled;
                    entry.notices = notices;
                    entry.board = board;
                    // Issue #881: the node rows for this run were folded from
                    // `WorkflowNodeFinished` events the engine wrote, and the
                    // engine reported a blocked node as `error` — honestly, in
                    // its own terms: the capability really did return an error,
                    // which is what halted the branch. The finish is the first
                    // point that knows *why*, so the relabelling happens here,
                    // on the read, rather than by rewriting the durable node
                    // rows. Same host-side reclassification the run record
                    // itself performs; see `workflows::runner`.
                    relabel_blocked(&mut entry.nodes, &blocked_nodes);
                    entry.blocked_nodes = blocked_nodes;
                    entry.approvals = approvals;
                    entry.degraded = false;
                    continue;
                }
                // …else stand alone. Two ways to get here, both legitimate: a
                // pre-#371 row (no run id, no start), and a #371 row whose start
                // fell off the readable journal. Either way the entry looks
                // exactly like the pre-#371 shape — no nodes, no start time, not
                // running — so old history renders unchanged.
                runs.push(WorkflowRunOutcome {
                    seq,
                    at_millis,
                    workflow_id,
                    scheduled,
                    run_id,
                    resume_semantic: None,
                    deliveries,
                    pending_approvals,
                    error,
                    nodes: Vec::new(),
                    // No start row means no node rows either — of either
                    // bracket (issue #1010).
                    started_nodes: Vec::new(),
                    started_at_millis: None,
                    running: false,
                    cancelled,
                    notices,
                    board,
                    // No start row means no node rows either, so there is
                    // nothing here to relabel — the blocked list is still
                    // carried, because it is the only thing that tells this
                    // orphaned row apart from a clean finish.
                    blocked_nodes,
                    approvals,
                    degraded: false,
                    // Re-derived by the single pass below, like the start arm's.
                    // Issue #1189: filled by the join in the tail, on the rows
                    // actually being returned. Zero here is the honest default —
                    // this row has not been reconciled against the queue yet.
                    stranded_approvals: 0,
                    verdict: WorkflowRunVerdict::Running,
                });
            }
            _ => {}
        }
    }

    (runs, read_through)
}

/// Cuts one page out of the folded run set and issues the cursor the *next*
/// page must be asked for (issue #1012).
///
/// Three quantities the page needs, kept apart because they are not the same
/// number:
///
/// * the **cut** — which `limit` runs this page carries,
/// * the **display order** — how those runs are listed,
/// * the **cursor** — the boundary the next request pages before.
///
/// # Why the cut is keyed on `seq` and the display is not
///
/// The page is cut by `seq` descending, and only then sorted for display by
/// `(at_millis, seq)` descending — issue #228's ordering, kept verbatim, just
/// applied *after* the cut rather than as the cut.
///
/// `seq` is the journal's own append position: monotonic, and the only key
/// [`EventLog::read_before`](crate::ports::EventLog::read_before) can bound a
/// read by. `at_millis` is wall-clock and is **not** monotonic in storage
/// order — a clock step backwards (NTP correction, a VM resume, an operator
/// setting the date) writes a row whose time is older than the row before it.
///
/// Cutting on `(at_millis, seq)` and then paging off the cut's boundary loses
/// runs outright. Take a run `R` appended after this page's boundary run `B`
/// under a regressed clock, so `R.seq > B.seq` but `R.at_millis < B.at_millis`.
/// `R` sorts *below* `B`, so the truncate drops it from this page; the next
/// request asks `before_seq = B.seq`, which excludes every row at or above
/// `B.seq` — `R` among them. `R` is on neither page, and because the boundary
/// only ever descends, on no later page either. It is permanently unreachable,
/// and `has_more` may well be `true` *because* of it — a page the caller can
/// see exists and can never fetch.
///
/// Cutting on `seq` closes that by construction rather than by any argument
/// about how large a clock step can be: the set served here is exactly
/// `{ seq >= next_before_seq }` of the candidates, and the next request asks
/// for `{ seq < next_before_seq }`. The two partition the run set on the same
/// key the read is bounded by, so nothing can fall between them.
///
/// The accepted cost, stated plainly: under a clock regression a run is served
/// on the page its `seq` puts it on — correctly ordered *within* that page, but
/// possibly out of order against the adjacent one. Under a monotonic clock
/// `seq` and `at_millis` order identically and this is indistinguishable from
/// cutting on the tuple. So the degradation fires only on the exact anomaly
/// that previously caused permanent loss, and it degrades to "wrong order at
/// one seam", never to "the run is gone".
///
/// Returns the page, whether an older page exists, and the cursor to ask for it
/// with — `None` when there is no older page, so a caller is never handed a
/// cursor for a page that does not exist.
fn select_run_page(
    mut runs: Vec<WorkflowRunOutcome>,
    limit: usize,
) -> (Vec<WorkflowRunOutcome>, bool, Option<u64>) {
    // The backward-paged read stopped once it had settled at least `limit + 1`
    // runs (or ran out of journal), precisely so this count is known here — one
    // more than fits on the page means there is a page after this one.
    let has_more = runs.len() > limit;
    // The cut: the `limit` highest-`seq` runs.
    runs.sort_by_key(|run| std::cmp::Reverse(run.seq));
    runs.truncate(limit);
    // The cursor: this page's low-water `seq`, which under the cut above is its
    // minimum and NOT the last row in display order — which is why the client
    // can no longer derive it and the host has to say it. Issued only when
    // something is actually behind it.
    let next_before_seq = if has_more {
        runs.iter().map(|run| run.seq).min()
    } else {
        None
    };
    // The display order (issue #228 / #1012): newest FINISH first, on the very
    // pair every row shows. Preserved exactly as merged; it only moves below
    // the cut, and neither the cut nor this sort touches a single field of a
    // row.
    runs.sort_by_key(|run| std::cmp::Reverse((run.at_millis, run.seq)));
    (runs, has_more, next_before_seq)
}

/// Finalizes every returned run's verdict — the last thing `list_runs` does to
/// `runs`, once every reconciliation pass ahead of it (the #1009 cross-check
/// that flips the dead rows to `error: INTERRUPTED_BY_RESTART`, and issue
/// #1189's stranded-approvals join) has settled every row.
///
/// Position is the correctness argument. Every input the verdict reads is
/// written by the settle arm *after* its row was pushed (`running`, `error`,
/// `cancelled`, `deliveries`, `pendingApprovals`, `blockedNodes`), two of them
/// are written again by the cross-check, and `strandedApprovals` is written by
/// the join. Deriving at construction — or, as it was until #1189, before the
/// join — scores the row against inputs that have since moved underneath it:
/// the exact staleness a *stored* verdict would have, reintroduced by
/// placement.
///
/// `run.degraded` is deliberately read exactly as the fold produced it —
/// never re-derived from `run.nodes` here. A node that settled `Error` is
/// already counted once by [`WorkflowRunOutcome::derive_verdict`]'s own scan
/// of `self.nodes` (`errored_nodes: self.nodes.iter().filter(|n| n.status ==
/// Error).count() + usize::from(self.degraded)`); OR-ing that same fact into
/// `degraded`, as an earlier revision of this function did, double-counts it
/// — a run with N errored nodes read as `errored_nodes == N + 1`. The wrong
/// count never surfaced in the verdict itself (`derive_verdict`'s only
/// consumer gates on `errored_nodes > 0`, so N vs. N + 1 picks the same arm),
/// but it did corrupt the serialized `degraded` field, which is documented as
/// carrying one specific fact `run.nodes` cannot: a progress-drain failure, or
/// a capped/budget-paused turn the journal still shows as `ok`. That fact must
/// stay independent of the node-status scan so the two never overlap.
fn settle_history_verdicts(runs: &mut [WorkflowRunOutcome]) {
    for run in runs {
        run.verdict = run.derive_verdict();
    }
}

/// `GET …/workflows/runs?workflow=&limit=&before_seq=` — the company's
/// finished workflow runs, **newest first** (issue #228), a page at a time
/// (issue #1012).
///
/// This is the durable half of the issue: a manual run's delivery rows used to
/// live only in the console drawer until it was dismissed, and a scheduled run's
/// only on host stdout. Folding
/// [`CompanyEvent::WorkflowRunFinished`](crate::ports::types::CompanyEvent) out
/// of the journal makes both survive a console reload, which is the whole point.
///
/// The fold walks the journal **backward**, in bounded
/// [`RUN_EVENT_PAGE`]-sized pages via
/// [`EventLog::read_before`](crate::ports::EventLog::read_before) — the same
/// pattern [`history_for_desk`](crate::server::chat_history::history_for_desk)
/// already uses for a desk transcript, which has the same
/// unbounded-forever-growing-journal problem and answers it the same way.
/// Before #1012 this read all of `read_from(0, MAX)` on every call (a company's
/// *entire* event history, not just its workflow runs — the same call
/// `chat_history` used to make too), which got slower as the journal grew and
/// never stopped growing; the backward-paged walk instead reads only as much
/// of the journal as it takes to answer this page's `limit`, plus one extra run
/// to know whether there is more (`hasMore`).
///
/// A run is a *group* of events (`Started`, N × node events, `Finished`), not
/// one — so a page of raw events is not a page of runs. See the loop below and
/// [`fold_run_events`]'s doc for how a bracket split across a page boundary is
/// handled.
async fn list_runs(
    company: ScopedCompany,
    Query(query): Query<RunsQuery>,
) -> Result<Json<WorkflowRunsResponse>, ApiError> {
    let limit = match query.limit {
        Some(0) | None => DEFAULT_RUN_LIMIT,
        Some(n) => n.min(MAX_RUN_LIMIT),
    };
    // The `?workflow=` filter is applied per event rather than after the `limit`
    // cut, so asking for one workflow returns that workflow's most recent N —
    // not "whichever of the last N happen to match".
    let wanted = query.workflow.as_deref();

    // A run counts as ready to answer with once its `Started` row has been
    // found (full data — `startedNodes`, `nodes`, `startedAtMillis` — is then
    // known), or it has no run id at all (a pre-#371/gapped orphan finish,
    // complete by definition — nothing more to wait for). A run whose
    // `Finished`/node row has been seen but whose `Started` has not (yet) is
    // still open: walking backward means its `Started` row, if it exists, is
    // further back than what has been read so far.
    let is_settled =
        |run: &WorkflowRunOutcome| run.run_id.is_none() || run.started_at_millis.is_some();

    let mut cursor = query.before_seq.map(EventSeq::new);
    let mut buffer: Vec<StoredEvent> = Vec::new();
    let mut runs: Vec<WorkflowRunOutcome> = Vec::new();
    let mut read_through = 0u64;
    // Whether the walk reached the true beginning of the journal — the only
    // condition under which an open (not-yet-settled) run can be trusted as
    // permanently orphaned rather than merely not-yet-resolved. See the
    // `retain` below. No placeholder initial value: every path out of the loop
    // assigns it before breaking, so the compiler can already prove it is set
    // by the time it is read after the loop. `mut` because the loop can
    // reassign it once per page before the page that finally breaks out.
    let mut exhausted;
    loop {
        let page = company
            .runtime
            .events()
            .read_before(company.id(), cursor, RUN_EVENT_PAGE)
            .await
            .map_err(ApiError)?;
        if page.is_empty() {
            exhausted = true;
            break;
        }
        // A page shorter than asked-for proves there is nothing older left to
        // read, without waiting for one more round trip that would only
        // confirm it empty.
        exhausted = page.len() < RUN_EVENT_PAGE;
        // `read_before` returns newest-first; its own last element is this
        // page's oldest row, and the correct cursor to resume strictly before.
        cursor = page.last().map(|event| event.seq);
        let mut chrono_page = page;
        chrono_page.reverse();
        buffer.splice(0..0, chrono_page);

        let (folded, through) = fold_run_events(buffer.clone(), wanted);
        // `read_through`'s meaning — the high-water mark of a "now" snapshot,
        // which the #1009 cross-check below resumes reading from — is fixed
        // by the FIRST page: `fold_run_events` computes it as the buffer's max
        // `seq`, and the buffer only grows *older* on every later page, so
        // this value cannot change after the first assignment. Reassigning it
        // unconditionally is simplest and gives the identical answer.
        read_through = through;
        let settled = folded.iter().filter(|run| is_settled(run)).count();
        runs = folded;
        if exhausted || settled > limit {
            break;
        }
    }
    if !exhausted {
        // Drop any run still open: walking further back might yet resolve it
        // (find its `Started` row) or might not, and returning it now, in the
        // fold's orphan placeholder shape, would risk showing a real run's
        // history as gapped when the only reason it looks that way is that
        // this page chose not to read far enough. Once the journal actually IS
        // exhausted, every remaining open row is a genuine orphan — see
        // `fold_run_events`'s doc — and is kept exactly as the fold shaped it.
        runs.retain(is_settled);
    }

    // Issue #1012: this cross-check only makes sense against "now" — an
    // older page (a `before_seq` cursor was given) is not the newest state,
    // so a `running: true` row on it is out of scope here: either it was
    // already resolved by an earlier newest-page read (whose synthetic
    // finish will surface naturally once an older page's window reaches
    // that seq), or it is a genuinely long-lived run outside what
    // pagination is meant to answer.
    if query.before_seq.is_none() {
        // Issue #1009: cross-check the still-`running` rows against the live run set
        // and settle the ones nobody is running.
        //
        // The fold above marks a start with no finish `running: true`, which is only
        // ever settled by the boot sweep ([`sweep_interrupted_runs`]). Three ways a
        // finish never lands — a task that panicked, an append that failed, a host
        // that died — therefore all read as an eternal spinner *until the next host
        // restart*, with a Stop button that cannot help and a 2s console poll that
        // never stops. This closes the gap between restarts: any run the fold thinks
        // is in flight whose id is **absent** from the supervisor's live set has no
        // task behind it here and now, so it is journaled a synthetic finish (the
        // same `INTERRUPTED_BY_RESTART` the boot sweep uses) and flipped in the
        // in-memory row, so this very response is already self-consistent.
        //
        // Keyed strictly on `live()` membership. A run the current process is
        // genuinely running is registered there and is left untouched — the watchdog
        // (issue #1009, path A) is what guarantees a *panicking* run never reaches
        // this predicate, because it journals its own finish before its guard drops.
        //
        // The one accepted false positive: a run that survived a live
        // `rebuild_company` swap is registered on the *old* supervisor and so is
        // absent from the successor's `live()`, so this could settle a run that is
        // still walking its graph. Accepted because (i) the watchdog keeps panics out
        // of this path entirely, (ii) that run's real finish lands later in journal
        // order and wins the read's last-writer-wins display, and (iii) it is the
        // same class the boot sweep already accepts — which is why that sweep gates
        // on the handover being absent (see the runtime builder call site). It never
        // corrupts the journal: that run's second, truthful finish lands *after* the
        // synthetic one and so supersedes it.
        //
        // That last argument turns on ORDER, and it does not carry to a run which
        // settles inside this request — there the truthful finish lands first and
        // loses. See the window handled below; it is closed rather than accepted.
        let live_ids: HashSet<String> = company
            .runtime
            .run_supervisor()
            .live()
            .into_iter()
            .map(|(run_id, _workflow_id)| run_id)
            .collect();
        let mut dead: Vec<usize> = Vec::new();
        for (index, entry) in runs.iter().enumerate() {
            if !entry.running {
                continue;
            }
            let Some(run_id) = entry.run_id.as_ref() else {
                continue;
            };
            if live_ids.contains(run_id) {
                continue;
            }
            dead.push(index);
        }

        // ── The window between the snapshot and `live()` ────────────────────────
        //
        // `live()` is consulted AFTER the journal snapshot was taken, and a run can
        // settle in between: it appends its finish (too late for the snapshot) and
        // then drops its guard (in time to be missing from `live()`). Such a run is
        // indistinguishable, on the two facts above, from one that died — but it is
        // the opposite, and settling it is worse than the hang this repairs.
        //
        // The ordering is what makes it worse rather than merely wrong. The rebuild
        // false positive this block already accepts is self-correcting because the
        // run's real finish lands *after* the synthetic one, and the fold settles an
        // entry from the last finish it sees. Here the real finish lands *first*, so
        // the synthetic one wins for good: a successful run reads
        // `INTERRUPTED_BY_RESTART` permanently, and because the fold overwrites
        // `deliveries` from whichever finish settles last, the record of what it
        // sent is replaced by an empty list.
        //
        // So before writing anything, read the journal on from where the snapshot
        // stopped and drop any candidate whose finish turns up there. That is
        // exact rather than a heuristic: a run whose start was in the snapshot was
        // registered before it (`begin` precedes both the spawn and the runner's
        // `WorkflowRunStarted`), so a candidate missing from `live()` has already
        // been deregistered — and a deregistered run journaled its finish first, if
        // it was ever going to. Anything appended before that point is at a higher
        // sequence than the whole snapshot, so this second read cannot miss it.
        //
        // Cheap where it matters: it runs only when there are candidates at all,
        // which after the first settle is nothing, and it reads only the tail.
        if !dead.is_empty() {
            match company
                .runtime
                .events()
                .read_from(
                    company.id(),
                    EventSeq::new(read_through.saturating_add(1)),
                    usize::MAX,
                )
                .await
            {
                Ok(tail) => {
                    let settled_since: HashSet<String> = tail
                        .into_iter()
                        .filter_map(|stored| match stored.event {
                            CompanyEvent::WorkflowRunFinished {
                                run_id: Some(run_id),
                                ..
                            } => Some(run_id),
                            _ => None,
                        })
                        .collect();
                    dead.retain(|index| {
                        runs[*index]
                            .run_id
                            .as_ref()
                            .is_none_or(|run_id| !settled_since.contains(run_id))
                    });
                }
                Err(err) => {
                    // Unprovable, so nothing is settled. The row keeps reporting
                    // `running` and a later poll retries — strictly better than
                    // stamping "interrupted" on a run that may well be finishing.
                    tracing::warn!(
                        company = %company.id(),
                        %err,
                        "could not re-read the journal to confirm a workflow run is dead; \
                         leaving it as running"
                    );
                    dead.clear();
                }
            }
        }

        for index in &dead {
            let entry = &mut runs[*index];
            let Some(run_id) = entry.run_id.clone() else {
                continue;
            };
            // Durable half: append the finish so it survives this response, folds
            // settled on the next `GET …/workflows/runs`, and stops the boot sweep
            // from having to. Best-effort by construction — a failed append leaves
            // the row as the in-memory flip below still makes it, and the next read
            // simply retries.
            crate::runtime::record_run_finished(
                company.runtime.events(),
                company.id(),
                &entry.workflow_id,
                entry.scheduled,
                &run_id,
                Err(crate::runtime::workflow_outcome::INTERRUPTED_BY_RESTART.into()),
            )
            .await;
            // In-memory half: flip the row this response returns, so the console does
            // not have to wait for the next poll to stop the spinner.
            entry.running = false;
            entry.error =
                Some(crate::runtime::workflow_outcome::INTERRUPTED_BY_RESTART.to_string());
        }

        // ── Serve the row the NEXT read will fold, identically ──────────────────
        //
        // The fold keys a settled entry on its **finish**, taking `seq` and
        // `at_millis` from that row. So flipping `running` while leaving the
        // start's values in place means this response and the one 2s later carry
        // *different* `seq` for the same run — and the console keys its history
        // rows on exactly that field (`RunHistoryPanel`: `key={run.seq}`, with
        // `selectedRunSeq` / `fixingRunSeq` / `fixReason.seq` compared against it).
        // The row remounts and any selection on it is dropped, in the one window
        // where an operator is most likely to be looking: the 2s recovery poll runs
        // precisely because someone is watching this run.
        //
        // So the appended rows are read back and their real `seq` / `at_millis`
        // stamped on. Read back rather than returned from `record_run_finished`,
        // which reports only whether the append happened — the values served are
        // then the durable ones rather than a second construction of them.
        if !dead.is_empty() {
            match company
                .runtime
                .events()
                .read_from(
                    company.id(),
                    EventSeq::new(read_through.saturating_add(1)),
                    usize::MAX,
                )
                .await
            {
                Ok(appended) => {
                    let stamped: std::collections::HashMap<String, (u64, u64)> = appended
                        .into_iter()
                        .filter_map(|stored| match stored.event {
                            CompanyEvent::WorkflowRunFinished {
                                run_id: Some(run_id),
                                ..
                            } => Some((run_id, (stored.seq.value(), stored.at_millis))),
                            _ => None,
                        })
                        .collect();
                    for index in &dead {
                        let entry = &mut runs[*index];
                        let Some((seq, at_millis)) =
                            entry.run_id.as_ref().and_then(|id| stamped.get(id))
                        else {
                            continue;
                        };
                        entry.seq = *seq;
                        entry.at_millis = *at_millis;
                    }
                }
                Err(err) => {
                    // The settle itself stands — it is already durable. Only the
                    // row's identity is left at the start's, which the next read
                    // corrects.
                    tracing::warn!(
                        company = %company.id(),
                        %err,
                        "settled a dead workflow run but could not read back its finish row; \
                         this response carries the start's seq and time"
                    );
                }
            }
        }
    }

    // Newest first: a history panel leads with the run that just happened.
    //
    // Issue #1012: sorted explicitly by `(at_millis, seq)` descending — the
    // very pair every row *displays* — rather than `reverse()`d. `reverse()`
    // only flips the fold's push order, which is the order runs *started* (an
    // entry is pushed once, at its `WorkflowRunStarted` row, and only mutated
    // in place — never re-pushed — when its `WorkflowRunFinished` row later
    // overwrites `seq`/`at_millis` to the finish's own). Two runs that
    // interleave (B starts after A, but finishes first) therefore used to come
    // back in *start* order while every row read as if it were ordered by
    // *finish* — the row for A would lead even though B's `seq`/`atMillis` say
    // B is newer. Sorting on the same field the row displays makes the two
    // agree by construction, for a run still in flight (which sorts on its own
    // start) exactly as for one already settled.
    //
    // The `limit` now cuts *runs* rather than journal rows, which is the
    // number the caller was asking about all along.
    //
    // Issue #1189: this stays ABOVE the verdict pass (moved to the very end,
    // below the reconciliation join). Neither the sort nor `truncate` touches
    // a single field of a row — they reorder and drop whole rows — so the
    // invariant the verdict pass is placed on ("derive after everything that
    // can still change its inputs") is not weakened by running them first.
    //
    // Issue #1012 follow-up: the sort above and the `limit` cut are no longer
    // the same operation. The cut is keyed on `seq` alone and the sort — which
    // is exactly the one described above — runs after it, because a cursor
    // derived from a non-monotonic key cannot partition the run set and
    // silently loses runs under a clock regression. The whole argument lives on
    // [`select_run_page`], which now owns both steps plus the cursor this page
    // hands back.
    let (mut runs, has_more, next_before_seq) = select_run_page(runs, limit);

    // Issues #1143 + #1189. A run's record of what it stopped for is a receipt
    // and cannot go stale — but the *question* it points at can. `ApprovalParked`
    // is journaled at `Durability::Process` on the stated reasoning that losing
    // it is harmless because "the agent parks it again on its next attempt".
    // That holds for a chat turn, which retries; it is false for a workflow run,
    // which halted at the gate and never re-enters it. So a run that outlived
    // its own approvals goes on reporting that it waits on cards the queue does
    // not have. #1145 carries the durability decision; this reconciliation
    // deliberately does not pre-empt it.
    //
    // TWO joins, because a run has two ways to stop for a person and they leave
    // different traces:
    //
    // * `blockedNodes[].approvalIds` — the ids an agent node's gated calls
    //   parked. Those cards are tool-call effects and carry no node id, so the
    //   only key is the id (issue #1143).
    // * `pendingApprovals` — the gate nodes the engine paused at. Their cards
    //   are `workflow.approve` effects that record no receipt and no
    //   blocked-node row at all, so #1143's id-keyed join structurally could not
    //   reach them; they are joined on `(run_id, node_id)` instead (issue
    //   #1189). This is the bigger half: 34 of the marketing tenant's 60 runs.
    //
    // Done HERE, on the read, for the same reason `relabel_blocked` above
    // relabels rather than rewriting the durable node rows: the journal records
    // what happened and is not edited to reflect what is true now. After the
    // truncate, so the join costs one journal snapshot and covers only the rows
    // actually being returned — and skipped entirely when no returned run
    // stopped for anybody, which is nearly every read.
    if runs.iter().any(|r| {
        !r.pending_approvals.is_empty()
            || r.blocked_nodes.iter().any(|b| !b.approval_ids.is_empty())
    }) {
        let live = company.runtime.live_approvals();
        for run in &mut runs {
            for blocked in &mut run.blocked_nodes {
                blocked.stranded = blocked
                    .approval_ids
                    .iter()
                    .filter(|id| !live.holds_id(id))
                    .count();
            }
            run.stranded_approvals = crate::ports::workflow_verdict::stranded_approvals(
                run.run_id.as_deref(),
                &run.pending_approvals,
                &run.blocked_nodes,
                &live,
            );
        }
    }

    // Position is the correctness argument — see `settle_history_verdicts`.
    settle_history_verdicts(&mut runs);

    Ok(Json(WorkflowRunsResponse {
        runs,
        has_more,
        next_before_seq,
    }))
}

/// The `GET …/workflows/runs` response body (issue #1012).
///
/// Wrapped rather than a bare array — as this route answered before — because
/// `hasMore` has nowhere else to ride: the console's history drawer cannot
/// otherwise tell "this is the whole history" from "this page was truncated at
/// `limit`", which is exactly the silent-truncation half of the issue.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkflowRunsResponse {
    runs: Vec<WorkflowRunOutcome>,
    /// Whether a further, older page exists behind [`next_before_seq`](Self::next_before_seq).
    has_more: bool,
    /// The cursor to pass as `?before_seq=` to fetch the page behind this one —
    /// this page's **lowest** `seq`, which is not in general the last row in
    /// display order (see [`select_run_page`]).
    ///
    /// Server-issued rather than client-derived. The console used to take
    /// `runs.at(-1)?.seq`, which was the same number only while the cut and the
    /// display order shared a key; they no longer do, and the page cut is the
    /// only place the boundary is known. Absent exactly when `has_more` is
    /// `false` — nothing older to ask for.
    ///
    /// A console predating this field falls back to its old derivation, which
    /// is why the field is omitted rather than sent as `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    next_before_seq: Option<u64>,
}

/// Relabels a run's node rows for the nodes it blocked on a human (issue #881).
///
/// The read-side half of the host reclassification. `WorkflowNodeFinished` is
/// written live, node by node, long before anything knows the run stopped for an
/// approval rather than a fault — so the durable row says `error` and stays that
/// way. Fixing it up here, against the finish's own blocked list, is what keeps
/// the history panel's node chips agreeing with the run's terminal reading; the
/// alternative is a run that says "blocked" beside a node chip that says
/// "failed".
fn relabel_blocked(nodes: &mut [WorkflowRunNode], blocked: &[crate::ports::WorkflowBlockedNode]) {
    if blocked.is_empty() {
        return;
    }
    for node in nodes.iter_mut() {
        if blocked.iter().any(|b| b.node_id == node.node_id) {
            node.status = WorkflowNodeStatus::Blocked;
        }
    }
}

#[cfg(test)]
#[path = "workflows_a_plain_errored_node_tests.rs"]
mod tests_a_plain_errored_node;
#[cfg(test)]
#[path = "workflows_editable_is_overlay_backed_tests.rs"]
mod tests_editable_is_overlay_backed;
#[cfg(test)]
#[path = "workflows_getting_a_malformed_workflow_tests.rs"]
mod tests_getting_a_malformed_workflow;
#[cfg(test)]
#[path = "workflows_hosted_a_paused_company_refuses_tests.rs"]
mod tests_hosted_a_paused_company_refuses;
#[cfg(test)]
#[path = "workflows_hosted_create_persists_and_reads_tests.rs"]
mod tests_hosted_create_persists_and_reads;
#[cfg(test)]
#[path = "workflows_hosted_edit_and_delete_serve_tests.rs"]
mod tests_hosted_edit_and_delete_serve;
#[cfg(all(test, feature = "openhuman"))]
#[path = "workflows_hosted_fix_error_resolution_prefers_tests.rs"]
mod tests_hosted_fix_error_resolution_prefers;
#[cfg(test)]
#[path = "workflows_hosted_revisions_list_is_metadata_tests.rs"]
mod tests_hosted_revisions_list_is_metadata;
#[cfg(test)]
#[path = "workflows_hosted_run_card_tests.rs"]
mod tests_hosted_run_card;
#[cfg(test)]
#[path = "workflows_hosted_run_history_groups_a_tests.rs"]
mod tests_hosted_run_history_groups_a;
#[cfg(test)]
#[path = "workflows_hosted_run_history_is_not_tests.rs"]
mod tests_hosted_run_history_is_not;
#[cfg(test)]
#[path = "workflows_hosted_run_history_leaves_a_tests.rs"]
mod tests_hosted_run_history_leaves_a;
#[cfg(test)]
#[path = "workflows_hosted_the_enabled_route_toggles_tests.rs"]
mod tests_hosted_the_enabled_route_toggles;
#[cfg(test)]
#[path = "workflows_running_a_dropped_connection_does_tests.rs"]
mod tests_running_a_dropped_connection_does;
#[cfg(test)]
#[path = "workflows_running_a_synchronous_run_cancelled_tests.rs"]
mod tests_running_a_synchronous_run_cancelled;
#[cfg(test)]
#[path = "workflows_running_dry_run_request_echoes_tests.rs"]
mod tests_running_dry_run_request_echoes;
#[cfg(test)]
#[path = "workflows_test_support.rs"]
mod workflows_test_support;
