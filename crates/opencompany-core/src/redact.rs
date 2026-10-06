//! Credential scrubbing for any text that is persisted, returned, or shown to
//! an agent.
//!
//! Three leak vectors are closed by [`scrub`]:
//!
//! 1. Upstream's `MCP HTTP {status} — {text}` embeds the raw response *body*.
//! 2. A `reqwest::Error`'s `Display` embeds the **full request URL including the
//!    query string** — lethal with [`AuthMaterial::QueryParam`], where the
//!    credential lives in the URL.
//! 3. Agent-visible endpoints could echo a URL query.
//!
//! [`AuthMaterial::QueryParam`]: crate::company::mcp::AuthMaterial::QueryParam
//!
//! [`scrub`] therefore (a) replaces every known credential substring with
//! `•••`, (b) strips the query string off any embedded URL, and (c)
//! UTF-8-safely truncates — and it is applied at **every** surfacing seam.
//! [`redact`] is passes (a) and (b) only, for payloads the caller bounds.
//!
//! Ungated: MCP, Composio, search and the turn settle path all surface through
//! it, so it compiles and is tested in every build.

/// The maximum byte length of any scrubbed, surfaced **message**.
///
/// Sized for a one-line operator/agent-facing sentence — an MCP failure
/// summary, a health string. It is **not** a size for a tool *body*: see
/// [`redact`] for why passing a successful tool result through [`scrub`] is a
/// bug rather than a conservative choice.
pub const SCRUB_MAX_BYTES: usize = 300;

/// Scrub a message so it can be safely persisted, returned, or shown to an agent.
///
/// Three passes, in order:
/// 1. Replace every known credential substring (from `secrets`) with `•••`.
/// 2. Strip the query string (and fragment) off **every** embedded URL — this is
///    what kills the `reqwest` full-URL leak when the credential rides in a
///    query parameter.
/// 3. UTF-8-safely truncate to [`SCRUB_MAX_BYTES`].
pub fn scrub(text: &str, secrets: &[String]) -> String {
    utf8_truncate(&redact(text, secrets), SCRUB_MAX_BYTES)
}

/// The **security** half of [`scrub`] — passes 1 and 2 only, with no length
/// cap. For tool *bodies*, which the caller must bound itself.
///
/// # Why this is separate (issue #410)
///
/// [`scrub`]'s third pass is a 300-byte message cap, and 300 bytes is right for
/// the sentence an MCP failure renders to. It is catastrophic for a successful
/// tool result: `composio_list_tools` routed its whole response through
/// [`scrub`], so an agent asking what a connected provider could do received the
/// first ~300 bytes of pretty-printed JSON — the first action and half of its
/// schema — ending in a bare `…` that says nothing about what was lost or how
/// to ask for less. The agent could tell actions existed but could not read the
/// name or parameters of the one it needed, so it reissued the identical call
/// until the repetition guard halted the run. Every Composio tool was affected,
/// including `composio_execute`, whose provider output was capped at 300 bytes
/// too.
///
/// Redaction is not the part that was wrong and must never be optional: this
/// still replaces every known credential with `•••` and strips the query string
/// off every embedded URL. Only the length decision moves to the caller, which
/// is the only place that knows what a sensible bound for *that* payload is and
/// how the agent could ask for a smaller one.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            out = out.replace(secret.as_str(), "•••");
        }
    }
    strip_url_queries(&out)
}

/// Cut the query/fragment off any `http(s)://…` URL embedded anywhere in `text`.
fn strip_url_queries(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = find_url_start(rest) {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        // The URL token runs until the next whitespace.
        let url_end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        let url = &tail[..url_end];
        let cut = url.find(['?', '#']).unwrap_or(url.len());
        out.push_str(&url[..cut]);
        rest = &tail[url_end..];
    }
    out.push_str(rest);
    out
}

/// The byte offset of the earliest `http://` or `https://` in `s`.
fn find_url_start(s: &str) -> Option<usize> {
    match (s.find("http://"), s.find("https://")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Truncate `s` to at most `max_bytes` on a char boundary, appending `…` when it
/// was cut. Never panics mid-codepoint.
fn utf8_truncate(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = s[..end].to_string();
    truncated.push('…');
    truncated
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;
