//! The host's own return leg for a TinyHumans key grant.
//!
//! `GET /auth/key/callback?company=…&state=…&code=…` is where the hub sends the
//! browser back when nothing better serves the console at this host's origin.
//! That is the desktop: its console is a webview whose requests reach the
//! embedded host through the shell's Rust proxy, so `credential/link/start`
//! sees no `Origin` to send the browser back to, and the host serves no page at
//! `/` for the console's own return logic to run on. Before this route the
//! bind fallback pointed the hub at `http://{bind}/` — a 404 with a spent code
//! on the address bar, and `http://127.0.0.1:0/` on top of that before the
//! shell recorded the port it actually bound, which Chrome refuses outright
//! (`ERR_UNSAFE_PORT`).
//!
//! Mounted at the **top level**, **without** console auth, exactly as the MCP
//! OAuth callback is (`server::mcp_oauth`, issue #90): a browser redirect from
//! the hub carries no console session, so the route cannot require one. Its
//! trust is the parked `state` — single-use, bound to one company, expired
//! after [`PENDING_TTL`](crate::server::hub_link::PENDING_TTL) — plus the PKCE
//! verifier that never left this process. Redemption itself is
//! [`redeem_link`](crate::server::ops::company_key::redeem_link), the same code
//! the console's `POST …/credential/link/finish` runs, so the two return legs
//! cannot store or journal differently.
//!
//! The console never links here: `credential/link/start` picks this path only
//! as its last fallback (see `callback_base` there). A hosted tenant and a
//! Vite dev server both keep returning to `/`, where `App.tsx` finishes the
//! exchange and can offer the model step a grant sometimes needs.

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::AppState;
use crate::ports::types::{Actor, ActorKind};
use crate::server::ops::company_key::redeem_link;

/// The route, as `credential/link/start` appends it to `host_base_url`.
pub const PATH: &str = "/auth/key/callback";

/// Who a grant redeemed on this route is journaled as: the person who approved
/// it on the hub. There is no console session to name, and `System` would say
/// a timer did it.
const ACTOR_ID: &str = "hub-key-grant";

/// The callback query. Every field is optional so a denial or a malformed
/// redirect gets a readable page rather than a `422` from the extractor.
#[derive(Debug, Deserialize)]
struct CallbackQuery {
    #[serde(default)]
    company: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    code: Option<String>,
    /// The hub's `error` code — `access_denied` when the person pressed Cancel.
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// The top-level, unauthenticated route fragment (merged in
/// [`crate::server::routes`]).
pub fn router() -> Router<AppState> {
    Router::new().route(PATH, get(callback))
}

async fn callback(State(state): State<AppState>, Query(query): Query<CallbackQuery>) -> Response {
    if let Some(error) = non_empty(query.error.as_deref()) {
        let detail = non_empty(query.error_description.as_deref()).unwrap_or(error);
        return failure_page(
            StatusCode::BAD_REQUEST,
            "Connection declined",
            &format!("TinyHumans did not issue a key: {detail}"),
        );
    }

    let (Some(company), Some(link_state), Some(code)) = (
        non_empty(query.company.as_deref()),
        non_empty(query.state.as_deref()),
        non_empty(query.code.as_deref()),
    ) else {
        return failure_page(
            StatusCode::BAD_REQUEST,
            "Invalid return",
            "The return from TinyHumans was missing its company, state or code. Start again from the app.",
        );
    };

    // The same company gate the scoped extractors apply, minus the session:
    // the parked link is what proves this request may act for the company,
    // and `redeem_link` refuses a `state` parked for any other one.
    let Some(runtime) = state
        .registry()
        .get(&crate::ports::types::CompanyId::new(company))
    else {
        return failure_page(
            StatusCode::NOT_FOUND,
            "Company not found",
            "The company this key was minted for is no longer on this host.",
        );
    };

    let actor = Actor {
        kind: ActorKind::Operator,
        id: ACTOR_ID.to_string(),
    };
    match redeem_link(&state, &runtime, &actor, link_state, code).await {
        Ok(done) => {
            tracing::info!(
                "[hub-link] key grant redeemed on the host's return route company={}",
                runtime.id().as_ref()
            );
            success_page(&done.note)
        }
        Err(error) => {
            // The message never carries the key — `redeem_key_grant` reports
            // the hub's status, and the fan-out reports slot names — but it
            // is shown in a bare tab, so it is escaped like anything else.
            tracing::warn!(
                "[hub-link] key grant failed on the host's return route company={}: {}",
                runtime.id().as_ref(),
                error.0
            );
            failure_page(
                error.status(),
                "Couldn't connect",
                &format!("{}. Start again from the app.", error.0),
            )
        }
    }
}

/// `Some(trimmed)` when a query value is present and non-blank.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn success_page(note: &str) -> Response {
    Html(page(
        "Connected",
        &format!(
            "{} You can close this tab and go back to OpenCompany.",
            escape(note)
        ),
    ))
    .into_response()
}

fn failure_page(status: StatusCode, title: &str, message: &str) -> Response {
    (status, Html(page(title, &escape(message)))).into_response()
}

/// Minimal HTML-escaping for text the hub or an error produced.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// One inline, dependency-free document: this is served to a bare browser tab
/// that has no console to load assets from.
fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title}</title>\
<style>body{{font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;\
background:#0b0f14;color:#e6edf3;display:flex;min-height:100vh;margin:0;\
align-items:center;justify-content:center}}.card{{max-width:26rem;padding:2rem;\
background:#111820;border:1px solid #1f2933;border-radius:12px;text-align:center}}\
h1{{font-size:1.25rem;margin:0 0 .75rem}}p{{margin:0;color:#9fb0c0;line-height:1.5}}\
</style></head><body><div class=\"card\"><h1>{title}</h1><p>{body}</p></div></body></html>",
        title = escape(title),
        body = body,
    )
}

#[cfg(test)]
#[path = "hub_link_callback_tests.rs"]
mod tests;
