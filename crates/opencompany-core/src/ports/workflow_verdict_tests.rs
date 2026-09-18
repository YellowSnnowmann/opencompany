use super::*;
use crate::ports::DeliveryReason;

fn row(status: DeliveryStatus, reason: DeliveryReason) -> DeliveryReport {
    DeliveryReport {
        node: "report".into(),
        kind: "channel".into(),
        target: Some("engineering".into()),
        status,
        detail: "detail".into(),
        reason,
    }
}

/// A run that finished cleanly and routed nothing — the base every case
/// below varies by exactly one fact.
fn clean() -> RunVerdictFacts<'static> {
    RunVerdictFacts {
        running: false,
        error: None,
        cancelled: false,
        blocked_nodes: 0,
        deliveries: &[],
        pending_approvals: 0,
        stranded_approvals: 0,
        errored_nodes: 0,
    }
}

#[test]
fn a_clean_run_is_ok() {
    assert_eq!(WorkflowRunVerdict::of(clean()), WorkflowRunVerdict::Ok);
}

/// The defect issue #981 filed: every node `ok`, no error, nothing
/// cancelled — and the report is gone.
#[test]
fn a_run_whose_only_failure_is_delivery_is_not_ok() {
    let dropped = [row(DeliveryStatus::Failed, DeliveryReason::ChannelNotWired)];
    let verdict = WorkflowRunVerdict::of(RunVerdictFacts {
        deliveries: &dropped,
        ..clean()
    });
    assert_eq!(verdict, WorkflowRunVerdict::Undelivered);
    assert_ne!(verdict, WorkflowRunVerdict::Ok);
    // …and it is not reported as a failure either. The nodes ran.
    assert_ne!(verdict, WorkflowRunVerdict::Failed);
}

/// The other two refusals issue #981 names reach the same verdict, and
/// through their own `DeliveryStatus` rather than through a shared one — so
/// this pins that the count is not accidentally reading only `Failed`.
#[test]
fn a_denied_or_skipped_report_is_undelivered_too() {
    for status in [DeliveryStatus::Denied, DeliveryStatus::Skipped] {
        let rows = [row(status, DeliveryReason::EmailNotGranted)];
        assert_eq!(
            WorkflowRunVerdict::of(RunVerdictFacts {
                deliveries: &rows,
                ..clean()
            }),
            WorkflowRunVerdict::Undelivered,
            "{status:?} is a report that did not go out"
        );
    }
}

#[test]
fn a_run_that_delivered_everything_is_unchanged() {
    let sent = [row(DeliveryStatus::Sent, DeliveryReason::ChannelPosted)];
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            deliveries: &sent,
            ..clean()
        }),
        WorkflowRunVerdict::Ok
    );
}

/// The more serious fact first: a run that broke mid-graph AND dropped its
/// report reports the break, not the drop.
#[test]
fn a_failed_run_that_also_dropped_a_report_reads_failed() {
    let dropped = [row(DeliveryStatus::Failed, DeliveryReason::ChannelNotWired)];
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            error: Some("node `draft` errored"),
            deliveries: &dropped,
            ..clean()
        }),
        WorkflowRunVerdict::Failed
    );
}

#[test]
fn the_precedence_order_is_the_whole_check() {
    let dropped = [row(DeliveryStatus::Failed, DeliveryReason::ChannelNotWired)];
    // Every arm asserted against a fact set that ALSO satisfies every arm
    // below it, so a reordering breaks this rather than passing by luck.
    let everything = RunVerdictFacts {
        running: true,
        error: Some("boom"),
        cancelled: true,
        blocked_nodes: 1,
        deliveries: &dropped,
        pending_approvals: 1,
        // Issue #1189: the one gate this run stopped for has no card left,
        // so the facts satisfy `stranded` too.
        stranded_approvals: 1,
        // Issue #1865: also satisfies `degraded`, so this fact set proves
        // every arm outranks it too.
        errored_nodes: 1,
    };
    assert_eq!(
        WorkflowRunVerdict::of(everything),
        WorkflowRunVerdict::Running
    );
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            running: false,
            ..everything
        }),
        WorkflowRunVerdict::Failed
    );
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            running: false,
            error: None,
            ..everything
        }),
        WorkflowRunVerdict::Stopped
    );
    // Issue #1189. It sits ABOVE `blocked` and above `awaiting-approval`,
    // and these facts satisfy both — which is the whole claim: a run whose
    // every gate lost its card must not be told to go and decide it.
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            running: false,
            error: None,
            cancelled: false,
            ..everything
        }),
        WorkflowRunVerdict::Stranded
    );
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            running: false,
            error: None,
            cancelled: false,
            stranded_approvals: 0,
            ..everything
        }),
        WorkflowRunVerdict::Blocked
    );
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            running: false,
            error: None,
            cancelled: false,
            stranded_approvals: 0,
            blocked_nodes: 0,
            ..everything
        }),
        WorkflowRunVerdict::Undelivered
    );
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            running: false,
            error: None,
            cancelled: false,
            stranded_approvals: 0,
            blocked_nodes: 0,
            deliveries: &[],
            ..everything
        }),
        WorkflowRunVerdict::AwaitingApproval
    );
    // Issue #1865: with every fact above cleared and only the errored node
    // left, the run reads `degraded` — not `ok`, and not `failed`.
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            running: false,
            error: None,
            cancelled: false,
            stranded_approvals: 0,
            blocked_nodes: 0,
            deliveries: &[],
            pending_approvals: 0,
            ..everything
        }),
        WorkflowRunVerdict::Degraded
    );
    // …and clearing the errored-node count too is the only way back to
    // `ok`, which is the base case this whole ladder falls through to.
    // Every field of `everything` is overridden here, so this is written
    // out in full rather than spread — the base case earns no shortcut.
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            running: false,
            error: None,
            cancelled: false,
            stranded_approvals: 0,
            blocked_nodes: 0,
            deliveries: &[],
            pending_approvals: 0,
            errored_nodes: 0,
        }),
        WorkflowRunVerdict::Ok
    );
}

/// A parked report is waiting on a person, not on a fix — so it must never
/// land in the undelivered count, which would badge a working approvals
/// queue as a failure.
#[test]
fn a_parked_report_is_awaiting_not_undelivered() {
    let parked = [row(
        DeliveryStatus::Pending,
        DeliveryReason::ParkedForApproval,
    )];
    assert_eq!(undelivered_count(&parked), 0);
    assert_eq!(awaiting_count(&parked, 0), 1);
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            deliveries: &parked,
            ..clean()
        }),
        WorkflowRunVerdict::AwaitingApproval
    );
}

/// Issue #846: a gated run reaches no `output` node, so its verdict has to
/// come off `pending_approvals` or it scores clean.
#[test]
fn a_gated_run_with_no_deliveries_is_awaiting() {
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            pending_approvals: 1,
            ..clean()
        }),
        WorkflowRunVerdict::AwaitingApproval
    );
}

/// An error string the host never writes, read the way the console reads
/// it: empty is not a failure.
#[test]
fn an_empty_error_is_not_a_failure() {
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            error: Some(""),
            ..clean()
        }),
        WorkflowRunVerdict::Ok
    );
}

/// The wire tokens are the console's eight words, and `as_str` may not
/// drift from them.
#[test]
fn the_wire_tokens_are_the_consoles_words() {
    for (verdict, token) in [
        (WorkflowRunVerdict::Running, "running"),
        (WorkflowRunVerdict::Failed, "failed"),
        (WorkflowRunVerdict::Stopped, "stopped"),
        (WorkflowRunVerdict::Stranded, "stranded"),
        (WorkflowRunVerdict::Blocked, "blocked"),
        (WorkflowRunVerdict::Undelivered, "undelivered"),
        (WorkflowRunVerdict::AwaitingApproval, "awaiting-approval"),
        (WorkflowRunVerdict::Degraded, "degraded"),
        (WorkflowRunVerdict::Ok, "ok"),
    ] {
        assert_eq!(
            serde_json::to_value(verdict).expect("serializes"),
            serde_json::Value::String(token.to_string())
        );
        assert_eq!(verdict.as_str(), token);
        assert_eq!(verdict.to_string(), token);
    }
}

/// Issue #981, the second half: a **test run** attempted nothing, on
/// purpose, so its rows are not reports that went missing.
///
/// This was a live false positive, not a theoretical one — `deliver_outputs_dry`
/// writes one `skipped`/`dry-run` row per routed `output` node, so before
/// this every single test run of a graph with a destination scored
/// `undelivered` and the console badged the safest thing an operator can do
/// as a failure.
#[test]
fn a_dry_run_is_not_undelivered() {
    let dry = [row(DeliveryStatus::Skipped, DeliveryReason::DryRun)];
    assert!(!is_undelivered(&dry[0]));
    assert_eq!(undelivered_count(&dry), 0);
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            deliveries: &dry,
            ..clean()
        }),
        WorkflowRunVerdict::Ok
    );
}

/// Issue #438: approving a gate re-runs the graph from the trigger, so an
/// `output` node upstream of the gate is reached a second time and
/// deliberately not sent again. The report is at its destination; the
/// continuation is not a run that lost one.
#[test]
fn an_already_delivered_report_is_not_undelivered() {
    let again = [row(
        DeliveryStatus::Skipped,
        DeliveryReason::AlreadyDelivered,
    )];
    assert!(!is_undelivered(&again[0]));
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            deliveries: &again,
            ..clean()
        }),
        WorkflowRunVerdict::Ok
    );
}

/// The deliberate **non**-move, and the reason the other two could move at
/// all: an `output` node with nowhere to send produced a report and lost it,
/// with nothing accounting for it. Issue #925 added the row precisely so
/// that case stops being indistinguishable from a graph that routed nothing
/// on purpose; excusing it here restores the silence issues #947 and #963
/// were filed about.
#[test]
fn an_output_node_with_no_destination_is_still_undelivered() {
    let nowhere = [row(
        DeliveryStatus::Skipped,
        DeliveryReason::NoDestinationConfigured,
    )];
    assert!(is_undelivered(&nowhere[0]));
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            deliveries: &nowhere,
            ..clean()
        }),
        WorkflowRunVerdict::Undelivered
    );
}

/// A row journaled before issue #248 added `reason` deserializes as
/// `Unspecified`, and an unreadable reason must not excuse a report from the
/// number an operator acts on.
#[test]
fn a_skipped_row_with_no_recorded_reason_still_counts() {
    let old = [row(DeliveryStatus::Skipped, DeliveryReason::Unspecified)];
    assert!(is_undelivered(&old[0]));
}

/// Only the `skipped` arm reads a reason. A `failed` row is a report that
/// was attempted and did not work, whatever it claims about why — so the
/// two exemptions cannot leak onto a status that means something broke.
#[test]
fn the_exemptions_are_scoped_to_skipped() {
    for status in [DeliveryStatus::Failed, DeliveryStatus::Denied] {
        for reason in [DeliveryReason::DryRun, DeliveryReason::AlreadyDelivered] {
            assert!(
                is_undelivered(&row(status, reason)),
                "{status:?}/{reason:?} is not a skip"
            );
        }
    }
}

// ── Issue #1189: a run nobody can act on any more ────────────────────────

/// The marketing tenant's 34 runs: three gate nodes on `pendingApprovals`,
/// no blocked-node rows at all, and an empty approvals queue.
///
/// Before this arm they scored `awaiting-approval` forever — a third of the
/// tenant's whole history claiming to wait on a person with nothing to
/// answer, and #1143's reconciliation could not reach them because it joins
/// on approval ids this shape never had.
#[test]
fn a_run_whose_every_gate_lost_its_card_is_stranded_not_awaiting() {
    let verdict = WorkflowRunVerdict::of(RunVerdictFacts {
        pending_approvals: 3,
        stranded_approvals: 3,
        ..clean()
    });
    assert_eq!(verdict, WorkflowRunVerdict::Stranded);
    assert_ne!(
        verdict,
        WorkflowRunVerdict::AwaitingApproval,
        "nothing in the queue is waiting on this run, so it must not say so"
    );
}

/// The negative that makes the test above mean anything.
///
/// A rule that fired on *any* stranded gate would satisfy it and be worse
/// than no rule: it would retire a run with a decision still sitting in the
/// queue. Two of the three are gone; the third can still be made, so the
/// verdict must go on saying so and the per-node count carries the loss.
#[test]
fn a_partly_stranded_run_is_still_awaiting() {
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            pending_approvals: 3,
            stranded_approvals: 1,
            ..clean()
        }),
        WorkflowRunVerdict::AwaitingApproval
    );
}

/// A parked **report** is a second thing waiting on a person, on its own
/// queue, and the gate join does not look at it. So a run whose gates are
/// all stranded but whose report is still parked is genuinely awaiting —
/// there really is a card to decide.
#[test]
fn a_stranded_run_with_a_parked_report_is_still_awaiting() {
    let parked = [row(
        DeliveryStatus::Pending,
        DeliveryReason::ParkedForApproval,
    )];
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            deliveries: &parked,
            pending_approvals: 2,
            stranded_approvals: 2,
            ..clean()
        }),
        WorkflowRunVerdict::AwaitingApproval
    );
}

/// The `feature_pipeline` shape from the issue: a blocked node whose every
/// card the queue has lost. #1143 already says so in the blocked-node list;
/// this is the verdict finally agreeing with it instead of reading
/// `blocked` — which, like `awaiting-approval`, tells the operator to go
/// and decide something that is not there.
#[test]
fn a_fully_stranded_blocked_run_reads_stranded_not_blocked() {
    let verdict = WorkflowRunVerdict::of(RunVerdictFacts {
        blocked_nodes: 1,
        pending_approvals: 1,
        stranded_approvals: 1,
        ..clean()
    });
    assert_eq!(verdict, WorkflowRunVerdict::Stranded);
    assert_ne!(verdict, WorkflowRunVerdict::Blocked);
}

/// A run with no gates at all is not stranded, whatever else is true of it.
/// `stranded` is a correction to `awaiting`, never a new way to score a run
/// that was waiting on nobody.
#[test]
fn a_run_that_stopped_for_nobody_is_never_stranded() {
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            pending_approvals: 0,
            stranded_approvals: 0,
            ..clean()
        }),
        WorkflowRunVerdict::Ok
    );
}

// ── Issue #1189: the fold that reconciles a gate against the live queue ──

/// A blocked node carrying `approval_ids`, for the id-keyed half of the
/// join.
fn blocked_node(node: &str, ids: &[&str]) -> WorkflowBlockedNode {
    WorkflowBlockedNode {
        node_id: node.to_string(),
        tools: vec!["shell".to_string()],
        approval_ids: ids.iter().map(|id| id.to_string()).collect(),
        unparkable: 0,
        stranded: 0,
        blockers: 0,
    }
}

fn nodes(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

/// The marketing tenant's shape: gate nodes on `pendingApprovals`, no
/// blocked-node rows at all, and an empty queue.
#[test]
fn a_gate_with_no_card_left_is_stranded() {
    let live = LiveApprovals::default();
    assert_eq!(
        stranded_approvals(
            Some("run-1"),
            &nodes(&["fetch_bbc", "fetch_espn", "fetch_guardian"]),
            &[],
            &live
        ),
        3
    );
}

/// The negative that makes the one above mean anything: a fold that marked
/// everything stranded would satisfy it and be worse than no fold at all.
#[test]
fn a_gate_whose_card_is_still_parked_is_not_stranded() {
    let mut live = LiveApprovals::default();
    live.insert_gate("run-1", "fetch_bbc");
    assert_eq!(
        stranded_approvals(Some("run-1"), &nodes(&["fetch_bbc"]), &[], &live),
        0
    );
}

/// Keyed on the **pair**. Two runs of one workflow park the same node id, so
/// a node-only key would keep every historical run of `daily-sports-news`
/// advertised as approvable for as long as any one of them has a live card.
#[test]
fn the_same_node_parked_under_a_different_run_does_not_count() {
    let mut live = LiveApprovals::default();
    live.insert_gate("run-2", "fetch_bbc");
    assert_eq!(
        stranded_approvals(Some("run-1"), &nodes(&["fetch_bbc"]), &[], &live),
        1
    );
}

/// The id-keyed half: a blocked node's cards carry no node id, so the node
/// is reached through `approval_ids`.
#[test]
fn a_blocked_node_is_live_if_any_of_its_approvals_is() {
    let mut live = LiveApprovals::default();
    live.insert_id("appr-2");
    let blocked = [blocked_node("backend", &["appr-1", "appr-2", "appr-3"])];
    assert_eq!(
        stranded_approvals(Some("run-1"), &nodes(&["backend"]), &blocked, &live),
        0,
        "one decidable call still makes the node a question"
    );
}

/// …and the same node with every id gone is stranded, which is the
/// `feature_pipeline` shape issue #1189 opens on.
#[test]
fn a_blocked_node_whose_every_approval_is_gone_is_stranded() {
    let live = LiveApprovals::default();
    let blocked = [blocked_node("backend", &["appr-1", "appr-2", "appr-3"])];
    assert_eq!(
        stranded_approvals(Some("run-1"), &nodes(&["backend"]), &blocked, &live),
        1
    );
}

/// A pre-#371 row has no run id, so the gate join has no key. Marking its
/// nodes stranded on the strength of a missing field would retire work an
/// operator can still act on.
#[test]
fn a_run_with_no_id_is_never_stranded() {
    let live = LiveApprovals::default();
    assert_eq!(
        stranded_approvals(None, &nodes(&["fetch_bbc"]), &[], &live),
        0
    );
}

// ── Issue #1865: a run whose node errored under `on_error: continue|route` ──

/// The defect this issue filed: a node under `on_error: continue|route`
/// errors, the graph keeps going, and the run reaches the end with no
/// error, no cancel, nothing blocked, nothing undelivered and nobody
/// awaited — which fell all the way through to `ok` before this arm
/// existed.
#[test]
fn a_run_with_an_errored_continue_node_is_degraded_not_ok() {
    let verdict = WorkflowRunVerdict::of(RunVerdictFacts {
        errored_nodes: 1,
        ..clean()
    });
    assert_eq!(verdict, WorkflowRunVerdict::Degraded);
    assert_ne!(verdict, WorkflowRunVerdict::Ok);
    assert_ne!(
        verdict,
        WorkflowRunVerdict::Failed,
        "the author asked for the branch to survive the error"
    );
}

/// A run with no errored node is unaffected — the new arm changes nothing
/// for the common case.
#[test]
fn a_clean_run_with_no_errored_nodes_is_still_ok() {
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            errored_nodes: 0,
            ..clean()
        }),
        WorkflowRunVerdict::Ok
    );
}

/// `degraded` is checked LAST — a run that is also genuinely `failed`
/// (an error the host actually recorded, distinct from a per-node
/// `on_error: continue` error) reports the failure, not the softer
/// reading.
#[test]
fn an_errored_node_never_hides_a_real_failure() {
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            error: Some("node `draft` errored"),
            errored_nodes: 1,
            ..clean()
        }),
        WorkflowRunVerdict::Failed
    );
}

/// Nor does it hide a decidable gate — a run that is both degraded and
/// still awaiting an answer reports the thing an operator can act on.
#[test]
fn an_errored_node_never_hides_awaiting_approval() {
    assert_eq!(
        WorkflowRunVerdict::of(RunVerdictFacts {
            pending_approvals: 1,
            errored_nodes: 1,
            ..clean()
        }),
        WorkflowRunVerdict::AwaitingApproval
    );
}

/// `sent` and `pending` are excused by **status**, so no reason can pull
/// them into the count either.
#[test]
fn sent_and_pending_are_never_undelivered() {
    for status in [DeliveryStatus::Sent, DeliveryStatus::Pending] {
        for reason in [
            DeliveryReason::ChannelPosted,
            DeliveryReason::ParkedForApproval,
            DeliveryReason::NoDestinationConfigured,
        ] {
            assert!(!is_undelivered(&row(status, reason)));
        }
    }
}
