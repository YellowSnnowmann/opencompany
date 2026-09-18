use std::sync::Arc;

use serde_json::{Value, json};

use super::tests_pass_1::agent_deps;
use super::workflow_build_fixtures_tests::*;
use super::*;
use crate::company::CompanyManifest;
use crate::ports::UsageMeter;
use crate::ports::runs::{NewRun, RunStatus};
use crate::ports::tasks::TaskTitle;
use crate::ports::types::CompanyId;

/// [`MANIFEST`] plus one desk, so the runtime's deliverable channel set is
/// exactly `["engineering"]` (issue #1191). The default fixture declares no
/// desk, which makes the set empty — enough to prove the "nowhere to deliver"
/// fallback, useless for telling an accepted channel target from a refused one.
pub(crate) const DESK_MANIFEST: &str = r#"
[company]
name = "Acme"

[[agent]]
id = "maya"
role = "Writer"
tools = ["docs", "web"]

[[group_chat]]
id = "engineering"
name = "Engineering"
members = ["maya"]

[policy]
mode = "full"

[tools]
allow = ["docs", "web"]
"#;

/// [`runtime_with`], on a company that has a desk to deliver to.
pub(crate) async fn runtime_with_desk(
    model: Arc<ScriptedModel>,
) -> (tempfile::TempDir, Arc<CompanyRuntime>) {
    let home = tempfile::Builder::new()
        .prefix("opencompany-builder-desk-")
        .tempdir()
        .expect("tempdir");
    let desk_manifest: CompanyManifest =
        toml::from_str(DESK_MANIFEST).expect("the desk fixture manifest parses");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), desk_manifest)
        .with_id(CompanyId::new("acme"))
        .build()
        .await
        .expect("runtime");
    assert_eq!(
        runtime.deliverable_channel_ids(),
        vec!["operator".to_string(), "engineering".to_string()],
        "the fixture must have the operator channel plus exactly one desk channel, or these \
         tests prove nothing"
    );
    runtime.set_builder(Arc::new(WorkflowBuilder::new(model, "chat-v1")));
    (home, Arc::new(runtime))
}

pub(crate) async fn runtime_with(
    model: Arc<ScriptedModel>,
) -> (tempfile::TempDir, Arc<CompanyRuntime>) {
    let home = tempfile::Builder::new()
        .prefix("opencompany-builder-")
        .tempdir()
        .expect("tempdir");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_id(CompanyId::new("acme"))
        .build()
        .await
        .expect("runtime");
    runtime.set_builder(Arc::new(WorkflowBuilder::new(model, "chat-v1")));
    (home, Arc::new(runtime))
}

/// A runtime wired for the create-time copilot AGENT path (issue #840): both a
/// [`WorkflowBuilder`] (the route-gate + metering slug) and the
/// [`HarnessDeps`](crate::harness::HarnessDeps) the agent is built from, over the
/// same native `model`. An optional recording meter becomes `runtime.usage()`, so
/// a test can read back what the turn metered.
pub(crate) async fn runtime_with_agent(
    model: Arc<NativeCopilotModel>,
    meter: Option<Arc<RecordingUsageMeter>>,
) -> (tempfile::TempDir, Arc<CompanyRuntime>) {
    let home = tempfile::Builder::new()
        .prefix("opencompany-copilot-")
        .tempdir()
        .expect("tempdir");
    let mut builder = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_id(CompanyId::new("acme"));
    if let Some(meter) = &meter {
        builder = builder.with_usage(meter.clone() as Arc<dyn UsageMeter>);
    }
    let mut runtime = builder.build().await.expect("runtime");
    let deps = agent_deps(&runtime, model.clone() as Arc<dyn HarnessModel>);
    runtime.set_builder(Arc::new(WorkflowBuilder::new(
        model as Arc<dyn HarnessModel>,
        "chat-v1",
    )));
    runtime.set_workflow_harness_deps(deps);
    (home, Arc::new(runtime))
}

/// A `workflow`-deliverable card sitting In Progress, with an optional plan.
pub(crate) fn card(id: &str, plan: Option<crate::ports::tasks::TaskPlan>) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        title: TaskTitle::authored("Automate the weekly digest"),
        note: Some("It should go out every Monday morning.".to_string()),
        column: COLUMN_IN_PROGRESS.to_string(),
        priority: "medium".to_string(),
        assignee: "maya".to_string(),
        updated_at_millis: 7,
        origin: None,
        parent_task_id: None,
        output: None,
        plan,
        planning_attempts: Vec::new(),
        deliverable: TaskDeliverable::Workflow,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    }
}

pub(crate) async fn read(runtime: &Arc<CompanyRuntime>, id: &str) -> TaskRecord {
    runtime
        .tasks()
        .list(runtime.id())
        .await
        .expect("board")
        .into_iter()
        .find(|t| t.id == id)
        .expect("the card exists")
}

/// Mints the attempt row the dispatch edge would, so the test can read its
/// settle status back.
pub(crate) async fn open_run(runtime: &Arc<CompanyRuntime>, task_id: &str) -> String {
    runtime
        .runs()
        .create_run(
            runtime.id(),
            NewRun::for_task(crate::ports::generate_id(), task_id, "maya"),
        )
        .await
        .expect("mint the attempt row")
        .id
}

pub(crate) async fn run_status(runtime: &Arc<CompanyRuntime>, run_id: &str) -> RunStatus {
    runtime
        .runs()
        .get_run(runtime.id(), run_id)
        .await
        .expect("read")
        .expect("the attempt row exists")
        .status
}

/// authority has something to dedup an id and name against.
pub(crate) async fn seed_workflow(runtime: &Arc<CompanyRuntime>, id: &str, name: &str) {
    let spec: WorkflowGraphSpec = serde_json::from_value(serde_json::json!({
        "id": id,
        "name": name,
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start" },
            { "id": "done", "kind": "output", "name": "Report" }
        ],
        "edges": [{ "from": "start", "to": "done" }]
    }))
    .unwrap();
    let raw = raw_workflow_from_spec(&spec).unwrap();
    crate::company::create_company_workflow(
        runtime.id(),
        runtime.source_dir(),
        runtime.store(),
        Some(runtime.events()),
        raw,
        None,
        None,
    )
    .await
    .expect("seed workflow");
}

/// A propose call carrying a valid graph, then a final reply — the happy path
/// script.
pub(crate) fn propose_step(summary: &str, workflow: Value) -> NativeStep {
    NativeStep::call(
        "propose_company_workflow",
        json!({ "summary": summary, "workflow": workflow }),
    )
}
