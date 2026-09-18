//! The notification feed: `GET`/`PUT {scope}/notifications`.
//!
//! The first consumer was the mention badge; the runtime's failure/expiry
//! producers (`dispatch_failed`, `approval_expired`, `workflow_run_*`) and
//! the week-1 nudge banner (issue #1845) followed. `NotificationStore` was
//! wired into the runtime and unused since it was written (issue #749); this
//! is the surface that makes it reachable.
//!
//! No kind allowlist: `notifications()` has exactly one writer set (the
//! runtime's own notification producers), all of them user-facing, so
//! filtering by kind server-side would only recreate the "was this reachable
//! at all" bug the honest-verdicts work exists to close — a durable row
//! written but never returned to the one client that reads this store. Every
//! caller gets every unread row and filters to what it cares about
//! client-side (`pickActiveNudge`, the mention badge's own kind check, …) —
//! see `a_dispatch_failure_reaches_the_feed_alongside_a_mention` below.
//!
//! # Why this exists at all, when there is an SSE feed
//!
//! The live feed only reaches a browser that is **open**. A mention that lands
//! while somebody is asleep has to still be there when they come back, and that
//! is the entire job of this store — the module header on
//! [`crate::ports::notifications`] says so. A badge built from the live stream
//! alone would clear itself every time a tab was closed.
//!
//! # Delivery is polled, not pushed
//!
//! There is deliberately no `mention` frame on the company SSE feed. That
//! stream has **no per-viewer projection**, which is the documented reason
//! `ReactionToggled` is dropped from it entirely — a mention frame would have
//! to carry either everyone's user ids or nobody's. So the console refetches
//! this route on the poll it already runs, on each `agent_reply`, and on window
//! focus. Say that out loud rather than letting a reader assume it was an
//! oversight.
//!
//! # Signed-in humans only
//!
//! Same `401` as [`read_state`](super::read_state): a notification is addressed
//! to a person, and a machine credential names none.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::company::week1_nudge;
use crate::ports::notifications::NotificationView;
use crate::server::error::ApiError;
use crate::server::ops::scope::{ScopedCompany, scoped};

pub fn router() -> Router<AppState> {
    scoped("/notifications", get(list).put(mark_read))
}

/// One notification, as the person it is for reads it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationDto {
    id: String,
    /// A free-form tag — `"mention"`, `"dispatch_failed"`,
    /// `"approval_expired"`, or `"workflow_run_*"`.
    kind: String,
    /// What it is about: `task` / `run` / `approval` / `workflow` / `message`.
    subject_kind: String,
    /// The subject's id in its own id space. For a `message` that is the chat
    /// message id, so the console can link straight at it.
    subject_id: String,
    /// The line a person reads.
    title: String,
    created_at: u64,
    /// When this person read it; absent while unread **for them**.
    #[serde(skip_serializing_if = "Option::is_none")]
    read_at: Option<u64>,
    /// The console channel this belongs to, so a badge can be placed without
    /// the transcript being loaded. Absent on rows that name no channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

impl From<NotificationView> for NotificationDto {
    fn from(view: NotificationView) -> Self {
        let NotificationView {
            notification,
            read_at,
        } = view;
        Self {
            id: notification.id,
            kind: notification.kind,
            subject_kind: notification.subject.kind.as_str().to_string(),
            subject_id: notification.subject.id,
            title: notification.title,
            created_at: notification.created_at,
            read_at,
            context: notification.context,
            // `audience` is deliberately NOT projected. It is the list of
            // everyone else who was mentioned, and handing each recipient the
            // user ids of all the others is a disclosure the badge has no use
            // for. Who else was named is already visible, as labels, on the
            // message itself.
        }
    }
}

/// `GET {scope}/notifications`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedDto {
    /// Newest first.
    notifications: Vec<NotificationDto>,
    /// How many are still unread for this person — what the badge renders.
    unread: usize,
}

/// `PUT {scope}/notifications` — mark read.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MarkReadBody {
    /// The notifications to mark. **Absent or null marks everything** this
    /// person can see, which is what "clear the badge" means.
    ///
    /// An explicitly empty array marks nothing — a real distinction from
    /// absent, and the one a client sends when it has computed a set and found
    /// it empty.
    #[serde(default)]
    ids: Option<Vec<String>>,
}

/// `PUT {scope}/notifications` response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MarkReadDto {
    /// Still unread for this person after the mark — returned rather than
    /// assumed, because marking is a latch and two tabs race.
    unread: u64,
}

async fn list(company: ScopedCompany) -> Result<Json<FeedDto>, crate::server::Rejection> {
    let Some(user) = actor_id(&company) else {
        return Err(unauthorized().into());
    };
    let rows = company
        .runtime
        .notifications()
        .list(company.id(), &user)
        .await
        .map_err(|e| ApiError(e).into_response())?;
    // This endpoint is the durable-notification contract for every kind the
    // runtime appends here — `mention` and, as of the honest-verdicts work,
    // `dispatch_failed` / `approval_expired` / `workflow_run_*` / the week-1
    // nudge (issue #1845). There is no kind allowlist: `notifications()` has
    // exactly one writer set (the runtime's own notification producers), all
    // of them user-facing, so filtering by kind would only recreate the "was
    // this reachable at all" bug the honest-verdicts work exists to close —
    // a durable row written but never returned to the one client that reads
    // this store. Read rows are still excluded: the client only needs the
    // actionable, unread set.
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|view| view.read_at.is_none())
        .collect();
    // PR #1878 review finding (comment 3879491539): a week-1 nudge row is
    // filed once by `LifecycleScheduler::tick` and, before this, cleared only
    // by the one client call site that calls `markNotificationsRead` after a
    // create — `WorkflowsView`'s own dialog. A workflow created through any
    // OTHER attributed path (accepting a Tasks proposal, a future second
    // create surface) left the row unread forever: nothing server-side ever
    // re-asked whether the fact the row is about had since become true.
    // Reconciling here, on every read, is what makes the banner self-heal
    // regardless of which surface satisfied it, rather than requiring every
    // future create path to remember to clear this one specific row. Scoped
    // internally to nudge-kind rows — this feed now carries every kind, not
    // only the nudge, since the kind-scoped `?kind=` query was retired above.
    let rows = reconcile_stale_nudges(&company, &user, rows).await;
    let unread = rows.len();
    Ok(Json(FeedDto {
        notifications: rows.into_iter().map(NotificationDto::from).collect(),
        unread,
    }))
}

/// Drops every unread week-1 nudge row out of `rows` — marking each read
/// server-side — once `user` has saved a workflow through any attributed
/// path, per `list`'s own call-site comment. Every non-nudge row passes
/// through untouched regardless of outcome; this only ever acts on the
/// nudge-kind subset of a feed that otherwise carries every kind.
///
/// Best-effort on both reads it makes (the user lookup and
/// `user_saved_workflow_in_week1`'s journal scan): either failing just
/// leaves the row(s) showing for one more poll rather than erroring the
/// whole feed over a reconciliation that can always run again next time —
/// the same non-fatal posture the client's own `refreshNudge` already takes
/// on its half of this (see its doc comment). A `mark_read` failure is
/// swallowed the same way: the row is still excluded from THIS response
/// (the caller has already learned the underlying fact), and a mark that
/// truly never lands just gets retried by the next poll's reconciliation.
async fn reconcile_stale_nudges(
    company: &ScopedCompany,
    user: &str,
    rows: Vec<NotificationView>,
) -> Vec<NotificationView> {
    let has_nudge_row = rows
        .iter()
        .any(|view| view.notification.kind == week1_nudge::NUDGE_KIND);
    if !has_nudge_row {
        return rows;
    }
    let Ok(Some(record)) = company.runtime.users().get_user(company.id(), user).await else {
        // No user record to read a signup instant from (a race with the
        // account being removed, or a store hiccup) — leave the row(s)
        // exactly as filed rather than guess.
        return rows;
    };
    let satisfied = week1_nudge::user_saved_workflow_in_week1(
        company.id(),
        company.runtime.events(),
        user,
        record.created_at_millis,
        crate::ports::now_millis(),
    )
    .await
    .unwrap_or(false);
    if !satisfied {
        return rows;
    }
    let stale_ids: Vec<String> = rows
        .iter()
        .filter(|view| view.notification.kind == week1_nudge::NUDGE_KIND)
        .map(|v| v.notification.id.clone())
        .collect();
    let _ = company
        .runtime
        .notifications()
        .mark_read(company.id(), user, Some(&stale_ids))
        .await;
    rows.into_iter()
        .filter(|view| view.notification.kind != week1_nudge::NUDGE_KIND)
        .collect()
}

async fn mark_read(
    company: ScopedCompany,
    body: axum::body::Bytes,
) -> Result<Json<MarkReadDto>, crate::server::Rejection> {
    let Some(user) = actor_id(&company) else {
        return Err(unauthorized().into());
    };
    // Read from raw bytes rather than through `Json`, so an **empty** body is
    // "mark everything" whatever the caller's `Content-Type` says.
    //
    // `Option<Json<_>>` looks like the idiomatic answer and is not: it yields
    // `None` only when the content type is absent, so the very common
    // `PUT` with `Content-Type: application/json` and no body is a `400` — a
    // client clearing a badge the obvious way gets an error for it. Clearing a
    // badge must not require knowing that.
    let ids = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice::<MarkReadBody>(&body) {
            Ok(parsed) => parsed.ids,
            Err(err) => {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({
                        "error": format!("body is not a mark-read request: {err}"),
                        "code": "invalid_request",
                    })),
                )
                    .into_response()
                    .into());
            }
        }
    };
    let unread = company
        .runtime
        .notifications()
        .mark_read(company.id(), &user, ids.as_deref())
        .await
        .map_err(|e| ApiError(e).into_response())?;
    Ok(Json(MarkReadDto { unread }))
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
            "error": "notifications are per person, and this credential names none",
            "code": "unauthorized",
        })),
    )
        .into_response()
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;
