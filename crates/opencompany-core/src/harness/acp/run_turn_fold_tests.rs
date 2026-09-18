use super::*;

/// An ACP agent's frames say which query they belong to, exactly as the
/// built-in harness's do.
///
/// `chat_ctx` used to take `chat_id` alone and hard-code `message_seq:
/// None`, so every ACP frame fell into the shared per-thread bucket — two
/// of its turns in one chat shared a single row-list, and arming the second
/// erased the first's timeline. That is the failure `messageSeq` exists to
/// stop, and it was left in place for one of the two harnesses (Codex on
/// #2069).
///
/// It now takes the whole `ChatTarget`, which is what keeps the two from
/// drifting again: there is no `chat_id`-only shape left to answer with.
#[test]
fn an_acp_chat_turn_carries_the_query_it_answers() {
    use crate::ports::types::EventSeq;

    let company = CompanyId::new("acp-msg-seq");

    let answering = AcpRunTurn::chat_ctx(
        &company,
        "product_manager",
        ChatTarget::channel(Some("general")).answering(Some(EventSeq::new(45))),
    );
    assert_eq!(answering.message_seq, Some(45));

    // And a turn answering no journaled message still says nothing, so the
    // consumer falls back to the thread exactly as it did before.
    let unaddressed = AcpRunTurn::chat_ctx(
        &company,
        "product_manager",
        ChatTarget::channel(Some("general")),
    );
    assert_eq!(unaddressed.message_seq, None);
}

pub(super) fn turn(updates: Vec<AcpUpdate>) -> AcpTurn {
    AcpTurn {
        updates,
        stop_reason: "end_turn".to_string(),
    }
}

fn turn_with_stop_reason(updates: Vec<AcpUpdate>, stop_reason: &str) -> AcpTurn {
    AcpTurn {
        updates,
        stop_reason: stop_reason.to_string(),
    }
}

#[test]
fn a_max_turn_requests_stop_is_the_tool_step_cap() {
    // Issue #1853 established that a stop must not fold identically to a
    // clean end_turn — the operator needs a cap signal. PR #1880 review:
    // `max_turn_requests` is ACP's analog of openhuman's tool-iteration
    // cap, and is the only stop reason that may set `hit_iteration_cap`,
    // because `workflows/caps` reports that flag as "stopped at the
    // max_tool_iterations cap".
    let outcome = fold(turn_with_stop_reason(vec![], "max_turn_requests"));
    assert!(
        outcome.hit_iteration_cap,
        "max_turn_requests is the tool-step cap, not a clean finish"
    );
    assert!(
        !outcome.reply.trim().is_empty(),
        "a capped turn must say so, not fold to a blank reply"
    );
}

#[test]
fn a_max_tokens_stop_is_not_the_tool_step_cap() {
    // A token-generation budget on a single response is a different cap
    // than the tool-iteration one (PR #1880 review) — conflating them
    // would make a workflow node's `LimitStop{"max_tool_iterations"}`
    // misreport which cap actually stopped the turn.
    let outcome = fold(turn_with_stop_reason(vec![], "max_tokens"));
    assert!(
        !outcome.hit_iteration_cap,
        "a max_tokens stop is not the tool-iteration cap"
    );
    assert!(
        !outcome.reply.trim().is_empty(),
        "a capped turn must say so, not fold to a blank reply"
    );
}

#[test]
fn acp_results_are_reduced_to_shape_not_remote_text() {
    let secret = "API key: do-not-publish";
    let outcome = fold(turn(vec![
        AcpUpdate::ToolCall {
            id: "t".into(),
            title: "Read".into(),
        },
        AcpUpdate::ToolCallUpdate {
            id: "t".into(),
            status: "completed".into(),
            result: Some(secret.into()),
        },
    ]));
    assert_eq!(outcome.steps[0].result.as_deref(), Some("23 characters"));
    assert!(!outcome.steps[0].result.as_deref().unwrap().contains(secret));
}
#[test]
fn a_tool_only_turn_gets_a_generic_reply_not_raw_tool_titles() {
    // No MessageChunk at all — the agent's entire turn was tool calls.
    // PR #1880 review: the reply must not copy the tools' raw ACP titles
    // — unlike the built-in harness's step label, a title comes straight
    // off the wire with no host-side bounding, and the timeline (already
    // carrying each ToolCall step's own title) is where that content
    // belongs, not a field meant to read as the agent's own words.
    let outcome = fold(turn(vec![
        AcpUpdate::ToolCall {
            id: "t1".into(),
            title: "Read".into(),
        },
        AcpUpdate::ToolCallUpdate {
            id: "t1".into(),
            status: "completed".into(),
            result: Some("2.4 kB".into()),
        },
        AcpUpdate::ToolCall {
            id: "t2".into(),
            title: "Write".into(),
        },
    ]));
    assert_eq!(outcome.reply, "[no reply text — see steps]");
    assert_eq!(outcome.steps[0].label, "Read");
    assert_eq!(outcome.steps[1].label, "Write");
    // A clean end_turn needs no stop-reason note on top of the synthesis.
    assert!(!outcome.reply.contains("[stopped"));
}

#[test]
fn a_refusal_is_surfaced_as_a_note_step_and_the_cap_stays_false() {
    // The agent had prose to say, then declined to continue. The note
    // must land regardless — a refusal is not a clean finish even when
    // there is a reply to read. PR #1880 review: it lands as a `Note`
    // step, not appended onto the agent's own reply text.
    let outcome = fold(turn_with_stop_reason(
        vec![AcpUpdate::MessageChunk("I can't help with that.".into())],
        "refusal",
    ));
    assert_eq!(
        outcome.reply, "I can't help with that.",
        "the agent's own prose is kept verbatim, with nothing appended"
    );
    assert!(
        outcome.steps.iter().any(|s| s.kind == TurnStepKind::Note
            && s.label == "[stopped: the agent declined to continue]"),
        "the refusal must be surfaced as a step, not silently swallowed: {:?}",
        outcome.steps
    );
    assert!(
        !outcome.hit_iteration_cap,
        "a refusal is not an iteration-cap pause"
    );
    // PR #1880 review: `hit_iteration_cap == false` used to be the only
    // signal `HarnessAgentRunner` read, so a refusal settled a workflow
    // node `Succeeded`/`Finished` — indistinguishable from the agent
    // having actually answered. This is the outcome-level fix, not just
    // the note above: see `workflows::caps::mod::test::an_abnormal_acp_stop_fails_the_workflow_node`
    // for the assertion that it actually stops the graph.
    assert_eq!(
        outcome.abnormal_stop.as_deref(),
        Some("[stopped: the agent declined to continue]"),
        "a refusal must carry a distinct abnormal-stop outcome, not just a note"
    );
}

#[test]
fn a_cancelled_turn_also_carries_an_abnormal_stop() {
    // Same shape as refusal, different trigger: an operator-initiated (or
    // upstream) cancel is just as much "not a resumable cap, not a clean
    // finish" as a refusal is.
    let outcome = fold(turn_with_stop_reason(vec![], "cancelled"));
    assert_eq!(
        outcome.abnormal_stop.as_deref(),
        Some("[stopped: cancelled before finishing]")
    );
    assert!(!outcome.hit_iteration_cap);
}

#[test]
fn an_end_turn_reply_is_left_verbatim() {
    // The ordinary case — and the one the pre-existing seam test already
    // pins — must not gain a note or any other alteration just because
    // this fold now reads `stop_reason`.
    let outcome = fold(turn(vec![AcpUpdate::MessageChunk("all done".into())]));
    assert_eq!(outcome.reply, "all done");
    assert!(!outcome.hit_iteration_cap);
    assert_eq!(
        outcome.abnormal_stop, None,
        "a clean end_turn is not an abnormal stop"
    );
}

#[test]
fn a_max_turn_requests_stop_is_a_cap_not_an_abnormal_stop() {
    // The cap path (issue #926 / #1880's `hit_iteration_cap` split) and
    // the abnormal-stop path (this PR's review) are deliberately
    // disjoint: a capped turn has a real, resumable checkpoint, which is
    // exactly what `abnormal_stop` says there is none of.
    let outcome = fold(turn_with_stop_reason(vec![], "max_turn_requests"));
    assert!(outcome.hit_iteration_cap);
    assert_eq!(
        outcome.abnormal_stop, None,
        "the cap flag already covers this stop; abnormal_stop must stay None"
    );
}

#[test]
fn an_unrecognized_stop_reason_is_surfaced_not_swallowed() {
    // A stop_reason this fold has never heard of must not silently pass
    // for a clean end_turn — it is carried into a note step so the
    // operator (and whoever reads the ticket) can see the turn stopped
    // abnormally.
    //
    // PR #1880 review: the raw string itself must NOT appear — an
    // unrecognized `stopReason` is unvalidated, unbounded text straight
    // off the wire from an external ACP agent, and this note step is not
    // a private log line: `workflows/caps::transcript_from_steps` maps a
    // `Note` step to `"agent_message"` in the engine transcript, which
    // can be replayed as prior context for later engine reasoning. The
    // fixed notice below carries the abnormal-stop signal without
    // reopening that channel.
    let raw = "some_new_reason_acp_added_later__with_diagnostic_junk_🔥";
    let outcome = fold(turn_with_stop_reason(
        vec![AcpUpdate::MessageChunk("partial thought".into())],
        raw,
    ));
    assert_eq!(outcome.reply, "partial thought");
    assert!(
        outcome
            .steps
            .iter()
            .any(|s| s.kind == TurnStepKind::Note
                && s.label == "[stopped: unrecognized stop reason]"),
        "an unrecognized stop must still be surfaced as a step: {:?}",
        outcome.steps
    );
    assert!(
        outcome.steps.iter().all(|s| !s.label.contains(raw)),
        "the raw wire value must never appear in a persisted step: {:?}",
        outcome.steps
    );
    assert!(!outcome.hit_iteration_cap);
    assert_eq!(
        outcome.abnormal_stop.as_deref(),
        Some("[stopped: unrecognized stop reason]"),
        "an unrecognized stop must carry a distinct abnormal-stop outcome, not just a note"
    );
    assert!(
        !outcome
            .abnormal_stop
            .as_deref()
            .unwrap_or_default()
            .contains(raw),
        "the raw wire value must never appear in the abnormal-stop message either"
    );
}

#[test]
fn classify_stop_reason_maps_the_known_shapes() {
    assert_eq!(classify_stop_reason("end_turn"), StopKind::EndTurn);
    assert_eq!(classify_stop_reason("max_tokens"), StopKind::MaxTokens);
    assert_eq!(
        classify_stop_reason("max_turn_requests"),
        StopKind::MaxTurnRequests
    );
    assert_eq!(classify_stop_reason("refusal"), StopKind::Refusal);
    assert_eq!(classify_stop_reason("cancelled"), StopKind::Cancelled);
    assert_eq!(classify_stop_reason("anything_else"), StopKind::Other);
    assert_eq!(classify_stop_reason(""), StopKind::Other);
}

#[test]
fn message_chunks_concatenate_in_order() {
    // ACP streams a reply in pieces; the outcome carries one string.
    let outcome = fold(turn(vec![
        AcpUpdate::MessageChunk("Hello".into()),
        AcpUpdate::MessageChunk(", ".into()),
        AcpUpdate::MessageChunk("world".into()),
    ]));
    assert_eq!(outcome.reply, "Hello, world");
    assert!(outcome.steps.is_empty(), "text alone produces no steps");
}

#[test]
fn a_run_of_thoughts_becomes_one_step() {
    // A model emits these by the hundred. One step per chunk would bury the
    // tool calls an operator is actually reading the timeline for.
    let outcome = fold(turn(vec![
        AcpUpdate::ThoughtChunk,
        AcpUpdate::ThoughtChunk,
        AcpUpdate::ThoughtChunk,
    ]));
    assert_eq!(outcome.steps.len(), 1);
    assert_eq!(outcome.steps[0].kind, TurnStepKind::Thinking);
    assert_eq!(outcome.steps[0].label, "Thinking");
}

#[test]
fn thinking_resumes_as_a_new_step_after_a_tool_call() {
    // Two separate bouts of reasoning either side of a call are two steps —
    // coalescing them would put the thinking in the wrong order relative to
    // the work it bracketed.
    let outcome = fold(turn(vec![
        AcpUpdate::ThoughtChunk,
        AcpUpdate::ToolCall {
            id: "t1".into(),
            title: "Read".into(),
        },
        AcpUpdate::ThoughtChunk,
    ]));
    let kinds: Vec<_> = outcome.steps.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TurnStepKind::Thinking,
            TurnStepKind::ToolCall,
            TurnStepKind::Thinking
        ]
    );
}

#[test]
fn a_tool_call_takes_its_final_status_and_result() {
    let outcome = fold(turn(vec![
        AcpUpdate::ToolCall {
            id: "t1".into(),
            title: "Read a file".into(),
        },
        AcpUpdate::ToolCallUpdate {
            id: "t1".into(),
            status: "completed".into(),
            result: Some("2.4 kB".into()),
        },
    ]));
    assert_eq!(outcome.steps.len(), 1, "the update amends, never appends");
    assert_eq!(outcome.steps[0].label, "Read a file");
    assert_eq!(outcome.steps[0].status, TurnStepStatus::Ok);
    assert_eq!(outcome.steps[0].result.as_deref(), Some("6 characters"));
}

#[test]
fn a_failed_tool_call_is_an_error_step() {
    let outcome = fold(turn(vec![
        AcpUpdate::ToolCall {
            id: "t1".into(),
            title: "Write".into(),
        },
        AcpUpdate::ToolCallUpdate {
            id: "t1".into(),
            status: "failed".into(),
            result: Some("permission denied".into()),
        },
    ]));
    assert_eq!(outcome.steps[0].status, TurnStepStatus::Error);
    assert!(outcome.steps[0].status.is_failure());
}

#[test]
fn a_tool_call_that_never_completes_stays_running() {
    // Exactly what `Running` means: started, no completion seen by the end
    // of the turn. Marking it `Ok` would report work that never finished as
    // having succeeded.
    let outcome = fold(turn(vec![AcpUpdate::ToolCall {
        id: "t1".into(),
        title: "Long thing".into(),
    }]));
    assert_eq!(outcome.steps[0].status, TurnStepStatus::Running);
}

#[test]
fn several_tool_calls_are_amended_independently() {
    // Interleaved calls are ordinary — an agent starts two and they finish
    // out of order. Each update has to find its own step.
    let outcome = fold(turn(vec![
        AcpUpdate::ToolCall {
            id: "a".into(),
            title: "First".into(),
        },
        AcpUpdate::ToolCall {
            id: "b".into(),
            title: "Second".into(),
        },
        AcpUpdate::ToolCallUpdate {
            id: "b".into(),
            status: "completed".into(),
            result: None,
        },
        AcpUpdate::ToolCallUpdate {
            id: "a".into(),
            status: "failed".into(),
            result: None,
        },
    ]));
    assert_eq!(outcome.steps.len(), 2);
    assert_eq!(outcome.steps[0].label, "First");
    assert_eq!(outcome.steps[0].status, TurnStepStatus::Error);
    assert_eq!(outcome.steps[1].label, "Second");
    assert_eq!(outcome.steps[1].status, TurnStepStatus::Ok);
}

#[test]
fn an_update_for_an_unknown_call_is_dropped_rather_than_invented() {
    // A step with no label is worse on a timeline than no step at all.
    let outcome = fold(turn(vec![AcpUpdate::ToolCallUpdate {
        id: "ghost".into(),
        status: "completed".into(),
        result: Some("x".into()),
    }]));
    assert!(outcome.steps.is_empty());
}
