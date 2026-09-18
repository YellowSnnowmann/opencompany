//! Per-person channel read markers: `GET`/`PUT {scope}/chat/read-state`
//! (issue #755).
//!
//! Unread used to be decided entirely in the browser, against a floor stamped
//! when the tab mounted. Everything older than that instant counted as read, so
//! a reload marked every channel caught up and two tabs of the same person
//! disagreed. This is the durable half: where each person has read to, per
//! channel, remembered by the host.
//!
//! **The count stays in the console.** Only the browser holds the transcript,
//! so only the browser can say how many messages sit past a marker. What moves
//! here is the *floor* that count is measured from — which is the part that has
//! to outlive the tab. Splitting it the other way would mean shipping every
//! channel's message count to the host on every poll.
//!
//! **Signed-in humans only.** A marker answers "how far has *this person*
//! read", and a machine credential is not a person — the platform scope reaches
//! these routes with [`ScopedCompany::actor`] as `None`. Rather than invent a
//! shared pseudo-user for it (which would let one tenant's automation clear a
//! real operator's badges), both routes answer `401` for a caller with no
//! person behind it.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::ports::read_state::ChannelRead;
use crate::server::ops::scope::{ScopedCompany, scoped};

pub fn router() -> Router<AppState> {
    scoped("/chat/read-state", get(list_read_state).put(mark_read))
}

/// One channel's marker, as the console reads it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadMarkerDto {
    channel_id: String,
    last_read_at: i64,
}

impl From<ChannelRead> for ReadMarkerDto {
    fn from(r: ChannelRead) -> Self {
        Self {
            channel_id: r.channel_id,
            last_read_at: r.last_read_at,
        }
    }
}

/// `GET {scope}/chat/read-state` — every marker this person holds.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadStateDto {
    /// Channels this person has opened, each with its floor.
    ///
    /// A channel absent from this list has never been opened by this person.
    /// The console decides what that means — it is the only side that knows
    /// whether the channel even has messages — rather than the host guessing a
    /// zero that would render a lifetime of history as unread.
    markers: Vec<ReadMarkerDto>,
}

/// `PUT {scope}/chat/read-state` — move one channel's floor forward.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MarkReadBody {
    channel_id: String,
    /// Milliseconds since the epoch, from the newest row the person has seen.
    last_read_at: i64,
}

async fn list_read_state(
    company: ScopedCompany,
) -> Result<Json<ReadStateDto>, crate::server::Rejection> {
    let Some(user) = actor_id(&company) else {
        return Err(unauthorized().into());
    };
    let markers = company
        .runtime
        .read_state()
        .list(company.id(), &user)
        .await?
        .into_iter()
        .map(ReadMarkerDto::from)
        .collect();
    Ok(Json(ReadStateDto { markers }))
}

async fn mark_read(
    company: ScopedCompany,
    Json(body): Json<MarkReadBody>,
) -> Result<Json<ReadMarkerDto>, crate::server::Rejection> {
    let Some(user) = actor_id(&company) else {
        return Err(unauthorized().into());
    };
    if body.channel_id.trim().is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "channelId must not be empty", "code": "invalid_request" })),
        )
            .into_response()
            .into());
    }
    // The stored marker is returned rather than the requested one, because
    // `mark` is monotonic: a late request carrying an earlier instant leaves the
    // marker where it was, and the console must see where it actually stands
    // rather than assume its own value took.
    let settled = company
        .runtime
        .read_state()
        .mark(company.id(), &user, &body.channel_id, body.last_read_at)
        .await?;
    Ok(Json(settled.into()))
}

/// The signed-in person behind the request, if there is one.
///
/// `Option` rather than `Result<_, Response>`: an axum `Response` is a large
/// error variant to carry through a helper, and the two call sites want the
/// same `401` anyway — so they build it once, from [`unauthorized`].
fn actor_id(company: &ScopedCompany) -> Option<String> {
    company.actor.as_ref().map(|a| a.id.clone())
}

/// The `401` for a caller with no person behind it.
///
/// See the module note: a machine credential has no person to attribute a
/// marker to, and inventing one would let automation clear a human's badges.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "read state is per person, and this credential names none",
            "code": "unauthorized",
        })),
    )
        .into_response()
}

#[cfg(test)]
#[path = "read_state_tests.rs"]
mod tests;
