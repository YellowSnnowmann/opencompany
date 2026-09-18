//! workflow_file: output destinations (issue #170) and trigger schedules (issue #169).

use super::*;

// --- Output destination (issue #170) ------------------------------------

/// A graph with one `output` node carrying `destination` of `kind`, plus an
/// optional `target` line.
fn with_destination(kind: &str, target: Option<&str>) -> String {
    let target_line = target
        .map(|t| format!("target = \"{t}\"\n"))
        .unwrap_or_default();
    format!(
        r#"
id = "wf"
name = "WF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "done"
kind = "output"
name = "Report back"
[node.destination]
kind = "{kind}"
{target_line}
[[edge]]
from = "start"
to = "done"
"#
    )
}

/// Each of the three destination kinds parses onto the first-class field
/// with its target contract intact.
#[test]
fn output_destinations_parse_for_every_kind() {
    let owner = parse_workflow(&with_destination("owner", None)).expect("owner parses");
    let dest = owner.nodes[1].destination.as_ref().expect("present");
    assert_eq!(dest.kind, "owner");
    assert_eq!(dest.target, None);

    let email =
        parse_workflow(&with_destination("email", Some("ada@example.com"))).expect("email parses");
    let dest = email.nodes[1].destination.as_ref().expect("present");
    assert_eq!(dest.kind, "email");
    assert_eq!(dest.target.as_deref(), Some("ada@example.com"));

    let channel =
        parse_workflow(&with_destination("channel", Some("operator"))).expect("channel parses");
    let dest = channel.nodes[1].destination.as_ref().expect("present");
    assert_eq!(dest.kind, "channel");
    assert_eq!(dest.target.as_deref(), Some("operator"));
}

/// The reachability predicate mirrors delivery's per-kind outcome: `owner`
/// always lands (the durable operator channel is its guaranteed fallback,
/// issue #1757), `email` needs a mailbox (issue #1046), `channel` needs a
/// wired, non-operator target, anything else never lands.
#[test]
fn destination_reachability_matches_delivery() {
    let owner = WorkflowDestinationDef {
        kind: "owner".to_string(),
        target: None,
    };
    // Owner lands with a mailbox (emails admins) AND without one (durable
    // operator channel) — issue #1757.
    assert!(destination_is_reachable(&owner, true, &[]));
    assert!(destination_is_reachable(&owner, false, &[]));

    let email = WorkflowDestinationDef {
        kind: "email".to_string(),
        target: Some("ada@example.com".to_string()),
    };
    assert!(destination_is_reachable(&email, true, &[]));
    assert!(!destination_is_reachable(&email, false, &[]));

    let eng = vec!["engineering".to_string()];
    let channel = WorkflowDestinationDef {
        kind: "channel".to_string(),
        target: Some("engineering".to_string()),
    };
    assert!(destination_is_reachable(&channel, false, &eng));
    // An unwired channel never lands, mailbox or not.
    let unwired = WorkflowDestinationDef {
        kind: "channel".to_string(),
        target: Some("marketing".to_string()),
    };
    assert!(!destination_is_reachable(&unwired, true, &eng));
    // `operator` IS reachable now (issue #1757): it is a durable channel the
    // company always wires, so `deliverable_channel_ids` lists it.
    let operator = WorkflowDestinationDef {
        kind: "channel".to_string(),
        target: Some(crate::runtime::channel::OPERATOR_CHANNEL.to_string()),
    };
    assert!(destination_is_reachable(
        &operator,
        true,
        &[crate::runtime::channel::OPERATOR_CHANNEL.to_string()],
    ));
    // But only when it is in the wired set — an empty runtime still can't.
    assert!(!destination_is_reachable(&operator, true, &[]));
}

/// The none-vs-any line the arm gate rides on. A graph with one unreachable
/// output (a channel to an unwired desk) and one reachable channel output is
/// deliverable — a partially-deliverable schedule still arms; only a graph
/// where **nothing** can land is refused.
///
/// Uses two `channel` outputs rather than owner+channel because `owner` now
/// always lands (issue #1757), so it can no longer play the unreachable half.
#[test]
fn mixed_graph_is_deliverable_when_any_output_lands() {
    let src = r#"
id = "wf"
name = "WF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
schedule = "0 9 * * *"
[[node]]
id = "to_unwired"
kind = "output"
name = "Unwired"
[node.destination]
kind = "channel"
target = "marketing"
[[node]]
id = "to_channel"
kind = "output"
name = "Channel"
[node.destination]
kind = "channel"
target = "engineering"
[[edge]]
from = "start"
to = "to_unwired"
[[edge]]
from = "start"
to = "to_channel"
"#;
    let file = parse_workflow(src).expect("parses");
    assert!(file.has_output_destination());
    let eng = vec!["engineering".to_string()];
    // The `marketing` output is dead (unwired), but the `engineering`
    // channel output lands.
    assert!(file.has_deliverable_output(false, &eng));
    // Wire nothing: now neither channel lands, so nothing does.
    assert!(!file.has_deliverable_output(false, &[]));
}

#[test]
fn unknown_destination_kind_is_rejected() {
    let err = parse_workflow(&with_destination("carrier_pigeon", Some("x"))).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("unknown `destination.kind`"), "{message}");
    // The message names what IS supported, not just what isn't.
    assert!(message.contains("owner"), "{message}");
}

/// An `email` destination MUST name an address. This is the validation half
/// of the security boundary: a workflow cannot mail "somebody" — the
/// recipient is pinned in the graph, where a reviewer can see it.
#[test]
fn email_destination_without_an_address_is_rejected() {
    let err = parse_workflow(&with_destination("email", Some("ada"))).unwrap_err();
    assert!(err.to_string().contains("not an email address"), "{err}");
    let err = parse_workflow(&with_destination("email", None)).unwrap_err();
    assert!(err.to_string().contains("not an email address"), "{err}");
}

#[test]
fn channel_destination_without_a_target_is_rejected() {
    let err = parse_workflow(&with_destination("channel", None)).unwrap_err();
    assert!(err.to_string().contains("no `target`"), "{err}");
}

/// `owner` resolves server-side, so a target on it is a mistake worth
/// naming — otherwise an author writes an address there and quietly gets
/// the admins instead.
#[test]
fn owner_destination_with_a_target_is_rejected() {
    let err = parse_workflow(&with_destination("owner", Some("ada@example.com"))).unwrap_err();
    assert!(err.to_string().contains("`owner` destination"), "{err}");
}

/// `repeatable` is a statement about a call, so a node that makes none has
/// no answer to give — and an inert declaration an author believes is a
/// guard is worse than no field at all (issue #850).
#[test]
fn repeatable_on_a_node_that_makes_no_call_is_rejected() {
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
        repeatable = false
        [[edge]]
        from = "start"
        to = "worker"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(
        err.to_string()
            .contains("only `tool_call` and `http_request` nodes make a call"),
        "{err}"
    );
}

/// The two kinds that do make a call accept it.
#[test]
fn repeatable_is_accepted_on_a_tool_call() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "publish"
        kind = "tool_call"
        name = "Publish"
        repeatable = false
        [node.config]
        slug = "shell"
        [[edge]]
        from = "start"
        to = "publish"
    "#;
    let wf = parse_workflow(src).expect("valid");
    let node = wf.nodes.iter().find(|n| n.id == "publish").expect("node");
    assert_eq!(node.repeatable, Some(false));
}

/// `repeatable` inside `config` would ride into the engine graph as an
/// inert key and guard nothing — reject it like the other reserved keys.
#[test]
fn repeatable_inside_config_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "publish"
        kind = "tool_call"
        name = "Publish"
        [node.config]
        slug = "shell"
        repeatable = false
        [[edge]]
        from = "start"
        to = "publish"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(
        err.to_string()
            .contains("puts `repeatable` inside `config`"),
        "{err}"
    );
}

/// Only `output` nodes report back, so a `destination` anywhere else is a
/// silent no-op waiting to happen.
#[test]
fn destination_on_a_non_output_node_is_rejected() {
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
        [node.destination]
        kind = "owner"
    "#;
    let err = parse_workflow(src).unwrap_err();
    assert!(
        err.to_string()
            .contains("only `output` nodes route a report"),
        "{err}"
    );
}

/// `destination` inside `config` would ride into the engine graph as an
/// inert key and deliver nothing — reject it like the other reserved keys.
#[test]
fn destination_inside_config_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [[node]]
        id = "done"
        kind = "output"
        name = "Done"
        [node.config]
        destination = "owner"
        [[edge]]
        from = "start"
        to = "done"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("destination"), "{message}");
    assert!(message.contains("inside `config`"), "{message}");
}

/// A destination-bearing graph renders back to TOML and re-parses to the
/// same model — the create route's persist path depends on this.
#[test]
fn destination_round_trips_through_render_and_parse() {
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
                id: "done".to_string(),
                kind: "output".to_string(),
                name: "Report".to_string(),
                summary: None,
                agent: None,
                schedule: None,
                config: None,
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: None,
                destination: Some(WorkflowDestinationDef {
                    kind: "email".to_string(),
                    target: Some("ada@example.com".to_string()),
                }),
                postcondition: None,
                verify: None,
            },
        ],
        edges: vec![RawEdge {
            from: "start".to_string(),
            to: "done".to_string(),
            label: None,
        }],
    };
    let toml_src = render_workflow(&raw).expect("renders");
    let file = parse_workflow(&toml_src).expect("re-parses");
    let dest = file.nodes[1].destination.as_ref().expect("present");
    assert_eq!(dest.kind, "email");
    assert_eq!(dest.target.as_deref(), Some("ada@example.com"));
}

/// A legacy graph (no `destination` anywhere) renders byte-identically to
/// what it rendered before the field existed — `skip_serializing_if` is what
/// keeps an unchanged file from churning on every re-save.
#[test]
fn a_graph_without_a_destination_renders_no_destination_key() {
    let raw = RawWorkflow {
        id: "wf".to_string(),
        name: "WF".to_string(),
        description: None,
        owner_desk: None,
        nodes: vec![RawNode {
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
        }],
        edges: Vec::new(),
    };
    let toml_src = render_workflow(&raw).expect("renders");
    assert!(!toml_src.contains("destination"), "{toml_src}");
}

// --- trigger schedule (issue #169) --------------------------------------

/// A trigger's `schedule` survives the render → parse round trip the create
/// endpoint runs, and lands on the parsed node.
#[test]
fn trigger_schedule_round_trips_render_and_parse() {
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
                schedule: Some("0 * * * *".to_string()),
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
                id: "done".to_string(),
                kind: "output".to_string(),
                name: "Done".to_string(),
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
            to: "done".to_string(),
            label: None,
        }],
    };
    let toml_src = render_workflow(&raw).expect("renders");
    let file = parse_workflow(&toml_src).expect("re-parses");
    let start = file.nodes.iter().find(|n| n.id == "start").unwrap();
    assert_eq!(start.schedule.as_deref(), Some("0 * * * *"));
    let done = file.nodes.iter().find(|n| n.id == "done").unwrap();
    assert!(done.schedule.is_none());
}

/// A trigger schedule parses from hand-authored TOML too, including the
/// named-weekday dialect the manifest `[[schedule]]` crons already accept.
#[test]
fn trigger_schedule_parses_from_toml() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        schedule = "0 9 * * MON"
    "#;
    let file = parse_workflow(src).expect("parses");
    assert_eq!(file.nodes[0].schedule.as_deref(), Some("0 9 * * MON"));
}

/// `schedule` says when the *workflow* starts, so it is trigger-only.
#[test]
fn schedule_on_a_non_trigger_node_is_rejected() {
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
        schedule = "0 * * * *"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("only `trigger` nodes carry a schedule"),
        "{message}"
    );
}

/// A malformed cron is rejected at validation with the parser's own
/// message, so it can never be persisted as an expression that never fires.
#[test]
fn invalid_trigger_schedule_cron_is_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        schedule = "every hour"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("not a valid cron"), "{message}");
    assert!(message.contains("needs 5 fields"), "{message}");
    assert!(message.contains("UTC"), "{message}");

    // An out-of-range field is caught by the same parser.
    let out_of_range = src.replace("every hour", "0 99 * * *");
    let err = parse_workflow(&out_of_range).unwrap_err();
    assert!(err.to_string().contains("not a valid cron"), "{err}");
}

/// Two scheduled triggers would double-run the workflow, and honoring only
/// the first would silently drop a schedule the operator saved — so the
/// graph is rejected, naming both offenders.
#[test]
fn two_scheduled_triggers_are_rejected() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "nightly"
        kind = "trigger"
        name = "Nightly"
        schedule = "0 2 * * *"
        [[node]]
        id = "hourly"
        kind = "trigger"
        name = "Hourly"
        schedule = "0 * * * *"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("at most one scheduled trigger"),
        "{message}"
    );
    assert!(message.contains("`nightly`"), "{message}");
    assert!(message.contains("`hourly`"), "{message}");
}

/// The at-most-one rule counts *schedules*, not triggers: a graph may still
/// have several triggers, and one of them may be scheduled.
#[test]
fn multiple_triggers_are_still_allowed_when_at_most_one_is_scheduled() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "manual"
        kind = "trigger"
        name = "Manual"
        [[node]]
        id = "webhook"
        kind = "trigger"
        name = "Webhook"
        [[node]]
        id = "nightly"
        kind = "trigger"
        name = "Nightly"
        schedule = "0 2 * * *"
    "#;
    let file = parse_workflow(src).expect("several triggers stay legal");
    assert_eq!(file.nodes.len(), 3);
    let scheduled: Vec<&str> = file
        .nodes
        .iter()
        .filter(|n| n.schedule.is_some())
        .map(|n| n.id.as_str())
        .collect();
    assert_eq!(scheduled, vec!["nightly"]);

    // And with no schedules at all, unchanged from before this rule.
    let bare = src.replace("schedule = \"0 2 * * *\"", "");
    assert!(parse_workflow(&bare).is_ok());
}

/// Two *malformed* schedules report the bad crons AND the at-most-one
/// problem together, matching the module's report-everything-at-once
/// contract.
#[test]
fn two_bad_schedules_report_every_problem_at_once() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "a"
        kind = "trigger"
        name = "A"
        schedule = "nightly"
        [[node]]
        id = "b"
        kind = "trigger"
        name = "B"
        schedule = "hourly"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("not a valid cron"), "{message}");
    assert!(
        message.contains("at most one scheduled trigger"),
        "{message}"
    );
}

/// `config.schedule` would be silently ignored (the first-class field wins),
/// so it is a reserved key like the other first-class node fields.
#[test]
fn config_schedule_is_rejected_as_a_reserved_key() {
    let src = r#"
        id = "wf"
        name = "WF"
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        [node.config]
        schedule = "0 * * * *"
    "#;
    let err = parse_workflow(src).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("`schedule` inside `config`"), "{message}");
}

/// A graph authored before `schedule` existed parses with the field unset
/// and re-renders byte-identically — the field is skipped when `None`, so
/// adding it rewrites nothing on disk.
#[test]
fn legacy_graph_without_schedule_re_renders_byte_identically() {
    let raw = RawWorkflow {
        id: "wf".to_string(),
        name: "WF".to_string(),
        description: Some("Legacy.".to_string()),
        owner_desk: None,
        nodes: vec![RawNode {
            id: "start".to_string(),
            kind: "trigger".to_string(),
            name: "Start".to_string(),
            summary: Some("Kicks off.".to_string()),
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
        edges: Vec::new(),
    };
    let first = render_workflow(&raw).expect("renders");
    assert!(
        !first.contains("schedule"),
        "an unset schedule must not be written: {first}"
    );

    let file = parse_workflow(&first).expect("parses");
    assert!(file.nodes[0].schedule.is_none());

    // Re-render the parsed graph through the same shape: byte-identical.
    let round_tripped = RawWorkflow {
        id: file.id.clone(),
        name: file.name.clone(),
        description: file.description.clone(),
        owner_desk: file.owner_desk.clone(),
        nodes: file
            .nodes
            .iter()
            .map(|n| RawNode {
                id: n.id.clone(),
                kind: n.kind.as_str().to_string(),
                name: n.name.clone(),
                summary: n.summary.clone(),
                agent: n.agent.clone(),
                schedule: n.schedule.clone(),
                config: None,
                on_error: n.on_error.clone(),
                retry: n.retry.clone(),
                requires_approval: n.requires_approval,
                repeatable: None,
                destination: n.destination.clone(),
                postcondition: None,
                verify: None,
            })
            .collect(),
        edges: Vec::new(),
    };
    assert_eq!(render_workflow(&round_tripped).expect("re-renders"), first);
}
