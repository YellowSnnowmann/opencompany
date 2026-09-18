//! Cross-origin support for the console, credentials included.
//!
//! Only needed for one shape: a Vite dev server on `:5173` talking to a host on
//! `:8080`. Same-origin deployments — the normal case, including hosted mode —
//! never exercise this. Use the Vite proxy and you will not need it at all.
//!
//! ## Why an allowlist, and not a wildcard
//!
//! The session is a cookie, so the browser only sends it cross-origin when the
//! response carries `Access-Control-Allow-Credentials: true`. The Fetch
//! standard forbids pairing that with `Access-Control-Allow-Origin: *` — a
//! wildcard is rejected outright by the browser. So the origin must be echoed
//! back explicitly, which means we must know which origins are allowed.
//!
//! That is not a formality. Echoing back whatever `Origin` arrives, with
//! credentials on, hands every site on the internet the ability to make
//! authenticated requests as the signed-in user and read the responses. This
//! module therefore echoes an origin **only** when it appears in a
//! deliberately configured list, and is off entirely by default.
//!
//! Hand-rolled rather than pulling `tower-http`: this is a handful of response
//! headers, and the crate has no tower middleware layer to hang one on.

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;

use crate::error::OpenCompanyError;

/// The schemes an allow-listed origin may use.
///
/// `http`/`https` cover browsers. The rest are the origins a native webview
/// reports itself as, which are **not** http URLs and were rejected outright
/// before — meaning a desktop client could not be allow-listed at all, even
/// deliberately:
///
/// - `tauri://` — Tauri on macOS and Linux.
/// - `capacitor://` — the same problem shape on mobile shells.
///
/// Tauri on Windows reports `http://tauri.localhost`, which needs nothing here.
///
/// This widens what may be *written* in the allowlist; it does not widen what
/// matches. Comparison stays byte-exact, so adding a scheme cannot turn into
/// prefix or suffix matching by accident — see
/// [`matching_is_exact`](test::matching_is_exact).
///
/// Note the desktop does not normally need any of this: it proxies its HTTP
/// through its own Rust core, where CORS does not apply. This exists so a
/// webview talking *directly* to a host is configurable rather than impossible.
const ALLOWED_SCHEMES: [&str; 4] = ["http://", "https://", "tauri://", "capacitor://"];

/// The origins permitted to make credentialed cross-origin requests.
///
/// Empty means CORS is off, which is the default and the right answer for every
/// same-origin deployment.
#[derive(Clone, Debug, Default)]
pub struct CorsConfig {
    /// Exact origins (scheme + host + port), e.g. `http://localhost:5173`.
    pub allowed_origins: Vec<String>,
}

impl CorsConfig {
    /// Reads `OPENCOMPANY_CORS_ORIGINS`: a comma-separated list of exact
    /// origins. Unset or empty disables CORS.
    ///
    /// Rejects `*` explicitly rather than letting it through as a literal
    /// origin that would never match: someone who writes it means "allow
    /// everything", and with credentials that is precisely what must not
    /// happen. Failing tells them; silently never matching would not.
    pub fn from_env() -> Result<Self, OpenCompanyError> {
        match std::env::var("OPENCOMPANY_CORS_ORIGINS") {
            Ok(raw) => Self::from_env_value(&raw),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Parses one `OPENCOMPANY_CORS_ORIGINS` value.
    ///
    /// Split out from [`from_env`](Self::from_env) so the rules below are
    /// testable without setting a process-global environment variable — which
    /// tests running in parallel cannot do safely, and which is why this
    /// module's validation went untested for so long.
    pub(crate) fn from_env_value(raw: &str) -> Result<Self, OpenCompanyError> {
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        let mut allowed_origins = Vec::new();
        for origin in raw.split(',').map(str::trim).filter(|o| !o.is_empty()) {
            if origin == "*" {
                return Err(OpenCompanyError::Config(
                    "OPENCOMPANY_CORS_ORIGINS cannot be '*': the session is a cookie, and a \
                     wildcard origin is forbidden with credentials. List exact origins, e.g. \
                     http://localhost:5173"
                        .to_string(),
                ));
            }
            if !ALLOWED_SCHEMES
                .iter()
                .any(|scheme| origin.starts_with(scheme))
            {
                return Err(OpenCompanyError::Config(format!(
                    "OPENCOMPANY_CORS_ORIGINS entry {origin:?} is not an origin; it needs a \
                     known scheme, e.g. http://localhost:5173 or tauri://localhost"
                )));
            }
            // An origin is scheme+host+port only. A trailing path never matches
            // what a browser sends, so it is a typo worth naming.
            if origin.matches('/').count() > 2 {
                return Err(OpenCompanyError::Config(format!(
                    "OPENCOMPANY_CORS_ORIGINS entry {origin:?} has a path; an origin is just \
                     scheme://host:port"
                )));
            }
            allowed_origins.push(origin.to_string());
        }
        Ok(Self { allowed_origins })
    }

    /// Whether any origin is allowed at all.
    pub fn is_enabled(&self) -> bool {
        !self.allowed_origins.is_empty()
    }

    /// The request's `Origin`, if this config permits it.
    fn permitted<'a>(&self, headers: &'a HeaderMap) -> Option<&'a str> {
        let origin = headers.get(header::ORIGIN)?.to_str().ok()?;
        self.allowed_origins
            .iter()
            .any(|allowed| allowed == origin)
            .then_some(origin)
    }

    /// The CORS headers to attach to a response, if any.
    ///
    /// Returns nothing for an origin that is not allowed, which the browser
    /// then blocks — the request may still have reached the handler, so this is
    /// not authorization. Authorization is the session; this only decides who
    /// may *read* the answer.
    pub fn headers_for(
        &self,
        request_headers: &HeaderMap,
    ) -> Vec<(header::HeaderName, HeaderValue)> {
        let Some(origin) = self.permitted(request_headers) else {
            return Vec::new();
        };
        let Ok(origin) = HeaderValue::from_str(origin) else {
            return Vec::new();
        };
        vec![
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, origin),
            (
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            ),
            // The origin is echoed per-request, so caches must key on it or one
            // origin's response could be served to another.
            (header::VARY, HeaderValue::from_static("Origin")),
        ]
    }

    /// The response to a preflight `OPTIONS`, if the origin is allowed.
    pub fn preflight(&self, request_headers: &HeaderMap) -> Option<Response> {
        use axum::response::IntoResponse;

        let mut response = StatusCode::NO_CONTENT.into_response();
        let cors = self.headers_for(request_headers);
        if cors.is_empty() {
            return None;
        }
        let headers = response.headers_mut();
        for (name, value) in cors {
            headers.insert(name, value);
        }
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS"),
        );
        // Echo what was asked for rather than guessing a list: the console sends
        // `content-type`, and this stays correct if that ever changes.
        let requested = request_headers
            .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("content-type"));
        headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, requested);
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        Some(response)
    }
}

/// Whether a method is a CORS preflight.
pub fn is_preflight(method: &Method) -> bool {
    method == Method::OPTIONS
}

#[cfg(test)]
#[path = "cors_tests.rs"]
mod tests;
