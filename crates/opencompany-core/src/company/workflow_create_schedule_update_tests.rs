//! workflow_create: undeliverable-schedule refusal (issue #1046) and issue #259's update path.

use super::test_support::*;
use super::tests_destination::draft_with_destination;
use super::*;

// --- Undeliverable-schedule refusal (issue #1046) ------------------------

/// A scheduled `trigger → agent → output` draft whose output delivers to
/// `(dest_kind, dest_target)`. The shape #1046 guards: a graph that runs a
/// stage but whose only report may land nowhere.
fn scheduled_output_draft(
    id: &str,
    name: &str,
    dest_kind: &str,
    dest_target: Option<&str>,
) -> RawWorkflow {
    let mut draft = draft_with_destination(id, name, dest_kind, dest_target);
    draft.nodes[0].schedule = Some("0 9 * * *".to_string());
    draft
}

/// Issue #1757 reverses one arm of #1046: a scheduled graph whose only report
/// goes to the owner on a company with **no mailbox** now ARMS. `owner` no
/// longer dead-ends on an in-memory buffer — it falls back to the durable
/// operator channel, which journals the report into the operator's main line,
/// so a scheduled owner report reliably lands and the schedule is honest.
#[tokio::test]
async fn arming_a_scheduled_owner_output_with_no_mailbox_now_arms() {
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
        scheduled_output_draft("digest", "Digest", "owner", None),
        None,
        None,
    )
    .await
    .expect("saving a stub is legitimate and must succeed");

    set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        None,
        "digest",
        true,
        // No mailbox, no wired channels: the owner report still lands, on the
        // durable operator channel (issue #1757).
        false,
        &[],
    )
    .await
    .expect("an owner report always lands, so its schedule must arm");
}

/// The manual half of the fix: the same undeliverable graph, but with **no**
/// schedule on its trigger, enables freely. Running a stub by hand — knowing
/// its report only reaches the run drawer — is the operator's own business;
/// only a *schedule* makes a delivery promise nobody is watching.
#[tokio::test]
async fn a_manual_undeliverable_graph_is_not_refused() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    // A genuinely undeliverable graph — a channel output to an unwired desk —
    // but drop the schedule so it is manual. (`owner` no longer qualifies:
    // since issue #1757 it always lands on the durable operator channel.)
    let mut draft = scheduled_output_draft("digest", "Digest", "channel", Some("marketing"));
    draft.nodes[0].schedule = None;
    create_company_workflow(&company, Some(dir.path()), &store, None, draft, None, None)
        .await
        .expect("saves");

    set_company_workflow_enabled(
        &company,
        Some(dir.path()),
        &store,
        None,
        "digest",
        true,
        false,
        &[],
    )
    .await
    .expect("a manual graph is never refused for undeliverable output");
}

/// The refusal is delivery-capability-specific, not a blanket ban on
/// owner outputs: the identical scheduled owner graph arms once a mailbox is
/// configured, because owner delivery can then email the company's admins.
#[tokio::test]
async fn arming_a_scheduled_owner_output_with_a_mailbox_arms() {
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
        scheduled_output_draft("digest", "Digest", "owner", None),
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
        "digest",
        true,
        // A mailbox is configured: owner reports can be emailed.
        true,
        &[],
    )
    .await
    .expect("an owner report can land once a mailbox exists");
}

/// A scheduled output to a wired channel arms even with no mailbox — the
/// channel is a real write path.
#[tokio::test]
async fn arming_a_scheduled_channel_output_to_a_wired_channel_arms() {
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
        scheduled_output_draft("digest", "Digest", "channel", Some("engineering")),
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
        "digest",
        true,
        false,
        &["engineering".to_string()],
    )
    .await
    .expect("a report to a wired channel can land");
}

/// A scheduled output to the operator channel now ARMS (issue #1757): the
/// operator channel is a durable, journal-backed surface the company always
/// wires, so `deliverable_channel_ids` lists it and a report posted there
/// lands in the standing Operator channel.
#[tokio::test]
async fn arming_a_scheduled_channel_output_to_operator_arms() {
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
        scheduled_output_draft("digest", "Digest", "channel", Some("operator")),
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
        "digest",
        true,
        false,
        // `operator` is a wired, durable channel now.
        &["operator".to_string()],
    )
    .await
    .expect("a report to the durable operator channel can land, so the schedule arms");
}

/// Switching an undeliverable scheduled graph **off** is always allowed — an
/// operator must be able to stop a thing, the same rule the stage-less guard
/// keeps.
#[tokio::test]
async fn an_undeliverable_scheduled_graph_can_still_be_switched_off() {
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
        scheduled_output_draft("digest", "Digest", "owner", None),
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
        "digest",
        false,
        false,
        &[],
    )
    .await
    .expect("pausing must never be refused");
}

/// A manifest-`enabled` id with no body in either source is shown by the
/// picker under its id, so a new workflow can't take that name either.
#[tokio::test]
async fn name_collides_with_a_bodiless_enabled_id() {
    let company = CompanyId::new("acme");
    let mut rec = record(&company, manifest_with_assistant());
    rec.manifest.workflows.enabled.push("legacy".to_string());
    let store = store_of(MemStore::seeded(rec));

    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        valid_draft("new", "  LEGACY  "),
        None,
        None,
    )
    .await
    .expect_err("name collides with the enabled-id fallback name");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
}

#[tokio::test]
async fn no_company_record_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("ghost");
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::default());
    let err = create_company_workflow(
        &company,
        Some(dir.path()),
        &store,
        None,
        valid_draft("wf", "WF"),
        None,
        None,
    )
    .await
    .expect_err("no record");
    assert!(
        matches!(err, OpenCompanyError::CompanyNotFound(_)),
        "{err:?}"
    );
}

// --- #259: update ------------------------------------------------------

/// Seeds a company with one created workflow and hands back the store plus
/// the version token a `GET` would have returned for it.
pub(super) async fn with_one_workflow(
    company: &CompanyId,
    id: &str,
    name: &str,
) -> (Arc<dyn CompanyStore>, String) {
    let store = store_of(MemStore::seeded(record(company, manifest_with_assistant())));
    create_company_workflow(
        company,
        None,
        &store,
        None,
        valid_draft(id, name),
        None,
        None,
    )
    .await
    .expect("seed create");
    let record = store.load(company).await.unwrap().unwrap();
    let version = workflow_version(&record.overlay_workflows[0].toml);
    (store, version)
}

#[tokio::test]
async fn updates_the_body_in_place_and_journals() {
    let company = CompanyId::new("acme");
    let (store, version) = with_one_workflow(&company, "greeter", "Greeter").await;
    let log = Arc::new(MemLog::default());
    let log_dyn: Arc<dyn EventLog> = log.clone();

    let mut draft = valid_draft("greeter", "Greeter");
    draft.nodes[0].schedule = Some("0 9 * * *".to_string());
    draft.description = Some("Now on a cron.".to_string());

    let file = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        Some(&log_dyn),
        draft,
        Some(&version),
        None,
    )
    .await
    .expect("updates");
    assert_eq!(file.nodes[0].schedule.as_deref(), Some("0 9 * * *"));

    let record = store.load(&company).await.unwrap().unwrap();
    // Replaced, not appended — an edit must never fork the graph in two.
    assert_eq!(record.overlay_workflows.len(), 1);
    assert_eq!(record.overlay_workflows[0].id, "greeter");
    // The manifest declaration is untouched — it says which workflows this
    // company has, not which of them are armed.
    assert_eq!(record.manifest.workflows.enabled, vec!["greeter"]);
    // …but this edit added a cron to a manual graph, so issue #276's disarm
    // rule switched it off in the same save. The draft above is exactly the
    // manual→automatic transition the rule exists for, which is why this
    // assertion lives on the general update test rather than only on the
    // dedicated one.
    assert!(
        !record.workflow_enabled("greeter"),
        "an edit that adds a schedule must leave the workflow switched off"
    );

    // What the union read path serves is what we returned.
    let reloaded = load_workflow_union(None, &record.overlay_workflows, "greeter")
        .expect("reloads")
        .expect("present");
    assert_eq!(reloaded, file);

    let events = log.events.lock().unwrap();
    assert_eq!(events.len(), 2, "the edit, then the disarm it triggered");
    match &events[0] {
        CompanyEvent::WorkflowUpdated {
            workflow_id, name, ..
        } => {
            assert_eq!(workflow_id, "greeter");
            assert_eq!(name, "Greeter");
        }
        other => panic!("expected WorkflowUpdated, got {other:?}"),
    }
    match &events[1] {
        CompanyEvent::WorkflowEnabledChanged {
            workflow_id,
            enabled,
            reason,
            ..
        } => {
            assert_eq!(workflow_id, "greeter");
            assert!(!enabled);
            assert_eq!(*reason, WorkflowEnabledReason::Disarmed);
        }
        other => panic!("expected WorkflowEnabledChanged, got {other:?}"),
    }
}

/// The version token is what makes concurrent edits safe. A caller holding a
/// token from before someone else's write must be refused, not silently win.
#[tokio::test]
async fn a_stale_version_is_refused_and_changes_nothing() {
    let company = CompanyId::new("acme");
    let (store, stale) = with_one_workflow(&company, "greeter", "Greeter").await;

    // Someone else edits first, unconditionally.
    let mut theirs = valid_draft("greeter", "Greeter");
    theirs.description = Some("Theirs landed first.".to_string());
    update_company_workflow(&company, None, &store, &revs(), None, theirs, None, None)
        .await
        .expect("first writer wins");

    // Our stale token is now wrong.
    let mut ours = valid_draft("greeter", "Greeter");
    ours.description = Some("Ours would clobber.".to_string());
    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        ours,
        Some(&stale),
        None,
    )
    .await
    .expect_err("stale version must be refused");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");

    // And the other writer's edit is intact — the refusal is not partial.
    let record = store.load(&company).await.unwrap().unwrap();
    let current = load_workflow_union(None, &record.overlay_workflows, "greeter")
        .unwrap()
        .unwrap();
    assert_eq!(current.description.as_deref(), Some("Theirs landed first."));
}

/// The fresh token from the *previous* write is accepted, so the
/// reload-and-retry loop the console offers actually terminates.
#[tokio::test]
async fn a_fresh_version_is_accepted() {
    let company = CompanyId::new("acme");
    let (store, first) = with_one_workflow(&company, "greeter", "Greeter").await;

    let mut once = valid_draft("greeter", "Greeter");
    once.description = Some("One.".to_string());
    update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        once,
        Some(&first),
        None,
    )
    .await
    .expect("first conditional write");

    let record = store.load(&company).await.unwrap().unwrap();
    let second = workflow_version(&record.overlay_workflows[0].toml);
    assert_ne!(second, first, "the token must move when the body does");

    let mut twice = valid_draft("greeter", "Greeter");
    twice.description = Some("Two.".to_string());
    update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        twice,
        Some(&second),
        None,
    )
    .await
    .expect("refreshed token is accepted");
}

/// No token at all is an unconditional write — the `curl` contract.
#[tokio::test]
async fn no_version_is_an_unconditional_write() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;
    let mut draft = valid_draft("greeter", "Greeter");
    draft.description = Some("No token needed.".to_string());
    update_company_workflow(&company, None, &store, &revs(), None, draft, None, None)
        .await
        .expect("unconditional write");
}

/// Re-saving without renaming must not collide with the workflow's own name.
#[tokio::test]
async fn keeping_the_same_name_is_not_a_self_conflict() {
    let company = CompanyId::new("acme");
    let (store, version) = with_one_workflow(&company, "greeter", "Greeter").await;
    let mut draft = valid_draft("greeter", "  greeter  ");
    draft.description = Some("Same name, different case and padding.".to_string());
    update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        draft,
        Some(&version),
        None,
    )
    .await
    .expect("own name must not conflict with itself");
}

/// …but a *sibling's* name is still guarded.
#[tokio::test]
async fn taking_another_workflows_name_is_a_conflict() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        valid_draft("other", "Other"),
        None,
        None,
    )
    .await
    .expect("second workflow");

    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        valid_draft("greeter", "OTHER"),
        None,
        None,
    )
    .await
    .expect_err("sibling name is taken");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
}

/// An edit runs the same shape validation a create does.
#[tokio::test]
async fn a_bad_edit_is_refused_on_the_same_terms_as_a_bad_create() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;

    // Zero triggers.
    let mut no_trigger = valid_draft("greeter", "Greeter");
    no_trigger.nodes[0].kind = "output".to_string();
    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        no_trigger,
        None,
        None,
    )
    .await
    .expect_err("no trigger");
    assert!(err.to_string().contains("exactly one `trigger`"), "{err}");

    // Off-roster teammate.
    let mut ghost = valid_draft("greeter", "Greeter");
    ghost.nodes[1].agent = Some("ghost".to_string());
    let err = update_company_workflow(&company, None, &store, &revs(), None, ghost, None, None)
        .await
        .expect_err("off-roster teammate");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );

    // And nothing was persisted by either attempt.
    let record = store.load(&company).await.unwrap().unwrap();
    assert_eq!(record.overlay_workflows.len(), 1);
    let current = load_workflow_union(None, &record.overlay_workflows, "greeter")
        .unwrap()
        .unwrap();
    assert_eq!(current.nodes.len(), 3);
}

/// **The core overlay-only rule for update.** A seed-backed id is refused,
/// because `load_workflow_union` gives the seed file precedence — persisting
/// the edit would store a graph the read path never serves.
#[tokio::test]
async fn updating_a_seed_backed_workflow_is_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("seeded.toml"), SEED_TOML).unwrap();

    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

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
    .expect_err("a source-defined workflow is not editable");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
    assert!(err.to_string().contains("source tree"), "{err}");
}

#[tokio::test]
async fn updating_an_unknown_workflow_is_not_found() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;
    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        valid_draft("ghost", "Ghost"),
        None,
        None,
    )
    .await
    .expect_err("unknown id");
    assert!(
        matches!(err, OpenCompanyError::CompanyNotFound(_)),
        "{err:?}"
    );
}

/// A manifest-`enabled` id with no body in either source has nothing to
/// replace — a 409 that says so beats a 404 that implies it never existed.
#[tokio::test]
async fn updating_a_bodiless_enabled_id_is_a_conflict() {
    let company = CompanyId::new("acme");
    let mut rec = record(&company, manifest_with_assistant());
    rec.manifest.workflows.enabled.push("legacy".to_string());
    let store = store_of(MemStore::seeded(rec));

    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        valid_draft("legacy", "Legacy"),
        None,
        None,
    )
    .await
    .expect_err("no body to replace");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
}

/// An edit must not reshuffle the picker.
#[tokio::test]
async fn an_edit_preserves_overlay_order() {
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

    let mut draft = valid_draft("a", "Alpha");
    draft.description = Some("Edited.".to_string());
    update_company_workflow(&company, None, &store, &revs(), None, draft, None, None)
        .await
        .expect("edit the first");

    let record = store.load(&company).await.unwrap().unwrap();
    let ids: Vec<&str> = record
        .overlay_workflows
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    assert_eq!(ids, vec!["a", "b", "c"], "an edit must not reorder");
}
