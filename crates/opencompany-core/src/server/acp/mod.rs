//! The Agent Client Protocol, served by this host.
//!
//! An ACP client — the desktop, Zed, `acpx` — connects here and drives a
//! company's agents with the same protocol it uses for a local harness. That is
//! the point of the split: one session model, whether the agent is a
//! subprocess on the operator's laptop or a desk on a hosted company.
//!
//! ## One endpoint, N sessions
//!
//! `/acp` is not company-scoped in the URL. The company is chosen inside
//! `session/new`, which is the only shape consistent with ACP's
//! multiple-sessions-per-connection model and with one host serving several
//! companies. Scoping the path would mean a connection per company and would
//! make `Acp-Connection-Id` meaningless.
//!
//! ## Why `/acp` must be a reserved prefix
//!
//! The console SPA is the fallback for unmatched paths. Without a reservation,
//! a build *without* the `acp` feature would answer a client's probe with
//! `index.html` and a `200` — and the client cannot tell "no ACP here" from
//! "here is your JSON-RPC". A 404 is the honest answer, and
//! `console_does_not_shadow_unmatched_reserved_paths` is what keeps it one.

pub mod approvals;
pub mod map;
pub mod session;
mod transport;

pub use approvals::parked_notification;
pub use session::{AcpSession, SessionRegistry, SessionSweeper};

/// The authenticated HTTP transport for ACP JSON-RPC requests.
pub fn router() -> axum::Router<crate::AppState> {
    transport::router()
}
