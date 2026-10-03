use super::*;
use serde_json::json;

fn belt() -> BTreeSet<String> {
    ["read_ledger", "list_desks", "write_file"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// What the provider does with a text-only response: hand the content and
/// the turn's offered tools to the recovery.
fn recover(text: &str) -> (String, Vec<ToolCall>) {
    recover_with_schemas(text, &BTreeMap::new())
}

/// [`recover`], with a per-tool JSON Schema `parameters` map to type an
/// undeclared markup `<parameter>` body against.
fn recover_with_schemas(text: &str, schemas: &BTreeMap<String, Value>) -> (String, Vec<ToolCall>) {
    match recover_text_tool_calls(text, &belt(), schemas) {
        Some((cleaned, calls)) => (cleaned, calls),
        // Nothing recovered: the caller leaves the content untouched.
        None => (text.to_string(), Vec::new()),
    }
}

/// The DeepSeek leak, verbatim from company chat: the whole call written as
/// marker-decorated markup, wrapped in an envelope, with the model's own
/// sentence in front of it.
#[test]
fn a_decorated_invoke_dialect_is_recovered() {
    let (cleaned, calls) = recover(concat!(
        "Let me check the ledger.\n\n",
        "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>\n",
        "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke name=\"read_ledger\">\n",
        "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter name=\"ledger\" string=\"true\">tasks",
        "</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter>\n",
        "</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke>\n",
        "</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>",
    ));

    assert_eq!(calls.len(), 1, "one call recovered");
    assert_eq!(calls[0].name, "read_ledger");
    assert_eq!(calls[0].arguments, json!({ "ledger": "tasks" }));
    // The narrative survives; not one tag of the markup does.
    assert_eq!(cleaned, "Let me check the ledger.");
}

/// The same dialect writing several calls in one envelope — what the
/// operator actually saw: four searches, none of which ran.
#[test]
fn every_call_in_one_decorated_envelope_is_recovered() {
    let (cleaned, calls) = recover(concat!(
        "Checking.\n",
        "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>\n",
        "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke name=\"list_desks\">\n",
        "</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke>\n",
        "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke name=\"read_ledger\">\n",
        "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter name=\"ledger\" string=\"true\">risks",
        "</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter>\n",
        "</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke>\n",
        "</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>",
    ));

    assert_eq!(
        calls
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["list_desks", "read_ledger"],
        "both calls, in the order written"
    );
    // A parameterless tag call is a call with no arguments, the way a
    // `function`-keyed object with none is.
    assert_eq!(calls[0].arguments, json!({}));
    assert_eq!(calls[1].arguments, json!({ "ledger": "risks" }));
    assert_eq!(cleaned, "Checking.");
    // Both halves of each cycle read the same id, or the result answers no
    // call. See the module docs.
    assert_eq!(calls[0].id, "salvaged_call_0");
    assert_eq!(calls[1].id, "salvaged_call_1");
}

/// An envelope tag that documents the syntax in prose, with no recovered
/// call inside it, is not this module's to edit — only a recovered call
/// elsewhere in the same message is (Codex review on #2093).
#[test]
fn an_envelope_tag_around_no_recovered_call_survives() {
    let text = concat!(
        "Here is the syntax: <tool_calls><invoke name=\"unlisted_tool\">",
        "</invoke></tool_calls>\n",
        "Now actually calling it.\n",
        "<invoke name=\"list_desks\"></invoke>",
    );
    let (cleaned, calls) = recover(text);

    assert_eq!(calls.len(), 1, "only the real call is recovered");
    assert_eq!(calls[0].name, "list_desks");
    assert!(
        cleaned.contains("<tool_calls><invoke name=\"unlisted_tool\">"),
        "the documented example must survive verbatim: {cleaned:?}"
    );
    assert!(
        !cleaned.contains("list_desks"),
        "the real call is still removed: {cleaned:?}"
    );
}

/// The undecorated form — Claude's own markup, which a model trained on it
/// writes whatever endpoint it is served from.
#[test]
fn a_bare_invoke_tag_is_recovered() {
    let (cleaned, calls) = recover(concat!(
        "One moment.\n",
        "<invoke name=\"read_ledger\">",
        "<parameter name=\"ledger\">tasks</parameter>",
        "</invoke>",
    ));

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, json!({ "ledger": "tasks" }));
    assert_eq!(cleaned, "One moment.");
}

/// A structured argument only survives by being read back as JSON, and a
/// `string="true"` body must not be.
#[test]
fn a_parameter_body_is_json_unless_declared_a_string() {
    let (_, calls) = recover(concat!(
        "<invoke name=\"write_file\">",
        "<parameter name=\"content\">{\"rows\":[1,2]}</parameter>",
        "<parameter name=\"path\" string=\"true\">2026</parameter>",
        "</invoke>",
    ));

    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].arguments,
        json!({ "content": { "rows": [1, 2] }, "path": "2026" }),
        "the object parses; the declared string stays a string"
    );
}

/// Claude's own bare markup has no `string="true"` escape at all, so a
/// numeric-looking `query` would otherwise become a JSON number and a
/// strictly-typed *string* parameter would reject the call outright. The
/// tool's own schema is what tells the recovery which way to guess right
/// (Codex review on #2093).
#[test]
fn a_schema_typed_string_parameter_is_not_coerced_to_a_number() {
    let schemas = BTreeMap::from([(
        "read_ledger".to_string(),
        json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
        }),
    )]);
    let (_, calls) = recover_with_schemas(
        concat!(
            "<invoke name=\"read_ledger\">",
            "<parameter name=\"query\">2026</parameter>",
            "</invoke>",
        ),
        &schemas,
    );

    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].arguments,
        json!({ "query": "2026" }),
        "the schema says string, so the digits stay a string"
    );
}

/// A parameter the schema does not mention at all falls back to this
/// module's ordinary parse-or-string guess, exactly as with no schema.
#[test]
fn a_parameter_absent_from_the_schema_still_gets_the_parse_or_string_guess() {
    let schemas = BTreeMap::from([(
        "read_ledger".to_string(),
        json!({ "type": "object", "properties": {} }),
    )]);
    let (_, calls) = recover_with_schemas(
        concat!(
            "<invoke name=\"read_ledger\">",
            "<parameter name=\"limit\">5</parameter>",
            "</invoke>",
        ),
        &schemas,
    );

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, json!({ "limit": 5 }));
}

/// The enclosing call owns its own argument: a JSON object in a
/// `<parameter>` body must not also be recovered as a call beside it.
#[test]
fn a_json_body_inside_a_call_is_not_a_second_call() {
    let (_, calls) = recover(concat!(
        "<invoke name=\"write_file\">",
        "<parameter name=\"content\">",
        "function_call:{\"call\":\"read_ledger\",\"arguments\":{\"ledger\":\"tasks\"}}",
        "</parameter>",
        "</invoke>",
    ));

    assert_eq!(
        calls
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["write_file"],
        "only the call that was actually made"
    );
}

/// Belt membership decides here exactly as it does for the object shapes:
/// the tag says a call was written, not that this turn offered it.
#[test]
fn an_invoke_naming_an_unoffered_tool_is_left_alone() {
    let text = "<invoke name=\"drop_database\"><parameter name=\"id\">1</parameter></invoke>";
    let (cleaned, calls) = recover(text);

    assert!(calls.is_empty(), "nothing recovered");
    assert_eq!(cleaned, text, "and the reply is untouched");
}

/// A model cut off mid-call leaves a tag with no end. Guessing where it
/// stopped would run a tool against arguments nobody finished writing.
#[test]
fn an_unclosed_invoke_is_left_verbatim() {
    let text = "<invoke name=\"read_ledger\"><parameter name=\"ledger\">tas";
    let (cleaned, calls) = recover(text);

    assert!(calls.is_empty());
    assert_eq!(cleaned, text);
}

/// A call closes but one of its arguments does not: the model was cut off
/// mid-parameter. Dispatching the call anyway with a partial (or empty)
/// argument object would run a tool against inputs the model never
/// finished writing — reject the whole invoke instead (Codex review on
/// #2093).
#[test]
fn an_invoke_with_an_unclosed_parameter_is_rejected_whole() {
    let text = concat!(
        "<invoke name=\"read_ledger\">",
        "<parameter name=\"ledger\">tasks</parameter>",
        "<parameter name=\"query\">unfinished",
        "</invoke>",
    );
    let (cleaned, calls) = recover(text);

    assert!(
        calls.is_empty(),
        "an incomplete argument object must not dispatch"
    );
    assert_eq!(cleaned, text, "left verbatim, like an unclosed invoke");
}

/// The decoration must be a dialect's marker, not any run of letters: an
/// ordinary word starting with the keyword is not a tag.
#[test]
fn an_ascii_lookalike_tag_is_not_a_call() {
    let text = "<invoker name=\"read_ledger\">whatever</invoker>";
    let (cleaned, calls) = recover(text);

    assert!(calls.is_empty(), "`<invoker>` is not `<invoke>`");
    assert_eq!(cleaned, text);
}

/// The smoking gun, verbatim from the 1/9 QA round: a `function_call:`
/// marker, a `call` name key, and prose on either side.
#[test]
fn function_call_marker_with_call_name_key_is_recovered() {
    let raw = "Let me proceed with querying the tasks ledger. \
               function_call:{\"id\":\"call_3rY\",\"call\":\"read_ledger\",\
               \"arguments\":{\"ledger\":\"tasks\",\"query\":\"2026-09-01\"}}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1, "the call must be recovered");
    assert_eq!(calls[0].name, "read_ledger");
    assert_eq!(calls[0].arguments["ledger"], "tasks");
    assert_eq!(
        calls[0].id, "salvaged_call_0",
        "a synthesized id is what keeps the cycle paired"
    );
    assert!(calls[0].invalid.is_none());
    assert!(
        !text.contains("function_call"),
        "the marker and object must not survive in chat: {text:?}"
    );
    assert!(
        text.starts_with("Let me proceed"),
        "the narrative must survive: {text:?}"
    );
}

/// The other observed shape: a lead-in sentence and a fenced block. The
/// fence must go with the call rather than being left empty in chat.
#[test]
fn fenced_block_is_recovered_and_the_empty_fence_removed() {
    let raw = "I'll share the company's task board.\n\n```json\n\
               {\n  \"call\": \"read_ledger\",\n  \"arguments\": {\n    \
               \"ledger\": \"tasks\"\n  }\n}\n```";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "read_ledger");
    assert_eq!(text, "I'll share the company's task board.");
}

/// The false positive a shared parser refuses to risk, and the reason this
/// one checks the turn's tools: an ordinary JSON answer naming a person.
#[test]
fn a_plain_json_reply_is_not_a_call() {
    let raw = "{\"name\":\"Alice\",\"input\":\"hi\"}";
    let (text, calls) = recover(raw);

    assert!(calls.is_empty(), "an unoffered name must never dispatch");
    assert_eq!(text, raw, "and the reply must reach chat unchanged");
}

/// Prose *about* a tool, carrying its name in a JSON object with unrelated
/// keys, is not a call to it.
#[test]
fn narrating_a_tool_name_is_not_a_call() {
    let raw = "The step ran: {\"name\":\"read_ledger\",\"duration_ms\":51}";
    let (_, calls) = recover(raw);

    assert!(
        calls.is_empty(),
        "an offered name with unrelated keys and no arguments is narrative"
    );
}

/// A tool that takes no arguments still has to be callable, or the recovery
/// would work for every tool except the simplest ones.
#[test]
fn a_bare_no_argument_call_is_recovered() {
    let raw = "{\"call\":\"list_desks\"}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "list_desks");
    assert_eq!(calls[0].arguments, empty_object());
    assert!(text.is_empty(), "the whole message was the call: {text:?}");
}

/// The OpenAI wire shape written out longhand, including the stringified
/// `arguments` providers use.
#[test]
fn the_function_wrapper_shape_with_stringified_arguments_is_recovered() {
    let raw = "Writing that file now. {\"type\":\"function\",\"function\":\
               {\"name\":\"write_file\",\"arguments\":\"{\\\"path\\\":\\\"x\\\"}\"}}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "write_file");
    assert_eq!(calls[0].arguments["path"], "x");
    assert_eq!(text, "Writing that file now.");
}

/// Two calls in one message get distinct ids, or the second would answer
/// the first's result and the cycle would be dropped as mismatched.
#[test]
fn two_recovered_calls_get_distinct_ids() {
    let raw = "{\"call\":\"list_desks\"}\nthen\n\
               {\"call\":\"read_ledger\",\"arguments\":{\"ledger\":\"tasks\"}}";
    let (_, calls) = recover(raw);

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].id, "salvaged_call_0");
    assert_eq!(calls[1].id, "salvaged_call_1");
}

/// A brace inside a JSON string must not end the span early — the case a
/// naive depth counter gets wrong, and a file path is the likeliest way to
/// hit it in this codebase.
#[test]
fn a_brace_inside_a_string_does_not_split_the_span() {
    let raw = "{\"call\":\"write_file\",\"arguments\":{\"path\":\"a{b\",\
               \"content\":\"}\"}}";
    let (_, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["path"], "a{b");
    assert_eq!(calls[0].arguments["content"], "}");
}

/// A turn that offered no tools recovers nothing — which is what makes the
/// tool-suppressed chat path inert without a second flag to keep in step.
#[test]
fn a_turn_that_offered_no_tools_never_recovers() {
    let raw = "{\"call\":\"read_ledger\",\"arguments\":{\"ledger\":\"tasks\"}}";
    assert!(recover_text_tool_calls(raw, &BTreeSet::new(), &BTreeMap::new()).is_none());
}

/// A fenced code block the narrative actually needs is not collateral.
#[test]
fn a_fenced_block_with_content_survives() {
    let raw = "Here is the fix:\n\n```rust\nlet x = 1;\n```\n\n\
               {\"call\":\"list_desks\"}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert!(
        text.contains("let x = 1;"),
        "an unrelated code block must survive: {text:?}"
    );
}

/// Arguments that are not an object are not dispatchable, and guessing an
/// empty object for them would run the tool with the wrong input.
#[test]
fn non_object_arguments_are_refused() {
    let raw = "{\"call\":\"read_ledger\",\"arguments\":\"the tasks one\"}";
    let (_, calls) = recover(raw);

    assert!(calls.is_empty());
}

/// Same rule inside the `function` wrapper. That branch skips the
/// bare-object check, so without this it would silently substitute an
/// empty object and run the tool on input the model never asked for.
#[test]
fn non_object_arguments_are_refused_inside_the_function_wrapper() {
    let raw = "Calling it: {\"function\":{\"name\":\"read_ledger\",\
               \"arguments\":\"the tasks one\"}}";
    let (text, calls) = recover(raw);

    assert!(calls.is_empty());
    assert_eq!(
        text, raw,
        "and a refused call stays visible, not silently dropped"
    );
}

/// Tidying the hole a removed call leaves must not reflow the narrative
/// around it. Indentation is content — a character scan that skips
/// whitespace after a newline eats every indented list and code block.
#[test]
fn indentation_in_the_surviving_narrative_is_preserved() {
    let raw = "Steps:\n  1. Review releases\n      - check the ledger\n\n\n\
               {\"call\":\"list_desks\"}\n\nDone.";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(
        text,
        "Steps:\n  1. Review releases\n      - check the ledger\n\nDone."
    );
}

/// A model that writes the marker on its own line above the object: the
/// marker is part of the call's layout, so it must go with it rather than
/// be left behind in the operator's reply.
#[test]
fn a_marker_on_its_own_line_is_removed_with_the_object() {
    let raw = "Let me look that up.\nfunction_call:\n{\"call\":\"list_desks\"}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(
        text, "Let me look that up.",
        "the marker must not survive into chat"
    );
}

/// The same, with CRLF line endings.
#[test]
fn a_marker_on_its_own_crlf_line_is_removed_with_the_object() {
    let raw = "Let me look that up.\r\nfunction_call:\r\n{\"call\":\"list_desks\"}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert!(
        !text.contains("function_call"),
        "the marker must not survive into chat: {text:?}"
    );
}

/// Where consuming the gap stops. A blank line between the two means the
/// word belongs to the paragraph above, so it stays as narrative — the
/// object is still recovered, but nothing is eaten out of the prose.
#[test]
fn a_blank_line_leaves_the_word_above_as_narrative() {
    let raw = "Here is what I found.\ncall:\n\n{\"call\":\"list_desks\"}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(text, "Here is what I found.\ncall:");
}

/// The false positive that belt membership alone cannot stop: a model asked
/// to *document* an offered tool writes the exact shape a call has.
///
/// `write_file` is on the belt and the object carries real `arguments`, so
/// every structural check passes. Only the absence of an intent signal — no
/// call-family key, no marker — separates this from a request, and running
/// it would write a file because a sentence said "example".
#[test]
fn a_documented_example_keyed_only_by_name_is_not_dispatched() {
    let raw = "Example: {\"name\":\"write_file\",\"arguments\":\
               {\"path\":\"demo\",\"content\":\"hi\"}}";
    let (text, calls) = recover(raw);

    assert!(
        calls.is_empty(),
        "a bare `name` object with no marker must not run a tool"
    );
    assert_eq!(text, raw, "and the example must reach chat unchanged");
}

/// The same object, with the model saying it is a call. Either signal is
/// enough — here it is the marker.
#[test]
fn a_name_keyed_object_behind_a_marker_is_dispatched() {
    let raw = "Writing it now. function_call:{\"name\":\"write_file\",\
               \"arguments\":{\"path\":\"demo\"}}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "write_file");
    assert_eq!(text, "Writing it now.");
}

/// And here it is the key: `call` is not a word ordinary JSON uses, so it
/// carries the intent by itself. This is the 1/9 fenced shape, which had no
/// marker at all.
#[test]
fn a_call_keyed_object_needs_no_marker() {
    let raw = "Here is the board.\n\n```json\n{\"call\":\"read_ledger\",\
               \"arguments\":{\"ledger\":\"tasks\"}}\n```";
    let (_, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "read_ledger");
}

/// `Recall:` ends in the `call:` marker. Cutting on a bare suffix match
/// takes two characters of an ordinary word with it and leaves the operator
/// reading "Re".
#[test]
fn a_marker_matched_inside_a_word_is_not_stripped() {
    let raw = "Recall: {\"call\":\"list_desks\"}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1, "the object is still a call");
    assert_eq!(text, "Recall:", "but the word must survive intact");
}

/// An inline triple-backtick span has no opener line and no body, so the
/// empty-fence sweep must leave it alone rather than delete its contents.
#[test]
fn an_inline_backtick_span_is_not_swept_as_an_empty_fence() {
    let raw = "Important ```do not delete``` and now: {\"call\":\"list_desks\"}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert!(
        text.contains("do not delete"),
        "an inline code span must survive: {text:?}"
    );
}

/// `tool_choice: "none"` authorizes nothing, whatever schemas rode along.
#[test]
fn tool_choice_none_authorizes_nothing() {
    let schemas = [ToolSchema::new("read_ledger", "d", serde_json::json!({}))];
    assert!(authorized_tool_names(&schemas, &ToolChoice::None).is_empty());
}

/// A pinned `tool_choice` authorizes that tool and no sibling on the wire.
#[test]
fn a_pinned_tool_choice_authorizes_only_that_tool() {
    let schemas = [
        ToolSchema::new("read_ledger", "d", serde_json::json!({})),
        ToolSchema::new("write_file", "d", serde_json::json!({})),
    ];
    let authorized = authorized_tool_names(&schemas, &ToolChoice::Tool("read_ledger".to_string()));

    assert_eq!(authorized.len(), 1);
    assert!(authorized.contains("read_ledger"));
    assert!(
        !authorized.contains("write_file"),
        "a sibling schema is not authorized by a pinned choice"
    );
}

/// A marker search that slices by byte offset must not panic when the
/// narrative in front of the object ends in a multi-byte character.
#[test]
fn a_multi_byte_character_before_the_object_does_not_panic() {
    let raw = "Checking the ledger… {\"call\":\"list_desks\"}";
    let (text, calls) = recover(raw);

    assert_eq!(calls.len(), 1);
    assert_eq!(text, "Checking the ledger…");
}
