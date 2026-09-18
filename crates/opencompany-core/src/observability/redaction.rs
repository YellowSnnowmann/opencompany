//! Removing credentials from text on its way out of the process.
//!
//! Applied to every string a crash report can carry — the event message, the
//! log entry, each exception value, each breadcrumb, and every string leaf of
//! the structured fields a `tracing` event brings with it
//! (`docs/spec/runtime/crash-reporting.md`).
//!
//! # Why this file is not behind the feature
//!
//! It names no `sentry::` type and it is the part that has to be right, so it
//! compiles and is tested in every build — including the default one, where a
//! `crash-reporting` lane would never run it. That is the same argument
//! `analytics::config` makes for the enable/disable decision, and it is the
//! reason the `crash-reporting` feature is a *second* gate rather than the only
//! one.
//!
//! # Why it is hand-written rather than a set of regexes
//!
//! The vendored runtime's equivalent (`core::log_redaction`) is seven regexes,
//! and its scars are instructive: `token[=:\s]+\S+` matched
//! `cancellation_token=` and `next_page_token=` until a `\b` was added, and the
//! generic `sk-[A-Za-z0-9]{20,}` left a trailing `_uv` behind on any key with a
//! separator in it until the character class grew.
//!
//! Both failures are the same failure — a regex over raw text has no idea where
//! a *word* begins and ends — so this splits the text into tokens first and
//! asks its questions of whole tokens. `cancellation_token` is one token and
//! normalises to `cancellationtoken`, which is not a secret-bearing key, so the
//! false positive cannot arise rather than being patched out of it. A secret
//! with a separator inside is one token, so there is no fragment to leave
//! behind.
//!
//! It also avoids making `regex` an unconditional dependency of this crate. It
//! is optional today (`Cargo.toml`, under `openhuman`), and a security control
//! that must run in every build cannot be built on a crate that does not.
//!
//! # What this is and is not
//!
//! It is a **last line of defence**, not the first. The first is not putting a
//! credential in a message: `SecretValue` (`ports::types`) exists so a
//! credential is not `Display`, and `analytics::config::ClientCredentials` and
//! [`super::config::Dsn`] both refuse to `Debug` themselves. A scrubber is
//! heuristic by construction — it cannot recognise a secret that looks like a
//! word — so a call site that relies on it is one release away from leaking.

use std::borrow::Cow;

/// What a redacted span is replaced with. Deliberately not the empty string: a
/// report that says a value was removed is diagnosable, and one that silently
/// lost a field looks like a bug in the reporter.
pub const REDACTED: &str = "[redacted]";

/// Prefixes that identify a credential on their own, whatever surrounds them.
///
/// This is the half that catches a secret nobody labelled — a bare key pasted
/// into a message, or one embedded in a provider's own error text, which is
/// where the vendored runtime found most of them.
///
/// Case-sensitive on purpose: `AKIA` is an AWS key id and `akia` is a word.
const SECRET_PREFIXES: &[&str] = &[
    // OpenAI, Anthropic, Stripe and everything that copied them.
    "sk-",
    "sk_",
    "rk_",
    "pk_",
    // GitHub: PATs, OAuth, server-to-server, user-to-server, refresh, and the
    // fine-grained form.
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "ghr_",
    "github_pat_",
    // GitLab.
    "glpat-",
    // Slack bot/user/app/legacy tokens and app-level tokens.
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "xoxe-",
    "xapp-",
    // AWS long-lived and session access key ids.
    "AKIA",
    "ASIA",
    // TinyHumans — this crate's own hosted credential
    // (`company::credentials::API_KEY_ENV`).
    "th_",
    // Shopify, npm, DigitalOcean, SendGrid.
    "shpat_",
    "shpss_",
    "npm_",
    "dop_v1_",
    "SG.",
];

/// The shortest a prefixed token may be before it is treated as a credential.
///
/// A floor rather than an exact length, because every issuer picks its own and
/// several have changed it. It exists to keep a sentence like "the sk- prefix"
/// from being redacted, not to validate anything.
const MIN_PREFIXED_LEN: usize = 12;

/// Keys whose *value* is a credential, normalised by [`normalize_key`].
///
/// The list is deliberately specific. `key` is not on it and neither is `id`:
/// both appear constantly in ordinary diagnostics, and a scrubber that eats
/// half of every message is one an operator turns off.
const SECRET_KEYS: &[&str] = &[
    "token",
    "accesstoken",
    "refreshtoken",
    "idtoken",
    "authtoken",
    "sessiontoken",
    "bearertoken",
    "apikey",
    "apitoken",
    "apisecret",
    "secret",
    "secretkey",
    "clientsecret",
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "authorization",
    "credential",
    "credentials",
    "privatekey",
    "signingkey",
    "dsn",
];

/// Query-parameter names whose value is a credential *in a URL*, on top of
/// everything in [`SECRET_KEYS`].
///
/// Separate from [`SECRET_KEYS`] because these words are only unambiguous
/// inside a query string. `code` is the magic-link sign-in code this crate
/// mints and the console redeems, and a 43-character `?code=` that reaches a
/// crash report is a working sign-in for whoever can read the operator's Sentry
/// project — but `code` is also an ordinary English word and the key in
/// `code=ECONNREFUSED`, so redacting it everywhere would eat diagnostics.
/// Inside `?…=` there is no such ambiguity.
const SECRET_QUERY_KEYS: &[&str] = &["code", "state", "sig", "signature"];

/// HTTP authentication schemes, as *values*: the word before the credential,
/// never the credential.
///
/// Exempt from redaction, because `Authorization: Bearer abc123` would
/// otherwise redact the scheme name and leave `abc123` standing — worse than
/// doing nothing, since the message then *looks* scrubbed. `token` is on this
/// list for GitHub's `Authorization: token <pat>` form.
const AUTH_SCHEMES: &[&str] = &["bearer", "basic", "digest", "negotiate", "token"];

/// The subset of [`AUTH_SCHEMES`] that, as a *key*, licenses redacting the next
/// token across a bare space — `Bearer abc123` has no `=` or `:` between the
/// two, and no other reading of those two words exists.
///
/// `token` is deliberately **not** here, though it is a scheme in
/// `Authorization: token <pat>`. As an English word it is far too common —
/// "the token was rejected by the provider" would lose `was` — and the ordinary
/// prose case is the one that has to keep working, or an operator turns this
/// off. The `Authorization: token <pat>` form is not lost in practice: a PAT
/// carries an issuer prefix and [`looks_like_a_secret`] catches it on its own.
const SCHEME_KEYS: &[&str] = &["bearer", "basic", "digest", "negotiate"];

/// Removes credentials from `text`.
///
/// Borrows when there is nothing to remove, which is the overwhelmingly common
/// case: this runs on every string of every event, and most events carry none.
pub fn scrub(text: &str) -> Cow<'_, str> {
    // Three passes, in the only order that works: the URL passes have to run
    // before the token pass, because `:`, `@`, `?` and `&` are the characters
    // that pass treats as separators, so a URL's credential is invisible to it.
    match scrub_url_userinfo(text) {
        Cow::Borrowed(borrowed) => match scrub_url_query(borrowed) {
            Cow::Borrowed(borrowed) => scrub_tokens(borrowed),
            Cow::Owned(owned) => Cow::Owned(scrub_tokens(&owned).into_owned()),
        },
        Cow::Owned(owned) => {
            let owned = scrub_url_query(&owned).into_owned();
            Cow::Owned(scrub_tokens(&owned).into_owned())
        }
    }
}

/// Bytes that may appear inside one token.
///
/// ASCII-only by construction, so every index this yields is a `char`
/// boundary. `=` and `/` are excluded even though base64 uses both: including
/// `=` would swallow the `token=value` separator this pass depends on, and
/// including `/` would make a whole URL one token. A trailing `==` of padding
/// left behind reveals nothing.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+' | b'~')
}

/// The comparable form of a key token.
///
/// Three normalisations, each earning its place:
///
/// * everything after the last `.`, so `config.token` and `settings.api_key`
///   are the keys they name rather than opaque paths;
/// * `-` and `_` removed, so `api-key`, `api_key`, `apiKey` and the `--api-key`
///   flag are one key;
/// * lower-cased.
///
/// The word-boundary bug this replaces is gone by construction:
/// `cancellation_token` is a single token and normalises to
/// `cancellationtoken`, which is not in [`SECRET_KEYS`].
fn normalize_key(token: &str) -> String {
    let tail = token.rsplit('.').next().unwrap_or(token);
    tail.chars()
        .filter(|c| *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether a token names itself as a credential.
fn looks_like_a_secret(token: &str) -> bool {
    if token.len() >= MIN_PREFIXED_LEN
        && SECRET_PREFIXES
            .iter()
            .any(|prefix| token.starts_with(prefix))
    {
        return true;
    }
    // A JWT: three base64url segments. The header segment always begins with
    // the base64 of `{"`, which is `eyJ`. Session cookies and platform bearers
    // in this crate are JWTs, and they carry no issuer prefix to match on.
    token.len() >= 20 && token.starts_with("eyJ") && token.matches('.').count() >= 2
}

/// Whether `key`, followed by `separator`, means the next token is its value.
///
/// The separator is what keeps prose out of this. "the token was rejected" has
/// a bare space between `token` and `was`, and redacting `was` would be
/// nonsense — so an assignment (`=` or `:`, in any of the shapes JSON, TOML,
/// a URL query and a log line write one) is required, with two exceptions that
/// are unambiguous without one:
///
/// * an auth scheme (`Bearer abc`), which is a credential by definition;
/// * a command-line flag (`--token abc`), which the leading `-` identifies.
fn key_directs_a_secret(key: &str, separator: &str) -> bool {
    // A separator that crosses a line is not an assignment; it is two
    // unrelated log lines that happen to be adjacent.
    if separator.contains('\n') || separator.contains('\r') {
        return false;
    }
    // Anything but the punctuation an assignment is written with — a word, a
    // comma, a bracket — means these two tokens are not a pair.
    if !separator
        .chars()
        .all(|c| matches!(c, ' ' | '\t' | ':' | '=' | '"' | '\'' | '>'))
    {
        return false;
    }
    let normalized = normalize_key(key);
    if SCHEME_KEYS.contains(&normalized.as_str()) {
        return true;
    }
    if !SECRET_KEYS.contains(&normalized.as_str()) {
        return false;
    }
    separator.contains('=') || separator.contains(':') || key.starts_with('-')
}

/// The token pass: split into words, then ask whole-word questions.
fn scrub_tokens(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut copied = 0usize;
    let mut changed = false;

    // The previous token's span, and where the run of separator characters
    // since it ended begins.
    let mut previous: Option<(usize, usize)> = None;
    let mut separator_start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        if !is_token_byte(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && is_token_byte(bytes[index]) {
            index += 1;
        }
        let end = index;
        let token = &text[start..end];

        let directed = previous.is_some_and(|(previous_start, previous_end)| {
            key_directs_a_secret(
                &text[previous_start..previous_end],
                &text[separator_start..start],
            )
        }) && !AUTH_SCHEMES.contains(&normalize_key(token).as_str());

        if looks_like_a_secret(token) || directed {
            out.push_str(&text[copied..start]);
            out.push_str(REDACTED);
            copied = end;
            changed = true;
        }

        previous = Some((start, end));
        separator_start = end;
    }

    if changed {
        out.push_str(&text[copied..]);
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// The URL pass: `https://user:pass@host/path` loses its userinfo.
///
/// Separate from the token pass because `:` and `@` are exactly the characters
/// that pass uses as separators, so a URL's credential is invisible to it. This
/// is not hypothetical here — `analytics::boot` records an authenticated
/// collector proxy writing its key into container logs through `reqwest`'s own
/// `Display`, and a connector URL takes the same shape.
fn scrub_url_userinfo(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut copied = 0usize;
    let mut changed = false;
    let mut index = 0usize;

    while let Some(offset) = text[index..].find("://") {
        let authority_start = index + offset + 3;
        let authority_end = text[authority_start..]
            .find(|c: char| matches!(c, '/' | '?' | '#') || ends_a_url(c))
            .map_or(text.len(), |n| authority_start + n);
        // `rfind`, not `find`: a password may itself contain an `@`, and the
        // last one is the delimiter the URL grammar means.
        if let Some(at) = text[authority_start..authority_end].rfind('@') {
            out.push_str(&text[copied..authority_start]);
            out.push_str(REDACTED);
            // The `@` stays, so the result still reads as a URL.
            copied = authority_start + at;
            changed = true;
        }
        index = authority_end;
        if index >= text.len() {
            break;
        }
    }

    if changed {
        out.push_str(&text[copied..]);
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// A credential-shaped string for tests, assembled rather than written down.
///
/// [`looks_like_a_secret`] reads a token's prefix and its length and nothing
/// else, so the high-entropy body a real credential carries is filler as far
/// as these tests are concerned — what [`scrub`] is handed is identical either
/// way.
///
/// Written out as a literal, though, `ghp_AAAA…` is byte-for-byte what a
/// leaked token looks like to everything that reads this repository: secret
/// scanners flag the file on every push, and after a genuine incident somebody
/// grepping the tree has to rule each fixture out by hand before they can
/// believe the tree is clean. A scanner that is permanently red about a test
/// fixture is a scanner nobody reads.
///
/// So the prefix — the part under test, and the part that has to stay
/// readable — is written down, and only the body is assembled here. No
/// credential-shaped literal is committed and no coverage is lost.
/// Whether a *key* names a credential, so its value goes whatever shape it is.
///
/// The structured counterpart to what [`scrub`] does inside a string. `scrub`
/// reads text, and a map entry `{"token": "hunter2"}` has no text to read:
/// `hunter2` is a word with no issuer prefix and no `token=` beside it, so the
/// string rule leaves it alone. The structure carries the label the flat form
/// would have carried inline, so a caller walking structured data has to ask
/// this question as well.
///
/// Exported because the callers are in [`super`], where the protocol types
/// live, and the vocabulary belongs here with the rest of it — the same split
/// that keeps this file free of `sentry::` types and testable in every build.
pub fn key_names_a_secret(key: &str) -> bool {
    SECRET_KEYS.contains(&normalize_key(key).as_str())
}

/// The characters that end a URL when one is written inside prose or a log
/// line. Shared by both URL passes so they agree on where a URL stops.
fn ends_a_url(c: char) -> bool {
    matches!(c, '"' | '\'' | '<' | '>' | ')' | ',' | ';') || c.is_whitespace()
}

/// The query pass: `?code=…&token=…` loses the values of the parameters that
/// name a credential.
///
/// Separate from the token pass for the reason the userinfo pass is: `?` and
/// `&` are separators there, so `?code=abc` is not a key and its value to it.
/// Separate from the userinfo pass because it runs on a different span of the
/// URL and answers a different question.
fn scrub_url_query(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut copied = 0usize;
    let mut changed = false;
    let mut index = 0usize;

    while let Some(offset) = text[index..].find('?') {
        let query_start = index + offset + 1;
        let query_end = text[query_start..]
            .find(|c: char| ends_a_url(c) || c == '#')
            .map_or(text.len(), |n| query_start + n);

        let mut pair_start = query_start;
        while pair_start < query_end {
            let pair_end = text[pair_start..query_end]
                .find('&')
                .map_or(query_end, |n| pair_start + n);
            if let Some(equals) = text[pair_start..pair_end].find('=') {
                let key = &text[pair_start..pair_start + equals];
                let value_start = pair_start + equals + 1;
                let normalized = normalize_key(key);
                let names_a_secret = SECRET_QUERY_KEYS.contains(&normalized.as_str())
                    || SECRET_KEYS.contains(&normalized.as_str());
                // An empty value is already telling nobody anything, and
                // replacing it would turn `?code=` into something that looks
                // like a redacted credential that was never there.
                if names_a_secret && value_start < pair_end {
                    out.push_str(&text[copied..value_start]);
                    out.push_str(REDACTED);
                    copied = pair_end;
                    changed = true;
                }
            }
            pair_start = pair_end + 1;
        }

        index = query_end;
        if index >= text.len() {
            break;
        }
    }

    if changed {
        out.push_str(&text[copied..]);
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
pub(crate) fn credential_shaped(prefix: &str, body_len: usize) -> String {
    format!("{prefix}{}", "A".repeat(body_len))
}

#[cfg(test)]
#[path = "redaction_tests.rs"]
mod tests;
