//! `GET {scope}/agents/{agent_id}/messages?after=&limit=` (OC-2): a teammate's
//! direct-message traffic in the company hive, as the journal recorded it.
//!
//! Since OC-2 one teammate reaches another with `hivemind_send_agent` rather
//! than a desk hand-off, and the answer comes back on a later turn. Those
//! lines never appear on a desk's chat — they are between two agents — so the
//! console reads them here: every [`CompanyEvent::HiveMessage`] the agent sent
//! or was sent directly, oldest first, after an exclusive journal cursor. The
//! cursor is the journal sequence of the last row the caller has, so a poller
//! passes back the `seq` of the newest row it got.

use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::ports::types::{CompanyEvent, EventSeq, HiveDestination, StoredEvent};
use crate::server::error::ApiError;
use crate::server::ops::{ScopedCompany, scoped};

/// The most rows one read returns, whatever `limit` asks for.
pub const MAX_MESSAGES: usize = 500;

/// The journal rows scanned past the cursor per read: the messages are found
/// among every other kind of row, so this bounds the work, not the answer.
const SCAN_WINDOW: usize = 20_000;

/// Builds the agent-messages route fragment.
pub fn router() -> Router<AppState> {
    scoped("/agents/{agent_id}/messages", get(agent_messages))
}

#[derive(Debug, Deserialize)]
struct AgentPath {
    agent_id: String,
}

/// The read's cursor and page size.
#[derive(Debug, Default, Deserialize)]
struct MessagesQuery {
    /// Exclusive journal sequence; rows after it are returned.
    #[serde(default)]
    after: Option<u64>,
    /// How many rows at most; capped at [`MAX_MESSAGES`].
    #[serde(default)]
    limit: Option<usize>,
}

/// One direct line between two teammates.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageDto {
    /// The journal sequence the line was recorded at — the next read's
    /// `after`.
    pub seq: u64,
    /// The hive transcript sequence it was accepted at.
    pub sequence: u64,
    /// Who sent it (a manifest agent id).
    pub sender: String,
    /// Who it was sent to (a manifest agent id).
    pub recipient: String,
    /// What was said.
    pub text: String,
    /// The episode it was said in, when the sender was in one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<String>,
    /// When the row was recorded, in Unix milliseconds.
    pub at_millis: u64,
}

/// The direct lines in `rows` that `agent` sent or received, at most `limit`.
#[must_use]
pub fn direct_messages(rows: &[StoredEvent], agent: &str, limit: usize) -> Vec<AgentMessageDto> {
    rows.iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::HiveMessage {
                sequence,
                sender,
                destination: HiveDestination::Agent(recipient),
                text,
                episode_id,
                ..
            } if sender == agent || recipient == agent => Some(AgentMessageDto {
                seq: row.seq.value(),
                sequence: *sequence,
                sender: sender.clone(),
                recipient: recipient.clone(),
                text: text.clone(),
                episode_id: episode_id.clone(),
                at_millis: row.at_millis,
            }),
            _ => None,
        })
        .take(limit)
        .collect()
}

/// `GET {scope}/agents/{agent_id}/messages` — 404 for an agent that is not on
/// the roster, so a typo is not an empty conversation.
async fn agent_messages(
    company: ScopedCompany,
    Path(AgentPath { agent_id }): Path<AgentPath>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<Vec<AgentMessageDto>>, ApiError> {
    let runtime = &company.runtime;
    let record = runtime.record();
    if !record.is_roster_agent(&agent_id) {
        return Err(ApiError::not_found(format!(
            "no teammate `{agent_id}` in this company"
        )));
    }
    let from = query.after.map_or(0, |after| after.saturating_add(1));
    let rows = runtime
        .events()
        .read_from(company.id(), EventSeq::new(from), SCAN_WINDOW)
        .await
        .map_err(ApiError::from)?;
    let limit = query.limit.unwrap_or(MAX_MESSAGES).clamp(1, MAX_MESSAGES);
    Ok(Json(direct_messages(&rows, &agent_id, limit)))
}

#[cfg(test)]
#[path = "agent_messages_tests.rs"]
mod tests;
