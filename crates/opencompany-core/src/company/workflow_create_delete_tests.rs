//! workflow_create: issue #259's delete path and the version token.

use super::test_support::*;
use super::tests_schedule_update::with_one_workflow;
use super::*;

// --- #259: delete ------------------------------------------------------

#[tokio::test]
async fn deletes_the_body_and_the_enabled_id_and_journals() {
    let company = CompanyId::new("acme");
    let (store, version) = with_one_workflow(&company, "greeter", "Greeter").await;
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    let name = delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires()),
        Some(&log_dyn),
        "greeter",
        Some(&version),
    )
    .await
    .expect("deletes");
    assert_eq!(name, "Greeter");

    let record = store.load(&company).await.unwrap().unwrap();
    // BOTH halves gone, in one save. Either alone would leave a workflow
    // that is half-present: a listed id with no graph, or a graph the
    // scheduler still fires.
    assert!(record.overlay_workflows.is_empty(), "body must be gone");
    assert!(
        record.manifest.workflows.enabled.is_empty(),
        "enabled id must be gone"
    );
    assert!(
        load_workflow_union(None, &record.overlay_workflows, "greeter")
            .unwrap()
            .is_none(),
        "the union read path must no longer serve it"
    );

    let events = log.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        CompanyEvent::WorkflowDeleted {
            workflow_id, name, ..
        } => {
            assert_eq!(workflow_id, "greeter");
            assert_eq!(name, "Greeter");
        }
        other => panic!("expected WorkflowDeleted, got {other:?}"),
    }
}

/// Issue #1017: deleting a paused workflow must purge its pause flag, so a
/// workflow later re-created under the same id starts armed instead of
/// inheriting a stale pause the operator never asked for. `disabled_workflows`
/// is keyed by id, and a re-created id reuses it, so a leftover entry would
/// silently keep the fresh workflow off its schedule.
#[tokio::test]
async fn deleting_a_paused_workflow_purges_the_stale_pause_for_a_re_create() {
    let company = CompanyId::new("acme");
    let (store, version) = with_one_workflow(&company, "greeter", "Greeter").await;

    // Pause it — the id lands in `disabled_workflows`.
    set_company_workflow_enabled(&company, None, &store, None, "greeter", false, true, &[])
        .await
        .expect("pausing must never be refused");
    let paused = store.load(&company).await.unwrap().unwrap();
    assert!(
        !paused.workflow_enabled("greeter"),
        "precondition: the workflow is paused"
    );

    // Delete it, then re-create the same id from scratch.
    delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        None,
        "greeter",
        Some(&version),
    )
    .await
    .expect("deletes");
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        valid_draft("greeter", "Greeter"),
        None,
        None,
    )
    .await
    .expect("re-create");

    let record = store.load(&company).await.unwrap().unwrap();
    assert!(
        !record.disabled_workflows.iter().any(|id| id == "greeter"),
        "the delete must purge the stale pause flag"
    );
    assert!(
        record.workflow_enabled("greeter"),
        "a re-created workflow must start armed, not inherit the deleted one's pause"
    );
}

/// Issue #1045: the REST create/delete persist path puts
/// `WorkflowCreated` / `WorkflowDeleted` on a stream a **live subscriber**
/// actually receives — the in-process delivery the console's SSE picker
/// depends on. A green characterization: it locates the reported "graph
/// authored elsewhere stays invisible" defect on the console side, not in a
/// dropped host frame.
///
/// The projection of these variants onto the `{type, workflowId, name}` wire
/// frame the console keys on is asserted next to `project_event` itself
/// (`server::operator` — `projects_workflow_created_without_the_actor`,
/// `projects_workflow_updated_and_deleted_without_the_actor`). This test
/// closes the remaining link: that the persist path emits those variants,
/// carrying the same id and name, onto a stream `subscribe` delivers.
#[tokio::test]
async fn create_and_delete_reach_a_live_subscriber() {
    use futures::StreamExt;

    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let log = Arc::new(BroadcastMemLog::new());
    let log_dyn: Arc<dyn EventLog> = log.clone();
    // Subscribe before the writes, exactly as the SSE handler does.
    let mut stream = log_dyn.subscribe(&company);

    create_company_workflow(
        &company,
        None,
        &store,
        Some(&log_dyn),
        valid_draft("greeter", "Greeter"),
        None,
        None,
    )
    .await
    .expect("creates");

    let created = stream
        .next()
        .await
        .expect("workflow_created delivered live");
    let created_event = match created {
        crate::ports::events::EventStreamItem::Event(ev) => ev,
        other => panic!("expected a live Event frame, got {other:?}"),
    };
    match &created_event.event {
        CompanyEvent::WorkflowCreated {
            workflow_id, name, ..
        } => {
            assert_eq!(workflow_id, "greeter");
            assert_eq!(name, "Greeter");
        }
        other => panic!("expected WorkflowCreated on the wire, got {other:?}"),
    }

    // Delete the same graph over the same persist path. `None` expected
    // version skips the optimistic-concurrency check — this test is about
    // the emitted frame, not the token.
    delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires()),
        Some(&log_dyn),
        "greeter",
        None,
    )
    .await
    .expect("deletes");

    let deleted = stream
        .next()
        .await
        .expect("workflow_deleted delivered live");
    let deleted_event = match deleted {
        crate::ports::events::EventStreamItem::Event(ev) => ev,
        other => panic!("expected a live Event frame, got {other:?}"),
    };
    match &deleted_event.event {
        CompanyEvent::WorkflowDeleted {
            workflow_id, name, ..
        } => {
            assert_eq!(workflow_id, "greeter");
            assert_eq!(name, "Greeter");
        }
        other => panic!("expected WorkflowDeleted on the wire, got {other:?}"),
    }
}

/// #708: a committed delete purges the schedule's durable fire ledger under
/// the exact `workflow-<id>` key, so a recreated same-id workflow inherits
/// no anchor and no stale claim.
#[tokio::test]
async fn deleting_a_workflow_purges_its_schedule_fire_ledger() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;

    // A ledger for greeter's schedule, plus a sibling schedule's row to
    // prove the purge is scoped to exactly the deleted workflow's key.
    let fires = Arc::new(MemFires::default());
    let greeter_key = workflow_schedule_id("greeter");
    fires.seed(&company, &greeter_key, 100);
    fires.seed(&company, &greeter_key, 101);
    fires.seed(&company, &workflow_schedule_id("other"), 100);
    let fires_dyn: Arc<dyn ScheduleFireStore> = fires.clone();

    delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires_dyn),
        None,
        "greeter",
        None,
    )
    .await
    .expect("deletes");

    assert!(
        fires.minutes(&company, &greeter_key).is_empty(),
        "the deleted workflow's whole fire ledger is purged"
    );
    assert_eq!(
        fires.minutes(&company, &workflow_schedule_id("other")),
        vec![100],
        "a sibling workflow's schedule ledger is untouched"
    );
}

/// #708: the purge is best-effort. A purge failure is logged, never rolled
/// back — the workflow is already gone, so the delete still succeeds.
#[tokio::test]
async fn a_failing_fire_ledger_purge_still_deletes_the_workflow() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;

    let fires = Arc::new(MemFires::default());
    fires.seed(&company, &workflow_schedule_id("greeter"), 100);
    fires.arm_delete_failure();
    let fires_dyn: Arc<dyn ScheduleFireStore> = fires.clone();

    let name = delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires_dyn),
        None,
        "greeter",
        None,
    )
    .await
    .expect("delete succeeds even when the purge cascade errors");
    assert_eq!(name, "Greeter");

    // The graph is gone despite the purge error.
    let record = store.load(&company).await.unwrap().unwrap();
    assert!(record.overlay_workflows.is_empty(), "body must be gone");
    assert!(record.manifest.workflows.enabled.is_empty());
}

/// The delete is durable across the #208 boot rebuild *because* the overlay
/// body is gone: `merge_enabled_workflows` re-derives `enabled` from seed
/// ids ∪ surviving overlay ids, so there is nothing left to resurrect. This
/// pins the invariant the delete's correctness rests on.
#[tokio::test]
async fn a_deleted_workflow_has_nothing_left_for_the_boot_merge_to_re_enable() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;
    delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires()),
        None,
        "greeter",
        None,
    )
    .await
    .expect("deletes");

    let record = store.load(&company).await.unwrap().unwrap();
    let surviving: Vec<&str> = record
        .overlay_workflows
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    assert!(
        !surviving.contains(&"greeter"),
        "no overlay body means the boot merge cannot re-enable it"
    );
    assert!(list_workflows_union(None, &record.overlay_workflows).is_empty());
}

#[tokio::test]
async fn deleting_with_a_stale_version_is_refused_and_keeps_the_workflow() {
    let company = CompanyId::new("acme");
    let (store, stale) = with_one_workflow(&company, "greeter", "Greeter").await;

    let mut theirs = valid_draft("greeter", "Greeter");
    theirs.description = Some("Edited after you loaded it.".to_string());
    update_company_workflow(&company, None, &store, &revs(), None, theirs, None, None)
        .await
        .expect("someone edits first");

    // A held fire store, seeded under the workflow's schedule key, proves the
    // refused delete purges NOTHING — the purge runs only after a committed
    // save, so a version-refused delete (which never saves) leaves the live
    // workflow's ledger intact (#708).
    let fires = Arc::new(MemFires::default());
    fires.seed(&company, &workflow_schedule_id("greeter"), 42);
    let fires_dyn: Arc<dyn ScheduleFireStore> = fires.clone();

    let err = delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires_dyn),
        None,
        "greeter",
        Some(&stale),
    )
    .await
    .expect_err("stale delete must be refused");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");

    let record = store.load(&company).await.unwrap().unwrap();
    assert_eq!(record.overlay_workflows.len(), 1, "nothing was removed");
    assert_eq!(
        fires.minutes(&company, &workflow_schedule_id("greeter")),
        vec![42],
        "a version-refused delete never reaches the purge — the ledger is intact"
    );
}

/// Deleting a source-defined workflow is refused: `merge_enabled_workflows`
/// would re-enable it from the seed id on the next boot, so the console
/// would be promising a removal it cannot keep.
#[tokio::test]
async fn deleting_a_seed_backed_workflow_is_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("seeded.toml"), SEED_TOML).unwrap();

    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    let err = delete_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        &revs(),
        Some(&fires()),
        None,
        "seeded",
        None,
    )
    .await
    .expect_err("a source-defined workflow is not deletable");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
    assert!(err.to_string().contains("source tree"), "{err}");
    // And the seed file is untouched — this path never writes to the tree.
    assert!(workflows.join("seeded.toml").is_file());
}

#[tokio::test]
async fn deleting_an_unknown_workflow_is_not_found() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;
    let err = delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires()),
        None,
        "ghost",
        None,
    )
    .await
    .expect_err("unknown id");
    assert!(
        matches!(err, OpenCompanyError::CompanyNotFound(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn deleting_a_traversal_id_is_invalid() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;
    let err = delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires()),
        None,
        "../secrets",
        None,
    )
    .await
    .expect_err("traversal id");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
}

/// Only the named workflow goes; siblings keep their bodies and their
/// enabled ids.
#[tokio::test]
async fn deleting_one_leaves_the_others_alone() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    for (id, name) in [("a", "Alpha"), ("b", "Bravo"), ("c", "Charlie")] {
        create_company_workflow(
            &company,
            None,
            &store,
            None,
            valid_draft(id, name),
            None,
            None,
        )
        .await
        .expect("seed");
    }

    delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires()),
        None,
        "b",
        None,
    )
    .await
    .expect("deletes the middle one");

    let record = store.load(&company).await.unwrap().unwrap();
    let ids: Vec<&str> = record
        .overlay_workflows
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    assert_eq!(ids, vec!["a", "c"]);
    assert_eq!(record.manifest.workflows.enabled, vec!["a", "c"]);
}

/// A save failure leaves the record exactly as it was — the same one-save
/// atomicity create relies on.
#[tokio::test]
async fn a_failing_save_leaves_the_workflow_in_place() {
    let company = CompanyId::new("acme");
    let mut rec = record(&company, manifest_with_assistant());
    rec.overlay_workflows.push(OverlayWorkflow {
        id: "greeter".to_string(),
        toml: render_workflow(&valid_draft("greeter", "Greeter")).unwrap(),
    });
    rec.manifest.workflows.enabled.push("greeter".to_string());
    let store = store_of(MemStore::failing(rec));

    delete_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&fires()),
        None,
        "greeter",
        None,
    )
    .await
    .expect_err("save fails");

    let record = store.load(&company).await.unwrap().unwrap();
    assert_eq!(record.overlay_workflows.len(), 1, "nothing was removed");
    assert_eq!(record.manifest.workflows.enabled, vec!["greeter"]);
}

// --- #259: the version token itself ------------------------------------

#[test]
fn the_version_token_is_stable_and_body_derived() {
    let a = workflow_version("id = \"x\"\n");
    assert_eq!(a, workflow_version("id = \"x\"\n"), "must be deterministic");
    assert_ne!(
        a,
        workflow_version("id = \"y\"\n"),
        "a different body must produce a different token"
    );
    // Hex sha256: 64 lowercase hex characters, so it is safe in a URL query
    // without escaping (the DELETE route passes it as `?expectedVersion=`).
    assert_eq!(a.len(), 64, "{a}");
    assert!(
        a.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
        "{a}"
    );
}
