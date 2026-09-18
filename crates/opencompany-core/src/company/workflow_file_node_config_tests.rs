//! workflow_file: per-node config, error/retry policy, and postcondition/verify field validation (P1).

use super::*;

// --- Per-node config / error / retry policy (P1) -----------------------

#[test]
fn node_config_parses_including_nested_tables() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "call"
        kind = "tool_call"
        name = "Export"
        [node.config]
        slug = "csv_export"
        [node.config.args]
        filename = "out.csv"
        data = "[]"
        [[edge]]
        from = "start"
        to = "call"
    "#;
    let file = parse_workflow(src).expect("config parses");
    let call = file.nodes.iter().find(|n| n.id == "call").unwrap();
    let config = call.config.as_ref().expect("config present");
    assert_eq!(config["slug"], "csv_export");
    // Nested table survives the TOML → JSON conversion.
    assert_eq!(config["args"]["filename"], "out.csv");
}

#[test]
fn non_finite_config_number_is_rejected_instead_of_dropped() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [node.config]
        threshold = nan
    "#;
    let err = parse_workflow(src).expect_err("non-JSON config must fail");
    assert!(err.to_string().contains("config"), "{err}");
}

#[test]
fn typed_error_retry_and_approval_fields_parse() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "call"
        kind = "tool_call"
        name = "Export"
        on_error = "continue"
        requires_approval = true
        [node.config]
        slug = "csv_export"
        [node.retry]
        max_attempts = 3
        backoff_ms = 100
        backoff = "exponential"
        [[edge]]
        from = "start"
        to = "call"
    "#;
    let file = parse_workflow(src).expect("parses");
    let call = file.nodes.iter().find(|n| n.id == "call").unwrap();
    assert_eq!(call.on_error.as_deref(), Some("continue"));
    assert_eq!(call.requires_approval, Some(true));
    let retry = call.retry.as_ref().expect("retry present");
    assert_eq!(retry.max_attempts, Some(3));
    assert_eq!(retry.backoff_ms, Some(100));
    assert_eq!(retry.backoff.as_deref(), Some("exponential"));
}

#[test]
fn bad_on_error_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        on_error = "explode"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("unknown `on_error`"), "{message}");
}

#[test]
fn retry_max_attempts_zero_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [node.retry]
        max_attempts = 0
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("at least 1"), "{err}");
}

#[test]
fn bad_retry_backoff_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [node.retry]
        backoff = "linear"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("unknown `retry.backoff`"), "{err}");
}

#[test]
fn reserved_config_keys_are_rejected() {
    // `on_error` inside `config` (not as a first-class field) is a footgun.
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [node.config]
        on_error = "route"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("inside `config`"), "{err}");
}

#[test]
fn postcondition_inside_config_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "worker"
        kind = "agent"
        name = "Worker"
        agent = "ceo"
        [node.config]
        postcondition = "non_empty"
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("inside `config`"), "{err}");
}

#[test]
fn postcondition_valid_on_an_agent_node_parses() {
    let src = r#"
        id = "wf"
        name = "WF"
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
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let file = parse_workflow(src).expect("a postcondition on an agent node is valid");
    let worker = file.nodes.iter().find(|n| n.id == "worker").unwrap();
    let postcondition = worker.postcondition.as_ref().expect("postcondition set");
    assert_eq!(postcondition.require, "field_present");
    assert_eq!(postcondition.field.as_deref(), Some("json.items"));
}

#[test]
fn postcondition_on_a_non_agent_node_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [node.postcondition]
        require = "non_empty"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(
        err.to_string()
            .contains("only `agent` nodes carry a postcondition"),
        "{err}"
    );
}

#[test]
fn semantic_verify_parses_only_on_agent_nodes() {
    let valid = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "worker"
        kind = "agent"
        name = "Worker"
        agent = "ceo"
        [node.verify]
        criteria = "Name one recommendation and its evidence."
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let file = parse_workflow(valid).expect("semantic verification is valid on an agent");
    assert_eq!(
        file.nodes[1]
            .verify
            .as_ref()
            .and_then(|verify| verify.criteria.as_deref()),
        Some("Name one recommendation and its evidence.")
    );

    let invalid = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [node.verify]
    "#;
    let err = parse_workflow(invalid).unwrap_err();
    assert!(
        err.to_string()
            .contains("only `agent` nodes carry semantic verification"),
        "{err}"
    );
}

#[test]
fn postcondition_with_an_unknown_require_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
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
        require = "smells_right"
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(
        err.to_string()
            .contains("unknown `postcondition.require` `smells_right`"),
        "{err}"
    );
}

#[test]
fn postcondition_field_present_without_a_field_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
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
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("but no `field`"), "{err}");
}

/// Codex #3893851369 on #1937: a bare structured field like `field =
/// "items"` validates today but can NEVER resolve at runtime —
/// `evaluate_postcondition` checks `field` against the `{ text,
/// agent_ref, json }` envelope `run_turn` builds, and a bare `items`
/// root is not one of those three keys, so `resolve_path` always returns
/// `None` regardless of what the agent replies. Refused at author time
/// instead of shipping a gate that can never pass. Before this fix this
/// assertion is RED: `parse_workflow` returns `Ok`, so `.unwrap_err()`
/// panics with "called `Result::unwrap_err()` on an `Ok` value".
#[test]
fn postcondition_field_with_a_bare_structured_root_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
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
        field = "items"
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("never lands at runtime") && message.contains("json.items"),
        "{message}"
    );
}

/// Companion GREEN: the documented `json.` prefix from the same field
/// name parses fine — the rejection above targets the missing prefix,
/// not the field name `items` itself.
#[test]
fn postcondition_field_with_the_json_prefix_still_parses() {
    let src = r#"
        id = "wf"
        name = "WF"
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
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let file = parse_workflow(src).expect("the documented `json.` prefix is accepted");
    let worker = file.nodes.iter().find(|n| n.id == "worker").unwrap();
    assert_eq!(
        worker
            .postcondition
            .as_ref()
            .and_then(|p| p.field.as_deref()),
        Some("json.items")
    );
}

/// Codex #3894162768 on #1937 — direct extension of the bare-root check
/// above: `text` and `agent_ref` are always plain strings in the
/// emitted output, so a dotted descendant like `text.foo` or
/// `agent_ref.id` can never resolve at runtime (`resolve_path` indexes a
/// `Value::String` with `.get("foo")`, which is always `None` — never a
/// panic, never a value). Before this fix this assertion is RED:
/// `parse_workflow` returns `Ok`, so `.unwrap_err()` panics.
#[test]
fn postcondition_field_dotted_into_text_or_agent_ref_is_rejected() {
    for field in ["text.foo", "agent_ref.id"] {
        let src = format!(
            r#"
            id = "wf"
            name = "WF"
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
            field = "{field}"
            [[edge]]
            from = "start"
            to = "worker"
        "#
        );
        let err =
            parse_workflow(&src).expect_err(&format!("field `{field}` dots into a scalar root"));
        assert!(
            err.to_string().contains("always a plain string"),
            "field `{field}`: {err}"
        );
    }
}

/// Companion GREEN: the exact, childless roots `text` and `agent_ref`
/// stay valid on their own — the rejection above targets a dotted
/// DESCENDANT, not the roots themselves (already proven fine by
/// `postcondition_field_that_merely_resembles_a_reserved_key_still_parses`'s
/// bare `"text"` case; pinned again here alongside `agent_ref` for
/// symmetry with the failing test above).
#[test]
fn postcondition_field_of_exactly_text_or_agent_ref_still_parses() {
    for field in ["text", "agent_ref"] {
        let src = format!(
            r#"
            id = "wf"
            name = "WF"
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
            field = "{field}"
            [[edge]]
            from = "start"
            to = "worker"
        "#
        );
        parse_workflow(&src)
            .unwrap_or_else(|err| panic!("field `{field}` on its own is valid: {err}"));
    }
}

/// Codex #3894277296 on #1937 — the fourth structurally-impossible-gate
/// finding: `non_empty_list` only ever accepts a `Value::Array`, but
/// `text`/`agent_ref` are real, exact, childless roots (they pass every
/// check above), and BOTH are unconditionally strings — no reply the
/// agent could ever give makes `resolve_path(output, "text")` or
/// `resolve_path(output, "agent_ref")` come back an array. Before this
/// fix this assertion is RED: `parse_workflow` returns `Ok`, so
/// `.unwrap_err()` panics.
#[test]
fn postcondition_non_empty_list_on_text_or_agent_ref_is_rejected() {
    for field in ["text", "agent_ref"] {
        let src = format!(
            r#"
            id = "wf"
            name = "WF"
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
            require = "non_empty_list"
            field = "{field}"
            [[edge]]
            from = "start"
            to = "worker"
        "#
        );
        let err = parse_workflow(&src).expect_err(&format!(
            "non_empty_list on `{field}` can never see an array"
        ));
        let message = err.to_string();
        assert!(
            message.contains("can never pass") && message.contains(field),
            "field `{field}`: {message}"
        );
    }
}

/// Companion GREEN, both halves of the rule: `non_empty_list` still
/// parses fine against `json` content (the root this predicate CAN be
/// satisfied through), and `field_present` still parses fine against
/// `text`/`agent_ref` (already pinned by
/// `postcondition_field_of_exactly_text_or_agent_ref_still_parses`
/// above — restated here as the other half of the same intersection
/// rule: `field_present` accepts any non-null kind, so it never
/// conflicts with a root's fixed kind, only `non_empty_list` does).
#[test]
fn postcondition_non_empty_list_on_json_content_still_parses() {
    for field in ["json", "json.items"] {
        let src = format!(
            r#"
            id = "wf"
            name = "WF"
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
            require = "non_empty_list"
            field = "{field}"
            [[edge]]
            from = "start"
            to = "worker"
        "#
        );
        parse_workflow(&src)
            .unwrap_or_else(|err| panic!("field `{field}` is `json` content: {err}"));
    }
}

/// Codex #3893619015 on #1937: `text`/`agent_ref` are the two top-level
/// keys the emitted output always carries (`run_turn` inserts the raw
/// reply string / real roster id, then merges the parsed reply's own
/// fields in with `or_insert` — base wins on any collision, so
/// `delivery.rs::report_text` keeps finding prose in the overwhelming
/// majority of nodes whose reply isn't structured at all). A `field`
/// drilling into the parsed reply's OWN `json.text`/`json.agent_ref` key
/// can never be validated consistently with what a downstream binding
/// reads — the gate would check whatever shape the model chose to put
/// under that key, but the emitted value stays the raw string / real
/// roster id regardless. Refused at author time rather than left as a
/// silent runtime divergence a workflow could actually ship with.
#[test]
fn postcondition_field_into_reserved_json_key_is_rejected() {
    for field in ["json.text", "json.agent_ref", "json.text.nested"] {
        let src = format!(
            r#"
            id = "wf"
            name = "WF"
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
            field = "{field}"
            [[edge]]
            from = "start"
            to = "worker"
        "#
        );
        let err = parse_workflow(&src)
            .expect_err(&format!("field `{field}` collides with a reserved key"));
        assert!(
            err.to_string().contains("reserved"),
            "field `{field}`: {err}"
        );
    }
}

/// Companion GREEN: a `field` that does NOT collide with either reserved
/// key — including one that merely starts with `text`/`agent_ref` as a
/// substring, or names the outer envelope's own `text` (not
/// `json.text`) — still parses. The rejection is exactly the two
/// reserved dotted paths, not a blanket ban on the words `text` or
/// `agent_ref` anywhere in a `field`.
#[test]
fn postcondition_field_that_merely_resembles_a_reserved_key_still_parses() {
    for field in [
        "json.items",
        "json.text_summary",
        "json.agent_reference",
        "text",
    ] {
        let src = format!(
            r#"
            id = "wf"
            name = "WF"
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
            field = "{field}"
            [[edge]]
            from = "start"
            to = "worker"
        "#
        );
        parse_workflow(&src).unwrap_or_else(|err| {
            panic!("field `{field}` does not collide with a reserved key: {err}")
        });
    }
}

#[test]
fn config_agent_ref_on_agent_node_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "worker"
        kind = "agent"
        name = "Worker"
        agent = "ceo"
        [node.config]
        agent_ref = "impostor"
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("agent_ref"), "{err}");
}

#[test]
fn route_without_error_edge_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "call"
        kind = "tool_call"
        name = "Call"
        on_error = "route"
        [[edge]]
        from = "start"
        to = "call"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(
        err.to_string().contains("no outgoing edge labeled `error`"),
        "{err}"
    );
}

#[test]
fn error_edge_without_route_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "call"
        kind = "tool_call"
        name = "Call"
        [[node]]
        id = "recover"
        kind = "output"
        name = "Recover"
        [[edge]]
        from = "start"
        to = "call"
        [[edge]]
        from = "call"
        to = "recover"
        label = "error"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("only a routing node"), "{err}");
}

#[test]
fn route_with_matching_error_edge_is_valid() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "call"
        kind = "tool_call"
        name = "Call"
        on_error = "route"
        [node.config]
        slug = "csv_export"
        [[node]]
        id = "recover"
        kind = "output"
        name = "Recover"
        [[edge]]
        from = "start"
        to = "call"
        [[edge]]
        from = "call"
        to = "recover"
        label = "error"
    "#;
    assert!(parse_workflow(src).is_ok());
}
