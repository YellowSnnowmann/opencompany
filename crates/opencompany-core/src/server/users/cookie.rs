//! Session carrier naming, parsing, and rendering: the per-company cookie a
//! browser uses, and the header form every non-browser client must use.
//!
//! Hand-rolled rather than pulled from a crate, for the same reason
//! [`bearer`](crate::server::platform_auth) is: it is a small parse of a header
//! we control both ends of. The value alphabet is base64url by construction
//! (see [`token`](super::token)), so percent-encoding, quoted values, and
//! `Expires` date parsing — the genuinely hard parts of RFC 6265 — are all dead
//! code here. Adding `axum-extra`'s cookie feature would pull a dependency;
//! `tower-cookies` would introduce the first tower middleware layer in a
//! codebase that has none.
//!
//! ## Why the cookie is named per company
//!
//! In hosted mode one container serves one company, so a fixed name would do.
//! But in local development one process serves many companies from one origin,
//! and a fixed name would mean logging into company B silently destroys your
//! session for company A — same origin, same cookie name, last write wins.
//! Naming the cookie `oc_session_<company>` keeps them independent.
//!
//! The name is how a cookie is *looked up* once the request has established
//! which company it addresses. It is not how that company is decided: the jar
//! belongs to the origin and holds one cookie per company signed into on it, so
//! reading a company out of it would answer from whichever the browser happened
//! to send rather than the one the request named.
//!
//! ## Why the name is validated
//!
//! [`CompanyId::new`](crate::ports::types::CompanyId) performs no validation —
//! any string is a company id. A company whose id contained `;` or `=` could
//! otherwise inject attributes into the `Set-Cookie` header we render
//! (`oc_session_evil; Path=/; HttpOnly=...`). [`session_cookie_name`] returns
//! `None` for such an id rather than emitting a forgeable header.

use std::collections::HashMap;

use axum::http::HeaderMap;
use axum::http::header::COOKIE;

use crate::ports::types::CompanyId;

/// The prefix every session cookie name carries.
const SESSION_COOKIE_PREFIX: &str = "oc_session_";

/// The header a non-browser client presents a session in.
///
/// Lowercase because [`HeaderMap`] lookup is case-insensitive only through a
/// `HeaderName`; a `&str` key is matched verbatim against the lowercased name
/// `http` stores.
pub const SESSION_HEADER: &str = "x-opencompany-session";

/// The header a client sets to ask for a session it can carry itself.
///
/// ## Why a login has to be asked which carrier to mint
///
/// A cookie is the right carrier for a console served by the host it talks to,
/// and it is simply unavailable to one that is not. A hub console on
/// `app.example.com` addressing a tenant on `acme.example.com` is cross-site,
/// so [`set_cookie`]'s `SameSite=Lax` withholds the session from every request
/// it would make. Loosening that to `SameSite=None` would only trade a cookie
/// the browser never sends for a third-party cookie Safari discards outright.
///
/// Such a client therefore asks for the token itself and presents it in
/// [`SESSION_HEADER`], exactly as a paired device already does. This header is
/// how it asks. The alternative — sniffing a cross-origin `Origin` — would make
/// the carrier a property of where the request came from rather than of what
/// the client is able to store, and would silently switch carriers on a console
/// that had a perfectly good cookie.
///
/// ## Why opting in this way is safe
///
/// A custom request header cannot be set by a cross-site HTML form, and a
/// cross-site `fetch` that sets one is preflighted — which
/// [`cors`](crate::server::cors) answers for allow-listed origins only. So a
/// hostile page cannot make someone's browser ask for the readable carrier on
/// its behalf. A console that never sends this header keeps the `HttpOnly`
/// cookie it has always had, unchanged.
pub const SESSION_CARRIER_HEADER: &str = "x-opencompany-session-carrier";

/// The [`SESSION_CARRIER_HEADER`] value that selects the header carrier.
const CARRIER_HEADER: &str = "header";

/// Whether this request asked for a session it will carry in a header.
///
/// Anything else — absent, empty, `cookie`, a value nobody defined — means the
/// cookie. An unrecognised carrier degrades to the safer one rather than to no
/// session at all.
pub fn wants_header_carrier(headers: &HeaderMap) -> bool {
    headers
        .get(SESSION_CARRIER_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case(CARRIER_HEADER))
}

/// Renders the [`SESSION_HEADER`] value for a freshly minted session.
///
/// Assembled here rather than by the client because the addressed company may
/// have been resolved through the single-company alias, where the caller never
/// learned an id to prefix with. Returns `None` for a company that cannot carry
/// a session, mirroring [`session_cookie_name`] — both carriers agree on which
/// ids are expressible, which is the whole reason [`may_carry_session`] is
/// shared between them.
pub fn session_header_value(company: &CompanyId, token: &str) -> Option<String> {
    let id = company.as_ref();
    if !may_carry_session(id) || token.is_empty() {
        return None;
    }
    Some(format!("{id}.{token}"))
}

/// Whether `id` may carry a session at all.
///
/// Restricted to `[A-Za-z0-9_-]`: a superset-safe subset of RFC 6265's token
/// characters, and enough for every id the runtime mints
/// (`{millis:012x}-{counter:012x}`) or a manifest slug produces.
///
/// Shared by both carriers on purpose, and it is the reason they live in one
/// file. The cookie needs it so a company id containing `;` or `=` cannot inject
/// attributes into a rendered `Set-Cookie`. The header needs it so the id cannot
/// contain the `.` that separates it from the token. Two rules that could drift
/// apart would mean a company addressable by one carrier and not the other.
fn may_carry_session(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The session cookie name for `company`, or `None` when the id cannot safely
/// name a cookie.
///
/// A company that cannot name a cookie cannot mint a session; the login route
/// refuses rather than rendering a header an attacker chose.
pub fn session_cookie_name(company: &CompanyId) -> Option<String> {
    let id = company.as_ref();
    if !may_carry_session(id) {
        return None;
    }
    Some(format!("{SESSION_COOKIE_PREFIX}{id}"))
}

/// Parses [`SESSION_HEADER`] into the company it names and the raw token.
///
/// ## Why the company travels inside the value
///
/// A cookie carries its company in its *name* (see above), which is what lets
/// the GraphQL handler find the company for a request whose company argument is
/// in the body. A header has no equivalent, so the value is `<company>.<token>`
/// and the header is self-describing in exactly the same way. Without that, a
/// header-authenticated GraphQL request would have nowhere to read the company
/// from, and the header form would work on the REST routes only.
///
/// `.` is unambiguous as the separator because [`may_carry_session`] excludes it
/// from the company id, so the *first* `.` is always the boundary.
///
/// ## Why this is not a CSRF regression
///
/// [`set_cookie`]'s `SameSite=Lax` is this codebase's CSRF defense. A header
/// does not weaken it: a cross-site HTML form cannot set a request header at
/// all, and a cross-site `fetch`/XHR that sets a custom one is preflighted,
/// which [`cors`](crate::server::cors) answers for allow-listed origins only.
/// The header form is, if anything, the stricter carrier — it is never attached
/// ambiently the way a cookie is.
pub fn session_from_header(headers: &HeaderMap) -> Option<(CompanyId, String)> {
    let raw = headers.get(SESSION_HEADER)?.to_str().ok()?.trim();
    let (company, token) = raw.split_once('.')?;
    if !may_carry_session(company) || token.is_empty() {
        return None;
    }
    Some((CompanyId::new(company), token.to_string()))
}

/// The company id embedded in a session cookie name, if it is one.
pub fn company_from_cookie_name(name: &str) -> Option<&str> {
    name.strip_prefix(SESSION_COOKIE_PREFIX)
        .filter(|id| !id.is_empty())
}

/// Parses a `Cookie` request header into name → value pairs.
///
/// Values are taken verbatim: we only ever set base64url values, so there is
/// nothing to decode. Browsers never send cookie *attributes*, so there are
/// none to skip. A later duplicate of a name wins, matching the fact that a
/// browser sends the most specific cookie last.
pub fn parse_cookies(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out = HashMap::new();
    // A client may legitimately send more than one Cookie header.
    for header in headers.get_all(COOKIE) {
        let Ok(raw) = header.to_str() else {
            continue;
        };
        for pair in raw.split(';') {
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            // `split_once` keeps any '=' inside the value, which base64url
            // padding would produce if we ever stopped stripping it.
            out.insert(name.to_string(), value.trim().to_string());
        }
    }
    out
}

/// Reads one cookie by name.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    parse_cookies(headers).remove(name)
}

/// Renders a `Set-Cookie` value for a freshly minted session.
///
/// - `HttpOnly`: JavaScript must not be able to read a session token, so an XSS
///   cannot exfiltrate it.
/// - `SameSite=Lax`: `Strict` would drop the cookie on the *first* landing from
///   the magic link, which is a cross-site top-level navigation out of a mail
///   client. `Lax` allows exactly that (top-level GET) while still withholding
///   the cookie from cross-site POSTs — and since every state-changing route
///   here is a POST, that is also the CSRF defense.
/// - `Secure` unless `insecure`, which is set only for plain-http loopback dev.
pub fn set_cookie(name: &str, value: &str, max_age_secs: u64, insecure: bool) -> String {
    let mut out = format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}");
    if !insecure {
        out.push_str("; Secure");
    }
    out
}

/// Renders the `Set-Cookie` value that deletes a session cookie.
///
/// Attributes must match the ones it was set with, or the browser treats it as
/// a different cookie and keeps the original.
pub fn clear_cookie(name: &str, insecure: bool) -> String {
    let mut out = format!("{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if !insecure {
        out.push_str("; Secure");
    }
    out
}

#[cfg(test)]
#[path = "cookie_tests.rs"]
mod tests;
