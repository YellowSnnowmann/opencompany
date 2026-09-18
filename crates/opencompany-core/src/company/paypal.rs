//! Secret-store keys and environment selection for the PayPal integration
//! (issue #789).
//!
//! Always compiled, for the same reason as [`crate::company::billing`]: the REST
//! client is gated on the `paypal` feature but the configuration surface is not,
//! so an operator sees "this build has no PayPal support" rather than a 404.

use serde::{Deserialize, Serialize};

/// Holds a company's PayPal REST app client id.
pub const CLIENT_ID_SECRET: &str = "paypal/client_id";

/// Holds a company's PayPal REST app secret.
pub const CLIENT_SECRET_SECRET: &str = "paypal/client_secret";

/// Holds `sandbox` or `live` — which PayPal environment the credentials belong
/// to.
///
/// Stored rather than inferred because the two are indistinguishable from the
/// credential itself: a sandbox client id against the live host authenticates
/// as nobody, and the error says "invalid client", which reads as a typo rather
/// than as pointing at the wrong world.
pub const ENVIRONMENT_SECRET: &str = "paypal/environment";

/// Which PayPal environment a company's credentials belong to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaypalEnvironment {
    /// developer.paypal.com test accounts. The default: a mis-set environment
    /// should read fake money, never move real money.
    #[default]
    Sandbox,
    /// Real accounts, real balances.
    Live,
}

impl PaypalEnvironment {
    /// The API base for this environment, without a trailing slash.
    pub fn base_url(self) -> &'static str {
        match self {
            Self::Sandbox => "https://api-m.sandbox.paypal.com",
            Self::Live => "https://api-m.paypal.com",
        }
    }

    /// Parses a stored/`PUT` value, defaulting to sandbox.
    ///
    /// Anything unrecognised is sandbox, not an error: the failure mode of
    /// guessing wrong here is reading a fake balance, whereas defaulting to
    /// `live` on a typo would point an agent at real money.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "live" | "production" => Self::Live,
            _ => Self::Sandbox,
        }
    }

    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Live => "live",
        }
    }
}

#[cfg(test)]
#[path = "paypal_tests.rs"]
mod tests;
