//! Presence and typing routes.
//!
//! * `GET {scope}/presence` — who is here right now.
//! * `PUT {scope}/presence` — a heartbeat, and a status.
//! * `DELETE {scope}/presence` — a clean disconnect.
//! * `POST {scope}/chat/typing` — one typing ping.
//!
//! # The subject is always the caller
//!
//! No body here names a user. The subject is taken from the session, every
//! time. A body that could name somebody else would let any member mark a
//! colleague online, offline, or typing, and nothing downstream could tell that
//! from the real thing — the frames are identical. This is the rule `block/buzz`
//! learned the hard way and states in its own presence code: a self-signed
//! presence event's subject is always its author, never a field inside it.
//!
//! # Signed-in humans only
//!
//! Same `401` as [`read_state`](super::read_state), for the same reason: a
//! machine credential names no person. Presence is *about* people, so a
//! credential with nobody behind it has nothing to announce and no dot to own.
//!
//! # Typing stores nothing
//!
//! `POST …/chat/typing` publishes one frame and returns `204`. There is no
//! typing registry and no "stopped typing" route: the frame carries its own
//! moment, the console expires it after a few seconds, and the absence of a
//! renewal *is* the stop signal. A console that closes mid-word therefore
//! clears itself with no teardown to get wrong.

use crate::server::error::Rejection;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::ports::now_millis;
use crate::server::ops::scope::{ScopedCompany, scoped};
use crate::server::presence::{PresenceStatus, PresenceView};
use crate::turn_stream::{PresenceFrame, TypingFrame};

pub fn router() -> Router<AppState> {
    scoped(
        "/presence",
        get(list_presence).put(announce).delete(disconnect),
    )
    .merge(scoped("/chat/typing", post(typing)))
}

/// `GET {scope}/presence` — everyone whose lease is still good.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PresenceListDto {
    /// Present people, most recently seen first.
    ///
    /// **This replica's view.** A second host serving the same tenant keeps its
    /// own map, so somebody connected there is absent here rather than shown as
    /// offline — which is why the console treats an absence as "no live signal"
    /// and falls back to the durable last-seen, instead of drawing a grey dot.
    people: Vec<PresenceView>,
}

/// `PUT {scope}/presence` — a heartbeat.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnounceBody {
    /// What to appear as. Note there is deliberately no `userId`: see the
    /// module header.
    status: PresenceStatus,
    /// Which of this person's open tabs is announcing (issue: multi-tab
    /// detach). An opaque value the console mints once per tab and holds for
    /// its lifetime — never a second identity, just a second dimension of the
    /// same authenticated one, so it carries no impersonation risk the module
    /// header's rule would need to police.
    ///
    /// Missing on an older console: every tab from one falls onto
    /// [`DEFAULT_CONSOLE`], which reproduces today's one-lease-per-user
    /// behaviour for a client that predates this field.
    #[serde(default)]
    console_id: Option<String>,
}

/// `DELETE {scope}/presence` — a clean disconnect, naming which tab left.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisconnectQuery {
    #[serde(default)]
    console_id: Option<String>,
}

/// The lease key an older console's requests — ones with no `consoleId` at
/// all — collapse onto, so every tab from one that predates this field keeps
/// sharing a single lease exactly as before.
const DEFAULT_CONSOLE: &str = "default";

/// `POST {scope}/chat/typing` — one ping.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TypingBody {
    /// The channel being typed in.
    chat_id: String,
    /// The thread inside it, when the composer is a thread's.
    #[serde(default)]
    parent_id: Option<String>,
}

async fn list_presence(
    State(state): State<AppState>,
    company: ScopedCompany,
) -> Result<Json<PresenceListDto>, Rejection> {
    let Some(_) = actor_id(&company) else {
        return Err(unauthorized().into());
    };
    Ok(Json(PresenceListDto {
        people: state.presence().list(company.id(), now_millis()),
    }))
}

async fn announce(
    State(state): State<AppState>,
    company: ScopedCompany,
    Json(body): Json<AnnounceBody>,
) -> Result<StatusCode, Rejection> {
    let Some(user) = actor_id(&company) else {
        return Err(unauthorized().into());
    };
    // An "offline" announcement is a disconnect, not a lease to store. `list`
    // only ever reads live leases, so an `Offline` entry inserted here would
    // still answer present to `GET /presence` — a viewer refreshing mid-TTL
    // would see somebody the live stream just told them left. Route it through
    // the same `detach` the DELETE takes so the registry and the wire agree.
    let console = body.console_id.as_deref().unwrap_or(DEFAULT_CONSOLE);
    if body.status == PresenceStatus::Offline {
        return disconnect(
            State(state),
            company,
            Query(DisconnectQuery {
                console_id: body.console_id,
            }),
        )
        .await;
    }
    let at = now_millis();
    // Published on a *change* only. A console beats every minute whether or not
    // anything moved, so announcing every renewal would put one frame per
    // person per minute on every open console for no visible difference.
    if state
        .presence()
        .beat(company.id(), &user, console, body.status, at)
    {
        crate::turn_stream::publish(
            company.id(),
            PresenceFrame {
                kind: "presence",
                user_id: user,
                status: body.status.as_str(),
                at_millis: at,
            },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn disconnect(
    State(state): State<AppState>,
    company: ScopedCompany,
    Query(query): Query<DisconnectQuery>,
) -> Result<StatusCode, Rejection> {
    let Some(user) = actor_id(&company) else {
        return Err(unauthorized().into());
    };
    let console = query.console_id.as_deref().unwrap_or(DEFAULT_CONSOLE);
    // Only announce a change the person's aggregate status actually made — a
    // duplicate teardown, or a tab closing while another the same person has
    // open already carried the same aggregate status, publishes nothing. When
    // it does change, publish *that* status: closing the online tab while an
    // away one is still open reports `away`, not `offline` — the person is
    // still here, just not at their most-present console anymore. `Offline`
    // only comes back once nobody is left.
    if let Some(status) = state
        .presence()
        .detach(company.id(), &user, console, now_millis())
    {
        crate::turn_stream::publish(
            company.id(),
            PresenceFrame {
                kind: "presence",
                user_id: user,
                status: status.as_str(),
                at_millis: now_millis(),
            },
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn typing(
    company: ScopedCompany,
    Json(body): Json<TypingBody>,
) -> Result<StatusCode, Rejection> {
    let Some(user) = actor_id(&company) else {
        return Err(unauthorized().into());
    };
    if body.chat_id.trim().is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "chatId must not be empty", "code": "invalid_request" })),
        )
            .into_response()
            .into());
    }
    crate::turn_stream::publish(
        company.id(),
        TypingFrame {
            kind: "typing",
            user_id: user,
            chat_id: body.chat_id,
            parent_id: body.parent_id,
            at_millis: now_millis(),
        },
    );
    Ok(StatusCode::NO_CONTENT)
}

/// The signed-in person behind the request, if there is one.
fn actor_id(company: &ScopedCompany) -> Option<String> {
    company.actor.as_ref().map(|a| a.id.clone())
}

/// The `401` for a caller with no person behind it.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "presence is about people, and this credential names none",
            "code": "unauthorized",
        })),
    )
        .into_response()
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod tests;
