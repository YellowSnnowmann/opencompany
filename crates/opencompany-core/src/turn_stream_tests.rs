use super::*;
use futures::StreamExt;

/// Unwraps the turn frame a test just published.
///
/// Every assertion here predates the bus carrying anything but turns, and
/// panicking on the wrong variant is the right failure: a test that
/// published a turn frame and received a presence one has found a real bug.
fn turn(frame: LiveFrame) -> TurnStreamEvent {
    frame
        .as_turn()
        .cloned()
        .expect("this bus published a turn frame")
}

fn frame(kind: &'static str, seq: u64) -> TurnStreamEvent {
    TurnStreamEvent {
        kind,
        seq,
        agent_id: None,
        chat_id: None,
        tool_call_id: Some("c1".to_string()),
        label: Some("Searching".to_string()),
        status: Some("running"),
        ..TurnStreamEvent::default()
    }
}

/// A published frame reaches an already-subscribed console, agent stamp and
/// all. (Broadcast only delivers to receivers that existed at publish time,
/// so subscribe first — exactly the SSE route's order.)
/// The whole reason [`LiveFrame`] is `untagged`: widening the bus must not
/// change one byte of what a turn frame looks like on the wire, or every
/// console in the field starts ignoring frames it used to render.
#[test]
fn wrapping_a_turn_frame_in_the_union_does_not_change_its_wire_form() {
    let event = frame("tool_call", 7);
    let bare = serde_json::to_string(&event).expect("serialize");
    let wrapped = serde_json::to_string(&LiveFrame::from(event)).expect("serialize");
    assert_eq!(bare, wrapped);
}

#[test]
fn presence_and_typing_carry_their_own_type_discriminant() {
    let presence = serde_json::to_value(LiveFrame::Presence(PresenceFrame {
        kind: "presence",
        user_id: "u1".to_string(),
        status: "online",
        at_millis: 5,
    }))
    .expect("serialize");
    assert_eq!(presence["type"], "presence");
    assert_eq!(presence["userId"], "u1");
    // No label: the console already holds the directory that names people.
    assert!(presence.get("label").is_none());

    let typing = serde_json::to_value(LiveFrame::Typing(TypingFrame {
        kind: "typing",
        user_id: "u1".to_string(),
        chat_id: "eng".to_string(),
        parent_id: None,
        at_millis: 5,
    }))
    .expect("serialize");
    assert_eq!(typing["type"], "typing");
    assert_eq!(typing["chatId"], "eng");
    assert!(
        typing.get("parentId").is_none(),
        "a channel-level typing frame omits the thread key"
    );
}

#[tokio::test]
async fn publish_reaches_subscriber() {
    let company = CompanyId::new("turn-stream-roundtrip");
    let mut stream = subscribe(&company);
    publish(
        &company,
        frame("tool_call", 0).with_agent("ceo").with_chat("General"),
    );
    let got = turn(stream.next().await.expect("a frame arrives"));
    assert_eq!(got.kind, "tool_call");
    assert_eq!(got.seq, 0);
    assert_eq!(got.agent_id.as_deref(), Some("ceo"));
    // The chat thread rides along so the console routes the frame to the same
    // thread the durable reply lands on.
    assert_eq!(got.chat_id.as_deref(), Some("General"));
    assert_eq!(got.tool_call_id.as_deref(), Some("c1"));
}

/// Two turns run by the SAME agent on DIFFERENT chat threads keep their live
/// frames separable by `chatId` — the console keys the in-flight tool
/// timeline on that, so concurrent sends never cross-attribute (PR #125
/// review: routing must be per-thread, not a single global ref keyed on the
/// responding member, which is identical across both turns here).
#[tokio::test]
async fn concurrent_threads_same_agent_route_by_chat() {
    let company = CompanyId::new("turn-stream-concurrent");
    let mut stream = subscribe(&company);
    // Same responding agent ("ceo"), two distinct desk threads in flight.
    publish(
        &company,
        frame("tool_call", 0).with_agent("ceo").with_chat("General"),
    );
    publish(
        &company,
        frame("tool_call", 1)
            .with_agent("ceo")
            .with_chat("eng_desk"),
    );
    let a = turn(stream.next().await.expect("first frame"));
    let b = turn(stream.next().await.expect("second frame"));
    assert_eq!(a.agent_id.as_deref(), Some("ceo"));
    assert_eq!(b.agent_id.as_deref(), Some("ceo"));
    // agentId alone is ambiguous; chatId disambiguates the two threads.
    assert_eq!(a.chat_id.as_deref(), Some("General"));
    assert_eq!(b.chat_id.as_deref(), Some("eng_desk"));
    assert_ne!(a.chat_id, b.chat_id);
}

/// Issue #1702: a workflow agent node's frame carries the workflow run +
/// node instead of a chat thread, and serializes them as `workflowRunId` /
/// `nodeId`. This is what lets the console's run-trace sheet key the live
/// tool timeline on the run.
#[test]
fn with_workflow_stamps_run_and_node_on_the_wire() {
    let f = frame("tool_call", 3)
        .with_agent("researcher")
        .with_workflow("wfr-42", "summarise");
    assert_eq!(f.workflow_run_id.as_deref(), Some("wfr-42"));
    assert_eq!(f.node_id.as_deref(), Some("summarise"));
    // A workflow node has no chat thread, so `with_workflow` must not invent
    // one — the two routing dimensions are mutually exclusive.
    assert!(f.chat_id.is_none());

    let j = serde_json::to_value(&f).expect("serialize");
    assert_eq!(j["workflowRunId"], "wfr-42");
    assert_eq!(j["nodeId"], "summarise");
    assert!(
        j.get("chatId").is_none(),
        "a workflow-tagged frame carries no chatId"
    );
}

/// The tagging is additive: a chat turn's frame still stamps `chatId` and
/// omits the workflow ids entirely, so an existing chat console reads the
/// wire form byte-for-byte as it did before #1702.
#[test]
fn a_chat_frame_omits_the_workflow_ids() {
    let j = serde_json::to_value(frame("tool_call", 0).with_chat("General")).expect("serialize");
    assert_eq!(j["chatId"], "General");
    assert!(
        j.get("workflowRunId").is_none(),
        "a chat frame must not carry a workflowRunId"
    );
    assert!(
        j.get("nodeId").is_none(),
        "a chat frame must not carry a nodeId"
    );
}

/// A publish with no subscriber is a silent no-op — a turn streams whether or
/// not a console is watching.
#[test]
fn publish_without_subscriber_is_noop() {
    let company = CompanyId::new("turn-stream-nobody");
    publish(&company, frame("tool_call", 0)); // must not panic
}

/// A subscriber that lags past [`CAPACITY`] must skip the gap and keep
/// reading — never block, and never end the stream. `subscribe`'s `loop`
/// treats `RecvError::Lagged` as a reason to keep polling; only `Closed`
/// ends the stream, and `REGISTRY` holds a sender for the process
/// lifetime, so that never fires here.
#[tokio::test]
async fn a_lagging_subscriber_skips_dropped_frames_instead_of_ending() {
    let company = CompanyId::new("turn-stream-overflow");
    let mut stream = subscribe(&company);

    // Publish well past the ring's capacity with nobody draining it, so
    // the receiver's cursor falls behind the oldest frame still buffered.
    let overflow = CAPACITY as u64 + 50;
    for seq in 0..overflow {
        publish(&company, frame("tool_call", seq));
    }

    let got = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .expect("a lagging subscriber must not block forever")
        .expect("the stream must not end on a lag");
    let got = turn(got);
    assert!(
        got.seq >= overflow - CAPACITY as u64,
        "a lagging read must resume at the oldest frame still in the ring, not replay one \
         that was already dropped: got seq {}",
        got.seq
    );
}

/// The wire shape carries a `type` discriminant (so the console switches on
/// it alongside the durable projections), camelCases its keys, and omits
/// empty optionals.
#[test]
fn serializes_to_typed_camelcase_wire_shape() {
    let f = TurnStreamEvent {
        kind: "tool_result",
        seq: 2,
        agent_id: None,
        chat_id: None,
        tool_call_id: Some("c1".to_string()),
        label: Some("Search".to_string()),
        detail: Some("brave · search".to_string()),
        result: Some("12 items".to_string()),
        failure: None,
        truncated: false,
        status: Some("ok"),
        elapsed_ms: Some(12),
        message_seq: None,
        workflow_run_id: None,
        node_id: None,
    };
    let j = serde_json::to_value(f.with_chat("General")).expect("serialize");
    assert_eq!(j["type"], "tool_result");
    assert_eq!(j["toolCallId"], "c1");
    assert_eq!(j["elapsedMs"], 12);
    assert_eq!(j["status"], "ok");
    assert_eq!(j["chatId"], "General");
    assert!(j.get("agentId").is_none(), "empty optional omitted");
}

/// Two turns racing in one chat are tellable apart, and by nothing else.
///
/// `chatId` is the *conversation*; two questions asked in one channel share
/// it. So does `agentId` whenever the same desk answers both — which is the
/// common case, not a corner. Before `messageSeq` those were the only
/// routing keys a chat frame carried, so a console had no way to say which
/// question a row belonged to and merged both into one timeline.
///
/// The observed cost was worse than a merged list: the console holds a
/// single row-list per thread, so arming the second turn cleared the first
/// turn's rows outright — and a turn blocked on a teammate emits nothing
/// further, so its steps never came back.
#[test]
fn two_queries_in_one_chat_are_told_apart_by_message_seq() {
    let first = serde_json::to_value(
        frame("tool_call", 0)
            .with_chat("general")
            .with_agent("product_manager")
            .with_message_seq(Some(45)),
    )
    .expect("serialize");
    let second = serde_json::to_value(
        frame("tool_call", 0)
            .with_chat("general")
            .with_agent("product_manager")
            .with_message_seq(Some(49)),
    )
    .expect("serialize");

    // Every key they previously routed on agrees — including `seq`, which is
    // per-turn and so restarts at 0 for each of them.
    assert_eq!(first["chatId"], second["chatId"]);
    assert_eq!(first["agentId"], second["agentId"]);
    assert_eq!(first["seq"], second["seq"]);
    // The query does not.
    assert_eq!(first["messageSeq"], 45);
    assert_eq!(second["messageSeq"], 49);
    assert_ne!(first["messageSeq"], second["messageSeq"]);
}

/// Omitted, not null, on a turn answering no journaled message — a relay, a
/// dispatched card, a workflow node. A console that keys on its presence
/// falls back to the thread, which is what every console did before the
/// field existed, so an older one reads the wire form unchanged.
#[test]
fn a_turn_answering_no_message_carries_no_message_seq() {
    let j = serde_json::to_value(frame("tool_call", 0).with_chat("general")).expect("serialize");
    assert!(
        j.get("messageSeq").is_none(),
        "absent rather than null, so presence is the check: {j}"
    );
}
