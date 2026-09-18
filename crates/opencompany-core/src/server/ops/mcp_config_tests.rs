//! The document→declaration rules: what an entry inherits, what counts as an
//! override, and how a credential header is read.
//!
//! Feature-free by construction (see `mcp_registry/mcp_registry_tests.rs` for why): these
//! exercise the pure projection, which is where every rule this surface adds
//! actually lives.

use super::*;

fn entry(json: serde_json::Value) -> ConfigEntryIn {
    serde_json::from_value(json).expect("the entry parses")
}

fn lower(name: &str) -> McpServer {
    McpServer {
        name: name.to_string(),
        endpoint: "https://mcp.example.com/mcp".to_string(),
        description: Some("declared in company.toml".to_string()),
        command: None,
        allowed_tools: vec!["search".to_string()],
        disallowed_tools: Vec::new(),
        read_only_tools: vec!["search".to_string()],
        timeout_secs: 45,
        enabled: true,
        auth_secret: None,
    }
}

/// The smallest useful entry is a URL; everything else takes its default.
#[test]
fn a_bare_url_is_a_complete_entry() {
    let server = server_from(
        "notion",
        &entry(serde_json::json!({"url": "https://x/mcp"})),
        None,
    )
    .expect("a URL is enough");
    assert_eq!(server.endpoint, "https://x/mcp");
    assert!(
        server.enabled,
        "a server is exposed unless it says otherwise"
    );
    assert_eq!(server.timeout_secs, 30);
    assert!(server.allowed_tools.is_empty());
}

/// An entry that only flips `enabled` keeps everything the lower layer declared
/// — the tool lists it never mentioned are not silently cleared.
#[test]
fn an_entry_inherits_what_it_does_not_mention() {
    let declared = lower("notion");
    let server = server_from(
        "notion",
        &entry(serde_json::json!({"enabled": false})),
        Some(&declared),
    )
    .expect("the lower layer supplies the rest");
    assert!(!server.enabled);
    assert_eq!(server.endpoint, declared.endpoint);
    assert_eq!(server.allowed_tools, declared.allowed_tools);
    assert_eq!(server.read_only_tools, declared.read_only_tools);
    assert_eq!(server.timeout_secs, 45);
    assert_eq!(server.description, declared.description);
}

/// Claude spells a disabled server `"disabled": true`. Both spellings work, and
/// `enabled` wins when a document carries both.
#[test]
fn claudes_disabled_spelling_is_honoured() {
    let off = server_from(
        "notion",
        &entry(serde_json::json!({"url": "https://x/mcp", "disabled": true})),
        None,
    )
    .expect("parses");
    assert!(!off.enabled);

    let both = server_from(
        "notion",
        &entry(serde_json::json!({"url": "https://x/mcp", "disabled": true, "enabled": true})),
        None,
    )
    .expect("parses");
    assert!(both.enabled, "`enabled` is the authoritative switch");
}

/// A stdio entry is refused by naming the real problem — this deployment has no
/// subprocess to launch — rather than as a missing URL.
#[test]
fn a_stdio_entry_is_refused_by_name() {
    let err = server_from("local", &entry(serde_json::json!({"command": "npx"})), None)
        .expect_err("stdio is not dialable here");
    let message = err.0.to_string();
    assert!(message.contains("HTTP only"), "{message}");
    assert!(message.contains("`command`"), "{message}");

    let err = server_from(
        "local",
        &entry(serde_json::json!({"type": "stdio", "url": "https://x"})),
        None,
    )
    .expect_err("an explicit stdio transport is refused too");
    assert!(err.0.to_string().contains("stdio"), "the refusal names it");
}

/// An entry saying exactly what the manifest already says writes no override —
/// saving an unedited document must not convert every declared server into an
/// operator override.
#[test]
fn an_unedited_entry_is_not_an_override() {
    let declared = lower("notion");
    let same =
        server_from("notion", &entry(serde_json::json!({})), Some(&declared)).expect("parses");
    assert!(!differs(&same, &declared), "nothing was said");

    let edited = server_from(
        "notion",
        &entry(serde_json::json!({"timeoutSecs": 10})),
        Some(&declared),
    )
    .expect("parses");
    assert!(
        differs(&edited, &declared),
        "a changed timeout is an override"
    );
}

/// Whitespace and blank entries in a tool list are not a difference: the
/// resolver normalizes them away, so treating them as an edit would write an
/// override that changes nothing.
#[test]
fn tool_list_whitespace_is_not_a_difference() {
    let declared = lower("notion");
    let padded = server_from(
        "notion",
        &entry(serde_json::json!({"allowedTools": [" search ", ""]})),
        Some(&declared),
    )
    .expect("parses");
    assert!(!differs(&padded, &declared));
}

/// `Authorization: Bearer …` rotates the same slot the console's Add token
/// button writes, so the two spellings are one credential rather than two.
#[test]
fn a_bearer_header_is_stored_as_a_bearer_token() {
    let headers = serde_json::json!({"Authorization": "Bearer sk-live-1"});
    let material =
        auth_from_headers("notion", headers.as_object().expect("object")).expect("one header");
    assert_eq!(material, AuthMaterial::Bearer("sk-live-1".to_string()));
}

/// Any other single header is stored as that named header.
#[test]
fn a_custom_header_is_stored_verbatim() {
    let headers = serde_json::json!({"X-Api-Key": "abc"});
    let material =
        auth_from_headers("notion", headers.as_object().expect("object")).expect("one header");
    assert_eq!(
        material,
        AuthMaterial::Header {
            name: "X-Api-Key".to_string(),
            value: "abc".to_string()
        }
    );
}

/// Several headers are refused rather than one being picked arbitrarily — a
/// silently dropped credential reads as "the server rejects my token".
#[test]
fn several_credential_headers_are_refused() {
    let headers = serde_json::json!({"Authorization": "Bearer a", "X-Api-Key": "b"});
    let err =
        auth_from_headers("notion", headers.as_object().expect("object")).expect_err("ambiguous");
    assert!(err.0.to_string().contains("one outbound credential header"));
}

/// An empty `headers` object is a mistake worth naming: it looks like clearing
/// the credential and does not, so it is refused with the way to say either.
#[test]
fn an_empty_headers_object_is_refused() {
    let headers = serde_json::Map::new();
    let err = auth_from_headers("notion", &headers).expect_err("says nothing");
    assert!(
        err.0.to_string().contains("Omit it"),
        "the refusal says how"
    );
}
