use super::*;

fn call(id: &str, label: &str) -> TurnStreamEvent {
    TurnStreamEvent {
        kind: "tool_call",
        seq: 1,
        tool_call_id: Some(id.to_string()),
        label: Some(label.to_string()),
        ..TurnStreamEvent::default()
    }
}

fn result(id: &str, status: &'static str) -> TurnStreamEvent {
    TurnStreamEvent {
        kind: "tool_result",
        seq: 2,
        tool_call_id: Some(id.to_string()),
        status: Some(status),
        ..TurnStreamEvent::default()
    }
}

#[test]
fn a_started_call_becomes_a_pending_tool_call() {
    let update = from_turn_stream(&call("t1", "Read the roster")).unwrap();
    assert_eq!(update["sessionUpdate"], "tool_call");
    assert_eq!(update["toolCallId"], "t1");
    assert_eq!(update["title"], "Read the roster");
    assert_eq!(update["status"], "pending");
}

#[test]
fn arguments_never_reach_the_raw_input_field() {
    // `detail` is a redacted one-liner. In `rawInput` it would claim to be
    // the real arguments, which is how a scrubbed value gets rendered as
    // truth — and the scrubbing exists precisely because the real ones must
    // not leave the host.
    let mut event = call("t1", "Send mail");
    event.detail = Some("to: <redacted>".to_string());
    let update = from_turn_stream(&event).unwrap();

    assert!(update.get("rawInput").is_none());
    assert_eq!(update["_meta"]["opencompany/detail"], "to: <redacted>");
}

#[test]
fn a_finished_call_amends_by_id_with_its_summary() {
    let mut event = result("t1", "ok");
    event.result = Some("12 items".to_string());
    let update = from_turn_stream(&event).unwrap();

    assert_eq!(update["sessionUpdate"], "tool_call_update");
    assert_eq!(update["toolCallId"], "t1");
    assert_eq!(update["status"], "completed");
    assert_eq!(update["content"][0]["content"]["text"], "12 items");
}

#[test]
fn a_failed_call_is_reported_as_failed() {
    let update = from_turn_stream(&result("t1", "error")).unwrap();
    assert_eq!(update["status"], "failed");
    assert!(
        update["_meta"]
            .get("opencompany/awaitingApproval")
            .is_none()
    );
}

#[test]
fn a_parked_call_is_distinguishable_from_a_failed_one() {
    // ACP has four statuses and none of them mean "waiting on a person".
    // Collapsing a park into a plain failure would render the one state an
    // operator can act on as a crash — the exact mistake #411 fixed in the
    // console's own timeline.
    let update = from_turn_stream(&result("t1", "awaiting_approval")).unwrap();
    assert_eq!(update["status"], "failed", "ACP has nothing better");
    assert_eq!(update["_meta"]["opencompany/awaitingApproval"], true);
}

#[test]
fn a_truncated_result_says_so() {
    let mut event = result("t1", "ok");
    event.truncated = true;
    let update = from_turn_stream(&event).unwrap();
    assert_eq!(update["_meta"]["opencompany/truncated"], true);
}

#[test]
fn a_thinking_marker_produces_no_empty_bubble() {
    // This host streams no reasoning text, so an `agent_thought_chunk`
    // would carry nothing and render as a blank bubble every turn.
    let event = TurnStreamEvent {
        kind: "thinking",
        ..TurnStreamEvent::default()
    };
    assert!(from_turn_stream(&event).is_none());
}

#[test]
fn the_notification_envelope_is_well_formed() {
    let wrapped = notification("sess-1", json!({ "sessionUpdate": "tool_call" }));
    assert_eq!(wrapped["jsonrpc"], "2.0");
    assert_eq!(wrapped["method"], "session/update");
    // A notification carries no id, or a conforming client tries to reply.
    assert!(wrapped.get("id").is_none());
    assert_eq!(wrapped["params"]["sessionId"], "sess-1");
}
