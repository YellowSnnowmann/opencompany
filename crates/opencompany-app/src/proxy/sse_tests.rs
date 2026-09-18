use super::*;

#[test]
fn a_whole_event_in_one_chunk_is_delivered() {
    let mut decoder = SseDecoder::new();
    assert_eq!(
        decoder.push("data: {\"type\":\"agent_reply\"}\n\n"),
        vec!["{\"type\":\"agent_reply\"}"]
    );
}

#[test]
fn a_codepoint_split_across_chunks_survives() {
    // THE reason `push_bytes` exists. A read boundary can fall inside a
    // multi-byte codepoint just as easily as between two lines. Decoding
    // each chunk with `from_utf8_lossy` first turns both halves into
    // U+FFFD, so an agent reply containing an emoji or accented text
    // arrives mangled — silently, and only under load.
    let payload = "data: {\"text\":\"héllo 🚀 done\"}\n\n";
    let bytes = payload.as_bytes();

    // Split at every byte offset, so the boundary lands mid-codepoint for
    // every multi-byte character in the payload.
    for split in 1..bytes.len() {
        let mut decoder = SseDecoder::new();
        let mut events = decoder.push_bytes(&bytes[..split]);
        events.extend(decoder.push_bytes(&bytes[split..]));
        assert_eq!(
            events,
            vec!["{\"text\":\"héllo 🚀 done\"}"],
            "split at byte {split} corrupted the payload"
        );
    }
}

#[test]
fn malformed_bytes_still_degrade_rather_than_stall() {
    // Truncation is held for the next chunk; a sequence that is simply
    // invalid is not, because the rest of it is never coming and waiting
    // would stop the stream.
    let mut decoder = SseDecoder::new();
    let mut raw = b"data: ".to_vec();
    raw.push(0xff);
    raw.extend_from_slice(b"\n\n");
    assert_eq!(decoder.push_bytes(&raw), vec!["\u{fffd}"]);
}

#[test]
fn an_event_split_across_chunks_is_reassembled() {
    // THE reason this is a chunk decoder. A read boundary lands wherever
    // the network puts it, and eventually that is mid-payload. A
    // line-oriented reader works until it doesn't, under load, once.
    let mut decoder = SseDecoder::new();
    assert!(decoder.push("data: {\"ty").is_empty());
    assert!(decoder.push("pe\":\"tool_call\"}").is_empty());
    assert_eq!(decoder.push("\n\n"), vec!["{\"type\":\"tool_call\"}"]);
}

#[test]
fn several_events_in_one_chunk_all_arrive() {
    let mut decoder = SseDecoder::new();
    assert_eq!(
        decoder.push("data: one\n\ndata: two\n\n"),
        vec!["one", "two"]
    );
}

#[test]
fn keep_alive_comments_are_not_delivered_as_data() {
    // Hosts ping with a bare comment to hold the connection open. Handing
    // that to the console as a message would make it try to `JSON.parse` a
    // heartbeat on every tick.
    let mut decoder = SseDecoder::new();
    assert!(decoder.push(": keep-alive\n\n").is_empty());
    assert_eq!(decoder.push("data: real\n\n"), vec!["real"]);
}

#[test]
fn crlf_line_endings_parse_the_same() {
    let mut decoder = SseDecoder::new();
    assert_eq!(decoder.push("data: windows\r\n\r\n"), vec!["windows"]);
}

#[test]
fn multi_line_data_joins_with_newlines() {
    // The format's own rule. A payload containing a newline arrives as two
    // `data:` lines and has to be rejoined, or the JSON is truncated.
    let mut decoder = SseDecoder::new();
    assert_eq!(
        decoder.push("data: {\ndata: \"a\": 1}\n\n"),
        vec!["{\n\"a\": 1}"]
    );
}

#[test]
fn a_named_event_is_dropped_rather_than_promoted() {
    let mut decoder = SseDecoder::new();
    assert!(decoder.push("event: ping\ndata: nope\n\n").is_empty());
    // An explicit `message` is the default type, so it does arrive.
    assert_eq!(decoder.push("event: message\ndata: yes\n\n"), vec!["yes"]);
}

#[test]
fn id_and_retry_fields_do_not_become_data() {
    let mut decoder = SseDecoder::new();
    assert_eq!(
        decoder.push("id: 42\nretry: 1000\ndata: payload\n\n"),
        vec!["payload"]
    );
}

#[test]
fn a_frame_with_no_data_yields_nothing() {
    let mut decoder = SseDecoder::new();
    assert!(decoder.push("id: 7\n\n").is_empty());
}

#[test]
fn a_value_keeps_its_internal_colons() {
    // JSON is full of colons; splitting on all of them would truncate every
    // payload after the first key.
    let mut decoder = SseDecoder::new();
    assert_eq!(
        decoder.push("data: {\"url\":\"https://x.test/a\"}\n\n"),
        vec!["{\"url\":\"https://x.test/a\"}"]
    );
}
