//! workflow_file: TOML round-tripping, owner-desk parsing, and basic graph validity (trigger, edges, node ids, unknown keys).

use super::*;

const LOADABLE_WORKFLOW: &str = r#"
    id = "valid"
    name = "Valid"
    [[node]]
    id = "start"
    kind = "trigger"
    name = "Start"
    [[node]]
    id = "done"
    kind = "output"
    name = "Done"
    [[edge]]
    from = "start"
    to = "done"
"#;

/// Issue #1862 prerequisite: a workflow TOML with no `owner_desk` key —
/// every graph saved before this field existed — parses to `None` rather
/// than failing. `LOADABLE_WORKFLOW` has no `owner_desk` line by
/// construction, so this is the back-compat case.
#[test]
fn a_workflow_toml_with_no_owner_desk_parses_to_none() {
    let file = parse_workflow(LOADABLE_WORKFLOW).expect("parses");
    assert_eq!(file.owner_desk, None);
}

/// **Regression, issue #1882 review ("preserve padded stale owners").**
/// A stored `owner_desk` carrying surrounding whitespace parses to the
/// TRIMMED value, and a blank one to `None` — the same "blank means
/// absent" rule [`RawWorkflow::normalize_owner_desk`] already applies at
/// every write boundary.
///
/// RED-FIRST: this load path used to carry `raw.owner_desk` through
/// verbatim, so the stored side of the unchanged-owner comparison in
/// `workflow_create::validate_draft_against_record` was padded while the
/// draft side had been trimmed on the way in. The two never compared
/// equal, which defeated the grandfathering: an unrelated edit could be
/// refused over a stale desk, or silently re-resolved onto a different
/// desk that had since taken the same display name.
#[test]
fn a_padded_owner_desk_parses_trimmed_and_a_blank_one_parses_absent() {
    let padded = LOADABLE_WORKFLOW.replace(
        "id = \"valid\"",
        "id = \"valid\"\n        owner_desk = \"  engineering  \"",
    );
    let file = parse_workflow(&padded).expect("parses");
    assert_eq!(file.owner_desk.as_deref(), Some("engineering"));

    let blank = LOADABLE_WORKFLOW.replace(
        "id = \"valid\"",
        "id = \"valid\"\n        owner_desk = \"   \"",
    );
    let file = parse_workflow(&blank).expect("parses");
    assert_eq!(
        file.owner_desk, None,
        "blank is absent, not `Some(\"   \")`"
    );
}

/// A workflow TOML that DOES carry `owner_desk` round-trips it — the
/// lenient load path (`validate(&raw, false)`) carries the value through
/// unvalidated; desk existence is checked only at author time.
#[test]
fn a_workflow_toml_with_owner_desk_parses_it_through() {
    let with_owner = LOADABLE_WORKFLOW.replacen(
        "id = \"valid\"",
        "id = \"valid\"\n        owner_desk = \"engineering\"",
        1,
    );
    let file = parse_workflow(&with_owner).expect("parses");
    assert_eq!(file.owner_desk.as_deref(), Some("engineering"));
}

/// The filename is the id-based lookup key. A different id inside the body
/// must never leak into a list as a row that `GET /workflows/{id}` cannot
/// open, while a valid sibling continues to load.
#[test]
fn load_company_workflows_skips_filename_id_mismatches() {
    let dir = tempfile::tempdir().unwrap();
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("valid.toml"), LOADABLE_WORKFLOW).unwrap();
    std::fs::write(
        workflows.join("wrong-stem.toml"),
        LOADABLE_WORKFLOW.replace("id = \"valid\"", "id = \"different\""),
    )
    .unwrap();

    let loaded =
        load_company_workflows(dir.path(), &["wrong-stem".to_string(), "valid".to_string()])
            .expect("a mismatched body is skipped rather than failing the load");

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "valid");
}

/// The directory scan shares the loader's stem/id choke point, so a bad
/// file is absent from the picker rather than listed under its embedded id.
#[test]
fn list_source_workflows_never_lists_a_mismatched_file() {
    let dir = tempfile::tempdir().unwrap();
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(
        workflows.join("wrong-stem.toml"),
        LOADABLE_WORKFLOW.replace("id = \"valid\"", "id = \"different\""),
    )
    .unwrap();

    assert!(list_source_workflows(Some(dir.path())).is_empty());
}

#[test]
fn render_workflow_round_trips_through_parse_workflow() {
    let raw = RawWorkflow {
        id: "wf".to_string(),
        name: "WF".to_string(),
        description: Some("A test graph.".to_string()),
        owner_desk: None,
        nodes: vec![
            RawNode {
                id: "start".to_string(),
                kind: "trigger".to_string(),
                name: "Start".to_string(),
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
            },
            RawNode {
                id: "worker".to_string(),
                kind: "agent".to_string(),
                name: "Worker".to_string(),
                summary: Some("Does the thing.".to_string()),
                agent: Some("ceo".to_string()),
                schedule: None,
                config: None,
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: None,
                destination: None,
                postcondition: None,
                verify: None,
            },
        ],
        edges: vec![RawEdge {
            from: "start".to_string(),
            to: "worker".to_string(),
            label: Some("ok".to_string()),
        }],
    };
    let toml_src = render_workflow(&raw).expect("renders");
    let file = parse_workflow(&toml_src).expect("re-parses the rendered graph");
    assert_eq!(file.id, "wf");
    assert_eq!(file.nodes.len(), 2);
    assert_eq!(file.edges.len(), 1);
    let worker = file.nodes.iter().find(|n| n.id == "worker").unwrap();
    assert_eq!(worker.agent.as_deref(), Some("ceo"));
    assert_eq!(file.edges[0].label.as_deref(), Some("ok"));
}

/// Issue #1866: a declared `postcondition` survives the render -> re-parse
/// round trip [`WorkflowSpecProjection`]/the console draft path relies on,
/// and — because [`WorkflowPostconditionDef`] is table-valued —
/// `toml::to_string` does not choke on field order the way it would if a
/// scalar field followed it (the reason `postcondition` sits beside
/// `retry`, before `destination`, on [`RawNode`]).
#[test]
fn render_workflow_round_trip_preserves_postcondition() {
    let raw = RawWorkflow {
        id: "wf".to_string(),
        name: "WF".to_string(),
        description: None,
        owner_desk: None,
        nodes: vec![
            RawNode {
                id: "start".to_string(),
                kind: "trigger".to_string(),
                name: "Start".to_string(),
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
            },
            RawNode {
                id: "worker".to_string(),
                kind: "agent".to_string(),
                name: "Worker".to_string(),
                summary: None,
                agent: Some("ceo".to_string()),
                schedule: None,
                config: None,
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: None,
                destination: None,
                postcondition: Some(WorkflowPostconditionDef {
                    require: "field_present".to_string(),
                    field: Some("json.items".to_string()),
                }),
                verify: None,
            },
        ],
        edges: vec![RawEdge {
            from: "start".to_string(),
            to: "worker".to_string(),
            label: None,
        }],
    };
    let toml_src = render_workflow(&raw).expect("renders, even with a table field present");
    let file = parse_workflow(&toml_src).expect("re-parses the rendered graph");
    let worker = file.nodes.iter().find(|n| n.id == "worker").unwrap();
    let postcondition = worker
        .postcondition
        .as_ref()
        .expect("the postcondition survived the round trip");
    assert_eq!(postcondition.require, "field_present");
    // Codex #3893851369 on #1937: the bare `items` this test used to
    // assert here validates but can never resolve at runtime (see
    // `postcondition_field_with_a_bare_structured_root_is_rejected`) —
    // the documented `json.items` form is the only one `parse_workflow`
    // now accepts.
    assert_eq!(postcondition.field.as_deref(), Some("json.items"));
}

/// Issue #1866: `postcondition` is operator-only policy, exactly like
/// `retry` and `requires_approval` beside it — the agent authoring schema
/// cannot express it, so [`project_workflow_spec`] must list it in
/// [`WorkflowSpecProjection::unexpressible`] rather than silently
/// dropping it on an agent-driven full-replacement edit.
#[cfg(feature = "openhuman")]
#[test]
fn project_workflow_spec_lists_postcondition_as_unexpressible() {
    let raw = RawWorkflow {
        id: "wf".to_string(),
        name: "WF".to_string(),
        description: None,
        owner_desk: None,
        nodes: vec![RawNode {
            id: "worker".to_string(),
            kind: "agent".to_string(),
            name: "Worker".to_string(),
            summary: None,
            agent: Some("ceo".to_string()),
            schedule: None,
            config: None,
            on_error: None,
            retry: None,
            requires_approval: None,
            repeatable: None,
            destination: None,
            postcondition: Some(WorkflowPostconditionDef {
                require: "non_empty".to_string(),
                field: None,
            }),
            verify: None,
        }],
        edges: Vec::new(),
    };
    let projection = project_workflow_spec(&raw);
    assert_eq!(projection.unexpressible.len(), 1);
    let (node_id, fields) = &projection.unexpressible[0];
    assert_eq!(node_id, "worker");
    assert!(
        fields.iter().any(|(name, _)| *name == "postcondition"),
        "postcondition must be named in the unexpressible residue: {fields:?}"
    );
    assert!(
        projection.unexpressible_summary().contains("postcondition"),
        "{}",
        projection.unexpressible_summary()
    );
    // And it does not leak into the agent-facing spec itself.
    let worker_spec = projection.spec["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "worker")
        .unwrap();
    assert!(worker_spec.get("postcondition").is_none());
}

/// A rendered graph that fails structural validation (no trigger) surfaces
/// the same prosumer-language problem `parse_workflow` gives a hand-authored
/// file — the create endpoint relies on this to turn a bad graph into a 4xx.
#[test]
fn render_workflow_of_an_invalid_graph_fails_reparse() {
    let raw = RawWorkflow {
        id: "wf".to_string(),
        name: "WF".to_string(),
        description: None,
        owner_desk: None,
        nodes: vec![RawNode {
            id: "only".to_string(),
            kind: "output".to_string(),
            name: "Only".to_string(),
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
        }],
        edges: vec![],
    };
    let toml_src = render_workflow(&raw).expect("renders even though invalid");
    let err = parse_workflow(&toml_src).unwrap_err();
    assert!(err.to_string().contains("trigger"), "{err}");
}

const CAMPAIGN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../companies/marketing_agency/workflows/campaign_pipeline.toml"
));

#[test]
fn parses_the_shipped_campaign_pipeline() {
    let workflow = parse_workflow(CAMPAIGN).expect("campaign pipeline is valid");
    assert_eq!(workflow.id, "campaign_pipeline");
    assert_eq!(workflow.name, "Campaign pipeline");
    assert_eq!(workflow.nodes.len(), 8);
    assert_eq!(workflow.edges.len(), 8);
    let strategist = workflow
        .nodes
        .iter()
        .find(|n| n.id == "strategist")
        .unwrap();
    assert_eq!(strategist.kind, WorkflowNodeKind::Agent);
    assert_eq!(strategist.agent.as_deref(), Some("brand_strategist"));
    let brief = workflow.nodes.iter().find(|n| n.id == "brief").unwrap();
    assert_eq!(brief.kind, WorkflowNodeKind::Trigger);
}

#[test]
fn edge_referencing_missing_node_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[edge]]
        from = "start"
        to = "ghost"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("ghost"), "{message}");
    assert!(message.contains("not a node"), "{message}");
}

#[test]
fn missing_trigger_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "only"
        kind = "output"
        name = "Only"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("trigger"), "{err}");
}

#[test]
fn empty_workflow_has_no_trigger() {
    let src = r#"
        id = "wf"
        name = "WF"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("trigger"), "{err}");
}

#[test]
fn duplicate_node_ids_and_self_loops_are_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "a"
        kind = "trigger"
        name = "A"
        [[node]]
        id = "a"
        kind = "output"
        name = "A2"
        [[edge]]
        from = "a"
        to = "a"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("more than once"), "{message}");
    assert!(message.contains("itself"), "{message}");
}

#[test]
fn unknown_kind_and_stray_agent_are_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "weird"
        kind = "teleport"
        name = "Weird"
        [[node]]
        id = "gate"
        kind = "condition"
        name = "Gate"
        agent = "someone"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("unknown `kind`"), "{message}");
    assert!(
        message.contains("only `agent` nodes name a teammate"),
        "{message}"
    );
}

#[test]
fn unknown_top_level_keys_are_tolerated() {
    let src = r#"
        id = "wf"
        name = "WF"
        canvas_zoom = 1.5
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        extra = "ignored"
    "#;
    assert!(parse_workflow(src).is_ok());
}
