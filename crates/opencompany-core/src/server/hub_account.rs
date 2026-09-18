//! Where a person goes to look after the TinyHumans account behind a company's
//! key: the page that mints and revokes keys, and the one that tops up the
//! balance those keys spend.
//!
//! The console can mint a key without anyone leaving it ([`hub_link`] and
//! `ops::company_key`), but two things it deliberately cannot do are *revoke*
//! one and *pay* for what it spends. Both live on the hub's own dashboard,
//! behind that person's own sign-in, which is the right place for them: one
//! ends an instance's access and the other moves money.
//!
//! So the console does not reimplement either. It links to them — and the whole
//! job of this module is to work out **which** hub's dashboard, so a console
//! pointed at staging never sends an operator to production's billing page to
//! wonder why the balance they topped up did not arrive.
//!
//! ## Why the web address is derived from the API address
//!
//! A deployment already names its hub once, as `TINYHUMANS_API_URL`. Asking it
//! to name the matching site a second time is a chance to disagree — and the
//! disagreement is silent, because a link that opens the wrong environment
//! looks exactly like a link that opens the right one. So the site is derived
//! from the API address by the naming convention the ecosystem actually uses:
//!
//! | API base | Site |
//! |---|---|
//! | `https://api.tinyhumans.ai` | `https://tinyhumans.ai` |
//! | `https://staging-api.tinyhumans.ai` | `https://staging.tinyhumans.ai` |
//!
//! [`TINYHUMANS_WEB_URL`](crate::app::config::WEB_URL_ENV) overrides the
//! derivation for a deployment whose site does not follow it — a preview
//! build, a local Next.js dev server, a white-labelled front end.
//!
//! An address the convention does not recognize derives **nothing**. A guess
//! would be a link to a host that may not exist, and the console renders no
//! link at all rather than one that 404s, which is the same rule
//! `hub_link: false` follows for the button.
//!
//! [`hub_link`]: crate::server::hub_link

/// The site page that starts a key grant: it names the instance asking, offers
/// the provider buttons, and hands off to the API's own `GET /auth/key`.
const CONNECT_PATH: &str = "/connect";

/// The dashboard tab that mints, lists and revokes API keys.
const KEYS_TAB: &str = "/dashboard?tab=api-keys";

/// The dashboard tab that holds the balance and the top-up flow. `billing` is
/// the hub's own slug for it; it is titled "Pay as you go" on the page.
const TOP_UP_TAB: &str = "/dashboard?tab=billing";

/// The site that belongs to `api_url`, by the ecosystem's naming convention.
///
/// `None` for anything the convention does not cover, which is the honest
/// answer: see the module docs.
pub fn site_for_api(api_url: &str) -> Option<String> {
    let trimmed = api_url.trim().trim_end_matches('/');
    let (scheme, rest) = trimmed.split_once("://")?;
    // Only the authority matters. A hub reached under a path prefix is not a
    // shape the convention describes, so it derives nothing.
    if rest.contains('/') {
        return None;
    }
    let host = rest.split_once(':').map_or(rest, |(host, _)| host);
    let site = match (host.strip_prefix("api."), host.strip_prefix("staging-api.")) {
        (Some(bare), _) => bare.to_string(),
        (_, Some(bare)) => format!("staging.{bare}"),
        _ => return None,
    };
    Some(format!("{scheme}://{site}"))
}

/// The site's provider chooser for a key grant, carrying the grant parameters
/// through to the API.
///
/// The API's `GET /auth/key` defaults to `provider=google` and redirects
/// there immediately, which is the right default for a machine and the wrong
/// one for a person: an operator who pressed "Connect TinyHumans" in their own
/// console arrived at a Google account picker, with nothing on the page saying
/// who had asked or how to sign in with the account they actually use. The
/// site page says both, and then sends them to the same endpoint with a
/// `provider` they chose.
///
/// The query is built by the caller and appended verbatim: every parameter the
/// API needs is one the host already assembled for
/// [`key_grant_url`](crate::server::hub_identity::key_grant_url), and re-deriving
/// them here would be a second place for the challenge to be wrong.
pub fn connect_url(site: &str, grant_query: &str) -> String {
    format!(
        "{}{}?{}",
        site.trim_end_matches('/'),
        CONNECT_PATH,
        grant_query
    )
}

/// Where this person manages the keys their company's credential came from —
/// including revoking one.
pub fn manage_keys_url(site: &str) -> String {
    format!("{}{}", site.trim_end_matches('/'), KEYS_TAB)
}

/// Where this person tops up the balance those keys spend.
pub fn top_up_url(site: &str) -> String {
    format!("{}{}", site.trim_end_matches('/'), TOP_UP_TAB)
}

#[cfg(test)]
#[path = "hub_account_tests.rs"]
mod tests;
