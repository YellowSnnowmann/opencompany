//! Recovering a tool call a native-tool-calling model wrote as prose.
//!
//! # The leak
//!
//! On the native transport the harness sends `tools` in the request and reads
//! `message.tool_calls` back. A model that honours that contract is the
//! reliable path and nothing here runs. A model that *sometimes* honours it is
//! the problem this module exists for: the same agent, in the same thread, one
//! turn apart, was observed executing `read_ledger` correctly and then writing
//!
//! ```text
//! function_call:{"id":"call_3rY…","call":"read_ledger",
//!                "arguments":{"ledger":"tasks","query":"2026-09-01"}}
//! ```
//!
//! into the message body instead. The tool never ran, the sentence the model
//! had already written ("I'll check the tasks ledger…") became a promise it
//! could not keep, and the operator saw raw JSON in company chat.
//!
//! # Why this is in the provider, and not in a dispatcher
//!
//! OpenHuman has a `ToolDispatcher` seam with a text-parsing fallback, and
//! [`AttrTolerantXmlDispatcher`](super::tool_dispatcher::AttrTolerantXmlDispatcher)
//! extends it for #105's attribute-form open tags. That seam is **not on this
//! turn path**. Since openhuman #4249 removed the legacy engine, every turn runs
//! through the tinyagents harness, and its loop takes a turn's tool calls from
//! exactly one place — `response.tool_calls()` on the structured
//! [`ModelResponse`](tinyinference::model::ModelResponse)
//! (`agent_loop/run_loop.rs`). It never parses model text. The injected
//! `ToolDispatcher` still renders the transcript and the prompt protocol, but
//! its `parse_response` is reached only by the end-of-turn wrap-up check.
//!
//! So the last point at which a text-shaped call can still become a real one is
//! where this crate turns the wire payload into a `ModelResponse`:
//! [`model_response_from_payload`](super::provider). That is where this is
//! called from, and it is why a fix at the dispatcher seam would have parsed
//! perfectly and executed nothing.
//!
//! # Who parses
//!
//! The grammars — `function_call:`/`tool_call:` markers with the drifted name
//! keys (`call`, `tool`, …), Claude's `<invoke>`/`<parameter>` markup and the
//! vendor-decorated forms of it (DeepSeek's `<｜｜DSML｜｜invoke …>`), fenced
//! examples left alone — are TinyTools' (`tinytools_agent::parse::parse_text`).
//! This module used to carry its own copy of them; it now keeps only what is
//! host policy and that the shared parser deliberately leaves to the host:
//!
//! * **Authorization.** A call is recovered only when its resolved name is a
//!   tool *this turn* offered the model under its `tool_choice`
//!   ([`authorized_tool_names`]). The shared parser flags an unresolved name and
//!   still returns it; here such a call is dropped, so the recovery is inert by
//!   construction on a turn that suppressed tools.
//! * **Typing.** A markup `<parameter>` body is text on the wire; the turn's own
//!   schema decides whether `"3"` is a number or a string
//!   ([`authorized_tool_schemas`]).
//! * **Ids.** The parser never mints one; see below.
//! * a turn that already produced structured calls is never touched — the
//!   caller only asks when the native channel came back empty.
//!
//! # Ids are synthesized, and that is load-bearing
//!
//! A recovered call has no provider id, and the agent loop pairs each tool
//! result back to its opener by id. Without one, the result answers no call:
//! the transcript's assistant opener and the tool message disagree, the cycle is
//! dropped on its way back to the wire, and the model never learns that its tool
//! ran or what it returned — so it re-narrates the same intention on the next
//! iteration. The tool still runs, and the operator is still billed for it. That
//! is the most misleading failure available here, and giving both halves the
//! same synthesized id is what avoids it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use tinyinference::model::ToolChoice;
use tinyinference::tool::{ToolCall, ToolSchema};
use tinytools_agent::{ParseOptions, ParsedToolCall};

/// The names this turn actually **authorized** the model to call, as the set
/// the recovery validates against.
///
/// Taken from the turn's own `ModelRequest`, not from a build-time belt: a turn
/// that suppresses tools (`#1725`'s chat/small-talk path) advertises none, and
/// this set is then empty — so the recovery is inert exactly when the model was
/// never invited to call anything, with no separate flag to keep in step.
///
/// `tool_choice` narrows it, because the schemas alone are not the
/// authorization (Codex review on #2011). A request that sends tools *and*
/// `tool_choice: "none"` has told the model not to call any of them, and a
/// request naming one tool has authorized exactly that one; recovering against
/// the full schema list in either case would dispatch something this turn
/// explicitly did not ask for.
pub fn authorized_tool_names(tools: &[ToolSchema], choice: &ToolChoice) -> BTreeSet<String> {
    match choice {
        // Told not to call anything. Nothing is recoverable, whatever the
        // schemas say.
        ToolChoice::None => BTreeSet::new(),
        // Pinned to one tool: it is the only authorization this turn carries,
        // and only if it is actually on the wire.
        ToolChoice::Tool(name) => tools
            .iter()
            .map(|tool| tool.name.clone())
            .filter(|offered| offered == name)
            .collect(),
        ToolChoice::Auto | ToolChoice::Required => {
            tools.iter().map(|tool| tool.name.clone()).collect()
        }
    }
}

/// The authorized tools' own JSON Schema `parameters`, keyed by name.
///
/// A markup-dialect `<parameter>` body is text on the wire whatever the
/// argument's real type is (see [`parameter_value`]), and a dialect marks the
/// exception (`string="true"`) rather than the rule — Claude's own bare form
/// marks nothing at all. Without the tool's own declared type, a scalar-
/// looking body is a guess either way: coerce it and a strictly-typed
/// *string* parameter gets a number; leave it a string and a strictly-typed
/// *number* parameter gets one instead. The schema is the one thing that
/// actually knows, so the recovery consults it before guessing (Codex review
/// on #2093). Same authorization rule as [`authorized_tool_names`] — this is
/// that same computation, carrying the schema instead of just the name.
pub fn authorized_tool_schemas(
    tools: &[ToolSchema],
    choice: &ToolChoice,
) -> BTreeMap<String, Value> {
    authorized_tool_names(tools, choice)
        .into_iter()
        .filter_map(|name| {
            let schema = tools.iter().find(|tool| tool.name == name)?;
            Some((name, schema.parameters.clone()))
        })
        .collect()
}

/// Recover tool calls a model wrote into `content` as text, when the turn's
/// structured `tool_calls` came back empty.
///
/// Returns the narrative with the recovered calls (and their markers) removed,
/// or `None` when nothing was recovered — in which case the caller must leave
/// `content` exactly as it was.
///
/// `offered` is what licenses reading a call out of prose at all; with an
/// empty set this always returns `None`. `schemas` is consulted only to type a
/// markup-dialect `<parameter>` body — see [`authorized_tool_schemas`].
pub fn recover_text_tool_calls(
    content: &str,
    offered: &BTreeSet<String>,
    schemas: &BTreeMap<String, Value>,
) -> Option<(String, Vec<ToolCall>)> {
    if offered.is_empty() || content.is_empty() {
        return None;
    }
    let known: Vec<String> = offered.iter().cloned().collect();
    // Bare JSON stays off: a whole-response JSON object is an ordinary answer
    // far more often than it is a call, and this path runs on every native turn
    // that came back without structured calls.
    let options = ParseOptions {
        allow_bare_json: false,
        ..ParseOptions::new().with_known_tools(&known)
    };
    let outcome = tinytools_agent::parse::parse_text(content, &options);
    let calls: Vec<ToolCall> = outcome
        .calls
        .into_iter()
        .filter(|call| offered.contains(&call.name))
        .enumerate()
        .map(|(index, call)| to_tool_call(index, call, schemas))
        .collect();
    if calls.is_empty() {
        return None;
    }
    tracing::warn!(
        recovered = calls.len(),
        tools = ?calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        "[harness] the model wrote a tool call as text instead of using the native \
         tool-calling channel; recovered it from the message body. A model salvaged \
         on most turns is not honouring `tools` — consider mapping this tier to one \
         that does (Settings → Inference)."
    );
    Some((outcome.text, calls))
}

/// One parsed call as the agent loop's `ToolCall`, with a synthesized id and
/// its arguments typed against the tool's own schema.
fn to_tool_call(index: usize, call: ParsedToolCall, schemas: &BTreeMap<String, Value>) -> ToolCall {
    let arguments = match (call.arguments, schemas.get(&call.name)) {
        (Value::Object(arguments), Some(schema)) => Value::Object(typed(arguments, schema)),
        (Value::Object(arguments), None) => Value::Object(arguments),
        // The loop dispatches an object; anything else is an empty call.
        (_, _) => Value::Object(Map::new()),
    };
    ToolCall {
        id: salvaged_call_id(index),
        name: call.name,
        arguments,
        // Recovered from a well-formed call, so there is nothing to declare
        // malformed. `Some(_)` is the provider's channel for "the model asked
        // for this and its body would not parse".
        invalid: None,
    }
}

/// Retypes string arguments the schema declares as something else.
///
/// A markup `<parameter>` body arrives as text whatever its real type, and the
/// dialects mark the exception (`string="true"`) rather than the rule. Only a
/// string whose declared property type is not `string` is touched, and only
/// when it parses as that type — so a strictly-typed string parameter is never
/// turned into a number, and an unparseable value is left for the tool's own
/// validation to report.
fn typed(mut arguments: Map<String, Value>, schema: &Value) -> Map<String, Value> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return arguments;
    };
    for (key, value) in arguments.iter_mut() {
        let Value::String(raw) = value else { continue };
        let declared = properties
            .get(key)
            .and_then(|property| property.get("type"))
            .and_then(Value::as_str);
        let parsed = match declared {
            Some("integer") => raw.trim().parse::<i64>().ok().map(Value::from),
            Some("number") => raw.trim().parse::<f64>().ok().map(Value::from),
            Some("boolean") => raw.trim().parse::<bool>().ok().map(Value::from),
            Some("object") | Some("array") => serde_json::from_str::<Value>(raw.trim())
                .ok()
                .filter(|v| v.is_object() == (declared == Some("object")) && (v.is_object() || v.is_array())),
            _ => None,
        };
        if let Some(parsed) = parsed {
            *value = parsed;
        }
    }
    arguments
}

/// The id given to a recovered call, by position within the response.
///
/// Both halves of the cycle — the assistant opener and the tool result — are
/// derived from this same call, so the two id sets match and the cycle survives
/// `pair_tool_cycles`. See the module docs.
fn salvaged_call_id(index: usize) -> String {
    format!("salvaged_call_{index}")
}

#[cfg(test)]
#[path = "native_salvage_tests.rs"]
mod tests;
