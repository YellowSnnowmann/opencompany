//! Feature-free core for authoring a new workflow graph (issue #112).
//!
//! [`create_company_workflow`] is the single validated-persist sequence for
//! creating a workflow graph, shared by **both** the console's
//! `POST …/workflows` route and the orchestrator's `create_workflow` tool so
//! they run exactly the same checks and land the exact same artifact — no
//! create-vs-run drift, one place to reason about safety.
//!
//! The graph body is persisted **on the company record** as an
//! [`OverlayWorkflow`], never written into the company source tree. The source
//! tree is the version-controlled seed and, in hosted mode, a read-only crate
//! mount — writing there failed every hosted tenant with `EROFS` (issue #168).
//! Readers union the two sources via
//! [`load_workflow_union`](crate::company::load_workflow_union), with the seed
//! file winning on an id collision.
//!
//! The sequence, in order, each step an actionable error before anything is
//! persisted:
//!
//! 1. the id is a safe filename stem (no slashes / `..`) and within length caps
//!    — it is still an id a seed file could carry, so the two sources stay
//!    interchangeable;
//! 2. the graph is within the node/edge size caps (a runaway graph can't be
//!    persisted);
//! 3. it names exactly one `trigger` (a freshly authored graph must say what
//!    starts it — stricter than [`parse_workflow`], which allows many);
//! 4. every `agent` node names a real roster teammate (manifest ∪ overlay);
//! 5. its id is unique against the company's seed ids ∪ overlay ids ∪
//!    manifest-enabled ids (a [`Conflict`](OpenCompanyError::Conflict));
//! 6. its display name is unique (case-insensitive) against the company's
//!    existing seed + overlay + manifest-enabled workflows;
//! 7. the rendered TOML re-parses through [`parse_workflow`] (the same
//!    structural validation a hand-authored file passes) and is within the byte
//!    cap;
//! 8. the body **and** the enabled id are pushed onto the record and persisted
//!    in **one** [`save`](CompanyStore::save) — a single atomic write, so there
//!    is no half-created state to roll back;
//! 9. a best-effort [`WorkflowCreated`](CompanyEvent::WorkflowCreated) audit
//!    event is journaled — never rolling the create back if the journal fails.
//!
//! Steps 4–8 run under the per-company [`company_write_lock`] so a concurrent
//! `create_workflow` (tool) and `POST …/workflows` (REST) can never clobber
//! each other's `overlay`/`enabled` write, the same primitive `add_agent` uses.
//! That lock is what makes the id-uniqueness check of step 5 atomic now that
//! the filesystem's `create_new(true)` no longer serializes the two surfaces.
//!
//! Compiled in the default build (no harness imports) so the REST route reaches
//! it without any feature gate.
//!
//! # Editing and removing (issue #259)
//!
//! [`update_company_workflow`] and [`delete_company_workflow`] complete the
//! write lifecycle. Both run the same validation and hold the same
//! [`company_write_lock`] as create, and both are **overlay-only**: they refuse
//! (with a [`Conflict`](OpenCompanyError::Conflict)) to touch an id backed by a
//! seed file or by a bodiless manifest-`enabled` entry. That is not squeamishness
//! about writing to disk, it is the only shape that is honest about what the
//! reader will do:
//!
//! * [`load_workflow_union`](crate::company::load_workflow_union) gives the
//!   **seed file precedence** on an id collision, so persisting an overlay edit
//!   for a seed-backed id would store a graph the read path never serves — the
//!   operator's change would appear to save and then silently not exist.
//! * `merge_enabled_workflows` (`src/runtime/builder.rs`, issue #208) rebuilds
//!   `[workflows].enabled` at boot from seed ids ∪ surviving overlay ids, so a
//!   "deleted" seed workflow would come back on the next restart.
//!
//! The same invariant is what makes an overlay delete *durable*: with no overlay
//! body left, the boot merge has nothing to re-enable.
//!
//! ## The version token
//!
//! [`workflow_version`] hashes the stored overlay TOML. `GET …/workflows/{wid}`
//! hands it out, a `PUT`/`DELETE` may hand it back, and the comparison happens
//! **inside** the write lock immediately before the mutation — so it is a real
//! optimistic-concurrency guard, not a check-then-act race. The token is opaque
//! on the wire (the contract is "echo back what the read returned"), so the
//! algorithm can change without a client migration.
//!
//! Passing no token is an unconditional write. That mirrors OpenHuman's
//! `flows_update`, whose `expected_version` is likewise `Option`: it keeps a
//! `curl` caller usable without a read-modify-write dance, while the console —
//! which has a stale-tab problem — always sends one.
//!
//! ## What is deliberately not here
//!
//! * **Revision history lives in its own store (issue #274).** OpenHuman keeps a
//!   bounded snapshot ring in a dedicated `flow_revisions` table; our overlay
//!   bodies live inside `CompanyRecord`, which is loaded and saved *whole* on
//!   every write, so a ring per workflow would bloat that hot path. It therefore
//!   got its own [`WorkflowRevisionStore`](crate::ports::WorkflowRevisionStore)
//!   port plus three backends rather than a field on the record.
//!   [`update_company_workflow`] captures the prior body into it under the write
//!   lock, and [`rollback_company_workflow`] restores one *through this same
//!   update path* — so a rollback re-validates against the current record and is
//!   itself undoable. Diffing/merging revisions stays out of scope.
//! * **No run-history reaping.** Past runs are
//!   [`WorkflowRunFinished`](CompanyEvent::WorkflowRunFinished) entries on the
//!   company's single append-only journal, interleaved with chat and audit. What
//!   a workflow did stays true after the workflow is gone, and `GET
//!   …/workflows/runs` keeps serving those rows.
//! * **No schedule re-registration.** Nothing to re-register:
//!   [`WorkflowScheduler::tick`](crate::runtime::WorkflowScheduler) re-reads the
//!   record and re-derives the schedule set from the overlay union every minute,
//!   so the tick *is* a continuous reconcile. OpenHuman needs
//!   `reconcile_schedule_triggers_on_boot` because a bound cron job lives in a
//!   second durable store (`cron.db`) that can drift from `flows.db`; we persist
//!   no registration at all, so that class of bug cannot arise here.
//!
//! # Arming, and the disarm rule (issue #276)
//!
//! A workflow's armed state is
//! [`CompanyRecord::disabled_workflows`](crate::ports::types::CompanyRecord::disabled_workflows),
//! read by [`WorkflowScheduler::tick`](crate::runtime::WorkflowScheduler) and by
//! nothing else that decides whether work happens. Three write paths touch it,
//! and **two of them can only ever disarm**:
//!
//! | Path | Writes |
//! | --- | --- |
//! | [`create_company_workflow`] | `false`, when the new graph carries a trigger schedule |
//! | [`update_company_workflow`] | `false`, when the edit adds a schedule to a graph that had none |
//! | [`set_company_workflow_enabled`] | whatever the operator asked for |
//!
//! **A schedule is armed only by a person saying so.** That is the rule, and it
//! is one-directional on purpose: an edit that *removes* a schedule does not
//! re-arm anything, re-saving an already-scheduled graph does not re-arm it, and
//! neither does deleting and recreating around it. A rule that could arm would
//! be a rule that could arm by accident.
//!
//! This is OpenHuman's "B29 Rule 1" — its `flows_update` forces `enabled =
//! false` when an edit turns a manual or absent trigger into an automatic one,
//! after a flow of its own started running on an unreviewed 8am schedule — with
//! one deliberate widening. **Create is covered too.** OpenHuman disarms only on
//! edit; here [`create_company_workflow`] is *also* the orchestrator's
//! `create_workflow` tool, so leaving create armed would mean an agent can put a
//! cron into production by authoring one, and an operator who wanted around the
//! rule would only have to write the graph fresh instead of editing it. Issue
//! #276 says it directly: a rule that does not cover create and update together
//! just moves the hole.
//!
//! **Changing an existing cron does not disarm.** `0 8 * * *` → `0 3 * * *` on
//! an already-armed workflow stays armed. The operator accepted automatic firing
//! for this workflow and is now correcting *when*; disarming there would put a
//! re-enable click behind every typo fix, which is how an operator learns to
//! click through the re-arm without reading it. The decision that was reviewed
//! is "automatic at all", and that one has not changed.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::company::{
    RawEdge, RawNode, RawWorkflow, WorkflowDestinationDef, WorkflowFile, WorkflowNodeKind,
    channel_destination_missing_target_message, list_workflows_union, parse_workflow,
    raw_workflow_from_toml, render_workflow, required_config_problems,
};
use crate::error::{OpenCompanyError, Result, WorkflowProblem};
use crate::ports::events::EventLog;
use crate::ports::now_millis;
use crate::ports::store::company_write_lock;
use crate::ports::types::{
    Actor, CompanyEvent, CompanyId, CompanyRecord, OverlayWorkflow, WorkflowEnabledReason,
};
use crate::ports::workflow_revisions::{WorkflowRevisionRecord, WorkflowRevisionStore};
use crate::ports::{CompanyStore, ScheduleFireStore};
use crate::runtime::workflow_schedule_id;
use crate::server::ops::language;

/// Max nodes a freshly authored graph may declare. A larger graph is refused
/// before anything is rendered or written.
pub(crate) const MAX_WORKFLOW_NODES: usize = 50;
/// Max edges a freshly authored graph may declare.
pub(crate) const MAX_WORKFLOW_EDGES: usize = 100;
/// Max size of the rendered `workflows/<id>.toml`, checked after render and
/// before the file is written.
pub(crate) const MAX_WORKFLOW_TOML_BYTES: usize = 64 * 1024;
/// Max length of a workflow id (also the on-disk filename stem).
pub(crate) const MAX_WORKFLOW_ID_LEN: usize = 64;
/// Max length of a workflow display name.
pub(crate) const MAX_WORKFLOW_NAME_LEN: usize = 200;

/// Authors and persists a new workflow graph for `company`, returning the
/// parsed [`WorkflowFile`] exactly as
/// [`load_workflow_union`](crate::company::load_workflow_union) would hand it
/// to the runner (so what a caller reads back and what runs are identical).
///
/// The body is persisted on the company record, so this works on a deployment
/// with **no** source directory at all — the hosted case that used to be
/// refused outright and then failed with `EROFS` anyway (issue #168).
/// `source_dir` is the company source directory (`companies/<name>`) when one
/// exists, read-only here: its `workflows/` subtree contributes the seed ids and
/// names the uniqueness checks guard against. `events` is the company event log
/// for the best-effort audit journal; pass `None` to skip journaling.
///
/// Errors map to the same HTTP statuses the REST route always returned:
/// [`InvalidRequest`](OpenCompanyError::InvalidRequest) → 400,
/// [`Conflict`](OpenCompanyError::Conflict) → 409.
///
/// `by` (issue #1843) is who to attribute the create to on
/// [`WorkflowCreated::by`](CompanyEvent::WorkflowCreated). The two REST call
/// sites (`POST …/workflows`, and applying a task's workflow proposal) pass
/// their [`ScopedCompany::actor`](crate::server::ops::scope::ScopedCompany::actor)
/// through unchanged — `Some` for a signed-in human, `None` for the platform
/// principal. The orchestrator's `create_workflow` tool passes `None`: an
/// agent authoring a graph on its own initiative is not the human activation
/// signal this field exists to capture, even though the graph it produces is
/// identical either way.
pub(crate) async fn create_company_workflow(
    company: &CompanyId,
    source_dir: Option<&Path>,
    store: &Arc<dyn CompanyStore>,
    events: Option<&Arc<dyn EventLog>>,
    mut draft: RawWorkflow,
    wired_channels: Option<&[String]>,
    by: Option<Actor>,
) -> Result<WorkflowFile> {
    // --- Input normalization (before validation or locking) ------------------
    draft.owner_desk = RawWorkflow::normalize_owner_desk(draft.owner_desk.take());

    // --- Input validation (no lock; pure function of the draft) -------------
    validate_draft_shape(&draft)?;

    // --- Serialized write section -------------------------------------------
    // Load record → roster check → id/name uniqueness → save record all under
    // the per-company write lock, so a concurrent create/add_agent can never
    // clobber the record's `enabled`/`overlay` write.
    let write_lock = company_write_lock(company);
    let _lock = write_lock.lock().await;

    let mut record = store
        .load(company)
        .await?
        .ok_or_else(|| OpenCompanyError::CompanyNotFound(company.to_string()))?;

    // Cross-check every `agent` node against the company's effective roster and
    // every `tool_call` node against the company's wired+granted tools.
    // `parse_workflow` checks the graph's own shape but has no record to validate
    // names against — the same helper gates create and update identically.
    // `previous_owner_desk: None` — nothing to grandfather a bad desk against
    // on a fresh create.
    //
    // The check may return a resolved `owner_desk` (issue #1882 review): see
    // the normalization note on `validate_draft_against_record` for why the
    // draft's field is overwritten with it before `render_workflow` persists.
    if let Some(resolved) =
        validate_draft_against_record(&draft, &record, source_dir, wired_channels, None)?
    {
        draft.owner_desk = Some(resolved);
    }

    // Id uniqueness against every id this company already answers for: the seed
    // files, the record's overlay bodies, and the manifest-enabled ids. The
    // write lock held above is what makes this atomic — it replaces the
    // filesystem `create_new(true)` that used to serialize the two surfaces.
    // The seed side is checked by path rather than by scanning: a *malformed*
    // seed file still owns its id (it would shadow the overlay body on read),
    // and a scan would silently skip it.
    let id_taken = seed_file_exists(source_dir, &draft.id)
        || record.overlay_workflows.iter().any(|w| w.id == draft.id)
        || record
            .manifest
            .workflows
            .enabled
            .iter()
            .any(|id| id == &draft.id);
    if id_taken {
        return Err(OpenCompanyError::Conflict(format!(
            "A workflow with id `{}` already exists. Pick a different id.",
            draft.id
        )));
    }

    // Case-insensitive display-name uniqueness against the company's existing
    // workflows (seed ∪ overlay ∪ manifest-enabled), so two differently-id'd
    // workflows can't share one indistinguishable name in the picker.
    let existing_names = existing_workflow_names(
        source_dir,
        &record.overlay_workflows,
        &record.manifest.workflows.enabled,
    );
    if existing_names.contains(&draft.name.trim().to_ascii_lowercase()) {
        return Err(OpenCompanyError::Conflict(format!(
            "A workflow named `{}` already exists. Pick a different name.",
            draft.name.trim()
        )));
    }

    // Render the candidate to TOML and re-parse it through `parse_workflow`,
    // the same structural validation a hand-authored file passes. Any problem
    // becomes an `InvalidRequest` (400), never the 500 a malformed on-disk file
    // gets from the read routes.
    let toml_src = render_workflow(&draft)?;
    if toml_src.len() > MAX_WORKFLOW_TOML_BYTES {
        return Err(over_cap_error(toml_src.len()));
    }
    let file = parse_workflow(&toml_src).map_err(|err| match err {
        // A structural validation failure of the rendered draft becomes a
        // structured `WorkflowInvalid` 400 (issue #1016). These graph-level
        // problems (an inescapable cycle, an unreachable node) name no single
        // node, so they carry `node_id: None` — the per-node/field problems come
        // from `validate_draft_against_record`, which runs first.
        OpenCompanyError::DataInvalid { problems, .. } => OpenCompanyError::WorkflowInvalid {
            problems: problems.into_iter().map(WorkflowProblem::from).collect(),
        },
        OpenCompanyError::DataParse { message, .. } => OpenCompanyError::InvalidRequest(message),
        other => other,
    })?;

    // Persist the graph body and the enabled id in ONE save. Both live on the
    // record, so there is no file-then-record window to roll back: the save
    // either lands both or neither. The version-controlled `company.toml` on
    // disk is never rewritten — the same team-overlay convention `add_agent`
    // follows.
    record.overlay_workflows.push(OverlayWorkflow {
        id: file.id.clone(),
        toml: toml_src,
    });
    if !record
        .manifest
        .workflows
        .enabled
        .iter()
        .any(|e| e == &file.id)
    {
        record.manifest.workflows.enabled.push(file.id.clone());
    }
    // Issue #276: a graph authored with a cron lands **switched off**. It is
    // saved, listed and runnable by hand; it just does not fire until someone
    // arms it. Written in the same save as the body and the enabled id, so a
    // freshly created schedule is never briefly live — there is no window
    // between "the scheduler can see this" and "the scheduler is told not to run
    // it", because a tick reads one record or the other, never a half of both.
    //
    // Note which surface this binds hardest: this function is also the
    // orchestrator's `create_workflow` tool, so an agent cannot arm a cron.
    let disarmed = file.trigger_schedule().is_some();
    if disarmed {
        record.set_workflow_enabled(&file.id, false);
    }
    store.save(&record).await?;

    // Drop the write lock before journaling: the audit event is best-effort and
    // never gates the create, so it needn't hold the serialization lock.
    drop(_lock);

    // Best-effort audit journal. A journal failure never rolls the create back
    // (the workflow is already persisted + enabled) — we only log it.
    if let Some(log) = events
        && let Err(err) = log
            .append(
                company,
                CompanyEvent::WorkflowCreated {
                    workflow_id: file.id.clone(),
                    name: file.name.clone(),
                    by,
                },
            )
            .await
    {
        tracing::warn!(
            company = %company,
            workflow = %file.id,
            error = %err,
            "workflow created but audit journal append failed"
        );
    }

    // Issue #276: say so, and say it was the rule rather than a person. A
    // scheduled workflow that never fires is otherwise indistinguishable from a
    // broken one, and this is the line that tells an operator to go arm it.
    if disarmed {
        journal_enabled_change(
            company,
            events,
            &file.id,
            &file.name,
            false,
            WorkflowEnabledReason::Disarmed,
        )
        .await;
        tracing::info!(
            company = %company,
            workflow = %file.id,
            "workflow created with a schedule and left switched off pending review"
        );
    }

    Ok(file)
}

// ---------------------------------------------------------------------------
// The workflow proposal's authoring payload (issue #580)
// ---------------------------------------------------------------------------

/// The `{id, name, description, nodes, edges}` graph a
/// [`TaskWorkflowProposal`](crate::ports::tasks::TaskWorkflowProposal) stores as
/// its `ops` — the same shape `POST …/workflows` accepts, but owned by the
/// company layer so the harness builder (which *produces* a proposal) and the
/// apply route (which *persists* it) rebuild a [`RawWorkflow`] from it the SAME
/// way.
///
/// **The host is the authority.** A proposal never stores a rendered graph; it
/// stores this payload, and apply re-derives and re-validates a `RawWorkflow`
/// from it through [`create_company_workflow`]. So a stored proposal is *input*
/// to the create checks, never a substitute for them — a graph cannot reach the
/// workflow list without passing exactly the validation a hand-authored one does.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowGraphSpec {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// The owning desk (issue #1862 prerequisite) — see
    /// [`WorkflowFile::owner_desk`]. Camel-cased `ownerDesk` on the wire, like
    /// every other field on this spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_desk: Option<String>,
    #[serde(default)]
    pub(crate) nodes: Vec<WorkflowNodeSpec>,
    #[serde(default)]
    pub(crate) edges: Vec<WorkflowEdgeSpec>,
}

/// One node of a [`WorkflowGraphSpec`]. Mirrors the create route's node body —
/// the subset a builder pass produces (`trigger`, `agent`, `tool_call`,
/// `condition`, `output`). `on_error`/`retry` are omitted because the builder
/// does not author them; they convert to `None` on a [`RawNode`].
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowNodeSpec {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) schedule: Option<String>,
    /// Free-form engine config as JSON (a `tool_call`'s `slug`, a condition's
    /// expression). Converted to a TOML value on the way into [`RawNode`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) requires_approval: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) destination: Option<WorkflowDestinationDef>,
}

/// One edge of a [`WorkflowGraphSpec`].
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowEdgeSpec {
    #[serde(default)]
    pub(crate) from: String,
    #[serde(default)]
    pub(crate) to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
}

/// Rebuilds a [`RawWorkflow`] from a stored proposal graph — the one conversion
/// both the builder pass and the apply route use, so what the builder validated
/// and what apply persists are the same graph.
///
/// The only fallible step is `config`: TOML (the node config's storage form) has
/// no `null`, so a JSON config carrying one is refused here with an actionable
/// message rather than silently dropped — the same rule the create route's own
/// body conversion applies.
pub(crate) fn raw_workflow_from_spec(spec: &WorkflowGraphSpec) -> Result<RawWorkflow> {
    let mut nodes = Vec::with_capacity(spec.nodes.len());
    for n in &spec.nodes {
        let config = match &n.config {
            Some(json) => Some(toml::Value::try_from(json).map_err(|err| {
                OpenCompanyError::InvalidRequest(format!(
                    "node `{}` has config that can't be stored ({err}) — TOML has no null; drop \
                     null-valued keys.",
                    n.id
                ))
            })?),
            None => None,
        };
        nodes.push(RawNode {
            id: n.id.clone(),
            kind: n.kind.clone(),
            name: n.name.clone(),
            summary: n.summary.clone(),
            agent: n.agent.clone(),
            schedule: n.schedule.clone(),
            config,
            on_error: None,
            retry: None,
            requires_approval: n.requires_approval,
            // Deliberately not carried, alongside `on_error` and `retry`: a
            // repeat guard is a safety declaration about a call reaching a
            // counterparty, which is the operator's to make. The copilot
            // proposes a graph; it does not decide what a continuation may send
            // twice. An operator sets it afterwards through the write route.
            repeatable: None,
            destination: n.destination.clone(),
            postcondition: None,
            verify: None,
        });
    }
    Ok(RawWorkflow {
        id: spec.id.clone(),
        name: spec.name.clone(),
        description: spec.description.clone(),
        // Normalized here, not carried verbatim: a blank/whitespace string
        // is not "unset" to serde, but every reader of `RawWorkflow::owner_desk`
        // treats it that way — most concretely `apply_workflow_proposal`'s
        // `is_none()` fallback (issue #1882 review), which this same
        // conversion feeds.
        owner_desk: RawWorkflow::normalize_owner_desk(spec.owner_desk.clone()),
        nodes,
        edges: spec
            .edges
            .iter()
            .map(|e| RawEdge {
                from: e.from.clone(),
                to: e.to.clone(),
                label: e.label.clone(),
            })
            .collect(),
    })
}

/// The inverse of [`raw_workflow_from_spec`] at the parsed-graph layer (issue
/// #840, PR-3): rebuilds a [`WorkflowGraphSpec`] from a persisted, validated
/// [`WorkflowFile`]. The fix-from-run copilot needs the failing workflow's saved
/// graph *as a spec* — both to hand the agent as evidence and to pin its identity
/// through the correction — and the read path produces a `WorkflowFile`, so this
/// is the one conversion between them.
///
/// `on_error`/`retry` have no field on [`WorkflowNodeSpec`] (the builder never
/// authors them), so they are dropped: the copilot re-proposes the graph's nodes,
/// and the host-owned id/name are what the fix path pins, not the engine policy.
///
/// Gated with the create-time copilot that is its only caller, the same footing
/// as [`courtesy_validate_draft`] and [`workflow_graph_from_spec`].
#[cfg(feature = "openhuman")]
pub(crate) fn workflow_spec_from_graph(file: WorkflowFile) -> WorkflowGraphSpec {
    WorkflowGraphSpec {
        id: file.id,
        name: file.name,
        description: file.description,
        owner_desk: file.owner_desk,
        nodes: file
            .nodes
            .into_iter()
            .map(|n| WorkflowNodeSpec {
                id: n.id,
                kind: n.kind.as_str().to_string(),
                name: n.name,
                summary: n.summary,
                agent: n.agent,
                schedule: n.schedule,
                config: n.config,
                requires_approval: n.requires_approval,
                destination: n.destination,
            })
            .collect(),
        edges: file
            .edges
            .into_iter()
            .map(|e| WorkflowEdgeSpec {
                from: e.from,
                to: e.to,
                label: e.label,
            })
            .collect(),
    }
}

/// Runs the full author-time validation on a candidate graph **without
/// persisting it** — the builder pass's courtesy check (issue #580), so a
/// proposal that could never be created never reaches In Review.
///
/// It runs exactly the checks [`create_company_workflow`] runs before its save —
/// shape (id/name/size/one-trigger), the render → byte-cap → `parse_workflow`
/// round trip, and the roster/tool cross-check against the loaded `record` — and
/// then throws the result away. The one thing it deliberately does **not** check
/// is id/name uniqueness, because that is a function of the live record at
/// *apply* time, not build time: a name free when the proposal was built can be
/// taken by the time it is approved, and that is the roster-drift case apply
/// surfaces by keeping the card In Review.
///
/// Ungated, and deliberately so. It used to be `#[cfg(feature = "openhuman")]`
/// because its only caller was `crate::harness::workflow_build`. Issue #1074
/// gave it a second one — `POST …/workflows/validate`, which is in the default
/// build — and gating a shared validator behind a feature the caller does not
/// have is precisely what let the create surfaces drift apart in issue #168
/// (see the `create_company_workflow` re-export note in `super`). Every callee
/// below is already ungated.
///
/// `source_dir` must be the SAME one the caller would hand
/// [`create_company_workflow`]. It feeds exactly one rule — `workflow_id_exists`
/// → [`seed_file_exists`] — and passing `None` where create passes a real
/// directory makes this refuse a `sub_workflow` node naming a graph that lives
/// only as `<source_dir>/workflows/<wid>.toml`, which create accepts. That is a
/// different *verdict*, not a different sentence, and it is exactly the failure
/// a pre-flight exists to prevent (review of #1074; hosted tenants have no
/// source dir and never saw it).
///
/// `wired_channels` is the deployment's deliverable channel set (issue #1191):
/// `None` means the caller cannot see the wiring, and the `channel`-target rule
/// is skipped rather than guessed at — see [`validate_draft_against_record`].
///
/// `previous_owner_desk` is the desk the draft's saved counterpart already
/// carries, for a caller that is pre-flighting an EDIT of an existing workflow
/// (issue #1882 review, PR #1882 bot finding, comment 3879878907). `None` for a
/// create-shaped pre-flight, which has no stored body to grandfather against.
/// See the `owner_desk` block in [`validate_draft_against_record`]: an unchanged
/// stale desk is carried forward rather than refused, so an unrelated
/// correction cannot be blocked by a field the caller never touched.
pub(crate) fn courtesy_validate_draft(
    draft: &RawWorkflow,
    record: &CompanyRecord,
    source_dir: Option<&Path>,
    wired_channels: Option<&[String]>,
    previous_owner_desk: Option<&str>,
) -> Result<()> {
    validate_draft_shape(draft)?;
    // Record cross-check BEFORE the render → parse round trip, matching
    // `create_company_workflow`'s order (`validate_draft_against_record` at the
    // top of its write section, `parse_workflow` after). It used to run after,
    // which meant a draft violating both a record rule and a graph rule was
    // refused here for the graph problem and there for the record one — the same
    // verdict, a different sentence. Issue #1074 made that visible by putting a
    // pre-flight route on this function: a pre-flight that names a different
    // problem than the submit is worse than none.
    //
    // `previous_owner_desk` is the caller's (issue #1882 review). A caller that
    // holds the saved body — the fix-from-run copilot, which seeds its spec from
    // exactly that body — passes it, and gets the same grandfathering
    // `update_company_workflow` applies under the write lock: an unchanged stale
    // desk is carried, not refused. A caller with no stored body to compare
    // against passes `None` and keeps the KNOWN, documented asymmetry that used
    // to be unconditional here: this lockless pre-flight can then return a
    // false-negative `400` on an edit the real save would accept, the same
    // tolerated direction as the id/name-uniqueness gap documented above
    // `validate_workflow`. Never the other way around: it cannot pass a desk the
    // write would refuse.
    //
    // The resolved-id return (issue #1882 review) is discarded here: this
    // draft is a caller's borrowed copy that this pre-flight never persists,
    // so there is nothing to normalize it into.
    validate_draft_against_record(
        draft,
        record,
        source_dir,
        wired_channels,
        previous_owner_desk,
    )?;
    let toml_src = render_workflow(draft)?;
    if toml_src.len() > MAX_WORKFLOW_TOML_BYTES {
        return Err(over_cap_error(toml_src.len()));
    }
    parse_workflow(&toml_src).map_err(|err| match err {
        // A structural validation failure of the rendered draft becomes a
        // structured `WorkflowInvalid` 400 (issue #1016). These graph-level
        // problems (an inescapable cycle, an unreachable node) name no single
        // node, so they carry `node_id: None` — the per-node/field problems come
        // from `validate_draft_against_record`, which runs first.
        OpenCompanyError::DataInvalid { problems, .. } => OpenCompanyError::WorkflowInvalid {
            problems: problems.into_iter().map(WorkflowProblem::from).collect(),
        },
        OpenCompanyError::DataParse { message, .. } => OpenCompanyError::InvalidRequest(message),
        other => other,
    })?;

    Ok(())
}

/// Lowers a copilot draft [`WorkflowGraphSpec`] into the tinyflows
/// [`WorkflowGraph`](tinyflows::model::WorkflowGraph) the run-time gates read,
/// through the SAME `RawWorkflow → render → parse → translate` pipeline the
/// create path uses (issue #840). It is the seam the create-time copilot's
/// `check_workflow` tool runs [`tinyflows::gates::failures`] over, so a draft is
/// checked against exactly the graph the engine would compile — not a second,
/// drifting translation.
///
/// Fallible on the render/parse half: a spec whose kind or shape `parse_workflow`
/// refuses cannot be translated, and the error is mapped to an actionable
/// [`InvalidRequest`](OpenCompanyError::InvalidRequest) the same way
/// [`courtesy_validate_draft`] maps it — so the tool hands the model one honest
/// sentence rather than a 500.
///
/// Gated with the copilot it serves (its only caller is
/// `crate::harness::workflow_build`), so it is not dead code in the default build.
#[cfg(feature = "openhuman")]
pub(crate) fn workflow_graph_from_spec(
    spec: &WorkflowGraphSpec,
) -> Result<tinyflows::model::WorkflowGraph> {
    let raw = raw_workflow_from_spec(spec)?;
    let toml_src = render_workflow(&raw)?;
    let file = parse_workflow(&toml_src).map_err(|err| match err {
        // A structural validation failure of the rendered draft becomes a
        // structured `WorkflowInvalid` 400 (issue #1016). These graph-level
        // problems (an inescapable cycle, an unreachable node) name no single
        // node, so they carry `node_id: None` — the per-node/field problems come
        // from `validate_draft_against_record`, which runs first.
        OpenCompanyError::DataInvalid { problems, .. } => OpenCompanyError::WorkflowInvalid {
            problems: problems.into_iter().map(WorkflowProblem::from).collect(),
        },
        OpenCompanyError::DataParse { message, .. } => OpenCompanyError::InvalidRequest(message),
        other => other,
    })?;
    Ok(crate::workflows::translate::translate(&file))
}

/// Journals a best-effort [`WorkflowEnabledChanged`](CompanyEvent::WorkflowEnabledChanged).
///
/// Best-effort in the same sense as every other write-path audit event here: the
/// flag is already persisted by the time this runs, so a journal failure is
/// logged and never rolls the change back. Shared by the three write paths so
/// the disarm rule and the operator toggle produce the same audit shape.
async fn journal_enabled_change(
    company: &CompanyId,
    events: Option<&Arc<dyn EventLog>>,
    wid: &str,
    name: &str,
    enabled: bool,
    reason: WorkflowEnabledReason,
) {
    if let Some(log) = events
        && let Err(err) = log
            .append(
                company,
                CompanyEvent::WorkflowEnabledChanged {
                    workflow_id: wid.to_string(),
                    name: name.to_string(),
                    enabled,
                    reason,
                    by: None,
                },
            )
            .await
    {
        tracing::warn!(
            company = %company,
            workflow = %wid,
            error = %err,
            "workflow enablement changed but audit journal append failed"
        );
    }
}

/// The validation that is a pure function of the draft — safe id, size caps,
/// exactly one `trigger` — shared verbatim by [`create_company_workflow`] and
/// [`update_company_workflow`] so a bad edit is refused on exactly the same
/// terms as a bad create. Runs before any lock is taken: nothing here reads the
/// company record.
fn validate_draft_shape(draft: &RawWorkflow) -> Result<()> {
    if !is_safe_workflow_id(&draft.id) {
        return Err(OpenCompanyError::InvalidRequest(
            language::WORKFLOW_ID_INVALID.to_string(),
        ));
    }
    if draft.id.len() > MAX_WORKFLOW_ID_LEN {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "a workflow id can be at most {MAX_WORKFLOW_ID_LEN} characters."
        )));
    }
    if draft.name.trim().len() > MAX_WORKFLOW_NAME_LEN {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "a workflow name can be at most {MAX_WORKFLOW_NAME_LEN} characters."
        )));
    }
    if draft.nodes.len() > MAX_WORKFLOW_NODES {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "a workflow can have at most {MAX_WORKFLOW_NODES} nodes (this one has {}).",
            draft.nodes.len()
        )));
    }
    if draft.edges.len() > MAX_WORKFLOW_EDGES {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "a workflow can have at most {MAX_WORKFLOW_EDGES} edges (this one has {}).",
            draft.edges.len()
        )));
    }

    // `parse_workflow` only rejects zero triggers (a saved graph may legally
    // have more than one entry point); the author-time path is stricter — a
    // graph written from the console must name exactly one starting point.
    let trigger_count = draft.nodes.iter().filter(|n| n.kind == "trigger").count();
    if trigger_count != 1 {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "a workflow needs exactly one `trigger` node to say what starts it (found {trigger_count})."
        )));
    }

    Ok(())
}

/// Cross-checks a draft graph against the company `record`: every `agent` node
/// names a real roster teammate (manifest agents ∪ operator overlay teammates),
/// and every `tool_call` node names a wired, granted tool. Shared verbatim by
/// [`create_company_workflow`] and [`update_company_workflow`] so a bad draft is
/// refused on exactly the same terms whichever surface authored it, and runs
/// **inside the write lock** because the roster and grants both come from the
/// loaded record.
///
/// This is an author-time convenience, not the enforcement point. The run-time
/// gate in [`WorkflowToolInvoker::invoke`](crate::workflows::caps) stays the
/// backstop: `[tools].allow` grants can be revoked after a graph is saved, and
/// seed / legacy graphs never pass through this create path at all — so passing
/// here means the draft was coherent when written, not that a run is forever
/// permitted.
///
/// `wired_channels` is the one fact here that is NOT a property of the record:
/// the deployment's deliverable channel set, supplied by the caller because
/// only a caller holding a `CompanyRuntime` can read it (issue #1191, following
/// the `mail_configured` / `wired_channels` precedent #1046 set in this file).
/// `None` means the caller cannot see the deployment's wiring — the agent tool
/// surfaces — and the `channel`-target rule is skipped rather than guessed at.
/// Returns the canonical desk id to normalize `draft.owner_desk` to (issue
/// #1882 review), or `None` when no normalization is needed — either the field
/// is unset/blank, it already holds the canonical id, or it failed to resolve
/// (grandfathered-unchanged or reported as a problem, both handled below).
/// `draft` stays `&RawWorkflow` here: this helper is also the lockless
/// pre-flight (`courtesy_validate_draft`), which validates a caller's copy it
/// never persists, so a mutable draft would be the wrong shape for that
/// caller. Only a caller that goes on to `render_workflow` the SAME draft it
/// passed in — `create_company_workflow`, `update_company_workflow` — needs to
/// apply the returned id back before rendering.
fn validate_draft_against_record(
    draft: &RawWorkflow,
    record: &CompanyRecord,
    source_dir: Option<&Path>,
    wired_channels: Option<&[String]>,
    previous_owner_desk: Option<&str>,
) -> Result<Option<String>> {
    let roster: HashSet<&str> = record
        .manifest
        .agents
        .iter()
        .map(|a| a.id.as_str())
        .chain(record.overlay_agents.iter().map(|a| a.id.as_str()))
        .collect();

    // Structured problems (issue #1016): the config gate, the sub_workflow
    // existence check, and dangling edge endpoints each carry the node id and
    // config field at fault, so the console can highlight the exact node + field.
    // They accumulate and are raised together as a `WorkflowInvalid` 400. The
    // roster/tool checks below keep their own early `InvalidRequest` (a bad
    // teammate/tool name is not a node-config problem the highlighter consumes).
    let mut problems: Vec<WorkflowProblem> = Vec::new();

    for node in &draft.nodes {
        match node.kind.as_str() {
            "agent" => match node.agent.as_deref() {
                Some(agent_id) if roster.contains(agent_id) => {}
                Some(agent_id) => {
                    return Err(OpenCompanyError::InvalidRequest(format!(
                        "node `{}` names teammate `{agent_id}`, which is not on this company's roster.",
                        node.id
                    )));
                }
                None => {
                    return Err(OpenCompanyError::InvalidRequest(format!(
                        "node `{}` is an agent node but names no teammate.",
                        node.id
                    )));
                }
            },
            "tool_call" => validate_tool_call_node(node, record)?,
            // Issue #981: an `email` destination on a company that does not
            // grant `email` is a graph every run denies. Checked here, beside
            // the `tool_call` grant gate, because it is the same *kind* of fact
            // — a record grant, not a live runtime one — and because this
            // helper is the shared create/update gate, so the orchestrator's
            // `create_workflow` tool is held to it too.
            //
            // Its sibling rule ("a `channel` target must be one this runtime can
            // deliver to") used to be excluded on the grounds that the
            // deliverable set is a property of the *running* company rather than
            // of its record — but that is an argument about data availability,
            // not about where the rule belongs, and #1046 had already settled it
            // the other way by threading `mail_configured` / `wired_channels` in
            // from the caller. #1191 moves it here for the same reason: living
            // on the write routes meant three of the five authoring paths
            // skipped it, and applying a copilot proposal persisted a graph the
            // editor then refused to save back.
            "output" => {
                #[cfg(feature = "openhuman")]
                validate_output_destination(node, record)?;
                problems.extend(output_destination_problems(node, wired_channels));
            }
            // Per-kind required config (issue #661, extended #1016): reject a
            // `condition` with no `field`, an `http_request` missing `method` or a
            // real `url`, a `switch` with no discriminant, a `transform` with no
            // `config.set`, a `split_out` with no `config.path`, or an
            // `output_parser` whose present keys are mistyped — the same gate the
            // on-disk `validate` applies, surfaced here as a structured 400 so the
            // console/builder draft path never persists a graph whose runtime
            // behaviour is silently wrong. `tool_call` keeps its richer
            // `validate_tool_call_node` (slug + namespace/grant); the structural
            // kinds share the helper.
            "condition" | "http_request" | "switch" | "transform" | "split_out"
            | "output_parser" => {
                let kind = match node.kind.as_str() {
                    "condition" => WorkflowNodeKind::Condition,
                    "http_request" => WorkflowNodeKind::HttpRequest,
                    "switch" => WorkflowNodeKind::Switch,
                    "transform" => WorkflowNodeKind::Transform,
                    "split_out" => WorkflowNodeKind::SplitOut,
                    _ => WorkflowNodeKind::OutputParser,
                };
                // Report EVERY missing-config problem for the node, not just the
                // first: an `http_request` missing both `method` AND `url` should
                // name both, since the draft path is where a human/model iterates.
                problems.extend(required_config_problems(
                    kind,
                    &node.id,
                    &format!("node `{}`", node.id),
                    node.config.as_ref(),
                ));
            }
            // A `sub_workflow` node names a saved workflow to run (issue #1016).
            // `parse_workflow` already rejects a missing/empty/inline/self `workflow_id`
            // structurally; here — where the record is available — reject a
            // `workflow_id` that names NO saved workflow this company can resolve,
            // so a rename or a typo is caught at author time instead of failing at
            // run. A self-reference is left to the structural check (it names a
            // clearer problem) rather than reported as "not saved".
            "sub_workflow" => {
                if let Some(wid) = node
                    .config
                    .as_ref()
                    .and_then(toml::Value::as_table)
                    .and_then(|table| table.get("workflow_id"))
                    .and_then(toml::Value::as_str)
                    && !wid.trim().is_empty()
                    && wid != draft.id
                    && !workflow_id_exists(source_dir, record, wid)
                {
                    problems.push(WorkflowProblem::node_field(
                        &node.id,
                        "workflow_id",
                        format!(
                            "node `{}` runs sub-workflow `{wid}`, which is not a saved workflow in \
                             this company — check the id or create the workflow first.",
                            node.id
                        ),
                    ));
                }
            }
            _ => {}
        }
    }

    // Dangling edge endpoints (issue #1016): an edge whose `from`/`to` names no
    // node is reported naming the ENDPOINT and the field, so the console can
    // highlight the id the author actually wrote. `parse_workflow` catches these
    // too, but only as flat `edge #N` strings — reporting them here first keeps
    // the structured node/field detail the flat pass would drop.
    let node_ids: HashSet<&str> = draft
        .nodes
        .iter()
        .filter(|n| !n.id.trim().is_empty())
        .map(|n| n.id.as_str())
        .collect();
    for edge in &draft.edges {
        if !edge.from.trim().is_empty() && !node_ids.contains(edge.from.as_str()) {
            problems.push(WorkflowProblem::node_field(
                &edge.from,
                "from",
                format!(
                    "an edge starts at `{}`, which is not a node in this workflow.",
                    edge.from
                ),
            ));
        }
        if !edge.to.trim().is_empty() && !node_ids.contains(edge.to.as_str()) {
            problems.push(WorkflowProblem::node_field(
                &edge.to,
                "to",
                format!(
                    "an edge points to `{}`, which is not a node in this workflow.",
                    edge.to
                ),
            ));
        }
    }

    // Owning desk (issue #1862 prerequisite): validated STRICTLY here, at
    // author time only — the same asymmetry #1757 already applies to output
    // destinations, and the reason is the same. `parse_workflow`'s lenient
    // load path (`validate(&raw, false)`) must NOT run this check: a saved
    // graph whose desk was since renamed or removed still has to load, or an
    // operator opening the editor on an otherwise-untouched workflow would be
    // greeted with a hard failure over a field they never looked at.
    //
    // Grandfathered when unchanged (issue #1882 review): the SAME "a field
    // nobody looked at" hazard applies to a *save*, not just a load, once the
    // console round-trips `ownerDesk` without offering any control to touch
    // it — an edit to an unrelated field would otherwise refuse to save at
    // all just because the desk it silently carries forward went stale.
    // `previous_owner_desk` is `None` on create (nothing to grandfather) and
    // the record's current stored value on update; only a desk that is both
    // unresolvable AND *different* from what was already on file is a
    // refusal — a newly typed/selected bad desk still is.
    //
    // Persist the resolved id, not the alias (issue #1882 review): a caller
    // may name the desk by its case-insensitive display name rather than its
    // id (`resolve_desk_id` accepts either), and `render_workflow` serializes
    // whatever string sits in `draft.owner_desk` verbatim — it has no access
    // to `record` to re-resolve at save time. Left alone, the stored graph
    // would carry the alias forward. If that overlay desk is later deleted
    // and a new one created reusing the same display name (desk creation
    // enforces id uniqueness, not name uniqueness), the stored alias would
    // silently start resolving to the NEW desk on next load, re-routing this
    // workflow's future blocker DMs to the wrong team with no edit ever made
    // to it. The id is stable for a desk's lifetime; the display name is not.
    //
    // Short-circuit an unchanged stored value BEFORE resolution runs at all
    // (issue #1882 review, PR #1882 bot finding, comment 3878829353): the
    // three arms below used to each re-derive their own "is this the same
    // as what's on file" guard (the `None` arm, and the ambiguous-arm's now-
    // removed `&& previous_owner_desk != Some(desk)`), but the `Some(resolved_id)`
    // arm that persists a resolution had none — and that is exactly the
    // grandfathering hole. A desk that owned this raw string can be deleted,
    // and a later, unrelated desk can take that same string as its *display
    // name* (id uniqueness is enforced, name uniqueness is not); on the next
    // unrelated save, `resolve_desk_id` answers with the new desk and this
    // code persisted that resolution — silently transferring ownership on an
    // edit that never touched `owner_desk`. Checking equality once, before
    // any of the three outcomes, means an unchanged raw value is never
    // resolved, normalized, ambiguity-checked, or refused — it is carried
    // forward exactly as stored, and only a genuinely NEW value reaches
    // `resolve_desk_id` at all.
    let mut resolved_owner_desk: Option<String> = None;
    if let Some(desk) = draft.owner_desk.as_deref()
        && !desk.trim().is_empty()
        && previous_owner_desk != Some(desk)
    {
        match record.resolve_desk_id(desk) {
            // Reject ambiguous display-name aliases (issue #1882 review, PR
            // #1882 bot finding, comment 3878620688): desk creation enforces
            // id uniqueness, not name uniqueness, so `resolve_desk_id`'s
            // alias pass can silently answer with whichever of two
            // same-named desks it iterates to first.
            Some(_) if record.desk_alias_is_ambiguous(desk) => {
                problems.push(WorkflowProblem {
                    node_id: None,
                    field: Some("owner_desk".to_string()),
                    message: format!(
                        "this workflow's owning desk `{desk}` names more than one desk on this \
                         company — use the desk's id instead of its display name to disambiguate."
                    ),
                });
            }
            Some(resolved_id) => {
                if resolved_id != desk {
                    resolved_owner_desk = Some(resolved_id);
                }
            }
            None => {
                problems.push(WorkflowProblem {
                    node_id: None,
                    field: Some("owner_desk".to_string()),
                    message: format!(
                        "this workflow's owning desk `{desk}` does not match any desk on this company \
                         — check the id or name, or clear the field."
                    ),
                });
            }
        }
    }

    if !problems.is_empty() {
        return Err(OpenCompanyError::WorkflowInvalid { problems });
    }

    // Condition branch labels must read `yes`/`no` at author time (issue #661).
    // `parse_workflow` is now LENIENT on this rule (issue #682) so pre-#661 saved
    // graphs still load, which means ALL author-time strictness for it has to
    // live here — mirroring the on-disk `validate` strict rule. The sole
    // exception is the `error` recovery edge of a condition that is also
    // `on_error = "route"`, whose routing is validated separately. The label is
    // lowercased + trimmed before matching (it is compared, never persisted as a
    // lookup key), matching the load rule's asymmetry vs the verbatim `slug`.
    let condition_ids: HashSet<&str> = draft
        .nodes
        .iter()
        .filter(|node| node.kind == "condition" && !node.id.trim().is_empty())
        .map(|node| node.id.as_str())
        .collect();
    let route_ids: HashSet<&str> = draft
        .nodes
        .iter()
        .filter(|node| node.on_error.as_deref() == Some("route") && !node.id.trim().is_empty())
        .map(|node| node.id.as_str())
        .collect();
    for edge in &draft.edges {
        if !condition_ids.contains(edge.from.as_str()) {
            continue;
        }
        let is_route_error =
            edge.label.as_deref() == Some("error") && route_ids.contains(edge.from.as_str());
        let is_yes_no = edge
            .label
            .as_deref()
            .map(|label| label.trim().to_ascii_lowercase())
            .is_some_and(|label| matches!(label.as_str(), "yes" | "no"));
        if !is_route_error && !is_yes_no {
            let shown = edge
                .label
                .as_deref()
                .map(|label| format!("`{label}`"))
                .unwrap_or_else(|| "no label".to_string());
            return Err(OpenCompanyError::InvalidRequest(format!(
                "an edge leaves condition node `{}` with {shown} — a condition's branches must be labeled `yes` or `no`.",
                edge.from
            )));
        }
    }

    Ok(resolved_owner_desk)
}

/// Author-time `tool_call` check: the slug must be a non-empty `config.slug`
/// string, and — under the `openhuman` feature — it must name a wired toolbelt
/// namespace the company's `[tools].allow` actually grants. These are the same
/// two gates [`WorkflowToolInvoker::invoke`](crate::workflows::caps) applies at
/// run time (`namespace_of` for "is it a wired tool", then the grant-glob rule
/// with the priced `search` family requiring an explicit grant), surfaced at
/// save so an author hears about an unwired or ungranted slug now instead of at
/// first run.
///
/// The namespace/grant half is `cfg(feature = "openhuman")` because
/// `namespace_of` / `grants_cover` live behind that feature; the slug-presence
/// half is unconditional. Under the default build only the presence check runs
/// and the run-time gate remains the backstop — `record` is still consumed
/// (the roster check in the caller always reads it), so the helper compiles
/// warning-free with and without the feature.
fn validate_tool_call_node(node: &RawNode, record: &CompanyRecord) -> Result<()> {
    // (a) UNGATED — a tool_call must name a non-empty `slug` string in `config`.
    let raw_slug = node
        .config
        .as_ref()
        .and_then(toml::Value::as_table)
        .and_then(|table| table.get("slug"))
        .and_then(toml::Value::as_str);
    // Absent, or empty / whitespace-only, names no tool at all.
    let Some(slug) = raw_slug.filter(|slug| !slug.trim().is_empty()) else {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "node `{}` is a tool_call but names no `slug` — set `config.slug` to the tool to run.",
            node.id
        )));
    };
    // The slug is stored and looked up at run time EXACTLY as written —
    // `render_workflow` persists the raw config, and `WorkflowToolInvoker` indexes
    // tools by literal `name()`. So a padded slug like `" csv_export "` would sail
    // through a trim-normalized check here yet be persisted (and looked up) padded,
    // halting the run on the very lookup this save-time gate promised to prevent.
    // Reject the padding rather than silently trimming, so the validated string
    // and the persisted/runtime string are the same literal.
    if slug != slug.trim() {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "node `{}` has a tool_call `slug` with leading or trailing whitespace (`{slug}`) — \
             set `config.slug` to the exact tool name.",
            node.id
        )));
    }

    #[cfg(feature = "openhuman")]
    {
        // (b) The slug must map to a wired toolbelt namespace — mirroring the
        // run-time gate's "not a wired workflow tool".
        let Some(namespace) = crate::harness::toolbelt::namespace_of(slug) else {
            return Err(OpenCompanyError::InvalidRequest(format!(
                "node `{}` calls tool `{slug}`, which is not a wired workflow tool.",
                node.id
            )));
        };
        // (b.1) The slug's namespace must be one the workflow `tool_call` invoker
        // actually wires (`WORKFLOW_TOOL_NAMESPACES` == shell/code/web/search).
        // `media` and `composio` map to a namespace but are agent-turn families the
        // invoker never builds, so a run passes the grant gate and then ALWAYS
        // misses the tool lookup ("not available in company workflows"). Reject them
        // here so this gate mirrors the run-time outcome instead of green-lighting a
        // slug that can never execute.
        if !crate::workflows::caps::WORKFLOW_TOOL_NAMESPACES.contains(&namespace) {
            return Err(OpenCompanyError::InvalidRequest(format!(
                "node `{}` calls tool `{slug}` (namespace `{namespace}`), which workflow \
                 `tool_call` nodes cannot run — `{namespace}` is an agent-turn tool family, not a \
                 workflow tool.",
                node.id
            )));
        }
        // (c) The company's `[tools].allow` must grant that namespace. The priced
        // `search` family needs an EXPLICIT `search` grant — a `*` wildcard never
        // confers it — while every other namespace uses the ordinary grant-glob
        // intersection, exactly the split `WorkflowToolInvoker::invoke` enforces.
        let grants = &record.manifest.tools.allow;
        if !crate::workflows::caps::grants_workflow_namespace(grants, namespace) {
            return Err(OpenCompanyError::InvalidRequest(format!(
                "node `{}` calls tool `{slug}` (namespace `{namespace}`), which this company's \
                 `[tools].allow` does not grant — grant it in `[tools].allow`.",
                node.id
            )));
        }
        // (d) Required args present (issue #813). The engine reads a `tool_call`'s
        // arguments from `config.args` (tinyflows
        // `nodes/integration/tool_call.rs`), so a known slug whose required args
        // are absent THERE runs and does nothing useful — the legal-but-empty
        // `read_workspace_state` (which cannot read a file anyway) was the case
        // that motivated this. Reject the missing args at author time, naming them
        // and what the tool is, so the console/copilot fixes it now instead of
        // shipping a dud node. Same philosophy as the #661 `required_config`
        // arm, one level down (the args sub-table, not the config root). Because
        // this is the SHARED create/update gate, a hand-author hears it at save
        // and the create-time copilot hears it via courtesy validation → one
        // corrective retry. A tool with no required args (`read_workspace_state`)
        // is unaffected here — its uselessness is handled by copilot grounding.
        if let Some(info) = crate::workflows::caps::workflow_tool_info(slug) {
            let args = node
                .config
                .as_ref()
                .and_then(toml::Value::as_table)
                .and_then(|table| table.get("args"))
                .and_then(toml::Value::as_table);
            let missing: Vec<&str> = info
                .required_args
                .iter()
                .copied()
                .filter(|arg| !tool_arg_present(args, arg))
                .collect();
            if !missing.is_empty() {
                return Err(OpenCompanyError::InvalidRequest(format!(
                    "node `{}` calls tool `{slug}` but its `config.args` is missing {} — {}.",
                    node.id,
                    missing
                        .iter()
                        .map(|arg| format!("`{arg}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    info.capability
                )));
            }
        }
    }
    #[cfg(not(feature = "openhuman"))]
    {
        // Under the default build the namespace/grant resolution is not compiled
        // in; the slug-presence check above still runs and the run-time gate
        // stays the backstop. Consume both bindings so neither warns.
        let _ = (slug, record);
    }

    Ok(())
}

/// The author-time destination problems for an `output` node, each carrying the
/// node id and the config field at fault so the console can anchor it on the
/// node the author actually wrote (issue #1191).
///
/// [`parse_workflow`] states the same rules and keeps them — it is the load
/// path, and a seed or legacy graph never passes through here. What it cannot
/// do is locate them: it builds flat `String`s, which
/// [`From<String>`](WorkflowProblem) turns into problems with no `node_id` and
/// no `field`, so the console rendered the sentence with no indication of where
/// it came from. Reported here first, with the locator; the flat pass never runs
/// because this returns before the render → parse round trip.
///
/// `wired_channels` is the deployment's deliverable set
/// ([`deliverable_channel_ids`](crate::company::CompanyRuntime::deliverable_channel_ids)),
/// or `None` when the caller has no runtime handle to read it from — the same
/// idiom [`workflow_effective_tool_slugs`] uses for its `wired` argument, and
/// the same meaning: `None` is "cannot see the wiring", never "nothing is
/// wired". `Some(&[])` DOES refuse every target, because a company with no
/// deliverable channel really can deliver nowhere.
///
/// Ungated on purpose: it reads nothing but the node and the `Vec<String>` the
/// caller supplies, so the default-lane HTTP routes are held to it exactly as
/// the harness lanes are.
fn output_destination_problems(
    node: &RawNode,
    wired_channels: Option<&[String]>,
) -> Vec<WorkflowProblem> {
    let mut problems = Vec::new();
    let Some(destination) = node.destination.as_ref() else {
        return problems;
    };
    if destination.kind.trim() != "channel" {
        return problems;
    }
    let target = destination.target.as_deref().map(str::trim).unwrap_or("");
    if target.is_empty() {
        problems.push(WorkflowProblem::node_field(
            &node.id,
            "destination.target",
            channel_destination_missing_target_message(&format!("node `{}`", node.id)),
        ));
        return problems;
    }
    // Issue #981, moved here by #1191: a `channel` target outside the set this
    // runtime can deliver to is a report that lands nowhere on EVERY run. It
    // used to live on the two write routes, which is why the three other callers
    // of this core — proposal apply, the orchestrator's `create_workflow` tool,
    // the agent `update_workflow` tool — skipped it entirely, and why its
    // refusal was a bare `InvalidRequest` with no `problems` array while every
    // sibling rule here answers with a located `WorkflowInvalid`.
    if let Some(wired) = wired_channels
        && !wired.iter().any(|id| id == target)
    {
        let live: Vec<&str> = wired.iter().map(String::as_str).collect();
        problems.push(WorkflowProblem::node_field(
            &node.id,
            "destination.target",
            // The sentence the write routes have always used, unchanged: the
            // node prefix, then the shared message delivery itself would carry.
            format!(
                "node `{}`: {}",
                node.id,
                crate::runtime::undeliverable_channel_message(target, &live)
            ),
        ));
    }
    problems
}

/// Author-time `output` destination check: an `email` destination needs the
/// company to grant the `email` namespace in `[tools].allow` (issue #981).
///
/// The mirror of delivery's FIRST email gate
/// ([`deliver_outputs`](crate::workflows::delivery), which answers a missing
/// grant with a `Denied` / [`EmailNotGranted`] row before it even looks at
/// whether a mailbox is wired), surfaced at save so an author hears about it
/// now instead of after a scheduled run nobody watched dropped its report. A
/// missing grant is a deployment-wide fact, exactly like naming the operator
/// channel: it denies *every* run of the graph, on every recipient, until
/// somebody edits the manifest.
///
/// **Only the grant half.** Delivery's later gates — a wired mailbox, and an
/// established inbound thread with the recipient (issue #170's reply-only
/// rule) — are per-run, per-recipient conditions an author-time check cannot
/// see, and refusing a save on them would refuse graphs that work. #1046 drew
/// the same line for the arm-time check
/// ([`destination_is_reachable`](crate::company::destination_is_reachable)),
/// which stops at the mailbox lever for the same reason.
///
/// Like the `tool_call` grant gate this is an author-time convenience, not the
/// enforcement point: a grant can be revoked after a graph is saved, and seed /
/// legacy graphs never pass through this create path at all, so delivery's own
/// refusal stays the backstop.
///
/// `cfg(feature = "openhuman")` because
/// [`grants_cover`](crate::harness::build::grants_cover) — the namespace
/// matcher delivery itself calls — lives behind that feature, as does the whole
/// `workflows` module that would run the graph. The default build links no
/// delivery path at all, so there is nothing there for this to guard.
///
/// [`EmailNotGranted`]: crate::ports::DeliveryReason::EmailNotGranted
#[cfg(feature = "openhuman")]
fn validate_output_destination(node: &RawNode, record: &CompanyRecord) -> Result<()> {
    let Some(destination) = node.destination.as_ref() else {
        return Ok(());
    };
    // Only `email`. A missing/unknown `kind` is `parse_workflow`'s to report —
    // it says something more specific, and reporting the wrong problem first is
    // worse than second. The `channel` arms are
    // `output_destination_problems`' (issue #1191).
    if destination.kind.trim() != "email" {
        return Ok(());
    }
    if crate::harness::build::grants_cover(&record.manifest.tools.allow, "email") {
        return Ok(());
    }
    Err(OpenCompanyError::InvalidRequest(format!(
        "node `{}` delivers its report to an email address, which this company's `[tools].allow` \
         does not grant — grant `email` in `[tools].allow`, or send the report to a wired channel.",
        node.id
    )))
}

/// Whether `[tools].allow` grants the namespace `info` belongs to — a catalogue
/// -shaped wrapper over
/// [`grants_workflow_namespace`](crate::workflows::caps::grants_workflow_namespace),
/// which is the rule itself and is shared with
/// [`validate_tool_call_node`] and the run-time
/// [`refusal_for`](crate::workflows::caps).
///
/// Exists only so the two grounding lists can filter
/// [`WORKFLOW_TOOL_CATALOG`](crate::workflows::caps) rows directly. Because the
/// catalogue is itself pinned to `namespace_of`, a slug passes here iff
/// validation would accept it — so what a caller is shown and what a proposed
/// `tool_call` node clears at courtesy validation cannot drift.
#[cfg(feature = "openhuman")]
fn grants_workflow_tool(
    grants: &[String],
    info: &crate::workflows::caps::WorkflowToolInfo,
) -> bool {
    crate::workflows::caps::grants_workflow_namespace(grants, info.namespace)
}

/// The tools a caller may ground a proposal on: catalogue, company grant, and
/// deployment wiring all agree (issues #753, #874). Both copilot surfaces read
/// it — the in-process create/fix builder and `GET …/workflows/tool-slugs` — so
/// neither can offer a tool the run would refuse.
///
/// `wired` is `None` when the deployment's wiring is not knowable (no harness
/// deps): the grant filter then stands alone, which is the widest honest answer
/// rather than a claim that nothing is wired.
///
/// Create validation intentionally remains **permissive** for a
/// granted-but-unwired tool so an operator may author now and wire the provider
/// later; this narrower set is grounding only, and
/// [`workflow_granted_but_unwired_tool_slugs`] names the difference so the gap
/// is reported rather than silently dropped.
///
/// Gated with the copilot it serves — the grant helpers live behind the
/// `openhuman` feature, so in the default build this would be dead code over
/// symbols that are not compiled.
#[cfg(feature = "openhuman")]
pub(crate) fn workflow_effective_tool_slugs(
    record: &CompanyRecord,
    wired: Option<&std::collections::BTreeSet<&'static str>>,
) -> Vec<String> {
    let grants = &record.manifest.tools.allow;
    crate::workflows::caps::WORKFLOW_TOOL_CATALOG
        .iter()
        .filter(|info| {
            grants_workflow_tool(grants, info)
                && wired.is_none_or(|namespaces| namespaces.contains(info.namespace))
        })
        .map(|info| info.slug.to_string())
        .collect()
}

/// The exact complement of [`workflow_effective_tool_slugs`] within the granted
/// set: tools this company holds a grant for that cannot run on **this**
/// deployment.
///
/// Reported rather than silently dropped (issue #874) so a reader can tell "this
/// company is not allowed that tool" — absent from both lists — from "allowed,
/// but nobody has configured the provider here". A copilot grounded on both
/// answers "that needs web search, which is not wired here" instead of either
/// proposing a doomed node or denying the tool exists.
///
/// Empty when `wired` is `None`: with the deployment unknowable, "which of these
/// are unwired" has no honest answer, and every granted slug stays in the
/// effective list.
#[cfg(feature = "openhuman")]
pub(crate) fn workflow_granted_but_unwired_tool_slugs(
    record: &CompanyRecord,
    wired: Option<&std::collections::BTreeSet<&'static str>>,
) -> Vec<String> {
    let grants = &record.manifest.tools.allow;
    crate::workflows::caps::WORKFLOW_TOOL_CATALOG
        .iter()
        .filter(|info| {
            grants_workflow_tool(grants, info)
                && !wired.is_none_or(|namespaces| namespaces.contains(info.namespace))
        })
        .map(|info| info.slug.to_string())
        .collect()
}

/// Whether a required `config.args` key is present and carries a usable value
/// (issue #813): a non-blank string — a `=`-expression that binds at run time
/// counts — or any non-null non-string value (a number, a non-empty array or
/// table). A blank string or an absent key is treated as missing.
#[cfg(feature = "openhuman")]
fn tool_arg_present(args: Option<&toml::Table>, key: &str) -> bool {
    match args.and_then(|table| table.get(key)) {
        Some(toml::Value::String(text)) => !text.trim().is_empty(),
        Some(toml::Value::Array(items)) => !items.is_empty(),
        Some(toml::Value::Table(table)) => !table.is_empty(),
        Some(_) => true, // integer / float / bool / datetime — presence is meaningful
        None => false,
    }
}

/// An opaque version token for a stored overlay body: the hex SHA-256 of the
/// TOML exactly as persisted.
///
/// The wire contract is "echo back what the read handed you" — callers must not
/// parse it, derive it, or compare it to anything but another token from the
/// same route. That is what lets the algorithm change later without a client
/// migration.
///
/// Hashing the body rather than stamping a counter or a timestamp means the
/// token is a pure function of what is stored: it needs no extra field on
/// [`OverlayWorkflow`], it is stable across a save that rewrites the record for
/// unrelated reasons, and two writers who happen to persist byte-identical TOML
/// do not conflict — because there is nothing to lose between them.
pub(crate) fn workflow_version(toml: &str) -> String {
    let digest = Sha256::digest(toml.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Whether `wid` is backed by a **seed file** in the company source tree.
///
/// Checked by path rather than by scanning: a *malformed* seed file still owns
/// its id — it would shadow an overlay body on read — and a scan that parses
/// would silently skip it.
///
/// This is the one probe behind three answers that must agree: create's
/// id-uniqueness 409, update/delete's "not editable from the console" 409, and
/// the read routes' `editable` flag. Duplicating it is how the console ends up
/// offering an Edit button for a graph the host will refuse.
/// The over-the-byte-cap refusal, in one place.
///
/// Create and the `/workflows/validate` pre-flight both raise it, and they used
/// to word it differently — "the rendered workflow is N bytes" vs "the proposed
/// workflow is N bytes". Same status and same verdict, but a pre-flight that
/// answers in different words than the submit is the drift #1074 is about, and
/// the PR promoted the identical-body claim to a documented contract. One
/// constructor is what makes the claim true by construction rather than by
/// two authors agreeing (review of #1074).
fn over_cap_error(len: usize) -> OpenCompanyError {
    OpenCompanyError::InvalidRequest(format!(
        "the rendered workflow is {len} bytes, over the {MAX_WORKFLOW_TOML_BYTES}-byte limit."
    ))
}

pub(crate) fn seed_file_exists(source_dir: Option<&Path>, wid: &str) -> bool {
    source_dir.is_some_and(|dir| dir.join("workflows").join(format!("{wid}.toml")).is_file())
}

/// Whether `wid` names a workflow this company can actually resolve and run as a
/// sub-workflow (issue #1016): a seed file, a saved overlay body, a
/// manifest-`enabled` id, or a global baseline workflow. Deliberately lenient —
/// it errs toward "exists" (globals are counted whether or not the company
/// disabled them) so a real reference is never rejected; the run-time resolver
/// stays the backstop for anything this author-time probe can't see. Used to
/// reject a `sub_workflow` node whose `workflow_id` names nothing at all.
fn workflow_id_exists(source_dir: Option<&Path>, record: &CompanyRecord, wid: &str) -> bool {
    seed_file_exists(source_dir, wid)
        || record.overlay_workflows.iter().any(|w| w.id == wid)
        || record.manifest.workflows.enabled.iter().any(|id| id == wid)
        || crate::globals::workflows().iter().any(|w| w.id == wid)
}

/// Resolves `wid` to the record's overlay body, or explains why it can't be
/// written to. Shared by update and delete so the two answer identically.
///
/// * seed-backed → [`Conflict`](OpenCompanyError::Conflict): the read path would
///   keep serving the seed, and a boot rebuild would resurrect it;
/// * enabled but bodiless → [`Conflict`](OpenCompanyError::Conflict): a
///   provisioned id with no graph in either source, so there is nothing to
///   replace or remove;
/// * unknown → [`CompanyNotFound`](OpenCompanyError::CompanyNotFound) (404).
///
/// Returns the index into `record.overlay_workflows`, so the caller can replace
/// the body **in place** and keep the picker's order stable across an edit.
fn locate_editable_overlay(
    record: &CompanyRecord,
    source_dir: Option<&Path>,
    wid: &str,
) -> Result<usize> {
    if seed_file_exists(source_dir, wid) {
        return Err(OpenCompanyError::Conflict(format!(
            "Workflow `{wid}` is defined by a file in the company source tree, so it can't be \
             changed or removed from the console. Edit `workflows/{wid}.toml` in the company \
             repository instead."
        )));
    }

    if let Some(index) = record.overlay_workflows.iter().position(|w| w.id == wid) {
        return Ok(index);
    }

    if record.manifest.workflows.enabled.iter().any(|id| id == wid) {
        return Err(OpenCompanyError::Conflict(format!(
            "Workflow `{wid}` is enabled for this company but has no saved graph to change or \
             remove — it was provisioned by name only."
        )));
    }

    Err(OpenCompanyError::CompanyNotFound(format!("workflow {wid}")))
}

/// Fails with a [`Conflict`](OpenCompanyError::Conflict) when the caller's
/// `expected` token disagrees with what is actually stored.
///
/// Called **inside** the write lock, immediately before the mutation — the whole
/// point is that the compare and the save are one critical section, so a writer
/// that lands in between cannot be overwritten. `None` is an unconditional
/// write; see the module docs for why that stays allowed.
fn check_expected_version(expected: Option<&str>, current_toml: &str) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let current = workflow_version(current_toml);
    if expected != current {
        return Err(OpenCompanyError::Conflict(format!(
            "This workflow changed since you loaded it (current version `{current}`). Reload it \
             to see the latest, then reapply your change."
        )));
    }
    Ok(())
}

/// Replaces an existing workflow graph wholesale, returning the parsed
/// [`WorkflowFile`] the union read path will now serve.
///
/// Runs [`create_company_workflow`]'s validation with three deltas:
///
/// 1. the id must resolve to an **overlay body** that is not shadowed by a seed
///    file (see [`locate_editable_overlay`]) — 409 if it is source-defined or
///    body-less, 404 if it is unknown;
/// 2. display-name uniqueness excludes this workflow's own current name, so
///    re-saving without renaming isn't a self-conflict;
/// 3. `expected_version`, when supplied, must match what is stored — compared
///    under the same lock as the save.
///
/// The overlay is replaced **in place** (order preserved, so the picker doesn't
/// reshuffle on an edit) and `[workflows].enabled` is left exactly as it was: a
/// workflow that was enabled stays enabled across an edit, and one that somehow
/// wasn't is not silently armed by saving it.
///
/// Issue #276 adds the disarm: if the stored graph had no trigger schedule and
/// the replacement does, the workflow is switched **off** in the same save, so a
/// cron introduced by an edit cannot fire before anyone has looked at it. See
/// the module docs for why an edit never arms in the other direction and why a
/// changed-but-already-present cron is left alone.
// Eight: the four store/log handles this write needs, the draft, the
// concurrency token, and (issue #1191) the deployment's deliverable channel set.
// Bundling them into a context struct would only move the same list one hop and
// break every caller for no legibility gain — `create_company_workflow` beside
// it takes the same shape minus the revision store and the token.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn update_company_workflow(
    company: &CompanyId,
    source_dir: Option<&Path>,
    store: &Arc<dyn CompanyStore>,
    revisions: &Arc<dyn WorkflowRevisionStore>,
    events: Option<&Arc<dyn EventLog>>,
    mut draft: RawWorkflow,
    expected_version: Option<&str>,
    wired_channels: Option<&[String]>,
) -> Result<WorkflowFile> {
    // --- Input normalization (before validation or locking) ------------------
    draft.owner_desk = RawWorkflow::normalize_owner_desk(draft.owner_desk.take());

    // --- Input validation (no lock; pure function of the draft) -------------
    validate_draft_shape(&draft)?;

    // --- Serialized write section -------------------------------------------
    let write_lock = company_write_lock(company);
    let _lock = write_lock.lock().await;

    let mut record = store
        .load(company)
        .await?
        .ok_or_else(|| OpenCompanyError::CompanyNotFound(company.to_string()))?;

    // Which overlay body are we replacing — and may we replace it at all?
    let index = locate_editable_overlay(&record, source_dir, &draft.id)?;

    // Optimistic concurrency, inside the lock and before any mutation.
    check_expected_version(expected_version, &record.overlay_workflows[index].toml)?;

    // Same record cross-check as create (roster + tool_call grants):
    // `parse_workflow` validates the graph's own shape but has no record to check
    // `agent`/`tool_call` node names against.
    //
    // `previous_owner_desk`, issue #1882 review: the desk on the body this
    // save is about to REPLACE, read under the same lock — so an unrelated
    // edit can still save when the workflow's desk went stale (renamed or
    // removed) since it was last touched, exactly as the lenient load path
    // already tolerates. A body that no longer parses grandfathers nothing,
    // the conservative reading matching `armed_before` below.
    let previous_owner_desk = parse_workflow(&record.overlay_workflows[index].toml)
        .ok()
        .and_then(|previous| previous.owner_desk);
    //
    // The check may return a resolved `owner_desk` (issue #1882 review): see
    // the normalization note on `validate_draft_against_record` for why the
    // draft's field is overwritten with it before `render_workflow` persists.
    if let Some(resolved) = validate_draft_against_record(
        &draft,
        &record,
        source_dir,
        wired_channels,
        previous_owner_desk.as_deref(),
    )? {
        draft.owner_desk = Some(resolved);
    }

    // Display-name uniqueness, MINUS this workflow's own current name — a
    // re-save that doesn't rename must not collide with itself. Every other
    // workflow's name is still guarded, so an edit can't take a sibling's name.
    let mut existing_names = existing_workflow_names(
        source_dir,
        &record.overlay_workflows,
        &record.manifest.workflows.enabled,
    );
    // A body that no longer parses contributes no name to the set above either
    // (the union scan skips it), so the two stay consistent.
    if let Ok(current) = parse_workflow(&record.overlay_workflows[index].toml) {
        existing_names.remove(&current.name.trim().to_ascii_lowercase());
    }
    if existing_names.contains(&draft.name.trim().to_ascii_lowercase()) {
        return Err(OpenCompanyError::Conflict(format!(
            "A workflow named `{}` already exists. Pick a different name.",
            draft.name.trim()
        )));
    }

    // Render and re-parse through the same structural validation a hand-authored
    // file passes, so a bad edit is a 400 rather than a persisted graph that
    // breaks the read routes.
    let toml_src = render_workflow(&draft)?;
    if toml_src.len() > MAX_WORKFLOW_TOML_BYTES {
        return Err(over_cap_error(toml_src.len()));
    }
    let file = parse_workflow(&toml_src).map_err(|err| match err {
        // A structural validation failure of the rendered draft becomes a
        // structured `WorkflowInvalid` 400 (issue #1016). These graph-level
        // problems (an inescapable cycle, an unreachable node) name no single
        // node, so they carry `node_id: None` — the per-node/field problems come
        // from `validate_draft_against_record`, which runs first.
        OpenCompanyError::DataInvalid { problems, .. } => OpenCompanyError::WorkflowInvalid {
            problems: problems.into_iter().map(WorkflowProblem::from).collect(),
        },
        OpenCompanyError::DataParse { message, .. } => OpenCompanyError::InvalidRequest(message),
        other => other,
    })?;

    // Issue #276: did this edit arm a schedule that was not armed before?
    // Measured against the body being REPLACED, read under the same lock as the
    // write — not against anything the caller supplied, which is what makes the
    // rule impossible to talk out of with a crafted request.
    //
    // A stored body that no longer parses counts as "had no schedule", so an
    // edit that repairs a corrupt graph into a scheduled one disarms. That is
    // the conservative reading of an unreadable prior state, and it is the right
    // one: nobody can have reviewed a schedule the host could not read.
    let armed_before = parse_workflow(&record.overlay_workflows[index].toml)
        .ok()
        .and_then(|previous| previous.trigger_schedule().map(str::to_string))
        .is_some();
    let disarmed = file.trigger_schedule().is_some() && !armed_before;

    // Issue #274: snapshot the body we are about to overwrite, so the edit is
    // undoable. Captured HERE — inside the write lock, holding the prior TOML —
    // because this is the only instant a snapshot is race-free (the same reason
    // OpenHuman's `flow_revisions` insert rides inside the guarded UPDATE).
    //
    // Ordering is load-bearing: push the revision BEFORE `store.save`. A failed
    // push aborts the edit and the prior body is still the live one, so nothing
    // is lost — which is the exact failure this feature exists to prevent.
    // Save-first would risk overwriting the body and then losing its only copy.
    //
    // Deduped against a no-op save: when the new TOML is byte-identical to the
    // prior, there is nothing to lose — the version token already defines those
    // two as the same graph — so no snapshot is taken.
    let prior_toml = record.overlay_workflows[index].toml.clone();
    if prior_toml != toml_src {
        // Name from the prior parsed body; fall back to the id when it no longer
        // parses (a corrupt body still deserves a recoverable snapshot).
        let prior_name = parse_workflow(&prior_toml)
            .map(|f| f.name)
            .unwrap_or_else(|_| file.id.clone());
        let revision =
            WorkflowRevisionRecord::new(file.id.clone(), prior_name, prior_toml, now_millis());
        revisions.push_revision(company, &revision).await?;
    }

    // Replace in place: same slot, same order, so the picker doesn't reshuffle.
    record.overlay_workflows[index] = OverlayWorkflow {
        id: file.id.clone(),
        toml: toml_src,
    };
    if disarmed {
        // Same save as the new body, for the same reason create writes both at
        // once: a tick reads one record or the other, so the newly scheduled
        // graph is never visible to the scheduler while still armed.
        record.set_workflow_enabled(&file.id, false);
    }
    store.save(&record).await?;

    drop(_lock);

    // Best-effort audit journal — id and name only, never the body.
    if let Some(log) = events
        && let Err(err) = log
            .append(
                company,
                CompanyEvent::WorkflowUpdated {
                    workflow_id: file.id.clone(),
                    name: file.name.clone(),
                    by: None,
                },
            )
            .await
    {
        tracing::warn!(
            company = %company,
            workflow = %file.id,
            error = %err,
            "workflow updated but audit journal append failed"
        );
    }

    if disarmed {
        journal_enabled_change(
            company,
            events,
            &file.id,
            &file.name,
            false,
            WorkflowEnabledReason::Disarmed,
        )
        .await;
        tracing::info!(
            company = %company,
            workflow = %file.id,
            "edit added a schedule; workflow switched off pending review"
        );
    }

    Ok(file)
}

/// Restores a workflow to one of its captured revisions (issue #274), returning
/// the parsed [`WorkflowFile`] the union read path will now serve.
///
/// A rollback is **not** a special write path — it is an ordinary edit whose new
/// body happens to be an old one. It loads the revision (scoped to `wid`, so one
/// workflow's snapshot can never be restored onto another), converts its stored
/// TOML back into a draft, and routes it through
/// [`update_company_workflow`] unchanged. That reuse is deliberate and buys four
/// properties for free:
///
/// * **Re-validation against the *current* record.** A revision that named a
///   teammate who has since been removed is a `400`, not a broken restore — the
///   same roster/tool check every edit passes.
/// * **The rollback is itself undoable.** `update_company_workflow` snapshots the
///   *current* body before overwriting it, so restoring A over B captures B — a
///   rollback can be rolled back.
/// * **Optimistic concurrency.** `expected_version`, when supplied, is the token
///   of the body being replaced; a stale one is a `409`, so a rollback cannot
///   silently clobber a concurrent edit.
/// * **The #276 disarm.** If the restored graph carries a schedule the live one
///   lacked, it lands switched **off** — a restored cron cannot fire before
///   anyone has reviewed it.
///
/// Statuses: `400` (the revision is invalid against the current record), `404`
/// (unknown `wid` or unknown `rev_id`), `409` (seed-backed / body-less `wid`, a
/// stale token, or a name collision).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn rollback_company_workflow(
    company: &CompanyId,
    source_dir: Option<&Path>,
    store: &Arc<dyn CompanyStore>,
    revisions: &Arc<dyn WorkflowRevisionStore>,
    events: Option<&Arc<dyn EventLog>>,
    wid: &str,
    rev_id: &str,
    expected_version: Option<&str>,
) -> Result<WorkflowFile> {
    if !is_safe_workflow_id(wid) {
        return Err(OpenCompanyError::InvalidRequest(
            language::WORKFLOW_ID_INVALID.to_string(),
        ));
    }

    // Load the snapshot, scoped to this workflow. Unknown wid or unknown rev both
    // read as "nothing to restore" → 404.
    let revision = revisions
        .get_revision(company, wid, rev_id)
        .await?
        .ok_or_else(|| {
            OpenCompanyError::CompanyNotFound(format!("workflow {wid} revision {rev_id}"))
        })?;

    // Convert the captured body back into an editable draft and pin its id to
    // `wid`: a rollback restores a workflow **in place**, never renames it, and a
    // hand-mangled revision body must not be able to retarget another workflow.
    let mut draft = raw_workflow_from_toml(&revision.toml)?;
    draft.id = wid.to_string();

    // `wired_channels: None` — a rollback restores a body this company already
    // saved, and the channel rule was never run on this path. Refusing a
    // rollback because a desk was renamed since would strand the operator on the
    // broken revision with no way back; the arm-time gate (#1046) and delivery's
    // own refusal still stand between a restored graph and a silent drop.
    update_company_workflow(
        company,
        source_dir,
        store,
        revisions,
        events,
        draft,
        expected_version,
        None,
    )
    .await
}

/// Switches a workflow on or off without touching its graph (issue #276).
///
/// Returns `true` when the record actually changed, `false` when the workflow
/// was already in the requested state — the caller reports `200` either way, so
/// a double-click is a no-op rather than a second journal entry.
///
/// # What may be toggled, and why it is a wider set than edit and delete
///
/// Any id the company actually answers for with a **graph** — a seed file or an
/// overlay body. Membership is decided by whether a body *exists*, not by
/// whether it parses: a stored graph the host can no longer read still toggles
/// (journalling under its id), because it is exactly the kind an operator most
/// wants stopped and refusing would leave them nothing to do about it. That is
/// deliberately broader than [`update_company_workflow`] and
/// [`delete_company_workflow`], which are overlay-only:
///
/// * A **seed-backed** workflow can be paused. Editing or deleting one is
///   refused because the read path would keep serving the seed and a boot
///   rebuild would resurrect the change — neither applies here. The switch lives
///   on the record, the source tree is untouched, and pausing can only ever
///   *remove* capability, so it cannot let a runtime write outlive a seed
///   rollback the way a record-wins `[tools]` or `[policy]` merge could. An
///   operator who cannot stop a committed cron without a redeploy has no pause
///   switch at all, which is issue #276(a) verbatim.
/// * A **bodiless** manifest-`enabled` id is refused with a
///   [`Conflict`](OpenCompanyError::Conflict), same as edit and delete: it is a
///   name with no graph, so there is no schedule to stop.
/// * An **unknown** id is a [`CompanyNotFound`](OpenCompanyError::CompanyNotFound)
///   (404).
///
/// # No version token, deliberately
///
/// `PUT`/`DELETE` take an `expectedVersion` because two consoles editing one
/// graph can silently lose an edit. A switch has no such hazard: it carries no
/// content to overwrite, both operators can see the resulting state, and
/// last-write-wins is what a light switch already means. Requiring a token would
/// also make a seed-backed workflow untoggleable, since only overlay bodies have
/// one.
///
/// `mail_configured` and `wired_channels` are the deployment's delivery
/// capability (issue #1046) — whether a mailbox is wired and which channels are
/// deliverable — which the arm-time undeliverable-schedule check needs and the
/// caller reads off the runtime.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn set_company_workflow_enabled(
    company: &CompanyId,
    source_dir: Option<&Path>,
    store: &Arc<dyn CompanyStore>,
    events: Option<&Arc<dyn EventLog>>,
    wid: &str,
    enabled: bool,
    mail_configured: bool,
    wired_channels: &[String],
) -> Result<bool> {
    if !is_safe_workflow_id(wid) {
        return Err(OpenCompanyError::InvalidRequest(
            language::WORKFLOW_ID_INVALID.to_string(),
        ));
    }

    let write_lock = company_write_lock(company);
    let _lock = write_lock.lock().await;

    let mut record = store
        .load(company)
        .await?
        .ok_or_else(|| OpenCompanyError::CompanyNotFound(company.to_string()))?;

    // Does this company answer for `wid` with a graph at all? Asked as "is there
    // a body" rather than "does it parse", because the two are different states
    // and only one of them is the operator's fault.
    //
    // `load_workflow_union` returns `Err` for a body that exists but no longer
    // parses, and `Ok(None)` only when there is nothing there. Collapsing both
    // into "not found" — which an earlier revision did, with `.ok().flatten()` —
    // sent a corrupt-graph id down the bodiless branch and told the operator it
    // "was provisioned by name only". That is false for a workflow they created,
    // and it leaves them with no way to pause it and no accurate reason why.
    //
    // A **global** graph counts too, as long as this company has not dropped it
    // outright via `[globals].disable` — a global has no seed file and no
    // overlay body of its own, so without this arm it read exactly like a
    // bodiless manifest-`enabled` id and could never be paused from here,
    // despite `disabled_workflows` (the pause flag this function writes) being
    // a wholly separate mechanism from the globals opt-out.
    let is_undisabled_global =
        !crate::globals::disabled(&record.manifest.globals.disable, "workflow", wid)
            && crate::globals::workflows().iter().any(|w| w.id == wid);
    let has_body = seed_file_exists(source_dir, wid)
        || record.overlay_workflows.iter().any(|w| w.id == wid)
        || is_undisabled_global;
    if !has_body {
        if record.manifest.workflows.enabled.iter().any(|id| id == wid) {
            return Err(OpenCompanyError::Conflict(format!(
                "Workflow `{wid}` is enabled for this company but has no saved graph, so there is \
                 no schedule to switch off — it was provisioned by name only."
            )));
        }
        return Err(OpenCompanyError::CompanyNotFound(format!("workflow {wid}")));
    }

    // The display name for the journal, read before anything changes. A body
    // that no longer parses still toggles — the same call
    // [`delete_company_workflow`] makes, and for the same reason: a graph the
    // host cannot read is exactly the kind an operator most wants to stop, and
    // refusing would leave them nothing to do about it. It just journals under
    // its id.
    let file = crate::company::load_workflow_with_globals(
        source_dir,
        &record.overlay_workflows,
        &record.manifest.globals.disable,
        wid,
    )
    .ok()
    .flatten();
    let name = file
        .as_ref()
        .map(|file| file.name.clone())
        .unwrap_or_else(|| wid.to_string());

    // Issue #976: arming is where the promise is made, so it is where the
    // promise is checked. A stage-less graph with a schedule fires on time, runs
    // nothing and reports nothing — `campaign` on staging is exactly that, one
    // resume away from a schedule it cannot keep.
    //
    // **Refused here rather than at save, deliberately.** Saving a stub
    // mid-authoring is legitimate and the module is built for it: `parse_workflow`
    // was made lenient on purpose (#661) so the console can drop a trigger and
    // add stages afterwards, and refusing an empty graph at save would also
    // refuse every existing seed and legacy body on its next edit. Saving
    // promises nothing; switching a schedule on promises that something happens.
    // This is also the human gate #276 already forces a scheduled graph through,
    // and it has the parsed graph in hand for the journal name above — so the
    // check costs no extra load.
    //
    // Only `enabled`, and only when a schedule is what is being armed. Switching
    // such a workflow OFF stays allowed: an operator must always be able to stop
    // a thing, which is the same call the unparseable-body case above makes.
    // A manual (unscheduled) graph is left alone — running a stub by hand is the
    // author's own business, and the run says so through
    // [`STAGELESS_WORKFLOW_NOTICE`](crate::company::STAGELESS_WORKFLOW_NOTICE).
    if enabled
        && let Some(file) = file.as_ref()
        && file.trigger_schedule().is_some()
        && !file.has_runnable_node()
    {
        return Err(OpenCompanyError::InvalidRequest(
            crate::company::STAGELESS_SCHEDULE_REFUSAL.to_string(),
        ));
    }

    // Issue #1046: the delivery-side sibling of the stage-less refusal above. A
    // scheduled graph that runs a stage but can only deliver its report to a
    // place this deployment cannot reach — `owner` with no mailbox (which falls
    // back to the operator channel and is discarded), or a channel that isn't
    // wired — fires on time, runs, and drops the report unseen every time. So
    // arming, where the delivery promise is made, is where it is checked.
    //
    // Reuses the `file` already parsed for the stage-less check and the journal
    // name — zero extra load. `mail_configured` and `wired_channels` are the
    // deployment's delivery capability, computed by the caller from
    // `runtime.mail()` and `runtime.deliverable_channel_ids()` (the console
    // picker's own source of truth, #813) — the arm path cannot see them
    // otherwise.
    //
    // None-vs-any, matching the stage-less guard's minimalism: refuse only when
    // the graph *asks* to deliver somewhere (`has_output_destination`) and
    // **nothing** it asks for can land (`!has_deliverable_output`). A
    // drawer-only graph (outputs with no destination) promises no delivery and
    // is left alone; a partially-deliverable graph still arms. Switching such a
    // workflow OFF stays allowed, and a manual (unscheduled) graph is untouched
    // — same reasoning as the guard above.
    if enabled
        && let Some(file) = file.as_ref()
        && file.trigger_schedule().is_some()
        && file.has_output_destination()
        && !file.has_deliverable_output(mail_configured, wired_channels)
    {
        return Err(OpenCompanyError::InvalidRequest(
            crate::company::UNDELIVERABLE_SCHEDULE_REFUSAL.to_string(),
        ));
    }

    if !record.set_workflow_enabled(wid, enabled) {
        return Ok(false);
    }
    store.save(&record).await?;

    drop(_lock);

    journal_enabled_change(
        company,
        events,
        wid,
        &name,
        enabled,
        WorkflowEnabledReason::Operator,
    )
    .await;
    tracing::info!(
        company = %company,
        workflow = %wid,
        enabled,
        "workflow switched by an operator"
    );

    Ok(true)
}

/// Removes a workflow: its overlay body **and** its id in
/// `[workflows].enabled`, in one save.
///
/// Both halves matter. Dropping only the body would leave an enabled id the
/// picker still lists (under the id as its name, per `list_workflows`'s
/// fallback); dropping only the enabled id would leave a body the union read
/// path still serves and the scheduler still fires. One atomic save means there
/// is no window where a half-deleted workflow exists.
///
/// Gated exactly like [`update_company_workflow`] — source-defined and bodiless
/// ids are refused rather than half-removed — and honours the same optional
/// version token, so "delete the thing I was looking at" can't remove something
/// that changed underneath the operator.
///
/// After the committed save, three best-effort cascades tear down what the
/// workflow leaves behind: its durable scheduler fire ledger (issue #708), its
/// revision history (issue #274), and an audit-journal entry. The fire-ledger
/// purge runs **first** and **only after** the save has committed and the write
/// lock is dropped: purging before a successful save could strip a still-live
/// workflow's claim rows on a save failure (a #241-class cross-replica
/// double-fire); purging after means a delete+recreate of the same id — which
/// reuses the restart-stable `workflow-<id>` schedule key — starts against an
/// empty ledger, with no inherited anchor and every past minute claimable
/// again. A purge failure is logged, never rolled back: the workflow is already
/// gone, and the worst case is one bounded, logged reinstatement of the old
/// pre-fix behaviour — the same contract as the revision cascade below.
///
/// Returns the removed workflow's display name for the audit journal (falling
/// back to the id when the stored body no longer parses).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn delete_company_workflow(
    company: &CompanyId,
    source_dir: Option<&Path>,
    store: &Arc<dyn CompanyStore>,
    revisions: &Arc<dyn WorkflowRevisionStore>,
    schedule_fires: Option<&Arc<dyn ScheduleFireStore>>,
    events: Option<&Arc<dyn EventLog>>,
    wid: &str,
    expected_version: Option<&str>,
) -> Result<String> {
    if !is_safe_workflow_id(wid) {
        return Err(OpenCompanyError::InvalidRequest(
            language::WORKFLOW_ID_INVALID.to_string(),
        ));
    }

    let write_lock = company_write_lock(company);
    let _lock = write_lock.lock().await;

    let mut record = store
        .load(company)
        .await?
        .ok_or_else(|| OpenCompanyError::CompanyNotFound(company.to_string()))?;

    let index = locate_editable_overlay(&record, source_dir, wid)?;
    check_expected_version(expected_version, &record.overlay_workflows[index].toml)?;

    // The display name, read before the body goes away, so the journal entry can
    // name what was removed. A body that no longer parses still deletes — it is
    // exactly the kind an operator most wants gone — it just journals under its
    // id.
    let name = parse_workflow(&record.overlay_workflows[index].toml)
        .map(|f| f.name)
        .unwrap_or_else(|_| wid.to_string());

    // Body and enabled id, one save. `merge_enabled_workflows` (#208) rebuilds
    // `enabled` at boot from seed ids ∪ surviving overlay ids — with the body
    // gone there is nothing left to re-enable, which is what makes this durable
    // across a restart.
    record.overlay_workflows.remove(index);
    record.manifest.workflows.enabled.retain(|id| id != wid);
    // Issue #1017: purge the pause flag too. `disabled_workflows` is keyed by id,
    // so a workflow re-created under this id later would otherwise inherit the
    // deleted one's pause and stay silently off its schedule. `retain` is a purge,
    // NOT a re-arm: the "enabled == true" state is only ever reached through the
    // explicit enable route (see `CompanyRecord::set_workflow_enabled`), and this
    // id has no body left to run regardless.
    record.disabled_workflows.retain(|id| id != wid);
    store.save(&record).await?;

    drop(_lock);

    // Issue #708: purge the schedule's durable fire ledger, minting the key at
    // the ONE authoritative site (`workflow_schedule_id`) so this key can never
    // drift from the scheduler's. This runs AFTER the committed save on purpose
    // (see the doc): purging before a save that then fails would strip a
    // still-live workflow's claim rows and risk a #241-class double-fire.
    // Best-effort, exactly like the revision cascade below: the workflow is
    // already gone, so a failure is logged rather than rolled back — a leftover
    // ledger merely re-instates the bounded pre-fix behaviour once, on a
    // recreate of the same id.
    //
    // `schedule_fires` is `Option` for the same reason `events` is: not every
    // caller wires it. The HTTP delete path passes the runtime store (`Some`) —
    // that is the path a scheduled workflow can be deleted from, so it is the
    // only one that can orphan a ledger. The agent `delete_workflow` tool passes
    // `None`, and correctly: it refuses to delete a scheduled workflow at all
    // (`refuse_scheduled`), so no fire ledger can exist for it to leave behind.
    if let Some(fires) = schedule_fires {
        let schedule_id = workflow_schedule_id(wid);
        if let Err(err) = fires.delete_schedule_fires(company, &schedule_id).await {
            tracing::warn!(
                company = %company,
                workflow = %wid,
                schedule = %schedule_id,
                error = %err,
                "workflow deleted but its schedule fire ledger could not be purged"
            );
        }
    }

    // Issue #274: cascade the workflow's revision history away with it, so a
    // removed workflow leaves no orphaned snapshots behind. Best-effort in the
    // same sense as the audit journal below: the workflow is already gone by the
    // time this runs, so a failure is logged rather than rolling the delete back
    // (and a leftover ring is harmless — a re-created id starts its own history,
    // and nothing reads another workflow's rows).
    if let Err(err) = revisions.delete_revisions(company, wid).await {
        tracing::warn!(
            company = %company,
            workflow = %wid,
            error = %err,
            "workflow deleted but its revision history could not be cleared"
        );
    }

    if let Some(log) = events
        && let Err(err) = log
            .append(
                company,
                CompanyEvent::WorkflowDeleted {
                    workflow_id: wid.to_string(),
                    name: name.clone(),
                    by: None,
                },
            )
            .await
    {
        tracing::warn!(
            company = %company,
            workflow = %wid,
            error = %err,
            "workflow deleted but audit journal append failed"
        );
    }

    Ok(name)
}

/// The set of existing workflow display names (trimmed, lowercased) for a
/// company: every graph the union read path can serve — the seed
/// `workflows/*.toml` files and the record's overlay bodies — plus the
/// id-as-name fallback for each manifest-`enabled` id that has neither, exactly
/// how `list_workflows` names the same set. A malformed graph contributes no
/// name (it's skipped, same as the picker), so it can't false-positive a
/// conflict; an absent or unreadable source tree simply degrades to overlay ∪
/// enabled.
fn existing_workflow_names(
    source_dir: Option<&Path>,
    overlays: &[OverlayWorkflow],
    enabled: &[String],
) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut seen_ids = HashSet::new();

    for file in list_workflows_union(source_dir, overlays) {
        names.insert(file.name.trim().to_ascii_lowercase());
        seen_ids.insert(file.id);
    }
    // A malformed graph is skipped by the union scan above but still owns its
    // id, so mark those seen too — otherwise the enabled-fallback below would
    // re-add them as id-named entries.
    for overlay in overlays {
        seen_ids.insert(overlay.id.clone());
    }

    // Manifest-enabled ids with no loadable graph fall back to the id as their
    // display name (what `list_workflows` shows), so a new workflow can't
    // collide with that fallback name either.
    for id in enabled {
        if !seen_ids.contains(id) {
            names.insert(id.trim().to_ascii_lowercase());
        }
    }

    names
}

/// Whether `wid` is a single safe on-disk filename stem — no path separators, no
/// `..`, not empty — so it can't escape the `workflows/` directory.
fn is_safe_workflow_id(wid: &str) -> bool {
    use std::path::Component;
    let mut comps = Path::new(wid).components();
    matches!(comps.next(), Some(Component::Normal(_))) && comps.next().is_none()
}

#[cfg(test)]
#[path = "workflow_create_test_support.rs"]
mod test_support;
#[cfg(test)]
#[path = "workflow_create_arming_tests.rs"]
mod tests_arming;
#[cfg(test)]
#[path = "workflow_create_condition_tests.rs"]
mod tests_condition;
#[cfg(test)]
#[path = "workflow_create_delete_tests.rs"]
mod tests_delete;
#[cfg(test)]
#[path = "workflow_create_destination_tests.rs"]
mod tests_destination;
#[cfg(test)]
#[path = "workflow_create_happy_guardrail_tests.rs"]
mod tests_happy_guardrail;
#[cfg(test)]
#[path = "workflow_create_problems_tests.rs"]
mod tests_problems;
#[cfg(test)]
#[path = "workflow_create_revision_tests.rs"]
mod tests_revision;
#[cfg(test)]
#[path = "workflow_create_schedule_update_tests.rs"]
mod tests_schedule_update;
#[cfg(test)]
#[path = "workflow_create_tool_call_tests.rs"]
mod tests_tool_call;
