//! Secret-store keys for a company's hosting provider connection.
//!
//! They live here, always compiled, rather than beside the harness wiring in
//! [`crate::harness::hosting`]: that module is gated on the `openhuman` feature,
//! but the **configuration surface** is not. An operator must be able to open
//! Settings → Hosting and see "this build has no hosting support" rather than a
//! 404. Sharing one definition is what keeps the write plane and the read plane
//! from drifting onto two spellings of the same key — the same argument as
//! [`crate::company::billing`].

/// Holds a company's hosting provider slug — today, `vercel`.
///
/// Stored rather than assumed because it decides which API the key is presented
/// to, and a key sent to the wrong provider fails in a way that reads like a bad
/// key.
pub const PROVIDER_SECRET: &str = "hosting/provider";

/// Holds a company's hosting provider API key, written by the console's Hosting
/// settings and read only to authenticate a call.
pub const API_KEY_SECRET: &str = "hosting/api_key";

/// Holds the team, organization, or account scope to deploy under. Optional: a
/// personal account has none.
pub const TEAM_SECRET: &str = "hosting/team";

/// The provider used when a company saved a key without naming one.
///
/// A default is safe here in a way it would not be for the key: the console
/// offers one provider today, and a company that saved a key through it meant
/// that provider. Adding a second provider makes this the *migration* value for
/// rows written before the field existed, which is exactly what it should be.
pub const DEFAULT_PROVIDER: &str = "vercel";
