use super::*;

#[test]
fn a_request_is_distinguished_from_a_notification_by_its_id() {
    // The whole classification, in one test. Getting it backwards means
    // either awaiting a reply that will never come, or replying to
    // something that must not be replied to.
    let request = decode(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).unwrap();
    assert!(matches!(request, Message::Request { .. }));

    let notification =
        decode(r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s"}}"#)
            .unwrap();
    assert!(matches!(notification, Message::Notification { .. }));
}

#[test]
fn a_string_id_round_trips() {
    // JSON-RPC allows either; an implementation that assumed numbers would
    // fail to correlate replies from an agent that uses strings.
    let encoded = encode_request(&RequestId::Text("abc".into()), "session/new", Value::Null);
    match decode(encoded.trim()).unwrap() {
        Message::Request { id, method, .. } => {
            assert_eq!(id, RequestId::Text("abc".into()));
            assert_eq!(method, "session/new");
        }
        other => panic!("expected a request, got {other:?}"),
    }
}

#[test]
fn a_notification_carries_no_id_on_the_wire() {
    // Not cosmetic: an `id` here makes a conforming agent reply, and a
    // cancel that waits for a reply is a cancel that hangs.
    let encoded = encode_notification("session/cancel", serde_json::json!({"sessionId": "s"}));
    let parsed: Value = serde_json::from_str(encoded.trim()).unwrap();
    assert!(parsed.get("id").is_none());
    assert_eq!(parsed["jsonrpc"], "2.0");
}

#[test]
fn a_response_is_recognised_and_correlated() {
    let decoded = decode(r#"{"jsonrpc":"2.0","id":7,"result":{"sessionId":"s1"}}"#).unwrap();
    match decoded {
        Message::Response { id, result } => {
            assert_eq!(id, RequestId::Number(7));
            assert_eq!(result["sessionId"], "s1");
        }
        other => panic!("expected a response, got {other:?}"),
    }
}

#[test]
fn an_error_response_keeps_its_code_and_message() {
    let decoded =
        decode(r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"no such method"}}"#)
            .unwrap();
    match decoded {
        Message::Error { id, code, message } => {
            assert_eq!(id, Some(RequestId::Number(3)));
            assert_eq!(code, -32601);
            assert_eq!(message, "no such method");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn absent_and_null_params_are_the_same_thing() {
    let absent = decode(r#"{"jsonrpc":"2.0","method":"x"}"#).unwrap();
    let null = decode(r#"{"jsonrpc":"2.0","method":"x","params":null}"#).unwrap();
    assert_eq!(absent, null);
}

#[test]
fn every_encoding_is_exactly_one_line() {
    // A message containing a newline would desynchronise the reader for the
    // rest of the session — every later message would be misframed.
    let with_newlines = serde_json::json!({ "text": "one\ntwo\nthree" });
    for encoded in [
        encode_request(
            &RequestId::Number(1),
            "session/prompt",
            with_newlines.clone(),
        ),
        encode_notification("session/update", with_newlines.clone()),
        encode_response(&RequestId::Number(1), with_newlines.clone()),
        encode_error(&RequestId::Number(1), -32000, "bad\nthing"),
    ] {
        assert_eq!(encoded.matches('\n').count(), 1, "{encoded:?}");
        assert!(encoded.ends_with('\n'));
        // And it still parses back, so the escaping is real rather than
        // stripped.
        assert!(decode(encoded.trim()).is_ok());
    }
}

#[test]
fn an_unroutable_message_is_refused_rather_than_guessed() {
    // No method and no id: nothing can be done with it, and inventing a
    // classification would put a bogus entry in the pending-request table.
    assert!(decode(r#"{"jsonrpc":"2.0"}"#).is_err());
    assert!(decode("not json at all").is_err());
    assert!(decode("[]").is_err());
}
