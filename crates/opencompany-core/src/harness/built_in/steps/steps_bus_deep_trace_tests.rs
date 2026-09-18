use super::steps_fixtures_tests::*;
use super::*;

// The incremental trace stays identical to the fold
// -----------------------------------------------------------------------

/// Materializes a [`StepTrace`] over `events` the way the run store does:
/// each yield writes to its ordinal, replacing whatever was there.
fn materialize(events: &[AgentProgress]) -> Vec<TurnStep> {
    let mut trace = StepTrace::default();
    let mut rows: Vec<Option<TurnStep>> = Vec::new();
    for event in events {
        for (seq, step, _) in trace.push(event) {
            let idx = seq as usize;
            if rows.len() <= idx {
                rows.resize(idx + 1, None);
            }
            rows[idx] = Some(step);
        }
    }
    assert_eq!(
        rows.len() as u32,
        trace.emitted(),
        "ordinals must be dense — a gap means a row nothing ever writes"
    );
    rows.into_iter()
        .map(|row| row.expect("every claimed ordinal is written"))
        .collect()
}

/// Issue #242: the incremental trace and the final fold are the SAME
/// timeline — which is what makes the chat bubble and the Attempts tab tell
/// one story. #411 widened what a step carries, so the park and the cut
/// result are in here too: a field enriched on one path only would split
/// the two surfaces apart again.
#[test]
fn incremental_trace_converges_on_the_folded_timeline() {
    let events = vec![
        thinking("hmm"),
        thinking("still hmm"),
        started("c1", "mcp_call_tool", Some("Searching the web")),
        completed(
            "c1",
            "mcp_call_tool",
            true,
            "ok",
            Some(serde_json::json!({
                "server": "brave", "tool": "search",
                "arguments": { "query": "rust async" }
            })),
            None,
        ),
        text("here you go"),
        thinking("again"),
        started("c2", "spawn_task", None),
        completed(
            "c2",
            "spawn_task",
            false,
            "boom",
            None,
            Some(classified(ToolFailureClass::Timeout, "it timed out")),
        ),
        // A park.
        started("c3", "send_email", Some("Send email")),
        completed(
            "c3",
            "send_email",
            false,
            &approval_refusal("send_email"),
            Some(serde_json::json!({ "to": "a@b.test" })),
            None,
        ),
        // A cut result.
        completed(
            "c4",
            "composio_list_tools",
            true,
            "…\n\n[truncated by tool cap: 900 more chars not shown]",
            None,
            None,
        ),
        // A completion whose start was never observed — the standalone arm.
        completed("c5", "query_company", true, "ok", None, None),
    ];

    assert_eq!(materialize(&events), fold_steps(events.clone()));
}

/// The one deliberate divergence, stated as a property rather than left to
/// be discovered: a tool call still in flight when the stream ends is
/// persisted `Running`.
#[test]
fn an_unfinished_tool_call_is_persisted_as_running() {
    let events = vec![started("c1", "mcp_call_tool", Some("Searching the web"))];
    let rows = materialize(&events);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, TurnStepStatus::Running);
    assert_eq!(rows[0].label, "Searching the web");
    assert_eq!(rows, fold_steps(events));
}

/// Ordinals are run-scoped, not turn-scoped: a second turn on the same
/// trace continues where the first stopped.
#[test]
fn ordinals_continue_across_turns_of_one_run() {
    let mut trace = StepTrace::default();
    let first = trace.push(&started("c1", "spawn_task", None));
    assert_eq!(first.len(), 1, "turn 1 step");
    assert_eq!(first[0].0, 0);
    let second = trace.push(&started("c9", "spawn_task", None));
    assert_eq!(second.len(), 1, "turn 2 step");
    assert_eq!(second[0].0, 1, "turn 2 must not reuse turn 1's ordinals");
    assert_eq!(trace.emitted(), 2);
}

// -----------------------------------------------------------------------
// The live bus carries the same projection
// -----------------------------------------------------------------------

#[test]
fn stream_event_from_maps_start_and_completion() {
    let mut thinking_open = false;
    let start = stream_event_from(
        &started("c1", "mcp_call_tool", Some("Searching")),
        0,
        &mut thinking_open,
    )
    .expect("start maps to a frame");
    assert_eq!(start.kind, "tool_call");
    assert_eq!(start.status, Some("running"));
    assert_eq!(start.tool_call_id.as_deref(), Some("c1"));
    assert_eq!(start.label.as_deref(), Some("Searching"));

    let done = stream_event_from(
        &completed(
            "c1",
            "mcp_call_tool",
            true,
            "[1,2]",
            Some(serde_json::json!({ "server": "brave", "tool": "search" })),
            None,
        ),
        1,
        &mut thinking_open,
    )
    .expect("completion maps to a frame");
    assert_eq!(done.kind, "tool_result");
    assert_eq!(done.status, Some("ok"));
    assert_eq!(done.tool_call_id.as_deref(), Some("c1"));
    assert_eq!(done.detail.as_deref(), Some("brave · search"));
    assert_eq!(done.result.as_deref(), Some("2 items"));
    assert_eq!(done.elapsed_ms, Some(42));
}

/// The live row must reach the same verdict the folded step does — a park
/// that reads as an error for the seconds before the reply lands is the same
/// bug, just briefer.
#[test]
fn stream_event_from_reports_a_park_as_a_park() {
    let frame = stream_event_from(
        &completed(
            "c1",
            "send_email",
            false,
            &approval_refusal("send_email"),
            None,
            None,
        ),
        0,
        &mut false,
    )
    .expect("frame");
    assert_eq!(frame.status, Some("awaiting_approval"));
    assert_eq!(frame.failure, None);
    assert_eq!(frame.result.as_deref(), Some(AWAITING_APPROVAL_RESULT));
}

#[test]
fn stream_event_from_carries_the_typed_failure() {
    let frame = stream_event_from(
        &completed("c1", "mcp_call_tool", false, "401 unauthorized", None, None),
        0,
        &mut false,
    )
    .expect("frame");
    assert_eq!(frame.status, Some("error"));
    assert_eq!(frame.failure, Some(TurnStepFailure::Unauthorized));
}

#[test]
fn stream_event_from_coalesces_thinking_and_ignores_text() {
    let mut open = false;
    let first = stream_event_from(&thinking("hmm"), 0, &mut open).expect("first delta → frame");
    assert_eq!(first.kind, "thinking");
    assert_eq!(first.label.as_deref(), Some("Thinking"));
    assert!(open, "run is now open");
    assert!(stream_event_from(&thinking("more"), 1, &mut open).is_none());
    assert!(stream_event_from(&text("hello"), 2, &mut open).is_none());
    assert!(!open, "text closed the run");
    assert!(stream_event_from(&thinking("again"), 3, &mut open).is_some());
}

/// The live frame is scrubbed exactly like the folded step.
#[test]
fn stream_event_from_never_leaks_remote_output() {
    let frame = stream_event_from(
        &completed(
            "c2",
            "mcp_call_tool",
            false,
            &format!("401 token={FAKE_SECRET}"),
            Some(serde_json::json!({
                "server": "brave", "tool": "search",
                "arguments": { "api_key": FAKE_SECRET }
            })),
            None,
        ),
        0,
        &mut false,
    )
    .expect("frame");
    let json = serde_json::to_string(&frame).expect("frame serialize");
    assert!(
        !json.contains(FAKE_SECRET),
        "a planted secret leaked into a live turn-stream frame: {json}"
    );
}

// -----------------------------------------------------------------------
// Deep trace: the unredacted companion
// -----------------------------------------------------------------------

mod deep {
    use super::*;

    /// Drains a trace over `events`, returning every (ordinal, step, detail).
    fn run(deep: bool, events: &[AgentProgress]) -> Vec<(u32, TurnStep, Option<TurnStepDetail>)> {
        let mut trace = if deep {
            StepTrace::deep()
        } else {
            StepTrace::default()
        };
        events.iter().flat_map(|e| trace.push(e)).collect()
    }

    /// THE guarantee, and the mirror of
    /// `planted_secret_never_reaches_serialized_steps`: with deep trace on
    /// the raw output DOES reach the detail, and STILL never reaches a
    /// serialized step. If the second half ever fails, the scrubbed
    /// timeline has started disclosing raw output.
    ///
    /// Note this is about **output**, which is dropped unconditionally.
    /// Arguments are a weaker contract — `approval_display` redacts by KEY
    /// NAME, and its own module doc says "an unlisted key holding a secret
    /// is not" safe — so the argument half is asserted separately below
    /// against a denylisted key.
    #[test]
    fn raw_output_reaches_the_detail_and_never_the_step() {
        let emitted = run(
            true,
            &[
                started("c1", "shell", None),
                completed(
                    "c1",
                    "shell",
                    true,
                    &format!("printed {FAKE_SECRET}"),
                    Some(serde_json::json!({ "command": "run" })),
                    None,
                ),
            ],
        );

        let details = serde_json::to_string(
            &emitted
                .iter()
                .filter_map(|(_, _, d)| d.clone())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(
            details.contains(FAKE_SECRET),
            "the deep store is the whole point: {details}"
        );

        let steps = serde_json::to_string(
            &emitted
                .iter()
                .map(|(_, s, _)| s.clone())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(
            !steps.contains(FAKE_SECRET),
            "raw output must never reach the scrubbed timeline: {steps}"
        );
    }

    /// A denylisted argument key is masked on the step and intact in the
    /// detail — the two halves of the split, on one call.
    #[test]
    fn a_denylisted_argument_is_masked_on_the_step_and_kept_in_the_detail() {
        let emitted = run(
            true,
            &[completed(
                "c1",
                "shell",
                true,
                "ok",
                Some(serde_json::json!({ "token": FAKE_SECRET })),
                None,
            )],
        );

        let steps = serde_json::to_string(
            &emitted
                .iter()
                .map(|(_, s, _)| s.clone())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(
            !steps.contains(FAKE_SECRET),
            "a denylisted key must be redacted on the step: {steps}"
        );

        let detail = emitted
            .iter()
            .find_map(|(_, _, d)| d.clone())
            .expect("a completed call has detail");
        assert!(
            detail.arguments.as_deref().unwrap().contains(FAKE_SECRET),
            "the deep half keeps what the operator view masks"
        );
    }

    /// With deep trace OFF, the scrubbed projection is byte-identical to
    /// what deep mode produces — deep adds detail, it never changes steps.
    #[test]
    fn deep_off_changes_nothing_about_the_steps() {
        /// The rows a store would hold: last write per ordinal wins, exactly
        /// as `append_run_step` replaces on `(run_id, step_seq)`.
        fn settled(emitted: &[(u32, TurnStep, Option<TurnStepDetail>)]) -> Vec<(u32, TurnStep)> {
            let mut rows: Vec<(u32, TurnStep)> = Vec::new();
            for (seq, step, _) in emitted {
                match rows.iter_mut().find(|(s, _)| s == seq) {
                    Some(slot) => slot.1 = step.clone(),
                    None => rows.push((*seq, step.clone())),
                }
            }
            rows
        }

        let events = [
            thinking("pondering"),
            started("c1", "shell", None),
            completed("c1", "shell", true, "done", None, None),
            text("here you go"),
        ];
        let shallow = run(false, &events);
        let deep = run(true, &events);

        assert!(
            shallow.iter().all(|(_, _, d)| d.is_none()),
            "a shallow trace yields no details at all"
        );
        assert_eq!(
            settled(&shallow),
            settled(&deep),
            "deep mode must not change the scrubbed projection"
        );
    }

    #[test]
    fn reasoning_is_captured_and_coalesced_under_one_ordinal() {
        let emitted = run(
            true,
            &[
                thinking("first "),
                thinking("second "),
                thinking("third"),
                text("answer"),
            ],
        );
        // One thinking step, however many deltas fed it.
        let ordinals: std::collections::BTreeSet<u32> =
            emitted.iter().map(|(seq, _, _)| *seq).collect();
        assert_eq!(ordinals.len(), 1, "a thinking run is ONE step");

        // `push` emits the first delta once and later flushes only the NEW
        // bytes; the sink concatenates the per-flush chunks, so the stored
        // reasoning is the whole thought without the first chunk repeating.
        let reasoning: String = emitted
            .iter()
            .filter_map(|(_, _, d)| d.as_ref())
            .filter_map(|d| d.reasoning.clone())
            .collect();
        assert_eq!(reasoning, "first second third");
    }

    /// The bug the vec return exists to prevent: a tool call closing a
    /// thinking run must finalize that run's reasoning, not drop it.
    #[test]
    fn reasoning_survives_a_tool_call_closing_the_run() {
        let emitted = run(
            true,
            &[
                thinking("I should "),
                thinking("run the program"),
                started("c1", "shell", None),
            ],
        );
        let reasoning: Vec<String> = emitted
            .iter()
            .filter_map(|(_, _, d)| d.as_ref())
            .filter_map(|d| d.reasoning.clone())
            .collect();
        assert_eq!(
            reasoning.concat(),
            "I should run the program",
            "the tail before a tool call was lost: {reasoning:?}"
        );
    }

    #[test]
    fn a_thinking_run_that_said_nothing_writes_no_detail() {
        // An empty delta must not mint a row saying the agent thought
        // nothing.
        let emitted = run(true, &[thinking(""), text("hi")]);
        assert!(
            emitted.iter().all(|(_, _, d)| d
                .as_ref()
                .is_none_or(|d| d.reasoning.is_none() || d.reasoning.as_deref() == Some(""))),
            "an empty thought produced a reasoning row"
        );
    }

    /// The EOF path: a turn that ends mid-thought has no `TextDelta` or tool
    /// call to close the run, so the tail below the interim flush threshold
    /// survives only because the collector calls [`StepTrace::finish`] when
    /// the stream drains.
    #[test]
    fn an_aborted_thought_is_flushed_when_the_trace_finishes() {
        let mut trace = StepTrace::deep();
        let mut emitted = Vec::new();
        emitted.extend(trace.push(&thinking("first ")));
        emitted.extend(trace.push(&thinking("second"))); // under DEEP_THINK_FLUSH_BYTES
        // No text, no tool call — the turn just ends.
        emitted.extend(trace.finish());

        let reasoning: String = emitted
            .iter()
            .filter_map(|(_, _, d)| d.as_ref())
            .filter_map(|d| d.reasoning.clone())
            .collect();
        assert_eq!(
            reasoning, "first second",
            "the tail of an aborted thought was dropped: {reasoning:?}"
        );
    }

    /// A flush on a trace with nothing open is a no-op — in particular it
    /// must not claim an ordinal or mint a step.
    #[test]
    fn finish_with_nothing_open_yields_nothing() {
        let mut trace = StepTrace::deep();
        assert!(
            trace.finish().is_empty(),
            "an idle trace must not emit on finish"
        );
        assert_eq!(trace.emitted(), 0);
        // A thought already closed by text has nothing left to flush.
        let mut closed = StepTrace::deep();
        closed.push(&thinking("done"));
        closed.push(&text("answer"));
        assert!(
            closed.finish().is_empty(),
            "a closed thinking run must not re-emit on finish"
        );
    }

    #[test]
    fn a_completed_call_carries_raw_arguments_and_output() {
        let emitted = run(
            true,
            &[completed(
                "c1",
                "shell",
                true,
                "837799\n",
                Some(serde_json::json!({ "command": "python3 solve.py" })),
                None,
            )],
        );
        let detail = emitted
            .iter()
            .find_map(|(_, _, d)| d.clone())
            .expect("a completed call has detail");
        assert_eq!(detail.output.as_deref(), Some("837799\n"));
        assert!(
            detail.arguments.as_deref().unwrap().contains("solve.py"),
            "{:?}",
            detail.arguments
        );
    }

    #[test]
    fn a_started_call_carries_no_arguments() {
        // Documented upstream: the tinyagents path sends `Null` on the
        // started event and real arguments only on completion. Pinning it
        // so a future change upstream shows up here rather than as a
        // mysteriously empty argument pane.
        let emitted = run(true, &[started("c1", "shell", None)]);
        let detail = emitted.iter().find_map(|(_, _, d)| d.clone());
        assert!(
            detail.is_none_or(|d| d.arguments.is_none()),
            "a started call should carry no unredacted arguments"
        );
    }
}
