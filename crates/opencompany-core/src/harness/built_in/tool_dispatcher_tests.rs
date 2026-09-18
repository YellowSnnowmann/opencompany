use super::*;

/// Build a [`ChatResponse`] carrying `text` the way the vendored parser
/// consumes it: text set, no native tool calls, no usage, no reasoning.
fn response(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.to_string()),
        ..Default::default()
    }
}

/// The smoking gun (#105): an attribute-form `<tool_call id="…">` open tag.
///
/// Pre-fix, the vendored parser matched no tag, so `calls` was empty and the
/// raw `<tool_call …>` block was returned as narrative text (the leak). With
/// the wrapper, the attribute is stripped, the tool is dispatched, and no
/// raw tag survives in the returned text.
#[test]
fn attribute_form_tool_call_is_parsed_and_dispatched() {
    let dispatcher = AttrTolerantXmlDispatcher::default();
    let raw = r#"<tool_call id="call_2">{"name":"write_file","arguments":{"path":"x","content":"y"}}</tool_call>"#;

    let (text, calls) = dispatcher.parse_response(&response(raw));

    assert_eq!(calls.len(), 1, "the attribute-form call must dispatch");
    assert_eq!(calls[0].name, "write_file");
    assert!(
        !text.contains("<tool_call"),
        "no raw <tool_call tag may leak into the narrative text: {text:?}"
    );
}

/// The already-working bare form must still parse unchanged (no regression):
/// the normalization is a no-op on a tag with no attributes.
#[test]
fn bare_tool_call_still_parses() {
    let dispatcher = AttrTolerantXmlDispatcher::default();
    let raw = r#"<tool_call>{"name":"foo","arguments":{}}</tool_call>"#;

    let (_text, calls) = dispatcher.parse_response(&response(raw));

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "foo");
}

/// The `<invoke name="…">` attribute form (whose attributes carry the tool
/// name) must be delegated untouched — proving the wrapper does not strip
/// `invoke` attributes and break the vendored invoke parser.
#[test]
fn invoke_attribute_form_still_parses() {
    let dispatcher = AttrTolerantXmlDispatcher::default();
    let raw = r#"<invoke name="bar">{"arguments":{}}</invoke>"#;

    let (_text, calls) = dispatcher.parse_response(&response(raw));

    assert_eq!(calls.len(), 1, "invoke must still dispatch: {calls:?}");
    assert_eq!(calls[0].name, "bar");
}

/// Plain narrative text with no tags dispatches nothing and is returned
/// unchanged.
#[test]
fn plain_text_is_untouched() {
    let dispatcher = AttrTolerantXmlDispatcher::default();
    let raw = "Just a normal sentence with no tool calls.";

    let (text, calls) = dispatcher.parse_response(&response(raw));

    assert!(calls.is_empty());
    assert_eq!(text.trim(), raw);
}

/// Direct check on the normalizer: attribute forms collapse to bare, bare is
/// a no-op (`Borrowed`), `<invoke …>` and closing tags are left alone.
#[test]
fn normalizer_scopes_correctly() {
    // Attribute form → bare.
    assert_eq!(
        normalize_tool_call_open_tags(r#"<tool_call id="call_2">body</tool_call>"#),
        "<tool_call>body</tool_call>"
    );
    // Bare open tag → borrowed no-op.
    assert!(matches!(
        normalize_tool_call_open_tags("<tool_call>body</tool_call>"),
        Cow::Borrowed(_)
    ));
    // The plural JSON key is not a tool_call open tag → untouched.
    assert!(matches!(
        normalize_tool_call_open_tags("<tool_calls>[]</tool_calls>"),
        Cow::Borrowed(_)
    ));
    // invoke is excluded → untouched.
    assert!(matches!(
        normalize_tool_call_open_tags(r#"<invoke name="bar">x</invoke>"#),
        Cow::Borrowed(_)
    ));
    // Hyphen + underscore variants both collapse.
    assert_eq!(
        normalize_tool_call_open_tags(r#"<tool-call x="1">b</tool-call>"#),
        "<tool-call>b</tool-call>"
    );
    assert_eq!(
        normalize_tool_call_open_tags(r#"<toolcall y='2'>b</toolcall>"#),
        "<toolcall>b</toolcall>"
    );
}
