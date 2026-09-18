//! The bundle's MCP declaration file: `companies/<name>/mcp.json`.
//!
//! A vertical's tool servers were declarable only two ways: an `[[mcp_server]]`
//! block in `company.toml`, or an operator adding one from the console. Both
//! work, and neither is how a template ships: of the bundles in `companies/`,
//! exactly two declared any server at all, so a law firm shipped with five
//! agents, three ledgers and a workflow graph shipped with no research tools,
//! and got them only if whoever booted it went and added them by hand.
//!
//! So a bundle may carry its servers the way it already carries its roster, its
//! ledgers and its workflows: one file, authored beside the company it belongs
//! to. [`load_dir_mcp_servers`] parses it and [`super::CompanyManifest`] merges
//! the result into `mcp_servers` before validation — see `company::manifest`.
//!
//! # Why JSON, and why the map key is the name
//!
//! The `{"mcpServers": {...}}` object is the shape every other MCP host already
//! uses, so a server can be copied from a vendor's setup instructions into a
//! bundle without being transcribed into a different syntax first — and a
//! transcription is where the endpoint typo comes from.
//!
//! The server's name is the **map key** rather than a field, which is the one
//! thing this shape gets more right than the TOML array: a key cannot disagree
//! with itself, so the `slug`-versus-filename refusal
//! [`super::ledger_file`] needs has no equivalent here.
//!
//! # Why bad entries are dropped rather than the file refused
//!
//! An `mcp.json` copied out of a vendor's README almost always carries a stdio
//! `command` and an `env` block, because that is what a desktop MCP client
//! wants and this runtime does not support (hosted v1 is HTTP-only). Refusing
//! the file would refuse the whole company over one row somebody pasted
//! hopefully — so an invalid *entry* is dropped and reported, while a malformed
//! *file* is a problem the manifest carries. `content_test` is what makes
//! either one fatal for a bundle this repo ships.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::company::types::McpServer;

/// The bundle file holding one company's MCP server declarations.
pub const MCP_FILE: &str = "mcp.json";

/// The on-disk shape of `mcp.json`.
///
/// `deny_unknown_fields` so a misspelled key is a reported problem rather than
/// a server that silently does not have the setting its author wrote. The
/// `$comment` escape hatch is whitelisted precisely so that strictness stays
/// affordable: JSON has no comments, every other bundle file in this repo
/// carries its reasoning inline, and prose with nowhere to live gets deleted.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpFile {
    /// Free-form note about the bundle's server choices. Ignored by the parser.
    #[serde(default, rename = "$comment")]
    _comment: Option<serde_json::Value>,
    /// Servers by name. A `BTreeMap` so iteration is sorted by name and two
    /// machines seed a company in the same order.
    #[serde(default, rename = "mcpServers")]
    servers: BTreeMap<String, McpEntry>,
}

/// One entry under `mcpServers`.
///
/// camelCase on the wire, matching both the host convention this shape comes
/// from and [`McpServer`]'s own console representation. Every field is optional:
/// what a server must actually have is decided by
/// [`validate_one`](super::mcp::validate_one), so this file and every other
/// declaration path stay on one validator.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct McpEntry {
    #[serde(default, rename = "$comment")]
    _comment: Option<serde_json::Value>,
    /// The MCP endpoint. `url` is the spelling every other host uses;
    /// `endpoint` is the spelling `company.toml` uses. Both are accepted, and
    /// declaring both is refused rather than resolved.
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// A stdio command. Unsupported in hosted v1, and parsed only so the
    /// refusal can name the real problem instead of "missing endpoint".
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    disallowed_tools: Vec<String>,
    #[serde(default)]
    read_only_tools: Vec<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    enabled: Option<bool>,
    /// Names a secret-store key holding this server's token — never the token.
    #[serde(default)]
    auth_secret: Option<String>,
}

/// Whether `dir` is a bundle carrying an `mcp.json`.
pub fn has_mcp_file(dir: &Path) -> bool {
    dir.join(MCP_FILE).is_file()
}

/// Loads the servers a bundle declares, from `<dir>/mcp.json`.
///
/// Returns the servers that are usable alongside every problem from the ones
/// that are not — never an `Err`. A missing file is not a problem to report:
/// most bundles declare no server of their own, which is a complete answer.
///
/// The caller decides what a problem costs. `CompanyManifest::from_located`
/// carries them into `validate()`, where a bundle this repo ships is caught by
/// `content_test` and a hand-edited one still reaches the console to be fixed.
pub fn load_dir_mcp_servers(dir: &Path) -> (Vec<McpServer>, Vec<String>) {
    let path = dir.join(MCP_FILE);
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), Vec::new()),
        Err(err) => {
            return (
                Vec::new(),
                vec![format!("`{MCP_FILE}` could not be read — {err}")],
            );
        }
    };
    parse_mcp_file(MCP_FILE, &src)
}

/// Parses one `mcp.json`, named by `file_name` for the problem messages.
///
/// Every problem is written in prosumer language and against the file that
/// carries it, matching [`super::ledger_file`] and [`super::agent_file`]: a
/// template author reads the message, not the serde path.
pub(crate) fn parse_mcp_file(file_name: &str, src: &str) -> (Vec<McpServer>, Vec<String>) {
    let file: McpFile = match serde_json::from_str(src) {
        Ok(file) => file,
        Err(err) => {
            return (
                Vec::new(),
                vec![format!("`{file_name}` is not valid JSON — {err}")],
            );
        }
    };

    let mut kept: Vec<McpServer> = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    for (name, entry) in file.servers {
        let name = name.trim().to_string();
        let label = format!("mcp server `{name}`");

        // Both spellings of the endpoint is an authoring mistake, not a
        // precedence question: whichever one this file chose to honour would be
        // the one somebody did not mean, and the other would silently vanish.
        let endpoint = match (entry.url.as_deref(), entry.endpoint.as_deref()) {
            (Some(url), Some(endpoint)) if url.trim() != endpoint.trim() => {
                problems.push(format!(
                    "{label} in `{file_name}` sets both `url` and `endpoint`, and they disagree — \
                     keep one."
                ));
                continue;
            }
            (Some(url), _) => url.trim().to_string(),
            (None, Some(endpoint)) => endpoint.trim().to_string(),
            (None, None) => String::new(),
        };

        let server = McpServer {
            name: name.clone(),
            endpoint,
            description: entry.description,
            command: entry.command,
            allowed_tools: entry.allowed_tools,
            disallowed_tools: entry.disallowed_tools,
            read_only_tools: entry.read_only_tools,
            timeout_secs: entry
                .timeout_secs
                .unwrap_or(super::mcp::DEFAULT_TIMEOUT_SECS),
            enabled: entry.enabled.unwrap_or(true),
            auth_secret: entry.auth_secret,
        };

        // The shared validator: name, `http(s)` endpoint, no stdio `command`, no
        // `user:pass@` userinfo. One set of rules for every declaration path.
        let shared = super::mcp::validate_one(&label, &server);
        if !shared.is_empty() {
            problems.extend(
                shared
                    .into_iter()
                    .map(|problem| format!("{problem} (in `{file_name}`)")),
            );
            continue;
        }

        // A token in the endpoint's query string is refused, not scrubbed: this
        // file is committed to the repo and ships to everyone who runs the
        // bundle, so a secret here is a secret everywhere. Scrubbing would ship
        // a server whose auth silently no longer works, which fails at an
        // agent's first tool call instead of here, where somebody is looking.
        // Unlike a packaged default, a bundle server may name an `auth_secret`:
        // that names a key, and the token itself is written per company.
        if super::mcp::has_query_credential(&server.endpoint) {
            problems.push(format!(
                "{label} in `{file_name}` has a credential in its `endpoint` query string — this \
                 file is committed, so name an `authSecret` key and write the token from the \
                 console instead."
            ));
            continue;
        }

        kept.push(server);
    }

    (kept, problems)
}

#[cfg(test)]
#[path = "mcp_file_tests.rs"]
mod tests;
