//! Validation and normalization of MCP server declarations: the hosted-v1
//! HTTP-only boundary, reserved names, and endpoint credential checks.

use super::*;

/// Validates a set of MCP server declarations, returning every problem in
/// prosumer language. Enforces unique names, an `http(s)://` endpoint, and the
/// hosted-v1 no-stdio boundary. Shared by manifest validation and the ops
/// add/update routes.
pub fn validate_servers(servers: &[McpServer]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (index, server) in servers.iter().enumerate() {
        let name = server.name.trim();
        let label = if name.is_empty() {
            format!("mcp server #{}", index + 1)
        } else {
            format!("mcp server `{name}`")
        };
        problems.extend(validate_one(&label, server));
        if !name.is_empty() && !seen.insert(name.to_string()) {
            problems.push(format!(
                "mcp server `name` `{name}` is used more than once — names must be unique."
            ));
        }
    }
    problems
}

/// Server names a company may not declare: the runtime merges servers by
/// name, last write wins, so a company server under one of these would stand
/// in for OpenCompany's own MCP server or OpenHuman's docs server.
pub const RESERVED_SERVER_NAMES: &[&str] = &["opencompany", "gitbooks"];

/// Validates a single server declaration under a caller-supplied `label`.
pub fn validate_one(label: &str, server: &McpServer) -> Vec<String> {
    let mut problems = Vec::new();
    let name = server.name.trim();
    let endpoint = server.endpoint.trim();

    if name.is_empty() {
        problems.push(format!("{label} is missing a `name`."));
    } else if let Some(reserved) = RESERVED_SERVER_NAMES
        .iter()
        .find(|reserved| name.eq_ignore_ascii_case(reserved))
    {
        problems.push(format!(
            "{label} uses the name `{reserved}`, which is reserved for a server OpenCompany runs itself — choose another name."
        ));
    }

    if server
        .command
        .as_deref()
        .is_some_and(|c| !c.trim().is_empty())
    {
        problems.push(format!(
            "{label} sets a stdio `command`, which is not supported in hosted v1 — declare an HTTP `endpoint` instead."
        ));
    }

    if endpoint.is_empty() {
        problems.push(format!(
            "{label} is missing an `endpoint` — an MCP server needs an `http(s)://` URL."
        ));
    } else if !is_http_url(endpoint) {
        problems.push(format!(
            "{label} `endpoint` must be an `http://` or `https://` URL — you wrote `{endpoint}`."
        ));
    } else if has_userinfo(endpoint) {
        // A `user:pass@host` endpoint smuggles a credential into the URL, which
        // then leaks into every log line and transport error. Reject it — the
        // operator should use a token / custom-header / query-parameter
        // credential (stored write-only) instead.
        problems.push(format!(
            "{label} `endpoint` must not embed credentials in the URL (the `user:pass@host` form) — leave the endpoint credential-free and set a token or query-parameter credential instead."
        ));
    }

    problems
}

/// True when `url` is an absolute `http://` or `https://` URL.
fn is_http_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// True when `url`'s query string carries something credential-shaped.
///
/// Distinct from [`has_userinfo`], which catches the `user:pass@host` form. A
/// token in a query parameter is the other way a credential reaches a URL, and
/// it is the shape a default is most likely to arrive in — an operator copying
/// a "your MCP URL" string out of a vendor dashboard.
pub(crate) fn has_query_credential(url: &str) -> bool {
    let Some(query) = url.split_once('?').map(|(_, q)| q) else {
        return false;
    };
    query.split('&').any(|pair| {
        // Decode before matching: `api%4Bey` is `apiKey`, and matching the raw
        // key would let an encoded credential name past the block to ship a
        // token-bearing default to every company (CWE-200).
        let key = percent_decode(pair.split('=').next().unwrap_or(""));
        let key = key.trim().to_ascii_lowercase().replace(['-', '_'], "");
        matches!(
            key.as_str(),
            "apikey" | "token" | "accesstoken" | "secret" | "password" | "auth" | "authorization"
        )
    })
}

/// Percent-decodes `s`, turning each `%XX` escape into its byte.
///
/// Inline rather than pulled from the `url` crate because this module compiles
/// in the default build, where `url` is gated behind the `mcp` feature. Only
/// used for the query-key scan above, so `+` is left literal (it has no
/// meaning in a parameter *name*) and a malformed or truncated escape is
/// passed through untouched rather than rejected.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
        {
            out.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Every `%XX` decodes to fewer bytes than it occupies, so lossy UTF-8
    // repair here can only shorten the input — never invent characters.
    String::from_utf8_lossy(&out).into_owned()
}

/// The nibble a hex digit encodes.
fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// Normalizes the install-wide `[[default_mcp_server]]` list (issue #527) into
/// the entries that may actually ship, dropping each one that cannot.
///
/// # Why entries are dropped rather than the list rejected
///
/// These servers auto-enable on every company of the install with no user
/// action, so one malformed row must not cost an operator the rows that are
/// fine — and it must not abort boot either, which would turn a typo in a
/// packaged config into an install that does not start. Every rejection is
/// returned alongside the survivors so the caller can log it: a default that
/// silently fails to ship looks exactly like one nobody configured.
///
/// # Why a credential is refused rather than stripped
///
/// A default is handed to every agent on the install unprompted, so it must be
/// safe unattended: public, and carrying no secret. An entry with a token in its
/// endpoint's query string, or an `auth_secret` naming a key, is **rejected, not
/// scrubbed** — scrubbing would ship a server whose auth silently no longer
/// works, which is worse than not shipping it, because it fails at an agent's
/// first tool call instead of here. A server that needs auth is added per
/// company at runtime, where its token goes to that company's own secret store.
///
/// [`McpServer`] is a typed struct with no free-form fields, so the two above
/// are the whole surface: an inline `token = "…"` in the TOML cannot reach a
/// field and is dropped by deserialization. That is why this does not carry the
/// dynamic "is any key credential-shaped?" scan a schemaless config would need —
/// the type already provides it.
///
/// The shared rules (name, `http(s)` endpoint, no stdio `command`, no
/// `user:pass@` userinfo) come from [`validate_one`], so defaults and every
/// other declaration path stay on one validator.
pub fn normalize_default_servers(raw: &[McpServer]) -> (Vec<McpServer>, Vec<String>) {
    let mut kept: Vec<McpServer> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (index, server) in raw.iter().enumerate() {
        let name = server.name.trim();
        let label = if name.is_empty() {
            format!("default mcp server #{}", index + 1)
        } else {
            format!("default mcp server `{name}`")
        };

        let shared = validate_one(&label, server);
        if !shared.is_empty() {
            problems.extend(shared);
            continue;
        }

        if has_query_credential(&server.endpoint) {
            problems.push(format!(
                "{label} has a credential in its `endpoint` query string — a default ships to every company unattended and must carry no secret. Add it per company from the console instead."
            ));
            continue;
        }

        if server
            .auth_secret
            .as_deref()
            .is_some_and(|k| !k.trim().is_empty())
        {
            problems.push(format!(
                "{label} names an `auth_secret` — a default must not depend on a credential. Declare it in the company's `company.toml`, or add it from the console, where the token is stored per company."
            ));
            continue;
        }

        // A duplicate name would put two rows in the list claiming one slug, and
        // which won would depend on merge order rather than on this config.
        if !seen.insert(name.to_string()) {
            problems.push(format!(
                "{label} repeats a `name` used earlier in the list — keeping the first."
            ));
            continue;
        }

        kept.push(server.clone());
    }

    (kept, problems)
}

/// Whether an endpoint's authority carries a `user[:pass]@` userinfo section.
/// Uses the same cheap authority-splitting as [`crate::redact::scrub`]
/// so a `?email=a@b` query never trips it.
fn has_userinfo(url: &str) -> bool {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    authority.contains('@')
}

/// A **non-blocking** advisory when an endpoint's query string carries a
/// key-ish parameter (`apiKey` / `token` / `secret` / …).
///
/// This is not an error: some providers legitimately put a *non-secret* id in
/// the URL (BrowserBase's `projectId`). But a real secret in the endpoint URL
/// leaks into logs and transport errors, so the ops layer surfaces this as a
/// gentle nudge toward the write-only query-parameter credential intake. Returns
/// `None` when nothing key-ish is present.
pub fn endpoint_secret_advisory(endpoint: &str) -> Option<String> {
    let (_, query) = endpoint.split_once('?')?;
    const KEYISH: [&str; 8] = [
        "apikey", "token", "secret", "password", "passwd", "access", "auth", "key",
    ];
    let hit = query
        .split(['&', ';'])
        .filter_map(|kv| kv.split('=').next())
        .any(|param| {
            let param = param.trim().to_ascii_lowercase();
            KEYISH.iter().any(|needle| param.contains(needle))
        });
    hit.then(|| {
        "the endpoint URL looks like it carries a secret in its query string — a credential in the URL can leak into logs and errors, so prefer the write-only query-parameter credential (only a non-secret id like a project id belongs in the URL)."
            .to_string()
    })
}

/// De-dupes and trims a tool-name list, dropping blanks.
pub(super) fn normalize_tools(tools: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tool in tools {
        let tool = tool.trim();
        if !tool.is_empty() && !out.iter().any(|existing| existing == tool) {
            out.push(tool.to_string());
        }
    }
    out
}
