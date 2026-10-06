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
//! to. [`load_dir_mcp_servers`] parses it and [`crate::company::CompanyManifest`] merges
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
//! [`crate::company::ledger_file`] needs has no equivalent here.
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
//!
//! # Who reads the document
//!
//! The `mcpServers` shape is read by tinymcp's
//! [`config_doc::parse_with`](tinymcp::registry::config_doc::parse_with) in
//! lenient mode, with the fields only this host understands registered as
//! [`HOST_FIELDS`]. This file adds what is particular to a committed bundle:
//! the `endpoint` spelling, `$comment`, and the refusal of anything that would
//! put a secret in the repository.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use tinymcp::Transport;
use tinymcp::registry::config_doc::{self, Declared, ParseOptions, ROOT_KEY};

use crate::company::McpServer;

/// The bundle file holding one company's MCP server declarations.
pub const MCP_FILE: &str = "mcp.json";

/// The entry fields this host reads beyond tinymcp's own.
///
/// `endpoint` is the spelling `company.toml` uses, accepted beside `url`.
/// `readOnlyTools` and `authSecret` are this host's policy and credential-key
/// fields. `$comment` carries an author's reasoning, since JSON has no comments.
pub const HOST_FIELDS: &[&str] = &["$comment", "endpoint", "readOnlyTools", "authSecret"];

/// The keys the document's root may carry.
const ROOT_FIELDS: &[&str] = &["$comment", ROOT_KEY];

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
/// Every problem is written against the file that carries it, matching
/// [`crate::company::ledger_file`] and [`crate::company::agent_file`]: a
/// template author reads the message, not a parser path.
pub(crate) fn parse_mcp_file(file_name: &str, src: &str) -> (Vec<McpServer>, Vec<String>) {
    let mut doc: Value = match serde_json::from_str(src) {
        Ok(doc) => doc,
        Err(err) => {
            return (
                Vec::new(),
                vec![format!("`{file_name}` is not valid JSON — {err}")],
            );
        }
    };
    let Some(root) = doc.as_object_mut() else {
        return (
            Vec::new(),
            vec![format!(
                "`{file_name}` holds an object with an `{ROOT_KEY}` key"
            )],
        );
    };
    if let Some(key) = root.keys().find(|key| !ROOT_FIELDS.contains(&key.as_str())) {
        return (
            Vec::new(),
            vec![format!(
                "`{file_name}` has a top-level `{key}` key it doesn't understand — servers live \
                 under `{ROOT_KEY}`"
            )],
        );
    }
    if !root.contains_key(ROOT_KEY) {
        return (Vec::new(), Vec::new());
    }

    // Sorted by name so two machines seed a company in the same order.
    let mut problems: Vec<String> = Vec::new();
    if let Some(Value::Object(entries)) = root.get_mut(ROOT_KEY) {
        let mut sorted: Vec<(String, Value)> = std::mem::take(entries).into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, mut entry) in sorted {
            match fold_endpoint(&mut entry) {
                Ok(()) => {
                    entries.insert(name, entry);
                }
                Err(problem) => problems.push(format!(
                    "mcp server `{}` in `{file_name}` {problem}",
                    name.trim()
                )),
            }
        }
    }

    let options = ParseOptions {
        host_fields: HOST_FIELDS,
        lenient: true,
    };
    let report = match config_doc::parse_with(&doc, &options) {
        Ok(report) => report,
        Err(err) => {
            problems.push(format!("`{file_name}` could not be read — {err}"));
            return (Vec::new(), problems);
        }
    };
    problems.extend(
        report
            .rejected
            .iter()
            .map(|rejected| format!("{} (in `{file_name}`)", rejected.detail)),
    );

    let mut kept: Vec<McpServer> = Vec::new();
    for declared in report.declared {
        let label = format!("mcp server `{}`", declared.name);
        match server_from(&declared) {
            Ok(server) => {
                if let Some(problem) = refusal(&label, file_name, &declared, &server) {
                    problems.extend(problem);
                } else {
                    kept.push(server);
                }
            }
            Err(problem) => problems.push(format!("{label} in `{file_name}` {problem}")),
        }
    }

    (kept, problems)
}

/// Folds an `endpoint` into `url`, so tinymcp reads either spelling.
///
/// Both spellings of the endpoint is an authoring mistake, not a precedence
/// question: whichever one this file chose to honour would be the one somebody
/// did not mean, and the other would silently vanish.
fn fold_endpoint(entry: &mut Value) -> Result<(), String> {
    let Some(fields) = entry.as_object_mut() else {
        return Ok(());
    };
    let Some(endpoint) = fields.remove("endpoint") else {
        return Ok(());
    };
    let Value::String(endpoint) = endpoint else {
        return Err("has an `endpoint` that is not a string.".to_string());
    };
    match fields.get("url").and_then(Value::as_str) {
        Some(url) if url.trim() != endpoint.trim() => {
            Err("sets both `url` and `endpoint`, and they disagree — keep one.".to_string())
        }
        Some(_) => Ok(()),
        None => {
            fields.insert("url".to_string(), Value::String(endpoint));
            Ok(())
        }
    }
}

/// The [`McpServer`] one declaration describes, or why its host fields are
/// unusable.
fn server_from(declared: &Declared) -> Result<McpServer, String> {
    let (endpoint, command) = match &declared.transport {
        Transport::HttpRemote { url } => (url.clone(), None),
        _ => (String::new(), Some(declared.command.clone())),
    };
    Ok(McpServer {
        name: declared.name.clone(),
        endpoint,
        description: declared.description.clone(),
        command,
        allowed_tools: declared.allowed_tools.clone(),
        disallowed_tools: declared.disallowed_tools.clone(),
        read_only_tools: read_only_tools(&declared.host_fields)?,
        timeout_secs: declared.timeout_secs.unwrap_or(super::DEFAULT_TIMEOUT_SECS),
        enabled: declared.enabled,
        auth_secret: auth_secret(&declared.host_fields)?,
    })
}

/// `readOnlyTools`, as a list of tool names.
fn read_only_tools(fields: &BTreeMap<String, Value>) -> Result<Vec<String>, String> {
    match fields.get("readOnlyTools") {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str().map(str::to_string).ok_or_else(|| {
                    "has a `readOnlyTools` entry that is not a tool name.".to_string()
                })
            })
            .collect(),
        Some(_) => Err("has a `readOnlyTools` that is not a list of tool names.".to_string()),
    }
}

/// `authSecret`, the name of a secret-store key — never the token.
fn auth_secret(fields: &BTreeMap<String, Value>) -> Result<Option<String>, String> {
    match fields.get("authSecret") {
        None => Ok(None),
        Some(Value::String(key)) => Ok(Some(key.clone())),
        Some(_) => Err("has an `authSecret` that is not a key name.".to_string()),
    }
}

/// Why a parsed server is not kept, when it is not.
///
/// The shared validator first: name, `http(s)` endpoint, no stdio `command`,
/// no `user:pass@` userinfo — one set of rules for every declaration path.
/// Then the two refusals particular to a committed file. A token in the
/// endpoint's query string or in a `headers` block is refused, not scrubbed:
/// this file ships to everyone who runs the bundle, so a secret here is a
/// secret everywhere. An `authSecret` names a key, and the token itself is
/// written per company.
fn refusal(
    label: &str,
    file_name: &str,
    declared: &Declared,
    server: &McpServer,
) -> Option<Vec<String>> {
    let shared = super::validate_one(label, server);
    if !shared.is_empty() {
        return Some(
            shared
                .into_iter()
                .map(|problem| format!("{problem} (in `{file_name}`)"))
                .collect(),
        );
    }
    if declared.credentials.is_some() {
        return Some(vec![format!(
            "{label} in `{file_name}` carries credentials inline — this file is committed, so \
             name an `authSecret` key and write the token from the console instead."
        )]);
    }
    if super::has_query_credential(&server.endpoint) {
        return Some(vec![format!(
            "{label} in `{file_name}` has a credential in its `endpoint` query string — this \
             file is committed, so name an `authSecret` key and write the token from the \
             console instead."
        )]);
    }
    None
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
