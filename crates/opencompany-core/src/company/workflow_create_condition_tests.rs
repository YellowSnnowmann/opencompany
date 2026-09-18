//! workflow_create: issue #661/#682 required config and condition labels on the draft path.

use super::test_support::*;
use super::*;

// --- issue #661/#682: required config + condition labels on the draft path

/// A minimal draft — trigger → condition `gate` → two outputs — with the
/// gate's `config.field` and both branch labels parameterised. Since
/// `parse_workflow` is now lenient on the #661 rules (issue #682), these are
/// the graphs that prove the create/update path still enforces them strictly.
fn condition_draft(
    field: Option<&str>,
    yes_label: Option<&str>,
    no_label: Option<&str>,
) -> RawWorkflow {
    let config = field.map(|field| {
        let mut table = toml::map::Map::new();
        table.insert("field".to_string(), toml::Value::String(field.to_string()));
        toml::Value::Table(table)
    });
    let node = |id: &str, kind: &str, config: Option<toml::Value>| RawNode {
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
    };
    RawWorkflow {
        id: "wf".to_string(),
        name: "WF".to_string(),
        description: None,
        owner_desk: None,
        nodes: vec![
            node("start", "trigger", None),
            node("gate", "condition", config),
            node("a", "output", None),
            node("b", "output", None),
        ],
        edges: vec![
            RawEdge {
                from: "start".to_string(),
                to: "gate".to_string(),
                label: None,
            },
            RawEdge {
                from: "gate".to_string(),
                to: "a".to_string(),
                label: yes_label.map(str::to_string),
            },
            RawEdge {
                from: "gate".to_string(),
                to: "b".to_string(),
                label: no_label.map(str::to_string),
            },
        ],
    }
}

/// A condition draft with no `config.field` is refused at author time even
/// though `parse_workflow` would now let it load.
#[tokio::test]
async fn draft_condition_without_field_is_invalid() {
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
        condition_draft(None, Some("yes"), Some("no")),
        None,
        None,
    )
    .await
    .expect_err("condition with no field");
    // Issue #1016: the config gate now raises a structured `WorkflowInvalid`.
    let problems = match &err {
        OpenCompanyError::WorkflowInvalid { problems } => problems,
        other => panic!("{other:?}"),
    };
    assert_eq!(problems[0].node_id.as_deref(), Some("gate"));
    assert_eq!(problems[0].field.as_deref(), Some("config.field"));
    assert!(err.to_string().contains("config.field"), "{err}");
}

/// A condition branch labeled anything but `yes`/`no` is refused at author
/// time — the load path is lenient, so this rule now lives entirely here.
#[tokio::test]
async fn draft_condition_with_non_yes_no_label_is_invalid() {
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
        condition_draft(Some("=item.ok"), Some("pass"), Some("no")),
        None,
        None,
    )
    .await
    .expect_err("off-vocabulary condition label");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("labeled `yes` or `no`"), "{err}");
}

/// The positive control: a condition with a `field` and `yes`/`no` branches
/// is accepted by the same author path.
#[tokio::test]
async fn draft_condition_with_field_and_yes_no_labels_is_valid() {
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
        condition_draft(Some("=item.approved"), Some("yes"), Some("no")),
        None,
        None,
    )
    .await
    .expect("a well-formed condition draft is accepted");
}

/// An http_request draft missing BOTH `method` and `url` reports both in one
/// 400 — the draft path collects every required-config problem for a node,
/// not just the first, so a human/model iterating hears the full list.
#[tokio::test]
async fn draft_http_request_missing_method_and_url_reports_both() {
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
                id: "fetch".to_string(),
                kind: "http_request".to_string(),
                name: "Fetch".to_string(),
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
        ],
        edges: vec![RawEdge {
            from: "start".to_string(),
            to: "fetch".to_string(),
            label: None,
        }],
    };
    let err = create_company_workflow(&company, None, &store, None, draft, None, None)
        .await
        .expect_err("http_request with no method or url");
    // Issue #1016: structured `WorkflowInvalid`, one problem per field, both
    // pinned to the offending node.
    let problems = match &err {
        OpenCompanyError::WorkflowInvalid { problems } => problems,
        other => panic!("{other:?}"),
    };
    assert!(
        problems
            .iter()
            .all(|p| p.node_id.as_deref() == Some("fetch")),
        "{problems:?}"
    );
    let fields: Vec<&str> = problems.iter().filter_map(|p| p.field.as_deref()).collect();
    assert!(fields.contains(&"config.method"), "{fields:?}");
    assert!(fields.contains(&"config.url"), "{fields:?}");
    let message = err.to_string();
    assert!(message.contains("config.method"), "{message}");
    assert!(message.contains("config.url"), "{message}");
}
