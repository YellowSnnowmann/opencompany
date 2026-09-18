//! workflow_create: issue #1016 structured, per-node/field workflow problems.

use super::test_support::*;
use super::tests_schedule_update::with_one_workflow;
use super::*;

// --- issue #1016: structured, per-node/field workflow problems -----------

/// A bare node with an optional config table.
fn oc_node(id: &str, kind: &str, config: Option<toml::Value>) -> RawNode {
    RawNode {
        id: id.to_string(),
        kind: kind.to_string(),
        name: id.to_string(),
        summary: None,
        agent: None,
        schedule: None,
        config,
        on_error: None,
        retry: None,
        requires_approval: None,
        repeatable: None,
        destination: None,
        postcondition: None,
        verify: None,
    }
}

/// A `trigger → <node>` two-node draft, so the node under test sits on a
/// reachable, single-trigger graph the shape check accepts.
fn one_node_draft(node: RawNode) -> RawWorkflow {
    let to = node.id.clone();
    RawWorkflow {
        id: "wf".to_string(),
        name: "WF".to_string(),
        description: None,
        owner_desk: None,
        nodes: vec![oc_node("start", "trigger", None), node],
        edges: vec![RawEdge {
            from: "start".to_string(),
            to,
            label: None,
        }],
    }
}

pub(super) fn problems_of(err: &OpenCompanyError) -> &[WorkflowProblem] {
    match err {
        OpenCompanyError::WorkflowInvalid { problems } => problems,
        other => panic!("expected WorkflowInvalid, got {other:?}"),
    }
}

/// RED-FIRST #1 (headline): a `transform` with no `config.set` is now rejected
/// at save, naming the node and `config.set`. Accepted on unpatched code.
#[tokio::test]
async fn draft_transform_without_set_is_rejected() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        one_node_draft(oc_node("tf", "transform", None)),
        None,
        None,
    )
    .await
    .expect_err("transform with no config.set");
    let problems = problems_of(&err);
    assert_eq!(problems[0].node_id.as_deref(), Some("tf"));
    assert_eq!(problems[0].field.as_deref(), Some("config.set"));
}

/// RED-FIRST #1 (split_out half): a `split_out` with no `config.path` is
/// rejected, naming `config.path`.
#[tokio::test]
async fn draft_split_out_without_path_is_rejected() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        one_node_draft(oc_node("so", "split_out", None)),
        None,
        None,
    )
    .await
    .expect_err("split_out with no config.path");
    let problems = problems_of(&err);
    assert_eq!(problems[0].node_id.as_deref(), Some("so"));
    assert_eq!(problems[0].field.as_deref(), Some("config.path"));
}

fn http_bad_url_draft() -> RawWorkflow {
    let mut config = toml::map::Map::new();
    config.insert("method".to_string(), toml::Value::String("GET".to_string()));
    config.insert(
        "url".to_string(),
        toml::Value::String("not-a-url".to_string()),
    );
    one_node_draft(oc_node(
        "greet",
        "http_request",
        Some(toml::Value::Table(config)),
    ))
}

/// RED-FIRST #2 (create): a create with an http_request `url` of `not-a-url`
/// yields a `WorkflowInvalid` whose first problem is pinned to `greet` /
/// `config.url`. Asserted on the STRUCT, not the joined message.
#[tokio::test]
async fn draft_http_request_bad_url_is_rejected_on_create() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        http_bad_url_draft(),
        None,
        None,
    )
    .await
    .expect_err("http_request url = not-a-url");
    let problems = problems_of(&err);
    assert_eq!(problems[0].node_id.as_deref(), Some("greet"));
    assert_eq!(problems[0].field.as_deref(), Some("config.url"));
}

/// RED-FIRST #2 (update): the same structured rejection on the update path.
#[tokio::test]
async fn draft_http_request_bad_url_is_rejected_on_update() {
    let company = CompanyId::new("acme");
    let (store, version) = with_one_workflow(&company, "wf", "WF").await;
    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        http_bad_url_draft(),
        Some(&version),
        None,
    )
    .await
    .expect_err("update to a bad url");
    let problems = problems_of(&err);
    assert_eq!(problems[0].node_id.as_deref(), Some("greet"));
    assert_eq!(problems[0].field.as_deref(), Some("config.url"));
}

/// RED-FIRST #3: a create whose edge has a dangling `from` yields a structured
/// problem naming the endpoint (`old-id`) and the `from` field, and the
/// message no longer leads with `edge #N`.
#[tokio::test]
async fn draft_dangling_from_edge_is_structured() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let draft = RawWorkflow {
        id: "wf".to_string(),
        name: "WF".to_string(),
        description: None,
        owner_desk: None,
        nodes: vec![oc_node("start", "trigger", None)],
        edges: vec![RawEdge {
            from: "old-id".to_string(),
            to: "start".to_string(),
            label: None,
        }],
    };
    let err = create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect_err("dangling from");
    let problems = problems_of(&err);
    assert_eq!(problems[0].node_id.as_deref(), Some("old-id"));
    assert_eq!(problems[0].field.as_deref(), Some("from"));
    assert!(!err.to_string().contains("edge #"), "{err}");
}

fn sub_workflow_draft(workflow_id: &str) -> RawWorkflow {
    let mut config = toml::map::Map::new();
    config.insert(
        "workflow_id".to_string(),
        toml::Value::String(workflow_id.to_string()),
    );
    one_node_draft(oc_node(
        "child",
        "sub_workflow",
        Some(toml::Value::Table(config)),
    ))
}

/// A `sub_workflow` naming a workflow id that this company cannot resolve is
/// rejected, pinned to the node and `workflow_id`.
#[tokio::test]
async fn draft_sub_workflow_with_unknown_id_is_rejected() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let mut draft = sub_workflow_draft("nope");
    draft.id = "parent".to_string();
    let err = create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect_err("unknown sub-workflow id");
    let problems = problems_of(&err);
    assert_eq!(problems[0].node_id.as_deref(), Some("child"));
    assert_eq!(problems[0].field.as_deref(), Some("workflow_id"));
}

/// A `sub_workflow` referencing an existing saved workflow passes the record
/// cross-check.
#[tokio::test]
async fn draft_sub_workflow_with_existing_id_is_accepted() {
    let company = CompanyId::new("acme");
    let (store, _) = with_one_workflow(&company, "greeter", "Greeter").await;
    let mut draft = sub_workflow_draft("greeter");
    draft.id = "parent".to_string();
    draft.name = "Parent".to_string();
    create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect("sub_workflow referencing a saved workflow is accepted");
}

/// A `sub_workflow` referencing its own id is still rejected (regression): the
/// structural self-reference check owns that message.
#[tokio::test]
async fn draft_sub_workflow_self_reference_is_rejected() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    // draft.id defaults to "wf" — a self reference to the same id.
    let draft = sub_workflow_draft("wf");
    let err = create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect_err("self-referencing sub_workflow");
    assert!(err.to_string().contains("run itself"), "{err}");
}

/// An `output_parser` with no schema (a pass-through identity parser) is
/// accepted; a `merge` with no config is accepted — the gate does not
/// over-reject the config-optional kinds.
#[tokio::test]
async fn draft_output_parser_and_merge_are_config_optional() {
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
        one_node_draft(oc_node("op", "output_parser", None)),
        None,
        None,
    )
    .await
    .expect("schema-less output_parser is accepted");

    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        one_node_draft(oc_node("mg", "merge", None)),
        None,
        None,
    )
    .await
    .expect("config-less merge is accepted");
}

/// A manifest with an `assistant` roster agent AND an `ops` desk that
/// agent sits on — issue #1862 prerequisite's "accept wired" case needs a
/// real desk to resolve against.
fn manifest_with_assistant_and_desk() -> CompanyManifest {
    toml::from_str(
        "[company]\nname = \"Acme\"\n[[agent]]\nid = \"assistant\"\nrole = \"Assistant\"\n\
         [[group_chat]]\nid = \"ops\"\nname = \"Ops\"\nmembers = [\"assistant\"]\n",
    )
    .expect("valid manifest")
}

/// Issue #1862 prerequisite, RED-FIRST: a draft naming an `owner_desk` that
/// resolves against NO desk on the company is rejected at author time,
/// naming the `owner_desk` field. Accepted on unpatched code, because the
/// field — and this check — did not exist.
#[tokio::test]
async fn draft_with_unknown_owner_desk_is_rejected() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let mut draft = valid_draft("wf", "WF");
    draft.owner_desk = Some("ghost-desk".to_string());
    let err = create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect_err("owner_desk naming no real desk");
    let problems = problems_of(&err);
    assert_eq!(problems[0].node_id, None, "graph-level, not node-scoped");
    assert_eq!(problems[0].field.as_deref(), Some("owner_desk"));
}

/// The accept half of the same gate: an `owner_desk` that resolves against
/// a real desk is accepted, and the saved graph carries it through.
#[tokio::test]
async fn draft_with_a_wired_owner_desk_is_accepted() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant_and_desk(),
    )));
    let mut draft = valid_draft("wf", "WF");
    draft.owner_desk = Some("ops".to_string());
    let file = create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect("owner_desk naming a real desk is accepted");
    assert_eq!(file.owner_desk.as_deref(), Some("ops"));
}

/// A blank/whitespace `owner_desk` is treated as unset rather than
/// resolved against the desk set — the same "empty means absent" leniency
/// the rest of this draft's optional fields get.
#[tokio::test]
async fn draft_with_blank_owner_desk_is_accepted() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let mut draft = valid_draft("wf", "WF");
    draft.owner_desk = Some("   ".to_string());
    create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect("a blank owner_desk is not resolved against the desk set");
}

/// **Regression, issue #1882 review.** An edit to a field that has
/// nothing to do with `owner_desk` must still save when the workflow's
/// STORED desk has since been renamed or removed — the same "a field
/// nobody looked at" leniency `parse_workflow`'s lenient load path
/// already grants. Before the fix, the strict desk-exists check
/// re-validated the untouched, unchanged desk on every save and refused
/// unconditionally — so once a desk went stale, an operator could not
/// save ANY edit from an editor that (correctly, per the round-trip fix)
/// carries `ownerDesk` forward with no control to clear it.
#[tokio::test]
async fn an_unrelated_update_survives_a_desk_that_went_stale() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant_and_desk(),
    )));
    let mut created = valid_draft("wf", "WF");
    created.owner_desk = Some("ops".to_string());
    create_company_workflow(&company, None, &store, None, created, None, None)
        .await
        .expect("creates with a real desk");
    let saved = store.load(&company).await.unwrap().unwrap();
    let version = workflow_version(&saved.overlay_workflows[0].toml);

    // The desk is renamed/removed underneath the workflow — nothing about
    // the workflow itself is touched.
    let mut stale_record = store.load(&company).await.unwrap().unwrap();
    stale_record.manifest = manifest_with_assistant();
    store.save(&stale_record).await.unwrap();

    // An edit to a completely unrelated field, carrying the stored
    // owner_desk forward unchanged rather than re-typing it — exactly
    // what a console round-tripping the read does.
    let mut edit = valid_draft("wf", "WF");
    edit.owner_desk = Some("ops".to_string());
    edit.description = Some("Renamed the description only.".to_string());
    let file = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        edit,
        Some(&version),
        None,
    )
    .await
    .expect("an unrelated edit must save even though the stored desk went stale");
    assert_eq!(
        file.owner_desk.as_deref(),
        Some("ops"),
        "the stale desk is grandfathered, not cleared"
    );

    // A DIFFERENT bad desk is still a refusal — grandfathering only covers
    // the value already on file, never a newly typed/selected one.
    let saved2 = store.load(&company).await.unwrap().unwrap();
    let version2 = workflow_version(&saved2.overlay_workflows[0].toml);
    let mut bad_edit = valid_draft("wf", "WF");
    bad_edit.owner_desk = Some("a-totally-different-ghost-desk".to_string());
    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        bad_edit,
        Some(&version2),
        None,
    )
    .await
    .expect_err("a newly typed desk that resolves to nothing is still refused");
    let problems = problems_of(&err);
    assert_eq!(problems[0].field.as_deref(), Some("owner_desk"));
}

/// **Regression, issue #1882 review ("preserve padded stale owners"),
/// RED-FIRST.** The grandfathering above compares the draft's
/// `owner_desk` against the STORED body's. The draft side is trimmed on
/// the way in (`normalize_owner_desk` at the top of
/// `update_company_workflow`); the stored side used to be whatever the
/// saved TOML literally held. A stored value with surrounding whitespace
/// therefore never compared equal to the same value round-tripped through
/// a GET/PUT, so the grandfathering did not apply and an unrelated edit
/// was refused once the padded desk went stale. `parse_workflow` now
/// normalizes the stored side too, so both are trimmed by construction.
#[tokio::test]
async fn an_unrelated_update_survives_a_padded_stored_desk_that_went_stale() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant_and_desk(),
    )));
    let mut created = valid_draft("wf", "WF");
    created.owner_desk = Some("ops".to_string());
    create_company_workflow(&company, None, &store, None, created, None, None)
        .await
        .expect("creates with a real desk");

    // Pad the STORED value. Every write boundary trims, so this stands in
    // for a body that reached the record by any other route — a
    // hand-authored graph, an import, a body written before the trim.
    let mut padded_record = store.load(&company).await.unwrap().unwrap();
    padded_record.overlay_workflows[0].toml = padded_record.overlay_workflows[0]
        .toml
        .replace("owner_desk = \"ops\"", "owner_desk = \"  ops  \"");
    assert!(
        padded_record.overlay_workflows[0]
            .toml
            .contains("owner_desk = \"  ops  \""),
        "the padded stored body must actually be in place for this test to mean anything"
    );
    // The desk goes stale underneath the workflow at the same time.
    padded_record.manifest = manifest_with_assistant();
    store.save(&padded_record).await.unwrap();
    let version = workflow_version(&padded_record.overlay_workflows[0].toml);

    // What a console round-trip sends back: the desk exactly as the read
    // route hands it out (trimmed), with an unrelated field edited.
    let mut edit = valid_draft("wf", "WF");
    edit.owner_desk = Some("ops".to_string());
    edit.description = Some("Renamed the description only.".to_string());
    let file = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        edit,
        Some(&version),
        None,
    )
    .await
    .expect("a padded stored desk must grandfather the same as an unpadded one");
    assert_eq!(
        file.owner_desk.as_deref(),
        Some("ops"),
        "the stale desk is carried forward, not cleared"
    );
}

/// **Regression, issue #1882 review (PR #1882 bot finding, comment
/// 3878829353), RED-FIRST.** The grandfathering above (previous test)
/// only covers a stored `owner_desk` that stays UNRESOLVABLE. This
/// covers the sharper case the bot flagged: the desk that used to own
/// the stored raw string is deleted, and a *different*, later desk is
/// created whose display name happens to equal that same string (desk
/// creation enforces id uniqueness, not name uniqueness — nothing stops
/// this). The stored value is now newly RESOLVABLE again, just to the
/// wrong desk. An unrelated edit that round-trips `owner_desk` unchanged
/// must not let that resolution through — a PUT that never touched the
/// field must never reassign a workflow's owning desk.
#[tokio::test]
async fn an_unrelated_update_does_not_retarget_a_desk_id_recycled_as_a_name() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant_and_desk(),
    )));
    let mut created = valid_draft("wf", "WF");
    created.owner_desk = Some("ops".to_string());
    create_company_workflow(&company, None, &store, None, created, None, None)
        .await
        .expect("creates with a real desk");
    let saved = store.load(&company).await.unwrap().unwrap();
    let version = workflow_version(&saved.overlay_workflows[0].toml);

    // The "ops" desk is deleted, and a brand-new, UNRELATED desk is
    // created whose display name happens to be the literal string
    // "ops" — the old desk's id, now recycled as someone else's name.
    let mut stale_record = store.load(&company).await.unwrap().unwrap();
    stale_record.manifest = manifest_with_assistant();
    stale_record.overlay_desks = vec![OverlayDesk {
        id: "sales_new".to_string(),
        name: "ops".to_string(),
        description: None,
        members: vec!["assistant".to_string()],
        responder: ResponderMode::default(),
        hive: Default::default(),
    }];
    store.save(&stale_record).await.unwrap();

    // An edit to a completely unrelated field, carrying the stored
    // owner_desk forward unchanged — exactly what a console
    // round-tripping the read does.
    let mut edit = valid_draft("wf", "WF");
    edit.owner_desk = Some("ops".to_string());
    edit.description = Some("Renamed the description only.".to_string());
    let file = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        edit,
        Some(&version),
        None,
    )
    .await
    .expect("an unrelated edit must save even though the stored desk was recycled");
    assert_eq!(
        file.owner_desk.as_deref(),
        Some("ops"),
        "the raw stored value must be carried forward untouched, not resolved to the \
         unrelated desk that recycled it as a display name"
    );
}

/// **Regression, issue #1882 review (PR #1882 bot finding).** A draft
/// naming its owner desk by DISPLAY NAME — `resolve_desk_id` accepts
/// either the id or a case-insensitive name — must be normalized to the
/// desk's canonical id before it is persisted. `render_workflow`
/// serializes `owner_desk` verbatim and has no `record` to re-resolve an
/// alias at save time, so leaving the alias in place would mean: if this
/// desk is later deleted and a new one created reusing the same display
/// name (desk creation enforces id uniqueness, not name uniqueness), the
/// stored alias would silently start resolving to the NEW desk on the
/// next load, re-routing this workflow's future blocker DMs to the wrong
/// team with no edit ever made to the workflow itself.
#[tokio::test]
async fn draft_naming_owner_desk_by_display_name_is_normalized_to_its_id() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant_and_desk(),
    )));
    // The desk's id is "ops", its display name "Ops" (see
    // `manifest_with_assistant_and_desk`) — supply the alias, not the id.
    let mut draft = valid_draft("wf", "WF");
    draft.owner_desk = Some("Ops".to_string());
    let file = create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect("owner_desk naming a real desk by display name is accepted");
    assert_eq!(
        file.owner_desk.as_deref(),
        Some("ops"),
        "the stored owner_desk must be the canonical id, not the display-name alias supplied"
    );

    // Same normalization on the update path, where the alias is
    // re-typed on an otherwise-untouched edit rather than the id the
    // create above just normalized.
    let saved = store.load(&company).await.unwrap().unwrap();
    let version = workflow_version(&saved.overlay_workflows[0].toml);
    let mut edit = valid_draft("wf", "WF");
    edit.owner_desk = Some("Ops".to_string());
    edit.description = Some("Renamed the description only.".to_string());
    let file2 = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        edit,
        Some(&version),
        None,
    )
    .await
    .expect("owner_desk re-typed as a display name alias is accepted on update");
    assert_eq!(
        file2.owner_desk.as_deref(),
        Some("ops"),
        "the update path must also normalize the alias to the canonical id"
    );
}

/// **Regression, issue #1882 review (PR #1882 bot finding, comment
/// 3878620688), RED-FIRST.** Two desks sharing the same display name are
/// not a future-recreation hazard like the test above — desk creation
/// enforces id uniqueness, not name uniqueness (see the comment above
/// `resolved_owner_desk` in `validate_draft_against_record`), so both can
/// coexist right now. `resolve_desk_id`'s alias pass answers with
/// whichever of the two it iterates to first; unpatched, this write
/// silently persists that arbitrary desk instead of refusing the
/// ambiguous name, which would route this workflow's future blocker DMs
/// to a team the caller never actually named.
#[tokio::test]
async fn draft_naming_an_ambiguous_desk_display_name_is_rejected() {
    let company = CompanyId::new("acme");
    let mut seed = record(&company, manifest_with_assistant());
    seed.overlay_desks = vec![
        OverlayDesk {
            id: "sales_us".to_string(),
            name: "Sales".to_string(),
            description: None,
            members: vec!["assistant".to_string()],
            responder: ResponderMode::default(),
            hive: Default::default(),
        },
        OverlayDesk {
            id: "sales_eu".to_string(),
            name: "Sales".to_string(),
            description: None,
            members: vec!["assistant".to_string()],
            responder: ResponderMode::default(),
            hive: Default::default(),
        },
    ];
    let store = store_of(MemStore::seeded(seed));
    let mut draft = valid_draft("wf", "WF");
    draft.owner_desk = Some("Sales".to_string());
    let err = create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect_err(
            "an owner_desk display name naming two desks must be refused, not silently \
             resolved to whichever one is iterated to first",
        );
    let problems = problems_of(&err);
    assert_eq!(problems[0].node_id, None, "graph-level, not node-scoped");
    assert_eq!(problems[0].field.as_deref(), Some("owner_desk"));
}
