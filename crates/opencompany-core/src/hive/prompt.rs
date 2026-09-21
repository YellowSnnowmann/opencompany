//! What one seat is handed for one turn.
//!
//! A seat runs on **one stable OpenHuman session** across every surface — the
//! runtime owns the thread, nothing is re-seeded — so a turn's message is the
//! delta, not the transcript: the desk rows the seat has not yet been shown
//! (`tinyhivemind::sharing::prepare_delta` over the journal, with a watermark
//! kept per seat on the episode's checkpoint), its assignment, and the one
//! instruction that makes a round a round: end the turn with exactly one
//! speech tool on the `opencompany` MCP server.
//!
//! Line one is a **sentinel**: `Hive turn: desk <deskId>, episode
//! <episodeId>, round <revision>.` The console's mock brain keys on it to
//! answer with a tool call (`frontend/test/e2e/mock-brain.mjs`), the driver's
//! tests assert it, and a transcript reader can tell a seat turn from a chat
//! turn by its first line. A retry turn keeps it in its last user message.
//!
//! Non-desk turns (a DM, `#general`, a workflow thread) get the delta prefix
//! and no fence: they answer with their reply, as they always have.

use tinyhivemind::aside::Viewer;
use tinyhivemind::speech::{ToolSpec, tool_specs};
use tinyhivemind::{
    Conversation, SESSION_WINDOW, Sequence, SessionAuthor, SessionLog, SessionMessage,
    SessionQuery, SharingPlan, SharingQuery, SharingState, initialized_state, prepare_delta,
    project_session,
};

/// The MCP server slug the speech tools are served under.
pub const SERVER_SLUG: &str = "opencompany";

/// The speech tools a seat on a desk of two or more may end its turn with.
pub const DESK_KINDS: &[&str] = &["post", "broadcast", "dm", "complete_episode"];

/// The narrower set a seat with nobody else in the room may use.
pub const SOLO_KINDS: &[&str] = &["post", "complete_episode"];

/// Line one of every seat turn.
#[must_use]
pub fn sentinel(desk_id: &str, episode_id: &str, revision: u64) -> String {
    format!("Hive turn: desk {desk_id}, episode {episode_id}, round {revision}.")
}

/// One seat turn's message, assembled from its parts.
#[derive(Clone, Debug)]
pub struct SeatPrompt<'a> {
    /// The desk.
    pub desk_id: &'a str,
    /// Its display name.
    pub desk_name: &'a str,
    /// The episode.
    pub episode_id: &'a str,
    /// The round revision (raw).
    pub revision: u64,
    /// The seat.
    pub agent_id: &'a str,
    /// What the seat is asked to do this round — the operator's message on
    /// the opening round, the broadcast that reopened it, or the standing
    /// instruction to carry its assignment on.
    pub assignment: &'a str,
    /// Rows the seat has not seen, oldest first.
    pub delta: &'a [SessionMessage],
    /// The speech tools the seat may end with.
    pub allowed: &'a [&'a str],
    /// A stricter reminder on a retry after a turn without a tool call.
    pub retry_note: Option<&'a str>,
}

impl SeatPrompt<'_> {
    /// Renders the message the seat receives.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = sentinel(self.desk_id, self.episode_id, self.revision);
        out.push_str("\n\n");
        if let Some(note) = self.retry_note {
            out.push_str("## Reminder\n");
            out.push_str(note);
            out.push_str("\n\n");
        }
        out.push_str(&format!(
            "You are @{} on desk #{} ({}).\n\n",
            self.agent_id, self.desk_id, self.desk_name
        ));
        out.push_str("## New desk messages\n");
        if self.delta.is_empty() {
            out.push_str("(none)\n\n");
        } else {
            out.push_str(&render_delta(self.delta));
            out.push_str("\n\n");
        }
        out.push_str("## This assignment\n");
        out.push_str(self.assignment.trim());
        out.push_str("\n\n");
        out.push_str(&tool_catalogue(self.allowed));
        out.push('\n');
        out.push_str(&fence(self.allowed));
        out
    }
}

/// The attributed rows, one per line, oldest first.
///
/// `@id (^seq): text` is the form a seat can cite back (`^seq`), and an
/// elided row keeps its sequence range so the seat knows an exchange it may
/// not read happened, and where its outcome will land.
#[must_use]
pub fn render_delta(messages: &[SessionMessage]) -> String {
    messages
        .iter()
        .map(|message| {
            let author = match &message.author {
                SessionAuthor::Operator => "operator".to_string(),
                SessionAuthor::Person { label, .. } => label.clone(),
                SessionAuthor::Agent { id, .. } => format!("@{id}"),
                SessionAuthor::System { kind, .. } => format!("[{kind}]"),
            };
            match (&message.elided, message.readable()) {
                (Some(elision), _) => format!(
                    "{author} (^{}–^{}): [{} private line(s) you may not read]",
                    message.sequence, elision.through, elision.messages
                ),
                (None, Some(text)) => format!("{author} (^{}): {text}", message.sequence),
                (None, None) => format!("{author} (^{}): [withheld]", message.sequence),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The speech tools, rendered verbatim from `tinyhivemind::speech`.
#[must_use]
pub fn tool_catalogue(allowed: &[&str]) -> String {
    let mut out = String::from("## How to speak\n");
    for spec in specs(allowed) {
        out.push_str(&format!("- `{}`: {}", spec.name, spec.description));
        let parameters: Vec<String> = spec
            .parameters
            .iter()
            .map(|parameter| {
                let mut line = format!("`{}`", parameter.name);
                if let Some(description) = parameter.description {
                    line.push_str(&format!(" — {description}"));
                }
                line
            })
            .collect();
        if !parameters.is_empty() {
            out.push_str(&format!(" Arguments: {}.", parameters.join("; ")));
        }
        out.push('\n');
    }
    out
}

/// The one-action rule, naming the server and the tools.
#[must_use]
pub fn fence(allowed: &[&str]) -> String {
    let names = specs(allowed)
        .map(|spec| format!("`{}`", spec.name))
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "You MUST end this turn with exactly one `mcp_call_tool` on server `{SERVER_SLUG}`: \
         tool {names}, with your message in the `message` argument (and the seat ids in `to` \
         for `dm`). Call it once, last. Text outside that call is your own thinking and \
         reaches nobody."
    )
}

/// The reminder a retry turn carries after a turn that called no speech tool.
#[must_use]
pub fn retry_note(attempt: u32) -> String {
    format!(
        "Your previous answer (attempt {attempt}) reached nobody: it did not end with a speech \
         tool call. Say what you have to say again, this time as ONE `mcp_call_tool` on server \
         `{SERVER_SLUG}`."
    )
}

fn specs<'a>(allowed: &'a [&'a str]) -> impl Iterator<Item = &'static ToolSpec> + 'a {
    tool_specs()
        .iter()
        .filter(move |spec| allowed.contains(&spec.name))
}

/// The delta prefix a non-desk turn (DM, `#general`, workflow) gets: the
/// rows the agent has not seen, or nothing when there are none.
#[must_use]
pub fn non_desk_prefix(delta: &[SessionMessage]) -> Option<String> {
    if delta.is_empty() {
        return None;
    }
    Some(format!(
        "## Since your last turn here\n{}\n\n",
        render_delta(delta)
    ))
}

/// The rows a seat has not been shown, and the sharing state to commit once
/// it has accepted them.
///
/// `state` is the seat's last committed progress on this conversation, or
/// `None` for its first turn here, in which case the seat is handed a bounded
/// recent window (`SESSION_WINDOW`) up to `before` and starts sharing from
/// there. A delta the library refuses to build incrementally (a gap wider
/// than its scan, a log that ended early) is answered the same way: a fresh
/// window, and a fresh watermark at `before`.
///
/// `before` is exclusive: the sequence of the row that triggered this turn
/// (the round's own `RoundStarted` row), so every chat row older than it is
/// delivered and the next turn starts exactly there.
pub async fn delta_for(
    log: &(dyn SessionLog + '_),
    conversation: &Conversation,
    viewer: &Viewer,
    state: Option<&SharingState>,
    before: Sequence,
) -> Result<(Vec<SessionMessage>, SharingState), tinyhivemind::Error> {
    if let Some(state) = state
        && before >= state.watermark
    {
        let query = SharingQuery {
            desired_conversation: conversation,
            current_conversation: conversation,
            state,
            viewer,
            before,
        };
        if let SharingPlan::Delta(delta) = prepare_delta(log, &query).await? {
            return Ok((delta.messages, delta.next_state));
        }
    }
    let messages = project_session(
        log,
        &SessionQuery {
            conversation: conversation.clone(),
            viewer: viewer.clone(),
            before: Some(before),
            window: SESSION_WINDOW,
        },
    )
    .await?;
    Ok((messages, initialized_state(conversation.clone(), before)))
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
