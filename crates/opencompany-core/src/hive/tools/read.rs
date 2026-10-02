//! The conversation `read`: `read_conversation`, which renders a desk's
//! history as one agent may see it, and `ConversationReadTool`, the `read` tool
//! on a pooled agent's own belt.

use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};
use tinyhivemind::speech;
use tinyhivemind::{SessionAuthor, SessionLog};
use tinyhivemind_core::aside::Viewer;
use tinyhivemind_embed::ConversationRef;
use tinytools::{Tool, ToolResult};

use super::{InFlightRegistry, speech_descriptor};
use crate::ports::events::EventLog;
use crate::ports::types::CompanyId;

/// The conversation `surface` names, as `agent_id` may see it: the most
/// recent `limit` rows, oldest first, one `[sequence] author: content` line
/// each.
///
/// # Errors
///
/// The journal could not be read. The text is written for the model.
pub async fn read_conversation(
    events: Arc<dyn EventLog>,
    company: &CompanyId,
    agent_id: &str,
    surface: &ConversationRef,
    limit: usize,
) -> Result<String, String> {
    let log = crate::hive::session_log::EventLogSessionLog::new(
        events,
        company.clone(),
        surface.id.clone(),
        surface.id.clone(),
        // No pair channels. This reads a desk's own history back to an agent
        // asking for it; a private exchange between two seats is theirs, and
        // reaching it needs the turn that is inside it, not a read of the
        // room. Empty admits none.
        Vec::new(),
    );
    let page = log
        .read_before(None, limit)
        .await
        .map_err(|error| format!("this conversation could not be read: {error}"))?;
    let viewer = Viewer::Agent {
        id: agent_id.to_string(),
    };
    let mut lines: Vec<String> = page
        .messages
        .iter()
        .filter(|message| {
            let author_id = match &message.author {
                SessionAuthor::Agent { id, .. } => Some(id.as_str()),
                _ => None,
            };
            message.audience.admits(&viewer, author_id)
        })
        .map(|message| {
            let author = match &message.author {
                SessionAuthor::Operator => "operator".to_string(),
                SessionAuthor::Person { label, .. } => label.clone(),
                SessionAuthor::Agent { id, .. } => id.clone(),
                SessionAuthor::System { kind, .. } => kind.clone(),
            };
            format!("[{}] {author}: {}", message.sequence.0, message.content)
        })
        .collect();
    lines.reverse();
    if page.messages.is_empty() {
        return Ok("Nothing has been said in this conversation yet.".to_string());
    }
    if lines.is_empty() {
        let mut body = format!(
            "None of the most recent {} messages here are visible to you (they are private \
             exchanges between other teammates). This does not mean the conversation is empty.",
            page.messages.len()
        );
        if page.next_before.is_some() {
            body.push_str(" There may be older messages that are not in this reply.");
        }
        return Ok(body);
    }
    let mut body = lines.join("\n");
    if page.next_before.is_some() {
        body.push_str(&format!(
            "\n\n(Showing the most recent {limit}. Older messages are not in this reply.)"
        ));
    }
    Ok(body)
}

/// The name [`ConversationReadTool`] is called by.
pub const READ_TOOL: &str = "read";

/// `read` on a pooled agent's own belt: the conversation its in-flight turn
/// answers in, read through [`read_conversation`].
///
/// Always on the belt, even for an agent built without an event log: a
/// resumed session keeps every tool declaration it was once sent, so `read`
/// must stay executable. Without a log it refuses.
///
/// Built before the runtime settles the agent's id, so the id is bound
/// afterwards through the shared cell handed to [`new`](Self::new).
pub struct ConversationReadTool {
    in_flight: Arc<InFlightRegistry>,
    runtime_agent_id: Arc<std::sync::OnceLock<String>>,
    events: Option<Arc<dyn EventLog>>,
    schema: Value,
}

impl fmt::Debug for ConversationReadTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationReadTool")
            .field("runtime_agent_id", &self.runtime_agent_id.get())
            .finish_non_exhaustive()
    }
}

impl ConversationReadTool {
    /// A `read` over `in_flight`, for the agent whose runtime id lands in
    /// `runtime_agent_id`, served from `events`; `None` refuses every call.
    #[must_use]
    pub fn new(
        in_flight: Arc<InFlightRegistry>,
        runtime_agent_id: Arc<std::sync::OnceLock<String>>,
        events: Option<Arc<dyn EventLog>>,
    ) -> Self {
        let schema = speech::tool_specs()
            .iter()
            .find(|spec| spec.name == READ_TOOL)
            .map(|spec| speech_descriptor(spec)["inputSchema"].clone())
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
        Self {
            in_flight,
            runtime_agent_id,
            events,
            schema,
        }
    }
}

#[async_trait::async_trait]
impl Tool for ConversationReadTool {
    fn name(&self) -> &str {
        READ_TOOL
    }

    fn description(&self) -> &str {
        "Read the recent messages of the conversation this turn answers in, oldest first. \
         Call it when you need more of the conversation than you were given."
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let limit = speech::read_limit(args.get("limit").and_then(Value::as_u64));
        let Some(events) = self.events.as_ref() else {
            tracing::debug!("[hive::tools] `read` called on an agent with no event log");
            return Ok(ToolResult::error(
                "refused: this agent has no conversation journal to read",
            ));
        };
        let turn = self
            .runtime_agent_id
            .get()
            .and_then(|id| self.in_flight.snapshot(id));
        let Some(turn) = turn else {
            tracing::debug!("[hive::tools] `read` called with no turn in flight");
            return Ok(ToolResult::error(
                "refused: no turn is in flight for this agent",
            ));
        };
        tracing::debug!(
            agent = %turn.runtime_agent_id,
            conversation = %turn.surface.id,
            limit,
            "[hive::tools] native `read`"
        );
        let read = read_conversation(
            Arc::clone(events),
            &turn.company,
            &turn.agent_id,
            &turn.surface,
            limit,
        )
        .await;
        Ok(match read {
            Ok(body) => ToolResult::success(body),
            Err(text) => ToolResult::error(text),
        })
    }
}
