//! Chargebee billing, integrated as backend service code and surfaced to agents
//! as callable tools (issue #788).
//!
//! The operator's problem this exists for: today they leave OpenCompany, log
//! into Chargebee, create a customer, fill out an invoice form and send it —
//! then repeat the trip to find out whether it was paid. With these tools they
//! say it in chat instead.
//!
//! # Shape
//!
//! - [`types`] — configuration, tool arguments, and the compact projections
//!   returned to the agent. Every money field is named `*_in_minor_units`,
//!   because Chargebee is minor-unit based and an agent reading "$100" from a
//!   prompt will otherwise raise a $1.00 invoice that succeeds.
//! - [`client`] — the REST v2 transport: form-encoded writes, HTTP Basic auth,
//!   bracket-array nesting.
//! - [`api`] — the billing operations themselves, validated before any call.
//!
//! The agent-facing bridge lives in [`crate::harness::chargebee`], which turns
//! each `api` function into a tool. The split is deliberate: these shapes are
//! testable without a harness, and a tool description can change without
//! anyone touching the wire format.
//!
//! # Not an MCP server
//!
//! An earlier revision of #788 asked for one, and one was built. The issue was
//! then rewritten to put the integration in the backend service layer instead,
//! which is what this is. The change is not cosmetic — it moves the credential
//! from a separate process's environment into the company's own
//! [`SecretStore`](crate::ports::SecretStore), which is what makes it
//! per-tenant and what lets the console's Billing settings (#527) own it.
//!
//! # Credentials
//!
//! A company's Chargebee API key and site identifier live in its `SecretStore`
//! under [`types::API_KEY_SECRET`] and [`types::SITE_SECRET`], written by the
//! console and never present in a manifest, a tool argument, a tool result, or
//! a log line. No environment variable is consulted: two companies on one host
//! bill two different Chargebee sites, so a process-wide credential could only
//! ever be wrong.

pub mod api;
pub mod client;
pub mod types;

pub use client::ChargebeeClient;
pub use types::{API_KEY_SECRET, ChargebeeConfig, SITE_SECRET};
