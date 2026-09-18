use super::*;

#[tokio::test]
async fn run_workflow_tool_errors_when_no_runner_is_wired() {
    let dir = tempfile::tempdir().unwrap();
    seed_demo_workflow(dir.path());
    // A valid workflow on disk, but an empty handle → not wired.
    let tool = RunWorkflowTool::new(
        CompanyId::new("acme"),
        Some(dir.path().to_path_buf()),
        Arc::new(MemStore::default()),
        WorkflowRunnerHandle::default(),
        crate::runtime::RunSupervisor::default(),
        None,
        WorkflowRefQueue::default(),
        RunOutputCache::default(),
        None,
    );
    let result = tool
        .execute(json!({ "id": "demo" }))
        .await
        .expect("execute");
    assert!(result.is_error, "expected an error result");
    assert!(result.output_for_llm(false).contains("wired"), "{result:?}");
}

/// Issue #1865 (PR #1883 review comment 3877185396): an agent-started run
/// that the engine returns `Err` on is the second run-outcome chokepoint
/// `WorkflowSpawn` does not cover — console, scheduled, and resumed
/// failures all file a `workflow_run_failed` notification through that
/// type, but this tool's own `Ok(Err(err))` arm used to journal a finish
/// and stop, leaving every agent-started failure invisible to an operator
/// not watching this turn. Reused `crate::store::FsOps` as the
/// notification-store double, the same one `WorkflowSpawn`'s own
/// equivalent test (`a_failed_run_does_not_leak_the_raw_engine_error_into_its_notification`
/// in `runtime::workflow_spawn`) uses.
#[tokio::test]
async fn run_workflow_tool_files_a_notification_when_the_engine_run_fails() {
    let dir = tempfile::tempdir().unwrap();
    seed_demo_workflow(dir.path());
    let runner: Arc<dyn WorkflowRunner> = Arc::new(FailingRunner);
    let handle = WorkflowRunnerHandle::default();
    handle.set(&runner);
    let company = CompanyId::new("acme");
    let notifications: Arc<dyn NotificationStore> =
        Arc::new(crate::store::FsOps::new(dir.path().to_path_buf()));

    let tool = RunWorkflowTool::new(
        company.clone(),
        Some(dir.path().to_path_buf()),
        Arc::new(MemStore::default()),
        handle,
        crate::runtime::RunSupervisor::default(),
        None,
        WorkflowRefQueue::default(),
        RunOutputCache::default(),
        Some(notifications.clone()),
    );
    let result = tool
        .execute(json!({ "id": "demo" }))
        .await
        .expect("execute");
    assert!(result.is_error, "the engine failed: {result:?}");

    let notes = notifications
        .list(&company, "anyone")
        .await
        .expect("list notifications");
    let failed = notes
        .iter()
        .find(|n| n.notification.kind == "workflow_run_failed")
        .expect(
            "an agent-started run that fails must file the same durable notification a \
             console, scheduled, or resumed run does",
        );
    assert!(
        failed
            .notification
            .title
            .contains(crate::runtime::RUN_FAILED_DETAIL),
        "{:?}",
        failed.notification.title
    );
}

#[tokio::test]
async fn run_workflow_tool_errors_on_unknown_id() {
    let dir = tempfile::tempdir().unwrap();
    seed_demo_workflow(dir.path());
    let runner: Arc<dyn WorkflowRunner> = Arc::new(StubRunner::empty());
    let handle = WorkflowRunnerHandle::default();
    handle.set(&runner);

    let tool = RunWorkflowTool::new(
        CompanyId::new("acme"),
        Some(dir.path().to_path_buf()),
        Arc::new(MemStore::default()),
        handle,
        crate::runtime::RunSupervisor::default(),
        None,
        WorkflowRefQueue::default(),
        RunOutputCache::default(),
        None,
    );
    let result = tool
        .execute(json!({ "id": "nope" }))
        .await
        .expect("execute");
    assert!(result.is_error);
    assert!(
        result.output_for_llm(false).contains("No workflow with id"),
        "{result:?}"
    );
}

#[tokio::test]
async fn run_workflow_tool_requires_an_id() {
    let tool = RunWorkflowTool::new(
        CompanyId::new("acme"),
        None,
        Arc::new(MemStore::default()),
        WorkflowRunnerHandle::default(),
        crate::runtime::RunSupervisor::default(),
        None,
        WorkflowRefQueue::default(),
        RunOutputCache::default(),
        None,
    );
    let result = tool.execute(json!({})).await.expect("execute");
    assert!(result.is_error);
    assert!(
        result.output_for_llm(false).contains("`id` is required"),
        "{result:?}"
    );
}

#[tokio::test]
async fn run_workflow_tool_rejects_traversal_ids() {
    let runner: Arc<dyn WorkflowRunner> = Arc::new(StubRunner::empty());
    let handle = WorkflowRunnerHandle::default();
    handle.set(&runner);
    let tool = RunWorkflowTool::new(
        CompanyId::new("acme"),
        Some(std::path::PathBuf::from("/tmp")),
        Arc::new(MemStore::default()),
        handle,
        crate::runtime::RunSupervisor::default(),
        None,
        WorkflowRefQueue::default(),
        RunOutputCache::default(),
        None,
    );
    let result = tool
        .execute(json!({ "id": "../secrets" }))
        .await
        .expect("execute");
    assert!(result.is_error);
}

#[tokio::test]
async fn create_workflow_tool_then_run_workflow_tool() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));

    // Author the graph.
    let create = CreateWorkflowTool::new(
        company.clone(),
        Some(dir.path().to_path_buf()),
        store.clone(),
        None,
        WorkflowRefQueue::default(),
    );
    let created = create.execute(greeter_body()).await.expect("execute");
    assert!(!created.is_error, "create should succeed: {created:?}");
    assert!(
        created.output_for_llm(true).contains("run_workflow"),
        "{created:?}"
    );

    // It's enabled on the record.
    let record = store.load(&company).await.unwrap().unwrap();
    assert!(
        record
            .manifest
            .workflows
            .enabled
            .contains(&"greeter".to_string())
    );

    // And immediately runnable via the run tool over the same source dir.
    let runner: Arc<dyn WorkflowRunner> = Arc::new(StubRunner::new(WorkflowRun {
        output: json!({ "nodes": { "worker": { "items": ["hi"] } } }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        cancelled: false,
        nodes: Vec::new(),
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    }));
    let handle = WorkflowRunnerHandle::default();
    handle.set(&runner);
    let run = RunWorkflowTool::new(
        company.clone(),
        Some(dir.path().to_path_buf()),
        store.clone(),
        handle,
        crate::runtime::RunSupervisor::default(),
        None,
        WorkflowRefQueue::default(),
        RunOutputCache::default(),
        None,
    );
    let result = run
        .execute(json!({ "id": "greeter" }))
        .await
        .expect("execute");
    assert!(!result.is_error, "run should succeed: {result:?}");
    assert!(
        result.output_for_llm(true).contains("Greeter"),
        "{result:?}"
    );
}

/// Issue #401: the orchestrator's run tool refuses when the company is
/// already at its in-flight run ceiling. The refusal is a
/// `ToolResult::error` the agent should treat as "wait / stop one", NOT an
/// `Err`, and it registers nothing — a held guard stands in for the
/// in-flight run, so no wall-clock and no real second run is needed.
#[tokio::test]
async fn run_workflow_tool_refuses_at_the_in_flight_cap() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));

    // Author a runnable graph on disk so `execute` reaches the cap check.
    let create = CreateWorkflowTool::new(
        company.clone(),
        Some(dir.path().to_path_buf()),
        store.clone(),
        None,
        WorkflowRefQueue::default(),
    );
    assert!(
        !create
            .execute(greeter_body())
            .await
            .expect("execute")
            .is_error
    );

    // A supervisor with room for one run, whose only slot is already taken
    // by a (simulated) in-flight run held for the length of the test.
    let supervisor = crate::runtime::RunSupervisor::with_limit(1);
    let (_ctx, _held) = supervisor
        .begin("greeter", false)
        .expect("the held run fills the cap of 1");

    let runner: Arc<dyn WorkflowRunner> = Arc::new(StubRunner::new(WorkflowRun {
        output: json!({}),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        cancelled: false,
        nodes: Vec::new(),
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    }));
    let handle = WorkflowRunnerHandle::default();
    handle.set(&runner);
    let run = RunWorkflowTool::new(
        company.clone(),
        Some(dir.path().to_path_buf()),
        store.clone(),
        handle,
        supervisor.clone(),
        None,
        WorkflowRefQueue::default(),
        RunOutputCache::default(),
        None,
    );

    let result = run
        .execute(json!({ "id": "greeter" }))
        .await
        .expect("execute");
    assert!(result.is_error, "a run over the cap is refused: {result:?}");
    let text = result.output_for_llm(false);
    assert!(
        text.contains("wasn't started") && text.contains("maximum"),
        "the refusal names the cap and is actionable: {text}"
    );
    assert_eq!(
        supervisor.len(),
        1,
        "the refused run registered nothing — only the held run remains"
    );
}

/// Issue #339: the *"build us a process for this"* card. The graph is the
/// deliverable, so authoring it stages a link even though nothing has run —
/// and when the same turn goes on to run it, the pair collapses to the run,
/// which is the stronger link because it can show what executed.
#[tokio::test]
async fn authoring_a_workflow_stages_a_link_and_running_it_upgrades_it() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let refs = WorkflowRefQueue::default();

    let create = CreateWorkflowTool::new(
        company.clone(),
        Some(dir.path().to_path_buf()),
        store.clone(),
        None,
        refs.clone(),
    );
    assert!(
        !create
            .execute(greeter_body())
            .await
            .expect("execute")
            .is_error
    );
    assert_eq!(refs.queued(), 1, "the saved graph is a deliverable");

    // A rejected draft persists nothing, so it must stage nothing either.
    assert!(
        create
            .execute(json!({ "id": "greeter", "name": "Greeter", "nodes": [] }))
            .await
            .expect("execute")
            .is_error
    );
    assert_eq!(refs.queued(), 1, "a rejected draft is not a deliverable");

    let runner: Arc<dyn WorkflowRunner> = Arc::new(StubRunner::new(WorkflowRun {
        output: json!({ "nodes": { "worker": { "items": ["hi"] } } }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        cancelled: false,
        nodes: Vec::new(),
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    }));
    let handle = WorkflowRunnerHandle::default();
    handle.set(&runner);
    let run = RunWorkflowTool::new(
        company.clone(),
        Some(dir.path().to_path_buf()),
        store.clone(),
        handle,
        crate::runtime::RunSupervisor::default(),
        None,
        refs.clone(),
        RunOutputCache::default(),
        None,
    );
    assert!(
        !run.execute(json!({ "id": "greeter" }))
            .await
            .expect("execute")
            .is_error
    );

    let staged = refs.drain();
    assert_eq!(staged.len(), 1, "one workflow, one link: {staged:?}");
    assert_eq!(staged[0].workflow_id, "greeter");
    assert_eq!(staged[0].action, TaskOutputAction::Ran);
    assert!(staged[0].run_id.is_some());
}

#[tokio::test]
async fn create_workflow_tool_guardrail_failure_is_error_result() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let tool = CreateWorkflowTool::new(
        company,
        Some(dir.path().to_path_buf()),
        store,
        None,
        WorkflowRefQueue::default(),
    );
    // Zero triggers — a guardrail failure must be an is_error ToolResult, not
    // a raised anyhow error.
    let result = tool
        .execute(json!({
            "id": "bad",
            "name": "Bad",
            "nodes": [ { "id": "a", "kind": "output", "name": "A" } ],
            "edges": []
        }))
        .await
        .expect("execute returns a result, not an error");
    assert!(result.is_error, "{result:?}");
    assert!(
        result.output_for_llm(false).contains("trigger"),
        "{result:?}"
    );
}

/// Issue #168: a hosted tenant has no source directory, and the tool must
/// still create — the graph body is persisted on the record. It used to
/// refuse outright with "nowhere to save".
#[tokio::test]
async fn create_workflow_tool_creates_without_source_dir() {
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
            "id": "hosted",
            "name": "Hosted",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                { "id": "done", "kind": "output", "name": "Done" }
            ],
            "edges": [ { "from": "start", "to": "done" } ]
        }))
        .await
        .expect("execute");
    assert!(!result.is_error, "{result:?}");

    let record = store.load(&company).await.unwrap().unwrap();
    assert_eq!(record.overlay_workflows.len(), 1);
    assert_eq!(record.overlay_workflows[0].id, "hosted");

    // And it runs, with no source directory anywhere in the picture.
    let runner: Arc<dyn WorkflowRunner> = Arc::new(StubRunner::new(WorkflowRun {
        output: json!({ "nodes": { "done": { "items": ["ok"] } } }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        cancelled: false,
        nodes: Vec::new(),
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    }));
    let handle = WorkflowRunnerHandle::default();
    handle.set(&runner);
    let run = RunWorkflowTool::new(
        company,
        None,
        store,
        handle,
        crate::runtime::RunSupervisor::default(),
        None,
        WorkflowRefQueue::default(),
        RunOutputCache::default(),
        None,
    );
    let result = run
        .execute(json!({ "id": "hosted" }))
        .await
        .expect("execute");
    assert!(!result.is_error, "run should succeed: {result:?}");
    assert!(result.output_for_llm(true).contains("Hosted"), "{result:?}");
}

#[tokio::test]
async fn create_workflow_tool_errors_on_unreadable_args() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::default());
    let tool = CreateWorkflowTool::new(
        CompanyId::new("acme"),
        Some(dir.path().to_path_buf()),
        store,
        None,
        WorkflowRefQueue::default(),
    );
    // A non-object payload can't deserialize into the create body.
    let result = tool.execute(json!(42)).await.expect("execute");
    assert!(result.is_error);
    assert!(
        result.output_for_llm(false).contains("Couldn't read"),
        "{result:?}"
    );
}

/// Issue #661 (H1): a `tool_call` node authored with `config.slug` persists
/// the slug into the saved graph — the tool advertises `tool_call` and can
/// now actually author a working one. Round-trip proof: the rendered TOML on
/// the record carries `slug = "web_fetch"`.
#[tokio::test]
async fn create_workflow_tool_persists_tool_call_config_slug() {
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
            "id": "fetcher",
            "name": "Fetcher",
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
            .contains("slug = \"web_fetch\""),
        "the persisted graph carries the tool slug: {}",
        record.overlay_workflows[0].toml
    );
}

/// Issue #1882 (tinysweeper): every other external boundary that turns a
/// caller-supplied `ownerDesk` into a [`RawWorkflow`] runs it through
/// [`RawWorkflow::normalize_owner_desk`] — the HTTP create route
/// (`server::ops::workflows`) and the proposal-apply path
/// (`workflow_create::raw_workflow_from_spec`) — so a blank/whitespace
/// string is stored as `None`, not `Some("   ")`. The orchestrator's
/// `create_workflow` tool passed `args.owner_desk` straight through
/// instead, so a whitespace `ownerDesk` persisted verbatim in the graph's
/// TOML and would defeat the `is_none()` fallback
/// `apply_workflow_proposal` relies on later.
#[tokio::test]
async fn create_workflow_tool_normalizes_a_blank_owner_desk() {
    let company = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record_with_assistant(&company)));
    let tool = CreateWorkflowTool::new(
        company.clone(),
        None,
        store.clone(),
        None,
        WorkflowRefQueue::default(),
    );

    let mut body = greeter_body();
    body["ownerDesk"] = json!("   ");
    let result = tool.execute(body).await.expect("execute");
    assert!(!result.is_error, "{result:?}");

    let record = store.load(&company).await.unwrap().unwrap();
    assert_eq!(record.overlay_workflows.len(), 1);
    assert!(
        !record.overlay_workflows[0].toml.contains("owner_desk"),
        "a blank owner_desk must normalize to None and be omitted from the \
         persisted TOML, matching every other boundary that builds a \
         RawWorkflow: {}",
        record.overlay_workflows[0].toml
    );
}

/// PR #1882 review (bot finding on `orchestrator.rs:4788`).
/// `UpdateWorkflowTool`'s description (built from this same schema via
/// `create_graph_schema`) tells the agent to send `"ownerDesk": null` to
/// unassign a desk, and `an_update_can_explicitly_clear_owner_desk_with_null`
/// proves `execute` honors that. But `execute` is called directly in that
/// test, bypassing the boundary a schema-constrained tool-calling client
/// actually enforces: before this fix `ownerDesk` was declared bare
/// `"type": "string"`, so such a client would reject the `null` argument
/// before the call ever reached `execute`'s presence check, leaving the
/// advertised clear operation reachable in tests but not in the field.
#[test]
fn owner_desk_schema_permits_null() {
    let schema = create_workflow_parameters_schema();
    let owner_desk_type = &schema["properties"]["ownerDesk"]["type"];
    let permits_null = owner_desk_type
        .as_array()
        .map(|types| types.iter().any(|t| t == "null"))
        .unwrap_or(false);
    assert!(
        permits_null,
        "ownerDesk schema type must include \"null\" so a schema-constrained \
         client can send the explicit-clear value the tool description \
         promises; got {owner_desk_type:?}"
    );
}

/// Issue #661 (H1): an `output` node's `destination` flows through into the
/// saved graph — the persisted TOML carries the routed address.
#[tokio::test]
async fn create_workflow_tool_persists_output_destination() {
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
            "id": "reporter",
            "name": "Reporter",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start" },
                {
                    "id": "done",
                    "kind": "output",
                    "name": "Report",
                    "destination": { "kind": "email", "target": "ada@example.com" }
                }
            ],
            "edges": [ { "from": "start", "to": "done" } ]
        }))
        .await
        .expect("execute");
    assert!(!result.is_error, "{result:?}");

    let record = store.load(&company).await.unwrap().unwrap();
    assert_eq!(record.overlay_workflows.len(), 1);
    let saved = &record.overlay_workflows[0].toml;
    assert!(
        saved.contains("target = \"ada@example.com\""),
        "the persisted graph routes to the destination address: {saved}"
    );
}
