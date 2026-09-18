use super::tests_core::*;

#[test]
fn gate_inner_call_reads_the_call_the_card_shows() {
    let args = json!({ "url": "https://www.bbc.co.uk/sport" });
    let gate = gate_with_call("web_fetch", &args);

    let (tool, read_back) = gate_inner_call(&gate).expect("a classified gate names its call");
    assert_eq!(tool, "web_fetch");
    assert_eq!(read_back, &args);
}

/// The wrapper is what a naive classifier sees, and it is not the call. This
/// is the whole reason the projection exists.
#[test]
fn gate_inner_call_is_not_the_effect_kind() {
    let gate = gate_with_call("web_fetch", &json!({ "url": "https://www.bbc.co.uk" }));
    assert_eq!(gate.kind, WORKFLOW_APPROVE_KIND);
    assert_eq!(
        gate_inner_call(&gate).map(|(tool, _)| tool),
        Some("web_fetch")
    );
}

/// A gate the host could not classify has no inner call to offer, and must
/// not invent one — it stays per-call, as it always did.
#[test]
fn gate_inner_call_is_none_when_the_call_was_never_named() {
    let bare = effect("sports_blog", "fetch_bbc", json!({}));
    assert!(gate_inner_call(&bare).is_none());
}

/// `args` is written only when the node had some, so tool-without-args is an
/// ordinary shape. It answers `Null`, which every argument-aware classifier
/// resolves in the cautious direction.
#[test]
fn gate_inner_call_answers_null_for_a_call_with_no_arguments() {
    let gate = gate_effect(
        "sports_blog",
        "fetch_bbc",
        &json!({}),
        "run-1",
        &[],
        &[],
        Some(GateCall {
            tool: "web_fetch",
            reason: None,
            args: None,
            target: None,
        }),
    );
    assert_eq!(gate_inner_call(&gate), Some(("web_fetch", &Value::Null)));
}

/// Kind-checked for the same reason [`gate_node_id`] is: a teammate's own
/// `web_fetch` effect carries a tool name too, and must not be read as a
/// gate.
#[test]
fn gate_inner_call_ignores_a_non_gate_effect() {
    let mut agent_call = gate_with_call("web_fetch", &json!({ "url": "https://x.test" }));
    agent_call.kind = "web_fetch".to_string();
    assert!(gate_inner_call(&agent_call).is_none());
}

/// The second half of why a job card was never grantable, independent of its
/// `agent: None`. Classifying the wrapper asks about `workflow.approve`, an
/// undeclared name that fails closed; classifying the inner call asks about
/// the `web_fetch` the operator is looking at.
#[test]
fn a_gate_is_classified_by_the_call_it_stops_not_by_its_wrapper() {
    let gate = gate_with_call(
        crate::policy::consequence::WEB_FETCH,
        &json!({ "url": "https://www.bbc.co.uk/sport" }),
    );

    assert!(
        !crate::policy::consequence_of(&gate.kind, &gate.payload)
            .standing
            .is_grantable(),
        "the wrapper kind is undeclared and must keep failing closed on its own"
    );
    assert!(
        gate.may_be_granted_standing(),
        "a BBC fetch is ScopedGrantable, and that is the call the card shows"
    );
}

/// A gate stopping a per-call tool stays per-call. Reading the inner call
/// must widen nothing on its own — the classifier still decides.
#[test]
fn a_gate_stopping_a_per_call_tool_is_still_not_grantable() {
    let gate = gate_with_call("shell", &json!({ "command": "echo hi" }));
    assert!(!gate.may_be_granted_standing());
}

/// No readable host means no scope, and `ScopedGrantable` falls back to
/// per-call rather than minting a permission that would admit everything.
#[test]
fn a_gate_whose_url_names_no_host_is_still_not_grantable() {
    let gate = gate_with_call(crate::policy::consequence::WEB_FETCH, &json!({}));
    assert!(!gate.may_be_granted_standing());
}

#[test]
fn gate_workflow_id_names_the_permission_subject() {
    let gate = effect("sports_blog", "fetch_bbc", json!({}));
    assert_eq!(gate_workflow_id(&gate), Some("sports_blog"));

    let mut not_a_gate = gate.clone();
    not_a_gate.kind = "web_fetch".to_string();
    assert!(gate_workflow_id(&not_a_gate).is_none());
}

#[test]
fn the_parked_effect_is_native_and_self_contained() {
    let e = effect("digest", "gate", serde_json::json!({ "request": "topic" }));
    assert_eq!(e.kind, WORKFLOW_APPROVE_KIND);
    // Native: routes to `execute_effect_once`, not to a tool grant — and
    // keeps the console from offering a standing permission that would mean
    // nothing.
    assert!(e.agent.is_none());
    // Self-contained: everything a resume needs survives a restart in the
    // journal, with no live state anywhere.
    assert_eq!(e.payload[PAYLOAD_WORKFLOW_ID], "digest");
    assert_eq!(e.payload[PAYLOAD_NODE_ID], "gate");
    assert_eq!(e.payload[PAYLOAD_INPUT]["request"], "topic");
    assert_eq!(e.run_id.as_deref(), Some("run-1"));
}

#[test]
fn the_same_gate_on_the_same_input_is_one_decision() {
    let a = effect("digest", "gate", serde_json::json!({ "request": "x" }));
    let mut b = effect("digest", "gate", serde_json::json!({ "request": "x" }));
    // A re-run mints a new run id; that must not make it a second card.
    b.run_id = Some("run-2".to_string());
    assert!(is_same_gate(&a, &b));
}

#[test]
fn a_different_gate_input_or_workflow_is_a_different_decision() {
    let base = effect("digest", "gate", serde_json::json!({ "request": "x" }));
    for other in [
        effect(
            "digest",
            "second-gate",
            serde_json::json!({ "request": "x" }),
        ),
        effect("other", "gate", serde_json::json!({ "request": "x" })),
        effect("digest", "gate", serde_json::json!({ "request": "y" })),
    ] {
        assert!(
            !is_same_gate(&base, &other),
            "these are two decisions and both must be asked about: {other:?}"
        );
    }
}

#[test]
fn approving_a_gate_adds_it_to_the_trigger_inputs_approvals() {
    let out = with_approvals(
        serde_json::json!({ "request": "topic" }),
        &["gate".to_string()],
    );
    assert_eq!(out["request"], "topic", "the original input is preserved");
    assert_eq!(out["approvals"], serde_json::json!(["gate"]));
}

#[test]
fn a_second_gate_does_not_un_approve_the_first() {
    // The two-gate graph. Without the union the re-run pauses at the gate
    // the operator already cleared and the workflow can never finish.
    let first = with_approvals(
        serde_json::json!({ "request": "topic" }),
        &["gate-a".to_string()],
    );
    let second = with_approvals(first, &["gate-b".to_string()]);
    assert_eq!(second["approvals"], serde_json::json!(["gate-a", "gate-b"]));
}

#[test]
fn approving_the_same_gate_twice_does_not_duplicate_it() {
    let once = with_approvals(serde_json::json!({}), &["gate".to_string()]);
    let twice = with_approvals(once, &["gate".to_string()]);
    assert_eq!(twice["approvals"], serde_json::json!(["gate"]));
}

#[test]
fn a_non_object_input_still_yields_a_resumable_one() {
    // `engine::resume`'s own tolerance: there is nowhere to put the array on
    // a bare string or null, so it becomes a fresh object holding just the
    // approvals rather than a panic or a lost gate.
    for input in [
        Value::Null,
        serde_json::json!("a bare topic"),
        serde_json::json!(42),
        serde_json::json!(["not", "an", "object"]),
        // A malformed `approvals` starts an empty set rather than erroring.
        serde_json::json!({ "approvals": "gate" }),
    ] {
        let out = with_approvals(input.clone(), &["gate".to_string()]);
        assert_eq!(
            out["approvals"],
            serde_json::json!(["gate"]),
            "input {input} must still produce a resumable trigger"
        );
    }
}

#[test]
fn non_string_entries_in_a_prior_approvals_array_are_dropped() {
    let out = with_approvals(
        serde_json::json!({ "approvals": ["a", 7, null] }),
        &["b".to_string()],
    );
    assert_eq!(out["approvals"], serde_json::json!(["a", "b"]));
}

// --- issue #438: the delivery ledger -------------------------------------

/// What the run delivered rides the card, so approving it can suppress a
/// second send. `Sent` and `Pending` are both "already delivered": a parked
/// cold-send card is durable and approving it sends, so re-parking would
/// stack a duplicate and approving both would mail twice.
#[test]
fn a_gate_card_carries_what_the_run_already_delivered() {
    let e = gate_effect(
        "digest",
        "gate",
        &serde_json::json!({ "request": "x" }),
        "run-1",
        &[
            delivery("owner_summary", "owner", DeliveryStatus::Sent),
            delivery("cold_note", "email", DeliveryStatus::Pending),
        ],
        &[],
        None,
    );
    assert_eq!(
        ledger(&e),
        vec![
            DeliveredReport {
                node: "owner_summary".into(),
                kind: "owner".into()
            },
            DeliveredReport {
                node: "cold_note".into(),
                kind: "email".into()
            },
        ]
    );
}

/// A row that never left the process is NOT on the ledger — nothing was
/// sent, so a continuation is free to try again. Suppressing these would
/// silently retire a report on the strength of a failure.
#[test]
fn a_report_that_did_not_go_out_stays_deliverable() {
    for status in [
        DeliveryStatus::Skipped,
        DeliveryStatus::Denied,
        DeliveryStatus::Failed,
    ] {
        let e = gate_effect(
            "digest",
            "gate",
            &Value::Null,
            "run-1",
            &[delivery("summary", "owner", status)],
            &[],
            None,
        );
        assert!(
            ledger(&e).is_empty(),
            "{status:?} sent nothing, so it must stay deliverable"
        );
    }
}

/// An `owner` destination fans out to one row per admin. The ledger is per
/// node, so it holds that node once rather than once per recipient.
#[test]
fn a_fanned_out_destination_is_one_ledger_row() {
    let e = gate_effect(
        "digest",
        "gate",
        &Value::Null,
        "run-1",
        &[
            delivery("summary", "owner", DeliveryStatus::Sent),
            delivery("summary", "owner", DeliveryStatus::Sent),
        ],
        &[],
        None,
    );
    assert_eq!(ledger(&e).len(), 1);
}

/// The ledger rides the continuation's trigger input — this is the whole
/// mechanism, since `deliver_outputs` reads it from there and nowhere else.
#[test]
fn the_ledger_rides_the_continuation_input() {
    let card = gate_effect(
        "digest",
        "gate",
        &serde_json::json!({ "request": "x" }),
        "run-1",
        &[delivery("summary", "owner", DeliveryStatus::Sent)],
        &[],
        None,
    );

    let input = single_continuation_input(&card).expect("a well-formed card continues");

    assert_eq!(input["approvals"], serde_json::json!(["gate"]));
    assert_eq!(
        input["request"], "x",
        "the original topic still rides along"
    );
    assert_eq!(
        delivered_in_input(&input),
        vec![DeliveredReport {
            node: "summary".into(),
            kind: "owner".into()
        }]
    );
}

/// **The two-gate case.** Approving the first gate starts a continuation
/// that skips the already-sent report and then pauses at the second gate.
/// That second card must carry the FIRST run's deliveries too — it delivered
/// nothing itself, so a ledger built only from its own rows would be empty
/// and approving it would send the report for real.
#[test]
fn the_ledger_accumulates_across_two_gates() {
    // Run 1 delivers the summary and pauses on gate-a.
    let first = gate_effect(
        "digest",
        "gate-a",
        &serde_json::json!({ "request": "x" }),
        "run-1",
        &[delivery("summary", "owner", DeliveryStatus::Sent)],
        &[],
        None,
    );
    let continuation = single_continuation_input(&first).expect("continues");

    // Run 2 skips the summary (delivering nothing) and pauses on gate-b.
    let second = gate_effect("digest", "gate-b", &continuation, "run-2", &[], &[], None);
    assert_eq!(
        ledger(&second),
        vec![DeliveredReport {
            node: "summary".into(),
            kind: "owner".into()
        }],
        "the second card must remember what the first run sent"
    );

    // And approving THAT still suppresses it, with both gates approved.
    let next = single_continuation_input(&second).expect("continues");
    assert_eq!(next["approvals"], serde_json::json!(["gate-a", "gate-b"]));
    assert_eq!(delivered_in_input(&next).len(), 1);
}

// --- issue #846: the outward-call ledger -------------------------------

/// One outward call, made once, across a whole lineage — **the headline
/// claim for issue #846**, in the shape #496 proved for the delivery half.
///
/// A `POST` upstream of two gates. Run 1 fires it and pauses; approving
/// starts run 2, which must NOT fire it again and must pause on the second
/// gate; approving that starts run 3, which must not fire it either. The
/// ledger has to accumulate down the lineage or run 3 posts for real —
/// exactly the trap the delivery ledger's own two-gate test exists for.
///
/// Asserted on the ledger the card carries and the input it produces,
/// because those are what decide whether the call is made: the graph rewrite
/// that consumes them is pinned in `crate::workflows::replay`, and the
/// invoker arm that answers it is pinned in `crate::workflows::caps::tools`.
#[test]
fn an_outward_call_is_made_once_across_two_gates() {
    let posted = PerformedCall {
        node: "notify".into(),
        tool: "http_request POST".into(),
        result: serde_json::json!({ "status": 201 }),
    };

    // Run 1: the POST fires, the run pauses on gate-a.
    let first = gate_effect(
        "digest",
        "gate-a",
        &serde_json::json!({ "request": "x" }),
        "run-1",
        &[],
        std::slice::from_ref(&posted),
        None,
    );
    let continuation = single_continuation_input(&first).expect("continues");
    assert_eq!(
        performed_in_input(&continuation),
        vec![posted.clone()],
        "the continuation must know what run 1 already posted"
    );

    // Run 2: the first POST is replayed (this run does not repeat it), and
    // a SECOND outward node fires before the run pauses on gate-b.
    //
    // The second call is what makes this the real two-gate case rather than
    // a walk-through. A card carrying a non-empty ledger REPLACES the input's
    // key rather than merging into it — `with_performed` documents why — so
    // if `performed_ledger` did not union the input's own entries first, run
    // 2's card would carry only its own call and run 3 would post the first
    // one a second time. Reverting that union fails this test on the
    // `notify` entry alone.
    let also_posted = PerformedCall {
        node: "escalate".into(),
        tool: "http_request POST".into(),
        result: serde_json::json!({ "status": 202 }),
    };
    let second = gate_effect(
        "digest",
        "gate-b",
        &continuation,
        "run-2",
        &[],
        std::slice::from_ref(&also_posted),
        None,
    );
    let next = single_continuation_input(&second).expect("continues");

    assert_eq!(
        performed_in_input(&next),
        vec![posted, also_posted],
        "run 3 must be told about BOTH earlier posts, not just the last one"
    );
    assert_eq!(next["approvals"], serde_json::json!(["gate-a", "gate-b"]));
}

/// The earliest result in the lineage wins, and a node is listed once.
///
/// The run that actually reached the counterparty is the one whose receipt
/// downstream nodes saw, so a later run must not overwrite it — and a ledger
/// that grew an entry per hop would bloat every card in a long lineage.
#[test]
fn the_outward_ledger_keeps_the_first_result_per_node() {
    let original = PerformedCall {
        node: "notify".into(),
        tool: "http_request POST".into(),
        result: serde_json::json!({ "id": "first" }),
    };
    let input = serde_json::json!({ CONTINUATION_PERFORMED_KEY: [original.clone()] });

    let card = gate_effect(
        "digest",
        "gate",
        &input,
        "run-2",
        &[],
        &[PerformedCall {
            node: "notify".into(),
            tool: "http_request POST".into(),
            result: serde_json::json!({ "id": "second" }),
        }],
        None,
    );

    let ledger: Vec<PerformedCall> = serde_json::from_value(
        card.payload
            .get(PAYLOAD_PERFORMED)
            .expect("carried")
            .clone(),
    )
    .expect("well-formed");
    assert_eq!(ledger, vec![original]);
}

/// A card with no outward ledger produces an input with no reserved key —
/// so a first run's trigger payload keeps exactly the shape it always had.
#[test]
fn an_empty_outward_ledger_leaves_the_input_untouched() {
    let card = gate_effect(
        "digest",
        "gate",
        &serde_json::json!({ "request": "x" }),
        "run-1",
        &[],
        &[],
        None,
    );
    let input = single_continuation_input(&card).expect("continues");
    assert!(
        input.get(CONTINUATION_PERFORMED_KEY).is_none(),
        "nothing to suppress must write nothing: {input}"
    );
    assert!(performed_in_input(&input).is_empty());
}

/// The outward ledger is NOT part of a gate's identity.
///
/// A continuation's input differs from the paused run's by exactly the
/// reserved ledger keys, so counting either would make every continuation
/// gate read as a new decision and stack a duplicate card for one question —
/// the failure `is_same_gate` exists to prevent. The delivery half of this
/// is pinned beside it; this is the same claim for the #846 key.
#[test]
fn the_outward_ledger_is_not_part_of_a_gates_identity() {
    let input = serde_json::json!({ "request": "x" });
    let paused = gate_effect("wf", "publish", &input, "run-1", &[], &[], None);

    let mut continuation = input.clone();
    continuation.as_object_mut().expect("object").insert(
        CONTINUATION_PERFORMED_KEY.to_string(),
        serde_json::json!([PerformedCall {
            node: "notify".into(),
            tool: "http_request POST".into(),
            result: serde_json::json!({ "status": 201 }),
        }]),
    );
    let re_reached = gate_effect("wf", "publish", &continuation, "run-2", &[], &[], None);

    assert!(
        is_same_gate(&paused, &re_reached),
        "a ledger key must not split one decision into two cards"
    );
}

/// A run that delivered nothing writes no reserved key at all, so an
/// ordinary continuation's input keeps exactly the shape it always had.
#[test]
fn a_lineage_that_delivered_nothing_threads_no_reserved_key() {
    let card = effect("digest", "gate", serde_json::json!({ "request": "x" }));
    let input = single_continuation_input(&card).expect("continues");
    assert!(input.get(CONTINUATION_DELIVERED_KEY).is_none(), "{input}");
    assert!(delivered_in_input(&input).is_empty());
}

/// A first run carries no answer, and reading one is not an error — it is
/// the ordinary case for every node that never blocked.
#[test]
fn an_input_with_no_answered_blocker_reads_as_nothing() {
    let input = serde_json::json!({ "request": "x" });
    assert!(
        blocker_answers_in_input(&input)
            .expect("readable")
            .is_empty()
    );
    assert!(
        blocker_answer_for(&input, "draft")
            .expect("readable")
            .is_none()
    );
}

/// The headline of the thread: the verdict and the operator's words ride
/// the continuation's trigger input, keyed by the node they re-enter.
#[test]
fn an_answer_rides_the_trigger_input_keyed_by_its_node() {
    use crate::ports::blockers::BlockerVerdict;
    let input = blocker_continuation_input(
        serde_json::json!({ "request": "x" }),
        "draft",
        &resolution(BlockerVerdict::Amend, "use gpt-4o-mini"),
    )
    .expect("continues");
    assert_eq!(input["request"], "x", "the blocked run's input is kept");
    let answer = blocker_answer_for(&input, "draft")
        .expect("readable")
        .expect("the answer is there");
    assert_eq!(answer.verdict, BlockerVerdict::Amend);
    assert_eq!(answer.answer, "use gpt-4o-mini");
    assert!(
        blocker_answer_for(&input, "other")
            .expect("readable")
            .is_none(),
        "an answer is about one node, not the graph"
    );
}

/// A two-blocker lineage must not forget the first node's answer when the
/// second is decided — the same accumulation the denial ledger keeps.
#[test]
fn a_second_nodes_answer_does_not_forget_the_first() {
    use crate::ports::blockers::BlockerVerdict;
    let first = blocker_continuation_input(
        serde_json::json!({ "request": "x" }),
        "draft",
        &resolution(BlockerVerdict::Skip, ""),
    )
    .expect("continues");
    let second =
        blocker_continuation_input(first, "review", &resolution(BlockerVerdict::Retry, ""))
            .expect("continues");
    assert_eq!(
        blocker_answers_in_input(&second).expect("readable").len(),
        2
    );
    assert_eq!(
        blocker_answer_for(&second, "draft")
            .expect("readable")
            .expect("kept")
            .verdict,
        BlockerVerdict::Skip
    );
}

/// A node retried, blocked again and then skipped is skipped: the operator's
/// newest decision is the one in force.
#[test]
fn a_later_answer_supersedes_an_earlier_one_for_the_same_node() {
    use crate::ports::blockers::BlockerVerdict;
    let first = blocker_continuation_input(
        serde_json::json!({ "request": "x" }),
        "draft",
        &resolution(BlockerVerdict::Retry, ""),
    )
    .expect("continues");
    let second = blocker_continuation_input(first, "draft", &resolution(BlockerVerdict::Skip, ""))
        .expect("continues");
    assert_eq!(
        blocker_answers_in_input(&second).expect("readable").len(),
        1,
        "one node carries one live decision"
    );
    assert_eq!(
        blocker_answer_for(&second, "draft")
            .expect("readable")
            .expect("kept")
            .verdict,
        BlockerVerdict::Skip
    );
}

/// The asymmetry with the ledgers, pinned: an unreadable answer is loud.
/// Degrading it to "nobody answered" would re-run the node into the
/// identical failure with the operator's decision gone.
#[test]
fn an_unreadable_answer_is_loud_rather_than_ignored() {
    let not_a_list = serde_json::json!({ CONTINUATION_BLOCKER_KEY: "retry" });
    assert!(blocker_answers_in_input(&not_a_list).is_err());

    let unknown_verdict = serde_json::json!({
        CONTINUATION_BLOCKER_KEY: [{ "node": "draft", "verdict": "shrug" }]
    });
    assert!(blocker_answers_in_input(&unknown_verdict).is_err());
    assert!(blocker_answer_for(&unknown_verdict, "draft").is_err());

    let no_node = serde_json::json!({
        CONTINUATION_BLOCKER_KEY: [{ "verdict": "retry" }]
    });
    assert!(blocker_answers_in_input(&no_node).is_err());
}
