use super::*;

/// The exact repro shape from #2094: a native model's own DSML-style
/// special-token wrapper around `tool_calls` / `invoke` / `parameter`.
#[test]
fn guard_replaces_the_reported_dsml_markup() {
    let leaked = "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>\n\
         <\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke name=\"workspace_search\">\n\
         <\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter name=\"query\" string=\"true\">team.md</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter>\n\
         </\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke>\n\
         </\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>"
        .to_string();
    assert_eq!(guard_suppressed_reply(leaked), FALLBACK_REPLY);
}

#[test]
fn guard_replaces_a_bare_tool_call_tag() {
    let leaked = r#"<tool_call id="call_1">{"name":"workspace_search","arguments":{}}</tool_call>"#
        .to_string();
    assert_eq!(guard_suppressed_reply(leaked), FALLBACK_REPLY);
}

#[test]
fn guard_replaces_an_invoke_tag() {
    let leaked = r#"<invoke name="workspace_search">{"query":"team.md"}</invoke>"#.to_string();
    assert_eq!(guard_suppressed_reply(leaked), FALLBACK_REPLY);
}

#[test]
fn guard_replaces_a_plain_text_function_call_marker() {
    let leaked =
        r#"function_call:{"call":"read_ledger","arguments":{"ledger":"tasks"}}"#.to_string();
    assert_eq!(guard_suppressed_reply(leaked), FALLBACK_REPLY);
}

#[test]
fn guard_leaves_an_ordinary_chat_reply_untouched() {
    let reply = "The growth desk is staffed by Priya and Sam.".to_string();
    assert_eq!(guard_suppressed_reply(reply.clone()), reply);
}

#[test]
fn guard_leaves_empty_and_whitespace_replies_untouched() {
    assert_eq!(guard_suppressed_reply(String::new()), "");
    assert_eq!(guard_suppressed_reply("   \n".to_string()), "   \n");
}
