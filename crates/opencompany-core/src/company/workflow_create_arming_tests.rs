//! workflow_create: issue #276 arming, disarming, and the operator switch.

use super::test_support::*;
use super::*;

// --- issue #276: arming, disarming, and the switch -----------------------

/// A draft whose trigger fires on `cron`.
fn scheduled_draft(id: &str, name: &str, cron: &str) -> RawWorkflow {
    let mut draft = valid_draft(id, name);
    draft.nodes[0].schedule = Some(cron.to_string());
    draft
}

/// A graph authored with a cron lands switched OFF.
///
/// The half of the disarm rule OpenHuman does not have, and the one that
/// matters most here: this function is also the orchestrator's
/// `create_workflow` tool, so this assertion is what stops an agent putting a
/// cron into production by writing one.
async fn create_scheduled(
    company: &CompanyId,
    dir: &std::path::Path,
    store: &Arc<dyn CompanyStore>,
    log: &Arc<dyn EventLog>,
    id: &str,
    name: &str,
    cron: &str,
) -> WorkflowFile {
    create_company_workflow(
        company,
        Some(dir),
        store,
        Some(log),
        scheduled_draft(id, name, cron),
        None,
        None,
    )
    .await
    .expect("creates")
}

#[tokio::test]
async fn creating_a_scheduled_workflow_leaves_it_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    create_scheduled(
        &company,
        dir.path(),
        &store,
        &log_dyn,
        "digest",
        "Digest",
        "0 9 * * *",
    )
    .await;

    let saved = store.load(&company).await.unwrap().unwrap();
    assert!(
        !saved.workflow_enabled("digest"),
        "a created schedule must not be armed"
    );
    // Still a normal, complete workflow otherwise — pausing stops the
    // schedule, not the workflow.
    assert_eq!(saved.overlay_workflows.len(), 1);
    assert!(
        saved
            .manifest
            .workflows
            .enabled
            .contains(&"digest".to_string()),
        "the manifest declaration is untouched by the arming decision"
    );

    // Journaled, and journaled as the rule rather than as a person.
    let events = log.events.lock().unwrap();
    let disarm = events
        .iter()
        .find(|e| matches!(e, CompanyEvent::WorkflowEnabledChanged { .. }))
        .expect("a disarm is journaled");
    match disarm {
        CompanyEvent::WorkflowEnabledChanged {
            workflow_id,
            enabled,
            reason,
            by,
            ..
        } => {
            assert_eq!(workflow_id, "digest");
            assert!(!enabled);
            assert_eq!(*reason, WorkflowEnabledReason::Disarmed);
            assert!(by.is_none(), "the rule is not a person");
        }
        other => panic!("expected WorkflowEnabledChanged, got {other:?}"),
    }
}

/// A graph authored WITHOUT a cron is armed, because there is nothing to
/// arm. The disarm rule must not make every manual workflow look paused in
/// the console.
#[tokio::test]
async fn creating_a_manual_workflow_leaves_it_switched_on() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    create_company_workflow(
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

    let saved = store.load(&company).await.unwrap().unwrap();
    assert!(saved.workflow_enabled("greeter"));
    assert!(
        saved.disabled_workflows.is_empty(),
        "a manual workflow must not be listed as paused"
    );
    assert!(
        !log.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, CompanyEvent::WorkflowEnabledChanged { .. })),
        "nothing changed, so nothing is journaled"
    );
}

/// **The safety-relevant half of issue #276.** An edit that turns a manual
/// workflow into a scheduled one switches it off, so a cron introduced by an
/// edit cannot fire before anyone has looked at it.
#[tokio::test]
async fn an_edit_that_adds_a_schedule_switches_the_workflow_off() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    create_company_workflow(
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
    assert!(
        store
            .load(&company)
            .await
            .unwrap()
            .unwrap()
            .workflow_enabled("greeter"),
        "armed before the edit, so the assertion below is about the edit"
    );

    update_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        &revs(),
        Some(&log_dyn),
        scheduled_draft("greeter", "Greeter", "0 8 * * *"),
        None,
        None,
    )
    .await
    .expect("updates");

    let saved = store.load(&company).await.unwrap().unwrap();
    assert!(
        !saved.workflow_enabled("greeter"),
        "an edit that adds a schedule must disarm it"
    );
}

/// Correcting an already-armed workflow's cron leaves it armed.
///
/// The deliberate limit of the rule: the reviewed decision is "automatic at
/// all", and that one has not changed. Disarming here would put a re-enable
/// click behind every typo fix.
#[tokio::test]
async fn changing_an_existing_schedule_does_not_disarm() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    create_scheduled(
        &company,
        dir.path(),
        &store,
        &log_dyn,
        "digest",
        "Digest",
        "0 9 * * *",
    )
    .await;
    // The operator reviews it and arms it.
    set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        Some(&log_dyn),
        "digest",
        true,
        true,
        &[],
    )
    .await
    .expect("arms");

    update_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        &revs(),
        Some(&log_dyn),
        scheduled_draft("digest", "Digest", "0 3 * * *"),
        None,
        None,
    )
    .await
    .expect("updates");

    assert!(
        store
            .load(&company)
            .await
            .unwrap()
            .unwrap()
            .workflow_enabled("digest"),
        "a cron correction must not disarm an already-armed workflow"
    );
}

/// An edit never arms. A paused workflow stays paused across a re-save, even
/// one that removes the schedule entirely — the rule has no arming
/// direction, which is what stops it arming by accident.
#[tokio::test]
async fn an_edit_never_re_arms_a_paused_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    create_scheduled(
        &company,
        dir.path(),
        &store,
        &log_dyn,
        "digest",
        "Digest",
        "0 9 * * *",
    )
    .await;

    // Re-save with the schedule removed: still paused.
    update_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        &revs(),
        Some(&log_dyn),
        valid_draft("digest", "Digest"),
        None,
        None,
    )
    .await
    .expect("updates");

    assert!(
        !store
            .load(&company)
            .await
            .unwrap()
            .unwrap()
            .workflow_enabled("digest"),
        "removing a schedule must not re-arm the workflow"
    );
}

/// The operator switch round-trips, journals once, and is idempotent.
#[tokio::test]
async fn the_operator_switch_toggles_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    create_company_workflow(
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
    let before = log.events.lock().unwrap().len();

    let changed = set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        Some(&log_dyn),
        "greeter",
        false,
        true,
        &[],
    )
    .await
    .expect("pauses");
    assert!(changed, "the first toggle changes the record");
    assert!(
        !store
            .load(&company)
            .await
            .unwrap()
            .unwrap()
            .workflow_enabled("greeter")
    );

    // Setting the state it already holds writes nothing and journals nothing
    // — a double-click is a no-op, not a second audit entry.
    let changed = set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        Some(&log_dyn),
        "greeter",
        false,
        true,
        &[],
    )
    .await
    .expect("no-ops");
    assert!(!changed);
    assert_eq!(
        log.events.lock().unwrap().len(),
        before + 1,
        "only the real transition is journaled"
    );

    // And back on, journaled as an operator decision rather than the rule.
    assert!(
        set_company_workflow_enabled(
            &company,
            Some(dir.path()),
            &store,
            Some(&log_dyn),
            "greeter",
            true,
            true,
            &[],
        )
        .await
        .expect("arms")
    );
    let events = log.events.lock().unwrap();
    match events.last().expect("an event") {
        CompanyEvent::WorkflowEnabledChanged {
            enabled, reason, ..
        } => {
            assert!(enabled);
            assert_eq!(*reason, WorkflowEnabledReason::Operator);
        }
        other => panic!("expected WorkflowEnabledChanged, got {other:?}"),
    }
}

/// An id with no graph anywhere is a 404, and a manifest-`enabled` id with no
/// body is a 409 — there is no schedule to switch off in either case.
#[tokio::test]
async fn toggling_an_id_with_no_graph_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let mut seed = record(&company, manifest_with_assistant());
    seed.manifest.workflows.enabled.push("ghost".to_string());
    let store = store_of(MemStore::seeded(seed));

    let err = set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        None,
        "nowhere",
        false,
        true,
        &[],
    )
    .await
    .expect_err("unknown id");
    assert!(
        matches!(err, OpenCompanyError::CompanyNotFound(_)),
        "{err:?}"
    );

    let err = set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        None,
        "ghost",
        false,
        true,
        &[],
    )
    .await
    .expect_err("bodiless id");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
}

/// A stored graph that no longer parses is pausable, and is NOT reported as
/// "provisioned by name only".
///
/// The bodiless-409 message says the id was provisioned by name — true for a
/// manifest entry with no graph, and false for a workflow whose saved body
/// simply broke. An earlier revision collapsed both into one branch by
/// swallowing `load_workflow_union`'s error, so a corrupt graph read back as
/// the wrong explanation with no way to act on it.
#[tokio::test]
async fn a_workflow_whose_stored_graph_no_longer_parses_can_still_be_paused() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let mut seed = record(&company, manifest_with_assistant());
    seed.overlay_workflows.push(OverlayWorkflow {
        id: "broken".to_string(),
        toml: "id = \"broken\"\nname = \"Broken\"\n".to_string(), // no nodes: fails validation
    });
    seed.manifest.workflows.enabled.push("broken".to_string());
    let store = store_of(MemStore::seeded(seed));
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    assert!(
        set_company_workflow_enabled(
            &company,
            Some(dir.path()),
            &store,
            Some(&log_dyn),
            "broken",
            false,
            true,
            &[],
        )
        .await
        .expect("an unreadable graph is still pausable")
    );
    assert!(
        !store
            .load(&company)
            .await
            .unwrap()
            .unwrap()
            .workflow_enabled("broken")
    );
    // Journals under the id, since there is no readable name to use.
    match log.events.lock().unwrap().last().expect("an event") {
        CompanyEvent::WorkflowEnabledChanged { name, .. } => assert_eq!(name, "broken"),
        other => panic!("expected WorkflowEnabledChanged, got {other:?}"),
    }
}

/// A **seed-defined** workflow can be paused, even though `PUT`/`DELETE`
/// refuse it with a 409.
///
/// This is the deliberate asymmetry, and the reason for it: an edit or a
/// delete would be undone by the read path's seed precedence and by the boot
/// rebuild, so refusing them is honesty about what the reader will do.
/// Pausing writes to the record, leaves the source tree alone, and only ever
/// removes capability — and without it an operator cannot stop a committed
/// cron without a redeploy, which is issue #276(a) with extra steps.
#[tokio::test]
async fn a_seed_defined_workflow_can_be_paused_even_though_it_cannot_be_edited() {
    let dir = tempfile::tempdir().unwrap();
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("seeded.toml"), SEED_TOML).unwrap();

    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    // Edit is refused, as it has been since #259 …
    let err = update_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        &revs(),
        None,
        valid_draft("seeded", "Seeded flow"),
        None,
        None,
    )
    .await
    .expect_err("a seed-defined graph cannot be replaced");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");

    // … and the switch still works.
    assert!(
        set_company_workflow_enabled(
            &company,
            Some(dir.path()),
            &store,
            None,
            "seeded",
            false,
            true,
            &[],
        )
        .await
        .expect("pauses a seed-defined workflow")
    );
    let saved = store.load(&company).await.unwrap().unwrap();
    assert!(!saved.workflow_enabled("seeded"));
    assert!(
        saved.overlay_workflows.is_empty(),
        "pausing must not materialize an overlay body for a seed graph"
    );
}
