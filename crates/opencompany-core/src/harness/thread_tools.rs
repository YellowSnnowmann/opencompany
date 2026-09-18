//! Reading another thread in this channel, on demand (issue #1890 F).
//!
//! # Why a tool at all
//!
//! Sub-issue A scoped a turn's context to its own thread, which is what stops
//! two conversations in one channel from bleeding into each other. Sub-issue E
//! then tells a turn what *else* its channel is about — and explicitly tells it
//! not to read any of it.
//!
//! That leaves one gap, and it is the one the operator opens themselves: "make
//! it match the tone of the launch email thread". The agent can see that thread
//! exists and cannot look at it.
//!
//! This is the looking. The epic's framing is **escalate on demand rather than
//! preload**: cost scales with how often threads are actually cross-referenced
//! instead of with how busy the channel is, and *isolation breaks only where
//! the operator asked it to*. A did not isolate threads because cross-reference
//! is bad — it isolated them because unconditional cross-reference is.
//!
//! # Scoped to the current channel
//!
//! Through the same `owns` predicate the seed uses, against the channel the
//! turn is ambiently in (`delegation::turn_conversation`). A tool able to read
//! any thread anywhere would reintroduce A's leak through the back door, so a
//! root in another channel is refused rather than silently returning nothing —
//! a refusal that says why is the difference between "not yours to read" and
//! "there is nothing there".
//!
//! # It declares its truncation
//!
//! `query_company` is the cautionary case the epic names: a full log handed the
//! orchestrator only its last ten rows and read as complete, so "we have no
//! record of that" became a conclusion it could reach from a partial list. A
//! thread cut short says how much it left.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use oh::tools::traits::{PermissionLevel, Tool, ToolResult};
use openhuman_core as oh;

use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId, EventSeq};

/// The tool name, shared with the belt that registers it.
pub const READ_THREAD_TOOL: &str = "read_thread";

/// How many turns of one thread are returned before the tail is declared.
///
/// A thread is one topic and one level deep, so this is generous for the shape
/// rather than a guess — and what does not fit is counted, never dropped in
/// silence.
const THREAD_TURN_LIMIT: usize = 40;

/// How much of the journal's tail is searched for the requested root.
///
/// Bounded for the reason every read in this epic is: cheap since #1890 G reads
/// a page from the end of the journal rather than streaming it from the head. A
/// root older than this page is reported as out of reach rather than hunted for
/// — which is honest, and is the gap `find_thread` exists to close.
const THREAD_SEARCH_PAGE: usize = 1024;

/// Reads one thread of the channel the current turn is in. Read-only.
pub struct ReadThreadTool {
    company: CompanyId,
    events: Arc<dyn EventLog>,
    /// Resolves the addressed chat id to the desk's `(id, name)` pair.
    ///
    /// Both are needed: a named desk's id and its display name are different
    /// strings and a message is journaled under whichever the caller used, so
    /// `owns` takes two terms. With one, a root stored under the other alias
    /// reads as belonging to a different channel and is refused — a same-desk
    /// thread the operator can see, that the tool insists is not theirs
    /// (codex + coderabbit on #1972).
    store: Arc<dyn crate::ports::store::CompanyStore>,
}

impl ReadThreadTool {
    pub fn new(
        company: CompanyId,
        events: Arc<dyn EventLog>,
        store: Arc<dyn crate::ports::store::CompanyStore>,
    ) -> Self {
        Self {
            company,
            events,
            store,
        }
    }
}

#[async_trait]
impl Tool for ReadThreadTool {
    fn name(&self) -> &str {
        READ_THREAD_TOOL
    }

    fn description(&self) -> &str {
        "Read one other conversation thread in THIS channel, by the id shown in square brackets \
         in the channel's thread list. USE ONLY when the message you are answering explicitly \
         refers to another thread — do not read one speculatively, and if a reference could mean \
         more than one thread, ask which rather than reading several."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "root": {
                    "type": "integer",
                    "description": "The thread's id, exactly as shown in square brackets in the \
                                    channel's thread list (e.g. 41 for `[41]`)."
                }
            },
            "required": ["root"],
            "additionalProperties": false
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(root) = args.get("root").and_then(Value::as_u64).map(EventSeq::new) else {
            return Ok(ToolResult::error(
                "`read_thread` needs `root`: the thread's id, as shown in square brackets in the \
                 channel's thread list."
                    .to_string(),
            ));
        };

        // The channel this turn is answering in. `None` is a refusal and not a
        // wildcard: a turn with no conversation — a dispatched card, a workflow
        // node — has no threads it is entitled to read.
        let Some(channel) = crate::runtime::delegation::turn_conversation() else {
            return Ok(ToolResult::error(
                "`read_thread` is only available while answering in a channel; this turn is not \
                 in one."
                    .to_string(),
            ));
        };

        // The desk's own id and name, resolved the way the seed resolves them.
        let (desk_id, desk_name) = crate::server::chat_history::resolve_seed_desk(
            &self.store,
            &self.company,
            Some(channel.as_str()),
        )
        .await;

        let page = match self
            .events
            .read_before(&self.company, None, THREAD_SEARCH_PAGE)
            .await
        {
            Ok(page) => page,
            Err(error) => {
                return Ok(ToolResult::error(format!(
                    "Could not read the channel's history: {error}."
                )));
            }
        };

        // Oldest-first, so the thread reads in the order it happened.
        let mut turns: Vec<String> = Vec::new();
        let mut found_root = false;
        let mut owned_elsewhere = false;
        for stored in page.iter().rev() {
            let in_channel = crate::server::chat_history::owns(&desk_id, &desk_name, &stored.event);
            let (parent, line) = match &stored.event {
                CompanyEvent::OperatorMessage { parent, text, .. } => {
                    (*parent, format!("operator: {text}"))
                }
                CompanyEvent::AgentReply {
                    parent,
                    agent_id,
                    text,
                    ..
                } => (*parent, format!("{agent_id}: {text}")),
                _ => continue,
            };
            let belongs = stored.seq == root || parent == Some(root);
            if !belongs {
                continue;
            }
            if !in_channel {
                // The root exists, but in a different channel. Named as a
                // refusal rather than answered with silence — "not yours to
                // read" and "there is nothing there" are different facts, and
                // the second invites a retry that will also fail.
                owned_elsewhere = true;
                continue;
            }
            if stored.seq == root {
                found_root = true;
            }
            turns.push(line);
        }

        if owned_elsewhere && !found_root {
            return Ok(ToolResult::error(format!(
                "Thread {root} belongs to a different channel. `read_thread` only reads threads \
                 in the channel you are answering in."
            )));
        }
        if !found_root {
            return Ok(ToolResult::error(format!(
                "No thread {root} in this channel's recent history. It may be older than the \
                 window this tool searches."
            )));
        }

        // Keep the NEWEST turns, not the oldest. `turns` is oldest-first, so a
        // plain `truncate` kept the opening of the thread and dropped its
        // conclusion — while the notice below said the opposite, so a request
        // about what was decided would be answered from the part before anyone
        // decided anything (codex + coderabbit on #1972).
        let omitted = turns.len().saturating_sub(THREAD_TURN_LIMIT);
        if omitted > 0 {
            turns.drain(..omitted);
        }
        let mut body = turns.join("\n");
        if omitted > 0 {
            // Declared, never silent — `query_company`'s lesson.
            body.push_str(&format!(
                "\n… and {omitted} earlier turn(s) in this thread, not shown."
            ));
        }
        Ok(ToolResult::success(body))
    }
}

#[cfg(test)]
#[path = "thread_tools_tests.rs"]
mod tests;
