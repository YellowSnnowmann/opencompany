//! workflow_create: happy-path creation, guardrail validation failures, and the no-source-directory (hosted) case.

use super::test_support::*;
use super::*;

// --- happy path ----------------------------------------------------------

#[tokio::test]
async fn creates_enables_and_journals() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    let file = create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        Some(&log_dyn),
        valid_draft("greeter", "Greeter"),
        None,
        None,
    )
    .await
    .expect("creates");

    assert_eq!(file.id, "greeter");
    assert_eq!(file.nodes.len(), 3);

    // The body landed on the RECORD, not in the (read-only in hosted mode)
    // source tree — the whole point of #168.
    let record = store.load(&company).await.unwrap().unwrap();
    assert_eq!(record.overlay_workflows.len(), 1);
    assert_eq!(record.overlay_workflows[0].id, "greeter");
    assert!(
        !dir.path().join("workflows").exists(),
        "creation must not write into the company source tree"
    );

    // The persisted body re-loads to exactly what we returned (contract).
    let reloaded = load_workflow_union(Some(dir.path()), &record.overlay_workflows, &file.id)
        .expect("reloads")
        .expect("one file");
    assert_eq!(
        reloaded, file,
        "returned WorkflowFile must equal what the union read path serves"
    );

    // Enabled on the record.
    assert!(
        record
            .manifest
            .workflows
            .enabled
            .contains(&"greeter".to_string())
    );

    // Journaled a WorkflowCreated audit event.
    let events = log.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        CompanyEvent::WorkflowCreated {
            workflow_id,
            name,
            by,
        } => {
            assert_eq!(workflow_id, "greeter");
            assert_eq!(name, "Greeter");
            assert!(
                by.is_none(),
                "the orchestrator/no-actor path must journal an unattributed create"
            );
        }
        other => panic!("expected WorkflowCreated, got {other:?}"),
    }
}

/// Issue #1843: the REST create path passes `ScopedCompany::actor` through
/// as `by`, and it must land verbatim on the journaled event — this is the
/// per-user attribution the activation funnel's `IntegrationConnected`-style
/// signals eventually build on. Sibling of `creates_enables_and_journals`
/// above, which pins the complementary `None` (orchestrator/platform) path.
#[tokio::test]
async fn a_signed_in_actor_is_attributed_on_the_journaled_create() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();
    let actor = crate::ports::types::Actor {
        kind: crate::ports::types::ActorKind::User,
        id: "user-42".to_string(),
    };

    create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        Some(&log_dyn),
        valid_draft("greeter", "Greeter"),
        None,
        Some(actor.clone()),
    )
    .await
    .expect("creates");

    let events = log.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        CompanyEvent::WorkflowCreated { by, .. } => {
            assert_eq!(
                by.as_ref(),
                Some(&actor),
                "the signed-in actor must be attributed on the journaled create"
            );
        }
        other => panic!("expected WorkflowCreated, got {other:?}"),
    }
}

// --- guardrail failures --------------------------------------------------

/// A second create with the same id collides against the record's overlay.
#[tokio::test]
async fn duplicate_id_is_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        valid_draft("dup", "First"),
        None,
        None,
    )
    .await
    .expect("first create");
    let err = create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        valid_draft("dup", "Second name"),
        None,
        None,
    )
    .await
    .expect_err("second create with same id");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
}

#[tokio::test]
async fn duplicate_name_case_insensitive_is_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        valid_draft("one", "Greeter"),
        None,
        None,
    )
    .await
    .expect("first");
    let err = create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        valid_draft("two", "  GREETER  "),
        None,
        None,
    )
    .await
    .expect_err("name collides case-insensitively");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
}

#[tokio::test]
async fn unknown_roster_teammate_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let mut draft = valid_draft("wf", "WF");
    draft.nodes[1].agent = Some("ghost".to_string());

    let err = create_company_workflow(&company, Some(dir.path()), &store, None, draft, None, None)
        .await
        .expect_err("unknown teammate");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("ghost"), "{err}");
}

#[tokio::test]
async fn missing_agent_on_agent_node_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let mut draft = valid_draft("wf", "WF");
    draft.nodes[1].agent = None;

    let err = create_company_workflow(&company, Some(dir.path()), &store, None, draft, None, None)
        .await
        .expect_err("agent node with no teammate");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn zero_or_two_triggers_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    // Zero triggers.
    let mut zero = valid_draft("z", "Z");
    zero.nodes[0].kind = "output".to_string();
    let err = create_company_workflow(&company, Some(dir.path()), &store, None, zero, None, None)
        .await
        .expect_err("no trigger");
    assert!(err.to_string().contains("exactly one `trigger`"), "{err}");

    // Two triggers.
    let mut two = valid_draft("t", "T");
    two.nodes[2].kind = "trigger".to_string();
    let err = create_company_workflow(&company, Some(dir.path()), &store, None, two, None, None)
        .await
        .expect_err("two triggers");
    assert!(err.to_string().contains("exactly one `trigger`"), "{err}");
}

#[tokio::test]
async fn traversal_id_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let err = create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        valid_draft("../secrets", "Escape"),
        None,
        None,
    )
    .await
    .expect_err("traversal id");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn oversized_node_count_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let mut draft = valid_draft("big", "Big");
    for i in 0..MAX_WORKFLOW_NODES {
        draft.nodes.push(RawNode {
            id: format!("n{i}"),
            kind: "output".to_string(),
            name: format!("N{i}"),
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
        });
    }
    assert!(draft.nodes.len() > MAX_WORKFLOW_NODES);
    let err = create_company_workflow(&company, Some(dir.path()), &store, None, draft, None, None)
        .await
        .expect_err("too many nodes");
    assert!(err.to_string().contains("at most"), "{err}");
}

#[tokio::test]
async fn oversized_toml_bytes_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    // Stay within the node cap but blow the byte cap with a huge summary.
    let mut draft = valid_draft("fat", "Fat");
    draft.nodes[0].summary = Some("x".repeat(MAX_WORKFLOW_TOML_BYTES + 10));
    let err = create_company_workflow(&company, Some(dir.path()), &store, None, draft, None, None)
        .await
        .expect_err("too many bytes");
    assert!(err.to_string().contains("byte"), "{err}");
}

/// The body and the enabled id land in ONE save, so a failing save leaves
/// the record exactly as it was — no orphaned body, no orphaned enabled id,
/// nothing to roll back. (Before #168 this needed a file-removal dance.)
#[tokio::test]
async fn store_save_failure_leaves_the_record_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::failing(record(
        &company,
        manifest_with_assistant(),
    )));

    let err = create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        valid_draft("rollback", "Rollback"),
        None,
        None,
    )
    .await
    .expect_err("save fails");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );

    let record = store.load(&company).await.unwrap().unwrap();
    assert!(record.overlay_workflows.is_empty(), "no orphaned body");
    assert!(
        record.manifest.workflows.enabled.is_empty(),
        "no orphaned enabled id"
    );
    assert!(
        !dir.path().join("workflows").join("rollback.toml").exists(),
        "nothing was written to the source tree"
    );
}

// --- #168: no source directory at all (the hosted case) ------------------

/// The direct #168 regression at the core level: a hosted tenant has no
/// source directory (its crate mount is read-only), and creation must still
/// succeed by persisting the body on the record.
#[tokio::test]
async fn creates_with_no_source_dir() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    let file = create_company_workflow(
        &company,
        None,
        &store,
        None,
        valid_draft("hosted", "Hosted"),
        None,
        None,
    )
    .await
    .expect("creates with no source dir");
    assert_eq!(file.id, "hosted");

    let record = store.load(&company).await.unwrap().unwrap();
    assert_eq!(record.overlay_workflows.len(), 1);
    assert_eq!(record.overlay_workflows[0].id, "hosted");
    assert!(
        record
            .manifest
            .workflows
            .enabled
            .contains(&"hosted".to_string())
    );
    // And it reads back as a full graph through the union path.
    let loaded = load_workflow_union(None, &record.overlay_workflows, "hosted")
        .expect("loads")
        .expect("present");
    assert_eq!(loaded, file);
}

/// An id already taken by a *seed* file is a 409 even though the new body
/// would live somewhere else entirely — the seed would shadow it on read.
#[tokio::test]
async fn id_colliding_with_a_seed_file_is_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("seeded.toml"), SEED_TOML).unwrap();

    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let err = create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        valid_draft("seeded", "Different name"),
        None,
        None,
    )
    .await
    .expect_err("id is taken by a seed file");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
    assert!(err.to_string().contains("seeded"), "{err}");
}

/// A *name* already used by a seed file collides too — the picker would show
/// two indistinguishable entries.
#[tokio::test]
async fn name_colliding_with_a_seed_file_is_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("seeded.toml"), SEED_TOML).unwrap();

    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let err = create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        valid_draft("other", "  seeded FLOW  "),
        None,
        None,
    )
    .await
    .expect_err("name collides with the seed file's name");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
}

/// With no source tree at all, the name guard still works — it degrades to
/// overlay ∪ enabled rather than erroring or silently allowing duplicates.
#[tokio::test]
async fn name_guard_works_without_a_source_tree() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    create_company_workflow(
        &company,
        None,
        &store,
        None,
        valid_draft("one", "Greeter"),
        None,
        None,
    )
    .await
    .expect("first");
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        valid_draft("two", "GREETER"),
        None,
        None,
    )
    .await
    .expect_err("name collides with the overlay body");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
}

/// A graph whose only node is its trigger, carrying a schedule — the
/// `campaign` shape from staging (issue #976).
fn stageless_scheduled_draft(id: &str, name: &str) -> RawWorkflow {
    let mut draft = valid_draft(id, name);
    // Keep only the trigger, and put a schedule on it. This is what the
    // console produces when somebody drops a Start node, sets a cron, and
    // saves before adding any stage.
    draft.nodes.retain(|n| n.kind == "trigger");
    draft.edges.clear();
    draft.nodes[0].schedule = Some("0 9 * * *".to_string());
    draft
}

/// Saving one is **allowed**, and that is the deliberate half of the fix.
/// Authoring is incremental: the console drops a Start node first and adds
/// stages after, `parse_workflow` was made lenient on purpose (#661) to
/// support exactly that, and refusing at save would also refuse every
/// existing seed and legacy body on its next edit.
#[tokio::test]
async fn a_stageless_graph_still_saves() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        stageless_scheduled_draft("campaign", "Campaign"),
        None,
        None,
    )
    .await
    .expect("a stub mid-authoring is legitimate and must save");
}

/// ...but switching its schedule on is refused. Arming is where the promise
/// is made, so it is where the promise is checked: resume this and it fires
/// on time, runs nothing, and reports nothing.
#[tokio::test]
async fn arming_a_stageless_schedule_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        stageless_scheduled_draft("campaign", "Campaign"),
        None,
        None,
    )
    .await
    .expect("saves");

    let err = set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        None,
        "campaign",
        true,
        true,
        &[],
    )
    .await
    .expect_err("a schedule that cannot run must not be armed");

    let rendered = err.to_string();
    assert!(
        rendered.contains("no stage to run"),
        "the operator is told WHAT is wrong: {rendered}"
    );
    assert!(
        rendered.contains("Add at least one node"),
        "...and the one thing they can do about it: {rendered}"
    );
}

/// Switching such a workflow **off** stays allowed. An operator must always
/// be able to stop a thing — the same call the unparseable-body case makes
/// — and a guard that trapped a workflow in the armed state would be worse
/// than the silence it replaced.
#[tokio::test]
async fn a_stageless_schedule_can_still_be_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        stageless_scheduled_draft("campaign", "Campaign"),
        None,
        None,
    )
    .await
    .expect("saves");

    set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        None,
        "campaign",
        false,
        true,
        &[],
    )
    .await
    .expect("pausing must never be refused");
}

/// A graph with a real stage arms normally. Without this the refusal above
/// would pass against a build that refused every schedule.
#[tokio::test]
async fn arming_a_scheduled_graph_with_a_stage_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    let mut draft = valid_draft("greeter", "Greeter");
    draft.nodes[0].schedule = Some("0 9 * * *".to_string());
    create_company_workflow(&company, Some(dir.path()), &store, None, draft, None, None)
        .await
        .expect("saves");

    set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        None,
        "greeter",
        true,
        true,
        &[],
    )
    .await
    .expect("a graph that can actually run may be armed");
}
