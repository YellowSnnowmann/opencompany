use super::*;
use crate::ports::types::EffectGroup;

/// An obvious fake. Never a real credential shape that could be mistaken
/// for one in a log or a diff.
const FAKE_SECRET: &str = "NOT-A-REAL-KEY-planted-for-tests";

fn effect(payload: Value) -> Effect {
    Effect {
        kind: "shell".into(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload,
        agent: Some("engineer".into()),
        run_id: None,
    }
}

fn rendered(payload: Value) -> String {
    serde_json::to_string(&display_payload(&effect(payload)).expect("a payload")).unwrap()
}

/// SECURITY: the guarantee this module exists to keep. A planted credential
/// at the top level, nested in an object, and inside an array must appear
/// in **no** serialized output.
#[test]
fn planted_secret_never_reaches_display_payload() {
    let out = rendered(serde_json::json!({
        "command": "deploy.sh",
        "api_key": FAKE_SECRET,
        "env": { "GITHUB_TOKEN": FAKE_SECRET, "HOME": "/workspace" },
        "headers": [
            { "Authorization": format!("Bearer {FAKE_SECRET}") },
            { "name": "accept", "value": "application/json" }
        ],
    }));

    assert!(!out.contains(FAKE_SECRET), "secret leaked into: {out}");
    // The non-sensitive siblings survive — this is a redactor, not a mute.
    assert!(out.contains("deploy.sh"));
    assert!(out.contains("application/json"));
    assert_eq!(out.matches(REDACTED).count(), 3);
}

/// The point of #372: a shell command reaches the card verbatim.
#[test]
fn shell_command_is_shown_in_full() {
    let payload = serde_json::json!({
        "command": "cargo test --locked --features openhuman -- --nocapture",
    });
    let out = display_payload(&effect(payload)).unwrap();
    assert_eq!(
        out["command"],
        Value::String("cargo test --locked --features openhuman -- --nocapture".into())
    );
}

/// ...and so do a glob's pattern and path.
#[test]
fn glob_pattern_and_path_are_shown_in_full() {
    let out = display_payload(&effect(
        serde_json::json!({ "pattern": "**/*.rs", "path": "src/runtime" }),
    ))
    .unwrap();
    assert_eq!(out["pattern"], Value::String("**/*.rs".into()));
    assert_eq!(out["path"], Value::String("src/runtime".into()));
}

/// The denylist must not eat ordinary words that merely share a prefix.
#[test]
fn ordinary_keys_survive_the_denylist() {
    for key in [
        // `author` is the canonical false positive: a bare-substring rule
        // over `authorization` would eat it.
        "author",
        "authors",
        "authored_by",
        "command",
        "pattern",
        "path",
        "keyword",
        "keys",
        "accessed_at",
        "client_name",
        "amount_usd",
    ] {
        assert!(!is_sensitive_key(key), "{key} should not be redacted");
    }
}

/// ...and must catch every spelling of a credential key we can name.
#[test]
fn credential_keys_are_redacted_in_every_spelling() {
    for key in [
        "api_key",
        "apiKey",
        "APIKEY",
        "x-api-key",
        "GITHUB_TOKEN",
        "access_token",
        "clientSecret",
        "CLIENT_SECRET",
        "client.secret",
        "Authorization",
        "password",
        "passwd",
        "private_key",
        "credentials",
        "session",
        "mytoken",
        "bearer",
    ] {
        assert!(is_sensitive_key(key), "{key} should be redacted");
    }
}

/// A redacted key is not descended into — a secret one level below a
/// credential-named object must not escape through the child walk.
#[test]
fn a_redacted_subtree_is_not_descended() {
    let out = rendered(serde_json::json!({
        "credentials": { "nested": { "deeper": FAKE_SECRET } },
    }));
    assert!(!out.contains(FAKE_SECRET));
    assert!(!out.contains("deeper"));
}

#[test]
fn long_strings_are_truncated_on_a_character_boundary() {
    // Multi-byte throughout: a byte slice here would panic mid-codepoint.
    let long: String = "é".repeat(MAX_STRING_CHARS + 500);
    let out = display_payload(&effect(serde_json::json!({ "command": long }))).unwrap();
    let shown = out["command"].as_str().unwrap();
    assert_eq!(shown.chars().count(), MAX_STRING_CHARS + 1);
    assert!(shown.ends_with('…'));
}

#[test]
fn a_string_at_the_cap_is_left_alone() {
    let exact = "x".repeat(MAX_STRING_CHARS);
    let out = display_payload(&effect(serde_json::json!({ "command": exact.clone() }))).unwrap();
    assert_eq!(out["command"], Value::String(exact));
}

#[test]
fn excessive_depth_fails_closed() {
    let mut deep = serde_json::json!(FAKE_SECRET);
    for _ in 0..(MAX_DEPTH + 4) {
        deep = serde_json::json!({ "next": deep });
    }
    let out = serde_json::to_string(&display_payload(&effect(deep)).unwrap()).unwrap();
    assert!(!out.contains(FAKE_SECRET), "secret escaped the depth bound");
    assert!(out.contains(UNRENDERABLE));
}

#[test]
fn excessive_breadth_fails_closed() {
    let wide: Vec<Value> = (0..(MAX_NODES + 50))
        .map(|i| Value::String(format!("item-{i}")))
        .collect();
    let out = serde_json::to_string(
        &display_payload(&effect(serde_json::json!({ "items": wide }))).unwrap(),
    )
    .unwrap();
    assert!(out.contains(UNRENDERABLE));
}

#[test]
fn empty_payloads_render_nothing() {
    for payload in [
        Value::Null,
        serde_json::json!({}),
        serde_json::json!([]),
        serde_json::json!("   "),
    ] {
        assert!(display_payload(&effect(payload)).is_none());
    }
}

/// Scalars are copied verbatim — the amount an operator is approving is
/// exactly the number the agent asked for.
#[test]
fn scalars_are_copied_verbatim() {
    let out = display_payload(&effect(
        serde_json::json!({ "amount_usd": 42.5, "dry_run": false, "note": null }),
    ))
    .unwrap();
    assert_eq!(out["amount_usd"], serde_json::json!(42.5));
    assert_eq!(out["dry_run"], Value::Bool(false));
    assert_eq!(out["note"], Value::Null);
}
