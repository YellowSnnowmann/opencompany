use super::*;
use crate::company::parse_workflow;
use serde_json::json;

#[test]
fn node_conversion_preserves_p1_fields_and_camelcases_retry() {
    let file = parse_workflow(
        r#"
        id = "wf"
        name = "Workflow"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        on_error = "continue"
        requires_approval = true
        [node.config]
        message = "hello"
        [node.retry]
        max_attempts = 3
        backoff_ms = 250
        backoff = "exponential"
        "#,
    )
    .expect("workflow parses");

    let gql = WorkflowGql::from(file);
    let node = &gql.nodes[0];
    assert_eq!(node.config.as_ref().unwrap().0, json!({"message": "hello"}));
    assert_eq!(node.on_error.as_deref(), Some("continue"));
    assert_eq!(node.requires_approval, Some(true));
    assert_eq!(
        serde_json::to_value(&node.retry.as_ref().unwrap().0).unwrap(),
        json!({
            "maxAttempts": 3,
            "backoffMs": 250,
            "backoff": "exponential"
        })
    );
    // A node with no destination carries none.
    assert!(node.destination.is_none());
}

/// An `output` node's destination reaches the GraphQL read shape too — the
/// console's REST path is not the only reader, and a resolver that dropped
/// it would report the graph as routing nowhere (issue #170).
#[test]
fn node_conversion_preserves_the_output_destination() {
    let file = parse_workflow(
        r#"
        id = "wf"
        name = "Workflow"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "done"
        kind = "output"
        name = "Report"
        [node.destination]
        kind = "email"
        target = "ada@example.com"
        [[edge]]
        from = "start"
        to = "done"
        "#,
    )
    .expect("workflow parses");

    let gql = WorkflowGql::from(file);
    let done = gql.nodes.iter().find(|n| n.id.as_str() == "done").unwrap();
    assert_eq!(
        serde_json::to_value(&done.destination.as_ref().unwrap().0).unwrap(),
        json!({ "kind": "email", "target": "ada@example.com" })
    );
    // `owner` carries no target, and the key stays absent rather than null.
    let owner = parse_workflow(
        r#"
        id = "wf2"
        name = "Workflow 2"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "done"
        kind = "output"
        name = "Report"
        [node.destination]
        kind = "owner"
        [[edge]]
        from = "start"
        to = "done"
        "#,
    )
    .expect("parses");
    let gql = WorkflowGql::from(owner);
    let done = gql.nodes.iter().find(|n| n.id.as_str() == "done").unwrap();
    assert_eq!(
        serde_json::to_value(&done.destination.as_ref().unwrap().0).unwrap(),
        json!({ "kind": "owner" })
    );
}

/// Bonus finding from the #1937 boundary sweep (issue #1866): a node's
/// declared `postcondition` was never exposed over GraphQL at all — every
/// sibling policy field (`config`/`on_error`/`retry`/`requires_approval`/
/// `destination`) had a field on `WorkflowNodeGql` and this didn't, so
/// `Company.workflow(id)` silently dropped a run-safety gate for any
/// console surface reading over GraphQL instead of REST. Same shape as
/// `node_conversion_preserves_the_output_destination` above.
#[test]
fn node_conversion_preserves_the_postcondition() {
    let file = parse_workflow(
        r#"
        id = "wf3"
        name = "Workflow 3"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "worker"
        kind = "agent"
        name = "Worker"
        agent = "ceo"
        [node.postcondition]
        require = "field_present"
        field = "json.items"
        [node.verify]
        criteria = "Name the evidence."
        [[edge]]
        from = "start"
        to = "worker"
        "#,
    )
    .expect("workflow parses");

    let gql = WorkflowGql::from(file);
    let worker = gql
        .nodes
        .iter()
        .find(|n| n.id.as_str() == "worker")
        .unwrap();
    assert_eq!(
        serde_json::to_value(&worker.postcondition.as_ref().unwrap().0).unwrap(),
        json!({ "require": "field_present", "field": "json.items" })
    );
    assert_eq!(
        serde_json::to_value(&worker.verify.as_ref().unwrap().0).unwrap(),
        json!({ "criteria": "Name the evidence." })
    );
}
