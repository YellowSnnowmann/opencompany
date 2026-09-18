//! workflow_file: the P2 node kinds, per-kind required config (issue #661), and inescapable-cycle/reachability checks (G15, issue #540).

use super::*;

// --- P2: the six new node kinds ----------------------------------------

/// Each new node kind parses to its enum variant, and `WORKFLOW_NODE_KINDS`
/// advertises all twelve.
#[test]
fn new_node_kinds_parse() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "sw"
        kind = "switch"
        name = "Switch"
        [node.config]
        field = "=item.kind"
        [[node]]
        id = "mg"
        kind = "merge"
        name = "Merge"
        [[node]]
        id = "so"
        kind = "split_out"
        name = "Split"
        [[node]]
        id = "tf"
        kind = "transform"
        name = "Transform"
        [[node]]
        id = "op"
        kind = "output_parser"
        name = "Parse"
        [[edge]]
        from = "start"
        to = "sw"
        [[edge]]
        from = "sw"
        to = "mg"
        [[edge]]
        from = "mg"
        to = "so"
        [[edge]]
        from = "so"
        to = "tf"
        [[edge]]
        from = "tf"
        to = "op"
    "#;
    let file = parse_workflow(src).expect("new kinds parse");
    let kind = |id: &str| file.nodes.iter().find(|n| n.id == id).unwrap().kind;
    assert_eq!(kind("sw"), WorkflowNodeKind::Switch);
    assert_eq!(kind("mg"), WorkflowNodeKind::Merge);
    assert_eq!(kind("so"), WorkflowNodeKind::SplitOut);
    assert_eq!(kind("tf"), WorkflowNodeKind::Transform);
    assert_eq!(kind("op"), WorkflowNodeKind::OutputParser);
    assert_eq!(WORKFLOW_NODE_KINDS.len(), 12);
    assert!(WORKFLOW_NODE_KINDS.contains(&"sub_workflow"));
}

/// A `sub_workflow` node with a non-empty `workflow_id` string is valid.
#[test]
fn sub_workflow_with_workflow_id_is_valid() {
    let src = r#"
        id = "parent"
        name = "Parent"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "child"
        kind = "sub_workflow"
        name = "Child"
        [node.config]
        workflow_id = "greet"
        [[edge]]
        from = "start"
        to = "child"
    "#;
    let file = parse_workflow(src).expect("sub_workflow parses");
    let child = file.nodes.iter().find(|n| n.id == "child").unwrap();
    assert_eq!(child.kind, WorkflowNodeKind::SubWorkflow);
    assert_eq!(child.config.as_ref().unwrap()["workflow_id"], "greet");
}

/// A `sub_workflow` node with no `config` is rejected — it names nothing to run.
#[test]
fn sub_workflow_without_config_is_rejected() {
    let src = r#"
        id = "parent"
        name = "Parent"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "child"
        kind = "sub_workflow"
        name = "Child"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("workflow_id"), "{err}");
}

/// A `sub_workflow` node with an empty `workflow_id` is rejected.
#[test]
fn sub_workflow_with_empty_workflow_id_is_rejected() {
    let src = r#"
        id = "parent"
        name = "Parent"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "child"
        kind = "sub_workflow"
        name = "Child"
        [node.config]
        workflow_id = ""
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("empty `workflow_id`"), "{err}");
}

/// A `sub_workflow` node naming its own workflow id is a static self-reference.
#[test]
fn sub_workflow_self_reference_is_rejected() {
    let src = r#"
        id = "loopy"
        name = "Loopy"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "child"
        kind = "sub_workflow"
        name = "Child"
        [node.config]
        workflow_id = "loopy"
        [[edge]]
        from = "start"
        to = "child"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("its own workflow id"), "{err}");
}

/// An inline `workflow` child graph is reserved — a sub_workflow must
/// reference a saved workflow by id so the child passes OpenCompany validation.
#[test]
fn sub_workflow_inline_child_graph_is_rejected() {
    let src = r#"
        id = "parent"
        name = "Parent"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "child"
        kind = "sub_workflow"
        name = "Child"
        [node.config]
        workflow_id = "greet"
        [node.config.workflow]
        id = "inlined"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("inline `workflow`"), "{err}");
}

/// On a `switch`, an `error`-labeled edge is a legitimate case name — it must
/// NOT trip the `error`-label ⇔ `on_error = "route"` coupling check.
#[test]
fn switch_error_label_is_a_case_not_a_route() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "sw"
        kind = "switch"
        name = "Switch"
        [node.config]
        field = "=item.kind"
        [[node]]
        id = "err_case"
        kind = "output"
        name = "Error case"
        [[node]]
        id = "ok_case"
        kind = "output"
        name = "OK case"
        [[edge]]
        from = "start"
        to = "sw"
        [[edge]]
        from = "sw"
        to = "err_case"
        label = "error"
        [[edge]]
        from = "sw"
        to = "ok_case"
        label = "ok"
    "#;
    assert!(
        parse_workflow(src).is_ok(),
        "an error-labeled switch case must be valid without on_error = route"
    );
}

// --- Per-kind required config (issue #661) ------------------------------

/// A `condition` node with no `config.field` still LOADS on the lenient
/// read path (issue #682: pre-#661 saved graphs must keep loading), but the
/// STRICT author-time pass reports it — without a field the engine tests the
/// whole item and the branch is silently meaningless. This is the regression
/// guard: a field-less-condition graph parses via `parse_workflow` yet is
/// rejected by `validate(_, true)`.
#[test]
fn condition_without_field_loads_leniently_but_strict_rejects() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "gate"
        kind = "condition"
        name = "Gate"
        [[node]]
        id = "yes_out"
        kind = "output"
        name = "Yes"
        [[node]]
        id = "no_out"
        kind = "output"
        name = "No"
        [[edge]]
        from = "start"
        to = "gate"
        [[edge]]
        from = "gate"
        to = "yes_out"
        label = "yes"
        [[edge]]
        from = "gate"
        to = "no_out"
        label = "no"
    "#;
    // Lenient load path accepts it — a graph persisted before #661 still loads.
    assert!(parse_workflow(src).is_ok());
    // Strict author-time pass reports the missing field.
    let raw: RawWorkflow = toml::from_str(src).expect("the fixture is valid TOML");
    let problems = validate(&raw, true).join("\n");
    assert!(problems.contains("config.field"), "{problems}");
}

/// A `condition` branch labeled anything but `yes`/`no` loads leniently
/// (issue #682) but is rejected by the STRICT author-time pass — an
/// off-vocabulary label silently maps onto the `true` port.
#[test]
fn condition_branch_with_non_yes_no_label_loads_leniently_but_strict_rejects() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "gate"
        kind = "condition"
        name = "Gate"
        [node.config]
        field = "=item.ok"
        [[node]]
        id = "a"
        kind = "output"
        name = "A"
        [[node]]
        id = "b"
        kind = "output"
        name = "B"
        [[edge]]
        from = "start"
        to = "gate"
        [[edge]]
        from = "gate"
        to = "a"
        label = "pass"
        [[edge]]
        from = "gate"
        to = "b"
        label = "no"
    "#;
    // Lenient load path accepts the off-vocabulary label.
    assert!(parse_workflow(src).is_ok());
    // Strict author-time pass reports it.
    let raw: RawWorkflow = toml::from_str(src).expect("the fixture is valid TOML");
    let problems = validate(&raw, true).join("\n");
    assert!(problems.contains("labeled `yes` or `no`"), "{problems}");
}

/// A well-formed `condition` — a `field` plus `yes`/`no` branches — parses.
#[test]
fn condition_with_field_and_yes_no_labels_is_valid() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "gate"
        kind = "condition"
        name = "Gate"
        [node.config]
        field = "=item.approved"
        [[node]]
        id = "a"
        kind = "output"
        name = "A"
        [[node]]
        id = "b"
        kind = "output"
        name = "B"
        [[edge]]
        from = "start"
        to = "gate"
        [[edge]]
        from = "gate"
        to = "a"
        label = "yes"
        [[edge]]
        from = "gate"
        to = "b"
        label = "no"
    "#;
    assert!(parse_workflow(src).is_ok());
}

/// An `http_request` node missing `config.method` / `config.url` loads
/// leniently (issue #682) but the STRICT pass reports BOTH missing keys.
#[test]
fn http_request_without_method_or_url_loads_leniently_but_strict_rejects() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "fetch"
        kind = "http_request"
        name = "Fetch"
        [[edge]]
        from = "start"
        to = "fetch"
    "#;
    // Lenient load path accepts it.
    assert!(parse_workflow(src).is_ok());
    // Strict author-time pass reports both missing config keys at once.
    let raw: RawWorkflow = toml::from_str(src).expect("the fixture is valid TOML");
    let message = validate(&raw, true).join("\n");
    assert!(message.contains("config.method"), "{message}");
    assert!(message.contains("config.url"), "{message}");
}

/// A `switch` node with neither `field` nor `expression` loads leniently
/// (issue #682) but the STRICT pass reports the missing discriminant.
#[test]
fn switch_without_discriminant_loads_leniently_but_strict_rejects() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "sw"
        kind = "switch"
        name = "Switch"
        [[node]]
        id = "case_a"
        kind = "output"
        name = "A"
        [[edge]]
        from = "start"
        to = "sw"
        [[edge]]
        from = "sw"
        to = "case_a"
        label = "a"
    "#;
    // Lenient load path accepts it.
    assert!(parse_workflow(src).is_ok());
    // Strict author-time pass reports the missing discriminant.
    let raw: RawWorkflow = toml::from_str(src).expect("the fixture is valid TOML");
    let problems = validate(&raw, true).join("\n");
    assert!(problems.contains("discriminant"), "{problems}");
}

/// Strict author-time parity (issue #661/#682): a `tool_call` with no `slug`
/// loads leniently now, but the STRICT pass reports it — the same shape the
/// console-draft path rejects — so `translate` never has to fall back to the
/// node id as a placeholder slug once a graph reaches an author surface.
#[test]
fn tool_call_without_slug_loads_leniently_but_strict_rejects() {
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
        [[edge]]
        from = "start"
        to = "call"
    "#;
    // Lenient load path accepts it.
    assert!(parse_workflow(src).is_ok());
    // Strict author-time pass reports the missing slug.
    let raw: RawWorkflow = toml::from_str(src).expect("the fixture is valid TOML");
    let problems = validate(&raw, true).join("\n");
    assert!(problems.contains("config.slug"), "{problems}");
}

// --- G15: inescapable cycles + reachability (issue #540) ----------------

/// A bare two-node cycle with no branch to leave it is a trap: once the run
/// reaches `a` it loops `a → b → a` forever. Rejected, naming both nodes.
/// (Negative control for the cycle check: delete the SCC-exit check and this
/// graph — which has no condition/switch at all — would wrongly pass.)
#[test]
fn multi_node_cycle_with_no_exit_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "a"
        kind = "agent"
        name = "A"
        agent = "ceo"
        [[node]]
        id = "b"
        kind = "agent"
        name = "B"
        agent = "ceo"
        [[edge]]
        from = "start"
        to = "a"
        [[edge]]
        from = "a"
        to = "b"
        [[edge]]
        from = "b"
        to = "a"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("form a loop"), "{message}");
    assert!(message.contains("`a`"), "{message}");
    assert!(message.contains("`b`"), "{message}");
}

/// A miniature of the shipped `game_build_pipeline` shape: a `condition`
/// guards the loop, with a `yes` branch that leaves it and a `no` branch
/// that loops back. That is a legal bounded retry — it must parse clean.
#[test]
fn condition_guarded_loop_is_valid() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "work"
        kind = "agent"
        name = "Work"
        agent = "ceo"
        [[node]]
        id = "gate"
        kind = "condition"
        name = "Good enough?"
        [node.config]
        field = "=item.good_enough"
        [[node]]
        id = "done"
        kind = "output"
        name = "Ship"
        [[edge]]
        from = "start"
        to = "work"
        [[edge]]
        from = "work"
        to = "gate"
        [[edge]]
        from = "gate"
        to = "done"
        label = "yes"
        [[edge]]
        from = "gate"
        to = "work"
        label = "no"
    "#;
    assert!(
        parse_workflow(src).is_ok(),
        "a condition-guarded retry loop must stay valid"
    );
}

/// The shipped guarded-retry preset itself must stay valid — its loop
/// (`gameplay → assets → balance → qa → gate → gameplay`) is escapable
/// because `gate` is a `condition` whose `yes` branch leaves the loop.
#[test]
fn the_shipped_guarded_loop_preset_is_valid() {
    const GAME: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../companies/game_studio/workflows/game_build_pipeline.toml"
    ));
    parse_workflow(GAME).expect("the game-studio guarded loop is valid");
}

/// A cycle that DOES contain a `condition`, but whose only edges all stay
/// inside the loop, is still inescapable — a branch that never leaves the SCC
/// buys nothing. (Negative control: the exit test, not merely "contains a
/// condition", is what this asserts.)
#[test]
fn inescapable_cycle_containing_condition_with_no_exit_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "a"
        kind = "agent"
        name = "A"
        agent = "ceo"
        [[node]]
        id = "gate"
        kind = "condition"
        name = "Gate"
        [[edge]]
        from = "start"
        to = "a"
        [[edge]]
        from = "a"
        to = "gate"
        [[edge]]
        from = "gate"
        to = "a"
        label = "again"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(err.to_string().contains("form a loop"), "{err}");
}

/// A node no edge ever reaches from the trigger would never execute. It is
/// rejected, naming the orphan. (Negative control for reachability.)
#[test]
fn unreachable_node_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "reached"
        kind = "output"
        name = "Reached"
        [[node]]
        id = "orphan"
        kind = "output"
        name = "Orphan"
        [[edge]]
        from = "start"
        to = "reached"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("cannot be reached"), "{message}");
    assert!(message.contains("`orphan`"), "{message}");
    // The reached node must NOT be named — only the genuine orphan.
    assert!(!message.contains("`reached`"), "{message}");
}

/// With no trigger at all, the reachability check stays silent: the
/// "needs at least one trigger" problem is the real one, and flagging every
/// node as unreachable on top of it would just be noise.
#[test]
fn no_trigger_skips_reachability_noise() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "only"
        kind = "output"
        name = "Only"
        [[node]]
        id = "other"
        kind = "output"
        name = "Other"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("trigger"), "{message}");
    assert!(
        !message.contains("cannot be reached"),
        "reachability must be skipped with no trigger: {message}"
    );
}

/// A trigger with an EMPTY id already fails id-validation, and it seeds no
/// entry into the reachability BFS. The gate keys off `trigger_ids` (usable
/// entries), not the raw trigger count, so the run's one valid node is NOT
/// piled with a bogus "cannot be reached" on top of the real "missing an
/// `id`" problem. (Regression, #540.)
#[test]
fn empty_id_trigger_skips_reachability_noise() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = ""
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "work"
        kind = "output"
        name = "Work"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("missing an `id`"), "{message}");
    assert!(
        !message.contains("cannot be reached"),
        "an id-less trigger must not spawn reachability noise: {message}"
    );
}

#[test]
fn legacy_files_without_new_fields_parse_unchanged() {
    // A graph authored before the P1 fields existed: every new field is None.
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
    "#;
    let file = parse_workflow(src).expect("legacy parses");
    let start = &file.nodes[0];
    assert!(start.config.is_none());
    assert!(start.on_error.is_none());
    assert!(start.retry.is_none());
    assert!(start.requires_approval.is_none());
    assert!(start.destination.is_none());
}
