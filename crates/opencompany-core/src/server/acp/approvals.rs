//! Approvals, outbound: this host does **not** implement
//! `session/request_permission`.
//!
//! ACP's permission model assumes a turn that *suspends*: the agent asks, the
//! client answers, the same tool call proceeds. This host's approvals do not
//! work that way, and the difference is structural rather than a matter of
//! plumbing.
//!
//! Read `harness/policy.rs`. OpenHuman resolves `RequireApproval` **fail-closed
//! and inline**: it blocks the call and feeds the model a refusal string. *The
//! call is gone.* There is no suspended continuation to resume. The projected
//! effect is parked after the turn, the turn completes with a reply, and a
//! human later resolves it — which mints a single-use grant and **re-dispatches
//! the agent**. That is a new turn.
//!
//! So by the time an approval exists to ask about, `session/prompt` has already
//! returned its `stopReason`. There is no point at which a blocking
//! `session/request_permission` could be issued, and implementing one would
//! mean it either never fires or fires against a turn that has ended.
//!
//! Instead a park is surfaced as a session-level notification carrying the
//! approval id ([`parked_notification`]), and the client resolves it over the
//! existing REST surface — which the desktop already has, because it runs the
//! same console.
//!
//! ## Inbound is a separate, already-answered question
//!
//! When this host is the ACP **client** — `src-tauri/src/acp/client.rs`
//! driving a locally-installed harness — an inbound `session/request_permission`
//! is answered synchronously by a `ClientHandler` implementation there, not
//! held for a human. `LocalAcpAgent`'s production handler (`AutoApprovingFiles`)
//! trusts the harness's own permission-mode config and auto-approves whatever
//! it still asks about; there is no queued, human-in-the-loop path on that
//! side today. A held-request TTL is meaningless without something that ever
//! holds a request, so this module carries none.

use serde_json::{Value, json};

/// The bare `SessionUpdate` for a parked effect.
///
/// Split from the notification envelope the same way [`super::map`] splits
/// [`from_turn_stream`](super::map::from_turn_stream) from
/// [`notification`](super::map::notification): a `session/prompt` **result**
/// carries update objects in its `updates` array, while a pushed
/// `session/update` carries one inside a JSON-RPC envelope. Offering only the
/// envelope made the result path embed a whole notification as an array
/// element, so a client switching on `sessionUpdate` — which every sibling
/// element answers — found nothing on that one.
///
/// Carries the approval id, because that is what the client resolves against
/// over REST. Sent as `_meta` rather than as a new protocol method: a
/// conforming client that has never heard of OpenCompany ignores it, which is
/// exactly what `_meta` is for.
pub fn parked_update(approval_id: &str, summary: &str) -> Value {
    json!({
        // No ACP variant means "a human must decide something". The closest
        // honest carrier is a session-info update whose `_meta` says what
        // actually happened.
        "sessionUpdate": "session_info_update",
        "_meta": {
            "opencompany/approval": {
                "id": approval_id,
                "summary": summary,
                // Where to resolve it. Told rather than assumed: a third-party
                // ACP client has no reason to know this host's REST shape.
                "resolve": "POST /api/v1/companies/{company}/approvals/{id}",
            }
        },
    })
}

/// [`parked_update`] wrapped in the `session/update` notification envelope, for
/// a transport that pushes frames mid-turn rather than returning them.
pub fn parked_notification(session_id: &str, approval_id: &str, summary: &str) -> Value {
    super::map::notification(session_id, parked_update(approval_id, summary))
}

#[cfg(test)]
#[path = "approvals_tests.rs"]
mod tests;
