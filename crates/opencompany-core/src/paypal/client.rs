//! A PayPal REST client: OAuth2 client-credentials, with the access token
//! cached for its lifetime.
//!
//! # Why the token is cached
//!
//! PayPal issues a bearer token from `POST /v1/oauth2/token` that lasts about
//! nine hours. Fetching one per call would double every request and, at any
//! volume, run into PayPal's rate limit on the token endpoint specifically —
//! which fails the *next* call rather than the one that caused it. So a token is
//! held until shortly before it expires and then re-fetched.
//!
//! The cache lives on the client, and the client is built per company, so one
//! company's token can never authenticate another's call.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

use crate::company::paypal::PaypalEnvironment;
use crate::error::{OpenCompanyError, Result};

/// A cached bearer token and when it stops being usable.
// No `Debug`. This holds a live bearer token, and a derived `Debug` prints it
// in full — one `{:?}` in a log line or a panic message is the whole
// credential. `PaypalClient` already implements `Debug` by hand and omits this
// field for the same reason; deriving it here would reopen the hole one level
// down.
#[derive(Clone)]
struct CachedToken {
    token: String,
    /// When to stop trusting it. Deliberately earlier than PayPal's own expiry
    /// — see [`PaypalClient::token`].
    good_until: Instant,
}

/// One company's PayPal credentials and environment.
#[derive(Clone)]
pub struct PaypalConfig {
    /// The REST app client id.
    pub client_id: String,
    /// The REST app secret.
    pub client_secret: String,
    /// Which PayPal world these belong to.
    pub environment: PaypalEnvironment,
}

/// Prints the environment and **redacts both halves of the credential**.
///
/// Same reasoning as `ChargebeeConfig`: this is reachable from large aggregates
/// that debugging code prints wholesale, and a derived `Debug` would put a live
/// PayPal secret into any log line that formatted one.
impl std::fmt::Debug for PaypalConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaypalConfig")
            .field("environment", &self.environment)
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

/// A PayPal API client bound to one company's credentials.
#[derive(Clone)]
pub struct PaypalClient {
    http: reqwest::Client,
    config: PaypalConfig,
    base_url: String,
    cached: Arc<Mutex<Option<CachedToken>>>,
}

impl std::fmt::Debug for PaypalClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaypalClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

/// Builds the crate error for a PayPal failure.
fn err(status: u16, code: &str, message: impl Into<String>) -> OpenCompanyError {
    OpenCompanyError::Paypal {
        status,
        code: code.to_string(),
        message: message.into(),
    }
}

/// Reports a body PayPal did not describe, WITHOUT putting it in the message.
///
/// Same rule as the Chargebee client's `unparsed_body_message`, for the same
/// reason: PayPal's own `message` / `error_description` is a classified failure
/// the agent should read, but a body carrying neither is unidentified text on a
/// payments API, and this string reaches the model's context and the turn's
/// durable transcript.
fn unparsed_body_message(status: u16, body: &str) -> String {
    tracing::warn!(
        status,
        body = %body.chars().take(200).collect::<String>(),
        "[paypal] response body carried no error description"
    );
    format!(
        "PayPal returned {status} with a body this host could not interpret. The body is in the \
         host log; it is not reproduced here because its contents are unknown and may carry \
         account data."
    )
}

/// Checks that `path` is a plain absolute path before anything pastes it onto
/// the base URL.
///
/// [`PaypalClient::get`] builds its URL by concatenation, and concatenation is
/// not host-safe: `"https://api.paypal.com"` followed by `"@evil.com/v1"` parses
/// as userinfo `api.paypal.com` against host `evil.com`, so the bearer token
/// goes to whoever owns that name. No caller passes a dynamic path today — both
/// call sites in [`crate::paypal::api`] are literals — but `get` is `pub`, this
/// is a payments client, and the obvious next operation takes an id from a tool
/// argument. Making the URL unforgeable here costs a comparison per call and
/// means that caller cannot introduce the hole by accident.
///
/// `?` and `#` are refused for a different reason: the query is a separate
/// argument that reqwest appends, so a path carrying its own would silently
/// merge with it or truncate it. `//` is not exploitable against a base URL that
/// already carries a scheme and host, but no PayPal endpoint has such a path,
/// and allowing it would mean a reader has to re-derive the URL grammar to
/// convince themselves of that.
fn check_path(path: &str) -> Result<()> {
    let rejected = !path.starts_with('/')
        || path.starts_with("//")
        || path.contains('@')
        || path.contains('?')
        || path.contains('#')
        || path.chars().any(|c| c.is_whitespace() || c.is_control());
    if rejected {
        return Err(err(
            0,
            "invalid_path",
            // The path itself is not echoed: it is the untrusted half of this
            // check, and this message reaches the model's context.
            "refusing to build a PayPal request from a path that is not a plain absolute path",
        ));
    }
    Ok(())
}

impl PaypalClient {
    /// Builds a client against the environment named in `config`.
    pub fn new(config: PaypalConfig) -> Result<Self> {
        let base = config.environment.base_url().to_string();
        Self::with_base_url(config, base)
    }

    /// Builds a client against an explicit base URL, with no trailing slash.
    pub fn with_base_url(config: PaypalConfig, base_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            // The token call carries HTTP Basic and every other call a bearer;
            // reqwest re-sends both across redirects, so a 30x to `http://`
            // would leak them. PayPal's API does not redirect.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| err(0, "client_build_failed", e.to_string()))?;
        Ok(Self {
            http,
            config,
            base_url: base_url.trim_end_matches('/').to_string(),
            cached: Arc::new(Mutex::new(None)),
        })
    }

    /// A usable bearer token, from cache when one is still good.
    async fn token(&self) -> Result<String> {
        let mut slot = self.cached.lock().await;
        if let Some(cached) = slot.as_ref()
            && Instant::now() < cached.good_until
        {
            return Ok(cached.token.clone());
        }

        let response = self
            .http
            .post(format!("{}/v1/oauth2/token", self.base_url))
            .basic_auth(&self.config.client_id, Some(&self.config.client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            // `without_url` keeps reqwest's cause — connection refused, TLS
            // failure, timeout — and drops the URL it would otherwise print.
            // The host is not a secret, but the query string is caller-shaped,
            // and this text reaches the model's context and the transcript.
            .map_err(|e| err(0, "transport_error", e.without_url().to_string()))?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| err(status, "unreadable_body", e.to_string()))?;
        let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);

        if !(200..300).contains(&status) {
            // PayPal answers bad credentials with `invalid_client`, which reads
            // like a typo. Naming the environment turns the commonest actual
            // cause — sandbox keys against live, or the reverse — into
            // something an operator can see.
            return Err(err(
                status,
                parsed
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("token_failed"),
                format!(
                    "could not obtain a PayPal token for the `{}` environment: {}",
                    self.config.environment.as_str(),
                    parsed
                        .get("error_description")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| unparsed_body_message(status, &body))
                ),
            ));
        }

        let token = parsed
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                err(
                    status,
                    "unexpected_response",
                    "no `access_token` in the reply",
                )
            })?
            .to_string();

        // Re-fetch before PayPal's own expiry rather than at it: a token that
        // expires mid-flight fails the call that carried it, and a minute of
        // margin costs nothing against a nine-hour lifetime.
        let lifetime = token_lifetime(
            parsed
                .get("expires_in")
                .and_then(Value::as_u64)
                .unwrap_or(300),
        );
        *slot = Some(CachedToken {
            token: token.clone(),
            good_until: Instant::now() + Duration::from_secs(lifetime),
        });
        Ok(token)
    }

    /// `GET path` with query parameters, returning the decoded JSON.
    ///
    /// `path` must be a plain absolute path — see [`check_path`]. It is checked
    /// before the token is fetched, so a rejected path costs no round trip and,
    /// more to the point, never puts a credential on the wire.
    pub async fn get(&self, path: &str, query: &[(String, String)]) -> Result<Value> {
        check_path(path)?;
        let token = self.token().await?;
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .bearer_auth(token)
            .query(query)
            .send()
            .await
            // `without_url` keeps reqwest's cause — connection refused, TLS
            // failure, timeout — and drops the URL it would otherwise print.
            // The host is not a secret, but the query string is caller-shaped,
            // and this text reaches the model's context and the transcript.
            .map_err(|e| err(0, "transport_error", e.without_url().to_string()))?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| err(status, "unreadable_body", e.to_string()))?;
        let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);

        if (200..300).contains(&status) {
            // A success whose body is not a JSON object is not a success we can
            // use: `Value::Null` flows on and every field read yields a default,
            // so a proxy's HTML 200 would read as an account with no balances
            // rather than a reported failure. Same rule as the Chargebee client.
            if !parsed.is_object() {
                return Err(err(
                    status,
                    "unexpected_response",
                    unparsed_body_message(status, &body),
                ));
            }
            return Ok(parsed);
        }
        Err(err(
            status,
            parsed
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            parsed
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| unparsed_body_message(status, &body)),
        ))
    }
}

/// How long to trust a token PayPal says lasts `expires_in` seconds.
///
/// Re-fetch before PayPal's own expiry rather than at it: a token that expires
/// mid-flight fails the call that carried it, and a minute of margin costs
/// nothing against a nine-hour lifetime.
///
/// The floor must not outlive the token it is a floor for. On a ten-second
/// `expires_in`, `saturating_sub(60)` is 0 and a bare `.max(30)` would cache a
/// credential three times longer than it is valid — handing out a dead token,
/// which is the exact failure the margin exists to prevent.
fn token_lifetime(expires_in: u64) -> u64 {
    expires_in.saturating_sub(60).max(30).min(expires_in)
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
