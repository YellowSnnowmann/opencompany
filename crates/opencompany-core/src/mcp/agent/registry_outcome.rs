//! The call outcome a directory install's `mcp_registry_tool_call` carries.
//!
//! OpenHuman's registry call tool reports a failed call as an ordinary result
//! whose JSON body is `{"result": "<error text>", "is_error": true}`, and a
//! call the server answered as `{"result": <CallToolResult>, ...}`. This module
//! reads that body into the same [`McpCallOutcome`] tinymcp's native
//! `mcp_call_tool` attaches, so the observer treats both bridges alike.

use serde_json::Value;
use tinymcp::tinymcp_bus::errors;
use tinymcp::{McpCallError, McpCallOutcome};
use tinytools::ToolResult;

/// The registry tool whose results carry a call outcome.
pub(crate) const REGISTRY_CALL_TOOL: &str = "mcp_registry_tool_call";

/// The code for error text no tinymcp rendering matches. Not one of tinymcp's
/// wire names, so the observer reads it as an unrecognised failure.
pub(crate) const UNCLASSIFIED: &str = "ai.tinyhumans.opencompany.Error.Unclassified";

/// `result` with the outcome of a call that reached the registry attached.
///
/// A result that already carries metadata keeps it.
pub(crate) fn attach_answer(result: ToolResult, server: &str, tool: &str) -> ToolResult {
    if result.metadata.is_some() {
        return result;
    }
    let outcome = match failure_text(&result.output()) {
        Some(text) => McpCallOutcome::failed(server, tool, call_error_from_text(&text)),
        None => McpCallOutcome::answered(server, tool),
    };
    with_outcome(result, &outcome)
}

/// `refusal` with the outcome of a call refused before it was dispatched.
pub(crate) fn attach_refusal(
    refusal: ToolResult,
    server: &str,
    tool: &str,
    code: &str,
) -> ToolResult {
    with_outcome(
        refusal,
        &McpCallOutcome::failed(server, tool, McpCallError::new(code)),
    )
}

/// The error text of a registry body that reports a failure, when it does.
///
/// A remote tool's own error result is an object, so only a string `result`
/// is a call that failed before the server answered it.
pub(crate) fn failure_text(output: &str) -> Option<String> {
    let body: Value = serde_json::from_str(output).ok()?;
    if body.get("is_error").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    body.get("result")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Classifies tinymcp's rendering of a call error back into its wire code.
///
/// The registry crosses a string boundary, so the error's variant is gone and
/// its `Display` text is all that is left to read.
pub(crate) fn call_error_from_text(text: &str) -> McpCallError {
    let lowered = text.to_ascii_lowercase();
    let code = if lowered.contains("mcp unauthorized for") {
        errors::UNAUTHORIZED
    } else if lowered.contains("mcp http ") {
        errors::HTTP
    } else if lowered.contains("mcp transport failure") {
        errors::TRANSPORT
    } else if lowered.contains("malformed mcp response") {
        errors::MALFORMED_RESPONSE
    } else if lowered.contains("mcp error response") {
        errors::RPC
    } else if lowered.contains("is not permitted on server") {
        errors::TOOL_NOT_ALLOWED
    } else if lowered.contains("is not connected") {
        errors::NOT_CONNECTED
    } else if lowered.contains("is disabled") {
        errors::SERVER_DISABLED
    } else if lowered.contains("unknown mcp server") {
        errors::UNKNOWN_SERVER
    } else if lowered.contains("invalid arguments for tool") {
        errors::INVALID_ARGUMENTS
    } else {
        UNCLASSIFIED
    };
    McpCallError {
        code: code.to_string(),
        unauthorized: code == errors::UNAUTHORIZED,
        advertises_oauth: false,
    }
}

fn with_outcome(mut result: ToolResult, outcome: &McpCallOutcome) -> ToolResult {
    result.metadata = serde_json::to_value(outcome).ok();
    result
}

#[cfg(test)]
#[path = "registry_outcome_tests.rs"]
mod tests;
