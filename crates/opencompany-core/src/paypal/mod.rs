//! PayPal wallet and transaction visibility (issue #789).
//!
//! Read-only by design. #789 lists `send_payment` as optional and requires a
//! scoping decision before implementation; moving money is not something to
//! ship on the strength of an "optional" line.
//!
//! # Relationship to Chargebee (#788)
//!
//! They are complementary, not alternatives. Invoices are created in Chargebee;
//! PayPal is configured as a payment method *inside* Chargebee, so an invoice's
//! payment routes through the connected PayPal account with no code here. What
//! this module adds is the read side — what is in the wallet, and what has
//! moved through it lately.
//!
//! # Credentials
//!
//! Client id + secret, per company, from that company's `SecretStore`. Not the
//! browser OAuth flow an earlier sketch described: that grants access to
//! *someone else's* PayPal account, and these tools read the company's own.
//! There is no third party for a popup to ask.

pub mod api;
pub mod client;

pub use client::{PaypalClient, PaypalConfig};
