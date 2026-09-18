//! workflow_create: issue #274 revision capture and rollback.

use super::test_support::*;
use super::*;

// --- issue #274: revision capture + rollback -----------------------------

/// The overlay TOML currently stored for `wid`.
async fn current_toml(store: &Arc<dyn CompanyStore>, company: &CompanyId, wid: &str) -> String {
    store
        .load(company)
        .await
        .unwrap()
        .unwrap()
        .overlay_workflows
        .into_iter()
        .find(|w| w.id == wid)
        .expect("overlay body exists")
        .toml
}

/// Creates `greeter`, then returns `(store, revisions, body_a)` ready to edit.
async fn seeded_greeter() -> (Arc<dyn CompanyStore>, Arc<MemRevisions>, String) {
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
        valid_draft("greeter", "Greeter"),
        None,
        None,
    )
    .await
    .expect("create");
    let body_a = current_toml(&store, &company, "greeter").await;
    (store, Arc::new(MemRevisions::default()), body_a)
}

#[tokio::test]
async fn update_snapshots_the_prior_body_exactly_once() {
    let company = CompanyId::new("acme");
    let (store, revs, body_a) = seeded_greeter().await;
    let revs_dyn: Arc<dyn WorkflowRevisionStore> = revs.clone();

    // Edit the description so the rendered body differs from A.
    let mut edit = valid_draft("greeter", "Greeter");
    edit.description = Some("edited once".to_string());
    update_company_workflow(&company, None, &store, &revs_dyn, None, edit, None, None)
        .await
        .expect("update");

    let history = revs.list_revisions(&company, "greeter").await.unwrap();
    assert_eq!(history.len(), 1, "one edit captures one snapshot");
    assert_eq!(
        history[0].toml, body_a,
        "the snapshot must hold the prior body byte-for-byte"
    );
    assert_eq!(history[0].workflow_id, "greeter");
    assert_eq!(history[0].name, "Greeter");
}

#[tokio::test]
async fn a_byte_identical_resave_snapshots_nothing() {
    let company = CompanyId::new("acme");
    let (store, revs, _body_a) = seeded_greeter().await;
    let revs_dyn: Arc<dyn WorkflowRevisionStore> = revs.clone();

    // Re-save the exact same graph: the rendered body is byte-identical, so
    // there is nothing to lose and no snapshot is taken.
    update_company_workflow(
        &company,
        None,
        &store,
        &revs_dyn,
        None,
        valid_draft("greeter", "Greeter"),
        None,
        None,
    )
    .await
    .expect("no-op resave");
    assert!(
        revs.list_revisions(&company, "greeter")
            .await
            .unwrap()
            .is_empty(),
        "a byte-identical re-save must not snapshot"
    );
}

#[tokio::test]
async fn the_ring_prunes_the_oldest_past_the_cap() {
    use crate::ports::workflow_revisions::MAX_WORKFLOW_REVISIONS;
    let company = CompanyId::new("acme");
    let (store, revs, _body_a) = seeded_greeter().await;
    let revs_dyn: Arc<dyn WorkflowRevisionStore> = revs.clone();

    // MAX+1 distinct edits capture MAX+1 prior bodies; the ring keeps MAX.
    for i in 0..=MAX_WORKFLOW_REVISIONS {
        let mut edit = valid_draft("greeter", "Greeter");
        edit.description = Some(format!("edit {i}"));
        update_company_workflow(&company, None, &store, &revs_dyn, None, edit, None, None)
            .await
            .expect("update");
    }
    let history = revs.list_revisions(&company, "greeter").await.unwrap();
    assert_eq!(
        history.len(),
        MAX_WORKFLOW_REVISIONS,
        "the ring is capped at MAX_WORKFLOW_REVISIONS"
    );
}

#[tokio::test]
async fn rollback_restores_the_body_and_is_itself_undoable() {
    let company = CompanyId::new("acme");
    let (store, revs, body_a) = seeded_greeter().await;
    let revs_dyn: Arc<dyn WorkflowRevisionStore> = revs.clone();

    // A → edit → B. One revision now holds A.
    let mut edit_b = valid_draft("greeter", "Greeter");
    edit_b.description = Some("this is B".to_string());
    update_company_workflow(&company, None, &store, &revs_dyn, None, edit_b, None, None)
        .await
        .expect("edit to B");
    let body_b = current_toml(&store, &company, "greeter").await;
    let rev_a = revs.list_revisions(&company, "greeter").await.unwrap()[0]
        .id
        .clone();

    // Restore A: the live body becomes A again, and B is captured as the new
    // newest revision — so the rollback can itself be rolled back.
    let restored = rollback_company_workflow(
        &company, None, &store, &revs_dyn, None, "greeter", &rev_a, None,
    )
    .await
    .expect("rollback to A");
    assert_eq!(restored.description.as_deref(), Some("A tiny graph."));
    assert_eq!(current_toml(&store, &company, "greeter").await, body_a);

    let history = revs.list_revisions(&company, "greeter").await.unwrap();
    assert_eq!(
        history.len(),
        2,
        "the restore captured the body it replaced"
    );
    assert_eq!(history[0].toml, body_b, "B is the newest snapshot now");

    // …and restoring that B snapshot puts B back.
    let rev_b = history[0].id.clone();
    rollback_company_workflow(
        &company, None, &store, &revs_dyn, None, "greeter", &rev_b, None,
    )
    .await
    .expect("rollback the rollback");
    assert_eq!(current_toml(&store, &company, "greeter").await, body_b);
}

#[tokio::test]
async fn rollback_of_a_revision_naming_a_removed_teammate_is_400_and_leaves_current() {
    let company = CompanyId::new("acme");
    let (store, revs, _body_a) = seeded_greeter().await;
    let revs_dyn: Arc<dyn WorkflowRevisionStore> = revs.clone();

    // Edit to capture a revision (which still names `assistant`), then set B.
    let mut edit_b = valid_draft("greeter", "Greeter");
    edit_b.description = Some("B, assistant still valid here".to_string());
    update_company_workflow(&company, None, &store, &revs_dyn, None, edit_b, None, None)
        .await
        .expect("edit to B");
    let body_b = current_toml(&store, &company, "greeter").await;
    let rev_a = revs.list_revisions(&company, "greeter").await.unwrap()[0]
        .id
        .clone();

    // Remove `assistant` from the roster: the captured revision now names a
    // teammate the current record does not know about.
    let mut rec = store.load(&company).await.unwrap().unwrap();
    rec.manifest = toml::from_str("[company]\nname = \"Acme\"\n").unwrap();
    store.save(&rec).await.unwrap();

    let err = rollback_company_workflow(
        &company, None, &store, &revs_dyn, None, "greeter", &rev_a, None,
    )
    .await
    .expect_err("a revision naming a removed teammate must not restore");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    // The current body is untouched — a rejected rollback writes nothing.
    assert_eq!(current_toml(&store, &company, "greeter").await, body_b);
}

#[tokio::test]
async fn rollback_with_a_stale_expected_version_is_409_and_writes_nothing() {
    let company = CompanyId::new("acme");
    let (store, revs, body_a) = seeded_greeter().await;
    let revs_dyn: Arc<dyn WorkflowRevisionStore> = revs.clone();

    let mut edit_b = valid_draft("greeter", "Greeter");
    edit_b.description = Some("B".to_string());
    update_company_workflow(&company, None, &store, &revs_dyn, None, edit_b, None, None)
        .await
        .expect("edit to B");
    let body_b = current_toml(&store, &company, "greeter").await;
    let rev_a = revs.list_revisions(&company, "greeter").await.unwrap()[0]
        .id
        .clone();

    let err = rollback_company_workflow(
        &company,
        None,
        &store,
        &revs_dyn,
        None,
        "greeter",
        &rev_a,
        Some("deadbeef-not-the-current-token"),
    )
    .await
    .expect_err("a stale token must refuse the restore");
    assert!(matches!(err, OpenCompanyError::Conflict(_)), "{err:?}");
    assert_eq!(
        current_toml(&store, &company, "greeter").await,
        body_b,
        "a 409 must leave the live body unchanged"
    );

    // The response token of a successful restore is the hash of the restored
    // body — echo the CURRENT token and the restore lands.
    let current = workflow_version(&body_b);
    let restored = rollback_company_workflow(
        &company,
        None,
        &store,
        &revs_dyn,
        None,
        "greeter",
        &rev_a,
        Some(&current),
    )
    .await
    .expect("the current token lets the restore through");
    // The restored live body is the captured A body, and the token the write
    // response carries is that body's hash.
    let restored_toml = current_toml(&store, &company, "greeter").await;
    assert_eq!(restored_toml, body_a, "restoring A puts A back verbatim");
    assert_eq!(restored.id, "greeter");
    assert_eq!(
        workflow_version(&restored_toml),
        workflow_version(&body_a),
        "the response token is the restored body's hash"
    );
}

#[tokio::test]
async fn rollback_disarms_a_restored_schedule() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let revs: Arc<MemRevisions> = Arc::new(MemRevisions::default());
    let revs_dyn: Arc<dyn WorkflowRevisionStore> = revs.clone();

    // A scheduled workflow lands disarmed on create (issue #276); arm it, so
    // the "restored cron re-arms" hazard is real to test.
    let mut scheduled = valid_draft("greeter", "Greeter");
    scheduled.nodes[0].schedule = Some("0 9 * * *".to_string());
    create_company_workflow(&company, None, &store, None, scheduled, None, None)
        .await
        .expect("create scheduled");
    set_company_workflow_enabled(&company, None, &store, None, "greeter", true, true, &[])
        .await
        .expect("arm it");

    // Edit the schedule away — the workflow stays armed (removal never
    // disarms), and the scheduled body is captured as a revision.
    let mut unscheduled = valid_draft("greeter", "Greeter");
    unscheduled.description = Some("no schedule now".to_string());
    update_company_workflow(
        &company,
        None,
        &store,
        &revs_dyn,
        None,
        unscheduled,
        None,
        None,
    )
    .await
    .expect("remove schedule");
    assert!(
        store
            .load(&company)
            .await
            .unwrap()
            .unwrap()
            .workflow_enabled("greeter"),
        "removing a schedule must not disarm"
    );
    let rev_scheduled = revs.list_revisions(&company, "greeter").await.unwrap()[0]
        .id
        .clone();

    // Restoring the scheduled body re-introduces a cron the live graph lacked
    // → it lands switched off pending review.
    rollback_company_workflow(
        &company,
        None,
        &store,
        &revs_dyn,
        None,
        "greeter",
        &rev_scheduled,
        None,
    )
    .await
    .expect("restore scheduled body");
    assert!(
        !store
            .load(&company)
            .await
            .unwrap()
            .unwrap()
            .workflow_enabled("greeter"),
        "a restored schedule must land disarmed (issue #276)"
    );
}

#[tokio::test]
async fn rollback_unknown_revision_is_not_found() {
    let company = CompanyId::new("acme");
    let (store, revs, _body_a) = seeded_greeter().await;
    let revs_dyn: Arc<dyn WorkflowRevisionStore> = revs.clone();
    let err = rollback_company_workflow(
        &company,
        None,
        &store,
        &revs_dyn,
        None,
        "greeter",
        "no-such-rev",
        None,
    )
    .await
    .expect_err("unknown revision");
    assert!(
        matches!(err, OpenCompanyError::CompanyNotFound(_)),
        "{err:?}"
    );
}
