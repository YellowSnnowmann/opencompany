use super::types_test_support::*;
use super::*;
use crate::ports::workflow_runner::DeliveryStatus;

/// ...and it must not be reachable by its **display name** either.
///
/// The guard narrows the key being asked for, so `{id: "main", name: "Front
/// office"}` slipped through it: `Front office` is not a General spelling,
/// the name match fired, and the resolver returned `main` — an id that
/// `GET .../desks` filters out and that every desk mutation refuses. Its
/// lead would answer, and the reply would be journaled under a thread the
/// console renders no channel for: a conversation with no way back.
///
/// An overlay desk on a General id is unaddressable by design; it must be
/// unaddressable by *every* address.
#[test]
fn an_overlay_desk_on_a_general_id_is_unreachable_by_name_too() {
    let mut record = desk_record(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n\
         [[agent]]\nid = \"eng\"\nrole = \"Engineer\"\n",
        Vec::new(),
    );
    record.overlay_desks.push(OverlayDesk {
        id: "main".into(),
        name: "Front office".into(),
        description: None,
        responder: Default::default(),
        members: vec!["eng".into()],
        hive: Default::default(),
    });
    assert_eq!(
        record.resolve_desk_id("Front office"),
        None,
        "an overlay desk whose id shadows General must not answer to its name"
    );
    assert_eq!(
        record.resolve_desk_id("front office"),
        None,
        "nor case-folded"
    );
    assert_eq!(record.resolve_desk_id("main"), None, "nor to the id itself");
    // An ordinary overlay desk is untouched — this narrows one desk, not the rule.
    let mut ordinary = desk_record(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"eng\"\nrole = \"Engineer\"\n",
        Vec::new(),
    );
    ordinary.overlay_desks.push(OverlayDesk {
        id: "ops".into(),
        name: "Front office".into(),
        description: None,
        responder: Default::default(),
        members: vec!["eng".into()],
        hive: Default::default(),
    });
    assert_eq!(
        ordinary.resolve_desk_id("Front office").as_deref(),
        Some("ops")
    );
    assert_eq!(ordinary.resolve_desk_id("ops").as_deref(), Some("ops"));
}

/// A desk the **manifest** declares under a General spelling is the
/// blueprint's own General desk, and this host has always honoured it
/// (issue #1743). The narrowing above is about overlay desks only; the
/// manifest arm of the resolver is searched first and is untouched.
#[test]
fn a_blueprint_desk_still_owns_a_general_spelling() {
    let record = desk_record(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n\
         [[agent]]\nid = \"eng\"\nrole = \"Engineer\"\n\
         [[group_chat]]\nid = \"main\"\nname = \"Front office\"\nmembers = [\"eng\"]\n",
        Vec::new(),
    );
    assert_eq!(record.resolve_desk_id("main").as_deref(), Some("main"));
    assert_eq!(
        record.effective_desk_members("main"),
        vec!["eng".to_string()]
    );
    // And by display name, the other spelling `resolve_desk_id` matches.
    let named = desk_record(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n\
         [[group_chat]]\nid = \"ops\"\nname = \"General\"\nmembers = [\"ceo\"]\n",
        Vec::new(),
    );
    assert_eq!(named.resolve_desk_id("General").as_deref(), Some("ops"));
    assert_eq!(named.resolve_desk_id("general").as_deref(), Some("ops"));
}

/// The overlay blob round-trips operator-created desks through its persisted
/// JSON form, so a created desk survives a store save/load cycle.
#[test]
fn overlay_blob_round_trips_desks() {
    let with_desks = r#"{"agents":[],"desk_members":[],"desks":[{"id":"growth","name":"Growth","members":["eng"]}]}"#;
    let blob = OverlayBlob::parse(with_desks).expect("object with desks");
    assert_eq!(blob.desks.len(), 1);
    assert_eq!(blob.desks[0].id, "growth");
    assert_eq!(blob.desks[0].members, vec!["eng".to_string()]);
    // Re-serialize and re-parse — the desk survives the round trip.
    let json = serde_json::to_string(&blob).expect("serialize");
    let again = OverlayBlob::parse(&json).expect("reparse");
    assert_eq!(again.desks, blob.desks);
}

// ── Issue #228: a workflow run's outcome is journaled ───────────────────

fn delivery(node: &str, status: DeliveryStatus) -> DeliveryReport {
    DeliveryReport {
        node: node.to_string(),
        kind: "owner".to_string(),
        target: Some("ada@example.com".to_string()),
        status,
        detail: "emailed the company's admin".to_string(),
        reason: crate::ports::DeliveryReason::OwnerEmailed,
    }
}

/// The full-bodied variant survives the JSONL round trip the journal puts
/// every event through — including the delivery rows, which are the whole
/// reason the event exists.
#[test]
fn workflow_run_finished_round_trips_with_every_field() {
    let event = CompanyEvent::WorkflowRunFinished {
        workflow_id: "digest".to_string(),
        scheduled: true,
        run_id: Some("run-1".to_string()),
        deliveries: vec![
            delivery("owner_summary", DeliveryStatus::Skipped),
            delivery("also_sent", DeliveryStatus::Sent),
        ],
        pending_approvals: vec!["review".to_string()],
        error: None,
        cancelled: false,
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    assert_eq!(round_trip(&event), event);
}

/// The failed-run shape round-trips too. This is the arm that today only
/// warns to host stdout, so it is the one an operator most needs read back.
#[test]
fn workflow_run_finished_round_trips_a_failed_run() {
    let event = CompanyEvent::WorkflowRunFinished {
        workflow_id: "digest".to_string(),
        scheduled: true,
        run_id: None,
        deliveries: Vec::new(),
        pending_approvals: Vec::new(),
        error: Some("agent node `worker` had no inference source".to_string()),
        cancelled: false,
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    assert_eq!(round_trip(&event), event);
}

/// The additive contract, both halves.
///
/// **Forward:** a minimal line — only the two required fields, exactly what
/// a future/older writer might emit — still loads, so no persisted journal
/// needs migrating.
///
/// **Backward:** an empty run serializes to *only* those two fields. Every
/// optional/collection field is `skip_serializing_if`, which is what keeps
/// the wire form of an outcome-less run minimal rather than littered with
/// nulls and `[]`s.
#[test]
fn workflow_run_finished_omits_and_defaults_its_optional_fields() {
    let json = r#"{"kind":"WorkflowRunFinished","workflow_id":"digest","scheduled":false}"#;
    let event: CompanyEvent = serde_json::from_str(json).expect("minimal line loads");
    assert_eq!(
        event,
        CompanyEvent::WorkflowRunFinished {
            workflow_id: "digest".to_string(),
            scheduled: false,
            run_id: None,
            deliveries: Vec::new(),
            pending_approvals: Vec::new(),
            error: None,
            cancelled: false,
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        }
    );
    // …and serializing it back emits nothing extra.
    let out = serde_json::to_string(&event).expect("serialize");
    assert!(!out.contains("run_id"), "{out}");
    assert!(!out.contains("deliveries"), "{out}");
    assert!(!out.contains("pending_approvals"), "{out}");
    assert!(!out.contains("error"), "{out}");
    // Issue #383's field joins the same contract, which is what makes it
    // replay-safe: absent decodes as `false`, and a non-cancelled run's line
    // is byte-identical to what it was before the field existed.
    assert!(!out.contains("cancelled"), "{out}");
}

/// Issue #661 (M5): a run's board rows round-trip, and a line written before
/// they existed still replays.
///
/// Three claims, and the last two are what make this additive rather than a
/// migration: the rows survive the round trip in camelCase; a run that touched
/// no card serializes with **no `board` key at all**, so every already-written
/// journal line stays byte-identical; and a pre-#661 line decodes as empty
/// rather than failing to decode.
#[test]
fn workflow_run_finished_round_trips_board_rows() {
    use crate::ports::workflow_runner::WorkflowBoardAction;

    let event = CompanyEvent::WorkflowRunFinished {
        workflow_id: "digest".to_string(),
        scheduled: true,
        run_id: Some("run-1".to_string()),
        deliveries: Vec::new(),
        pending_approvals: Vec::new(),
        error: None,
        cancelled: false,
        notices: Vec::new(),
        board: vec![WorkflowRunBoardRow {
            action: WorkflowBoardAction::Spawned,
            task_id: Some("card-1".to_string()),
            title: Some("Reply to the auditor".to_string()),
            assignee: None,
        }],
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    assert_eq!(round_trip(&event), event);
    let out = serde_json::to_string(&event).expect("serialize");
    assert!(out.contains("\"action\":\"spawned\""), "{out}");
    assert!(out.contains("\"taskId\":\"card-1\""), "{out}");
    // Absent rather than null on the arm that has nothing to say.
    assert!(!out.contains("assignee"), "{out}");

    // A run that touched no card is byte-unchanged from pre-#661.
    let untouched = CompanyEvent::WorkflowRunFinished {
        workflow_id: "digest".to_string(),
        scheduled: true,
        run_id: Some("run-1".to_string()),
        deliveries: Vec::new(),
        pending_approvals: Vec::new(),
        error: None,
        cancelled: false,
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    let out = serde_json::to_string(&untouched).expect("serialize");
    assert!(!out.contains("board"), "{out}");

    // And a line written before the field existed replays as empty.
    let legacy = serde_json::json!({
        "kind": "WorkflowRunFinished",
        "workflow_id": "digest",
        "scheduled": true,
        "run_id": "run-1"
    });
    let loaded: CompanyEvent =
        serde_json::from_value(legacy).expect("a pre-#661 journal line replays");
    let CompanyEvent::WorkflowRunFinished { board, .. } = loaded else {
        panic!("expected a WorkflowRunFinished");
    };
    assert!(board.is_empty());
}

/// Issue #383: a cancelled run round-trips, and is distinguishable from a
/// failed one by more than the absence of an error.
///
/// The pairing is the assertion. A cancelled run carries `cancelled: true`
/// **and** `error: None` — so a reader that only ever looked at `error`
/// (every reader before #383) sees a clean finish, which is exactly why the
/// console needed a new field rather than a new error string.
#[test]
fn workflow_run_finished_round_trips_a_cancelled_run() {
    let event = CompanyEvent::WorkflowRunFinished {
        workflow_id: "digest".to_string(),
        scheduled: false,
        run_id: Some("run-1".to_string()),
        deliveries: Vec::new(),
        pending_approvals: Vec::new(),
        error: None,
        cancelled: true,
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    assert_eq!(round_trip(&event), event);

    let out = serde_json::to_string(&event).expect("serialize");
    assert_eq!(
        out,
        r#"{"kind":"WorkflowRunFinished","workflow_id":"digest","scheduled":false,"run_id":"run-1","cancelled":true}"#,
        "the cancelled line pins its exact wire shape"
    );
}

/// A pre-#383 line — the overwhelming majority of every journal on disk —
/// loads as not cancelled rather than failing to decode.
#[test]
fn a_pre_383_finished_line_loads_as_not_cancelled() {
    let line = r#"{"kind":"WorkflowRunFinished","workflow_id":"digest","scheduled":true,"run_id":"run-9","error":"it broke"}"#;
    let event: CompanyEvent = serde_json::from_str(line).expect("pre-#383 line loads");
    let CompanyEvent::WorkflowRunFinished {
        cancelled, error, ..
    } = &event
    else {
        panic!("expected a WorkflowRunFinished");
    };
    assert!(!cancelled, "an old failed run must not read as cancelled");
    assert_eq!(error.as_deref(), Some("it broke"));
    // And re-serializing it stays byte-identical — the field is absent
    // going out as well as coming in.
    assert_eq!(
        serde_json::to_string(&event).expect("serialize"),
        line,
        "re-writing an old line must not add the new field"
    );
}

/// Issue #371's opening bracket round-trips through the JSONL the journal
/// puts every event through.
#[test]
fn workflow_run_started_round_trips() {
    let event = CompanyEvent::WorkflowRunStarted {
        workflow_id: "digest".to_string(),
        run_id: "run-1".to_string(),
        scheduled: true,
        started_by: Some(StartedBy::Operator),
        resume_semantic: None,
    };
    assert_eq!(round_trip(&event), event);
}

/// Every [`StartedBy`] arm round-trips, including the fielded `Agent` one —
/// the shape a parked blocker's sender resolution reads back.
#[test]
fn started_by_round_trips_all_arms() {
    for started_by in [
        StartedBy::Operator,
        StartedBy::Agent("ceo".to_string()),
        StartedBy::Schedule,
    ] {
        let event = CompanyEvent::WorkflowRunStarted {
            workflow_id: "digest".to_string(),
            run_id: "run-1".to_string(),
            scheduled: matches!(started_by, StartedBy::Schedule),
            started_by: Some(started_by.clone()),
            resume_semantic: None,
        };
        assert_eq!(
            round_trip(&event),
            event,
            "{started_by:?} did not round-trip"
        );
    }
}

/// A `WorkflowRunStarted` line written before this field existed (issue
/// #1862 prerequisite) still replays, with `started_by` reading back
/// `None` rather than failing to parse. Pinned against a hand-written
/// legacy payload rather than a round-trip, for the same reason
/// `a_pre_881_run_finished_line_still_replays` is: a round-trip can only
/// ever prove the new shape agrees with itself.
#[test]
fn a_pre_1862_run_started_line_still_replays_with_no_sender() {
    let legacy = serde_json::json!({
        "kind": "WorkflowRunStarted",
        "workflow_id": "digest",
        "run_id": "run-1",
        "scheduled": false
    });
    let event: CompanyEvent =
        serde_json::from_value(legacy).expect("a pre-#1862 journal line must still parse");
    let CompanyEvent::WorkflowRunStarted { started_by, .. } = &event else {
        panic!("expected a WorkflowRunStarted, got {event:?}");
    };
    assert_eq!(started_by, &None, "a legacy line names no sender");
}

/// Both node outcomes round-trip, including the elapsed reading — the field
/// that turns "it finished" into "it took this long", which is what tells a
/// slow run from a wedged one.
#[test]
fn workflow_node_finished_round_trips_both_statuses() {
    for status in [
        WorkflowNodeStatus::Ok,
        WorkflowNodeStatus::Error,
        // Issue #881's third arm. Pinned in the same loop rather than a
        // test of its own so a fourth reading cannot be added without
        // someone editing this list.
        WorkflowNodeStatus::Blocked,
        WorkflowNodeStatus::Declined,
    ] {
        let event = CompanyEvent::WorkflowNodeFinished {
            workflow_id: "digest".to_string(),
            run_id: "run-1".to_string(),
            node_id: "ceo".to_string(),
            status,
            elapsed_ms: 1234,
            diagnostics: Vec::new(),
            agent_run_id: None,
        };
        assert_eq!(round_trip(&event), event);
    }
}

/// A `WorkflowRunFinished` line written before #881 / #880 still replays.
///
/// **This is not a nicety.** The event is folded at boot, so a new field
/// without `#[serde(default)]` would make every pre-existing journal line
/// fail to parse — and the failure mode is a company silently losing its
/// whole run history, not a compile error. Pinned against a hand-written
/// legacy payload rather than a round-trip, because a round-trip can only
/// ever prove the new shape agrees with itself.
#[test]
fn a_pre_881_run_finished_line_still_replays() {
    let legacy = serde_json::json!({
        "kind": "WorkflowRunFinished",
        "workflow_id": "digest",
        "scheduled": true,
        "run_id": "run-1",
        "pending_approvals": ["review"],
        "cancelled": false
    });
    let event: CompanyEvent =
        serde_json::from_value(legacy).expect("a pre-#881 journal line must still parse");
    let CompanyEvent::WorkflowRunFinished {
        blocked_nodes,
        approvals,
        pending_approvals,
        ..
    } = &event
    else {
        panic!("expected a WorkflowRunFinished, got {event:?}");
    };
    assert!(blocked_nodes.is_empty());
    assert!(approvals.is_empty());
    assert_eq!(pending_approvals, &vec!["review".to_string()]);
}

/// A run that blocked on nobody serializes byte-for-byte as it did before
/// #881 / #880 — which is nearly every run, so this is what keeps the
/// journal from growing two empty arrays per line.
#[test]
fn a_run_that_blocked_on_nobody_adds_no_keys() {
    let event = CompanyEvent::WorkflowRunFinished {
        workflow_id: "digest".to_string(),
        scheduled: false,
        run_id: Some("run-1".to_string()),
        deliveries: Vec::new(),
        pending_approvals: Vec::new(),
        error: None,
        cancelled: false,
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    let json = serde_json::to_value(&event).expect("serialize");
    assert!(json.get("blocked_nodes").is_none(), "{json}");
    assert!(json.get("approvals").is_none(), "{json}");
}

/// Every field on both #371 variants is required, and that is the point:
/// the correlation id is what groups a run's nodes with its outcome, so a
/// line without one would be unfoldable. Nothing is `skip_serializing_if`
/// — except `WorkflowRunStarted::started_by` (issue #1862 prerequisite),
/// which is additive and `None` here on purpose, so the wire form stays
/// self-describing for every field that predates it.
#[test]
fn workflow_progress_variants_serialize_every_field() {
    let started = serde_json::to_string(&CompanyEvent::WorkflowRunStarted {
        workflow_id: "digest".to_string(),
        run_id: "run-1".to_string(),
        scheduled: false,
        started_by: None,
        resume_semantic: None,
    })
    .expect("serialize");
    assert_eq!(
        started,
        r#"{"kind":"WorkflowRunStarted","workflow_id":"digest","run_id":"run-1","scheduled":false}"#
    );

    let node = serde_json::to_string(&CompanyEvent::WorkflowNodeFinished {
        workflow_id: "digest".to_string(),
        run_id: "run-1".to_string(),
        node_id: "ceo".to_string(),
        status: WorkflowNodeStatus::Error,
        elapsed_ms: 7,
        diagnostics: Vec::new(),
        agent_run_id: None,
    })
    .expect("serialize");
    assert_eq!(
        node,
        r#"{"kind":"WorkflowNodeFinished","workflow_id":"digest","run_id":"run-1","node_id":"ceo","status":"error","elapsed_ms":7}"#
    );
}

/// The replay guarantee #371 rests on, stated as a test: adding these two
/// variants cannot change how an already-persisted line loads. A journal
/// written before #371 contains neither `kind`, and the pre-#371 wire form
/// of the variant they sit beside still decodes byte-for-byte as it did.
#[test]
fn pre_371_journal_lines_are_unaffected_by_the_new_variants() {
    let line = r#"{"kind":"WorkflowRunFinished","workflow_id":"digest","scheduled":true,"pending_approvals":["review"]}"#;
    let event: CompanyEvent = serde_json::from_str(line).expect("pre-#371 line loads");
    assert_eq!(
        event,
        CompanyEvent::WorkflowRunFinished {
            workflow_id: "digest".to_string(),
            scheduled: true,
            run_id: None,
            deliveries: Vec::new(),
            pending_approvals: vec!["review".to_string()],
            error: None,
            cancelled: false,
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        }
    );
}

/// Issue #327: the workspace announcement survives the JSONL round trip the
/// journal puts every event through, and carries its discriminant.
#[test]
fn workspace_changed_round_trips() {
    let event = CompanyEvent::WorkspaceChanged {
        node_id: "n-1".to_string(),
        change: "updated".to_string(),
    };
    assert_eq!(round_trip(&event), event);
    assert_eq!(event.kind(), "WorkspaceChanged");
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains(r#""kind":"WorkspaceChanged""#), "{json}");
    assert!(json.contains(r#""node_id":"n-1""#), "{json}");
}

/// The one variant whose retention class diverges from its sibling's, so
/// the choice is pinned rather than left to the next reader's memory.
///
/// `WorkspaceChanged` is Prunable: it is high-volume machine exhaust whose
/// whole meaning is "re-read the tree", nothing addresses it by sequence,
/// and nothing folds it at boot. `TaskCardChanged` stays Permanent because
/// a board card's lifecycle is the company's work history.
#[test]
fn a_workspace_announcement_is_prunable_though_its_board_sibling_is_not() {
    use crate::ports::events::RetentionClass;

    assert_eq!(
        CompanyEvent::WorkspaceChanged {
            node_id: "n-1".to_string(),
            change: "updated".to_string(),
        }
        .retention_class(),
        RetentionClass::Prunable
    );
    assert_eq!(
        CompanyEvent::TaskCardChanged {
            task_id: "t-1".to_string(),
            change: "opened".to_string(),
            column: Some("todo".to_string()),
        }
        .retention_class(),
        RetentionClass::Permanent
    );
}

/// Issue #529: the delivered-report event survives the JSONL round trip the
/// journal puts every event through, `target` and all — the whole reason it
/// exists is to be read back after a crash.
#[test]
fn workflow_report_delivered_round_trips() {
    let event = CompanyEvent::WorkflowReportDelivered {
        workflow_id: "digest".to_string(),
        run_id: "run-1".to_string(),
        node: "owner_summary".to_string(),
        kind: "owner".to_string(),
        target: Some("ada@example.com".to_string()),
    };
    assert_eq!(round_trip(&event), event);
}

/// Issue #529: the wire shape is pinned, and `target` is omitted entirely
/// when a destination named none — the same `skip_serializing_if` economy
/// every optional field on this enum keeps, so a channel line stays minimal.
#[test]
fn workflow_report_delivered_pins_its_wire_shape_and_omits_absent_target() {
    let with_target = serde_json::to_string(&CompanyEvent::WorkflowReportDelivered {
        workflow_id: "digest".to_string(),
        run_id: "run-1".to_string(),
        node: "owner_summary".to_string(),
        kind: "owner".to_string(),
        target: Some("ada@example.com".to_string()),
    })
    .expect("serialize");
    assert_eq!(
        with_target,
        r#"{"kind":"WorkflowReportDelivered","workflow_id":"digest","run_id":"run-1","node":"owner_summary","destination_kind":"owner","target":"ada@example.com"}"#
    );

    let no_target = serde_json::to_string(&CompanyEvent::WorkflowReportDelivered {
        workflow_id: "digest".to_string(),
        run_id: "run-1".to_string(),
        node: "notice".to_string(),
        kind: "channel".to_string(),
        target: None,
    })
    .expect("serialize");
    assert_eq!(
        no_target,
        r#"{"kind":"WorkflowReportDelivered","workflow_id":"digest","run_id":"run-1","node":"notice","destination_kind":"channel"}"#,
        "an absent target must not ride the line as a null"
    );
    // …and a line with no `target` loads back as `None` rather than failing.
    let decoded: CompanyEvent = serde_json::from_str(&no_target).expect("minimal line loads");
    assert_eq!(
        decoded,
        CompanyEvent::WorkflowReportDelivered {
            workflow_id: "digest".to_string(),
            run_id: "run-1".to_string(),
            node: "notice".to_string(),
            kind: "channel".to_string(),
            target: None,
        }
    );
}
