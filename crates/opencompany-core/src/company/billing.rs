//! Secret-store keys for the Chargebee billing integration (issue #788).
//!
//! They live here, always compiled, rather than beside the REST client in
//! `crate::chargebee`: that module is gated on the `chargebee` feature, but the
//! **configuration surface** is not. An operator must be able to open Settings →
//! Billing and see "this build has no Chargebee support" rather than a 404, and
//! the webhook route ships in every build too. Sharing one definition is what
//! keeps the write plane and the read plane from drifting onto two spellings of
//! the same key.

/// Holds a company's Chargebee API key, written by the console's Billing
/// settings (#527) and read only to authenticate a call.
pub const API_KEY_SECRET: &str = "chargebee/api_key";

/// Holds a company's Chargebee site identifier — the `acme-test` in
/// `acme-test.chargebee.com`.
///
/// Stored beside the key because the pair only makes sense together: a site
/// without its key cannot be called, and a key pointed at the wrong site fails
/// in a way that reads like a bad key.
pub const SITE_SECRET: &str = "chargebee/site";

/// Holds the `username:password` pair Chargebee is configured to present on its
/// webhook deliveries, verified by `POST /hooks/{company}/chargebee`.
pub const WEBHOOK_SECRET_KEY: &str = "chargebee/webhook_secret";
