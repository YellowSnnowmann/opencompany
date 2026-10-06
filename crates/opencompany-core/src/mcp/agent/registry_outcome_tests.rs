//! Tests for the call outcome a registry tool call carries, and for the
//! text classifier staying in step with tinymcp's own error renderings.

use serde_json::json;
use tinymcp::tinymcp_bus::errors;

use super::*;

fn outcome(result: &ToolResult) -> McpCallOutcome {
    McpCallOutcome::from_metadata(result.metadata.as_ref().expect("metadata attached"))
        .expect("an mcp call outcome")
}

#[test]
fn an_answered_body_is_an_answered_call() {
    let body = json!({ "result": { "content": [], "isError": false }, "is_error": false });
    let result = attach_answer(ToolResult::success(body.to_string()), "install", "search");
    let outcome = outcome(&result);
    assert!(outcome.ok);
    assert_eq!(outcome.server, "install");
    assert_eq!(outcome.tool, "search");
}

#[test]
fn a_remote_tool_error_object_is_still_answered() {
    let body = json!({ "result": { "content": [], "isError": true }, "is_error": true });
    let result = attach_answer(ToolResult::success(body.to_string()), "install", "search");
    assert!(outcome(&result).ok);
}

#[test]
fn a_string_error_body_is_a_failed_call_classified_by_its_text() {
    let body = json!({
        "result": "mcp unauthorized for `https://x.test/mcp` (HTTP 401)",
        "is_error": true
    });
    let result = attach_answer(ToolResult::success(body.to_string()), "install", "search");
    let outcome = outcome(&result);
    assert!(!outcome.ok);
    let error = outcome.error.expect("an error");
    assert_eq!(error.code, errors::UNAUTHORIZED);
    assert!(error.unauthorized);
}

#[test]
fn existing_metadata_is_kept() {
    let mut result = ToolResult::success("{}");
    result.metadata = Some(json!({ "kind": "other" }));
    let result = attach_answer(result, "install", "search");
    assert_eq!(result.metadata, Some(json!({ "kind": "other" })));
}

#[test]
fn a_refusal_carries_its_code() {
    let result = attach_refusal(
        ToolResult::error("no"),
        "install",
        "delete",
        errors::TOOL_NOT_ALLOWED,
    );
    let outcome = outcome(&result);
    assert!(!outcome.ok);
    assert_eq!(outcome.error.unwrap().code, errors::TOOL_NOT_ALLOWED);
}

#[test]
fn unrecognised_text_is_unclassified() {
    assert_eq!(call_error_from_text("something odd").code, UNCLASSIFIED);
}

/// Each rendering this classifier reads, produced by tinymcp itself, maps back
/// to that error's own wire name — so a wording change upstream fails here.
#[test]
fn the_classifier_matches_tinymcps_own_renderings() {
    let cases = [
        tinymcp::Error::Unauthorized {
            endpoint: "https://x.test/mcp".to_string(),
            resource_metadata: None,
        },
        tinymcp::Error::Http {
            endpoint: "https://x.test/mcp".to_string(),
            status: 503,
            body: String::new(),
        },
        tinymcp::Error::MalformedResponse {
            detail: "not json".to_string(),
        },
        tinymcp::Error::Rpc {
            message: "bad cursor".to_string(),
        },
        tinymcp::Error::ToolNotAllowed {
            server: "docs".to_string(),
            tool: "delete".to_string(),
        },
        tinymcp::Error::UnknownServer {
            server: "docs".to_string(),
        },
    ];
    for error in cases {
        assert_eq!(
            call_error_from_text(&error.to_string()).code,
            error.wire_name(),
            "{error}"
        );
    }
}
