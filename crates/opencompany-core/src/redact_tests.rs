use super::*;

#[test]
fn scrub_replaces_known_secret() {
    let out = scrub(
        "token was sk-canary-123 here",
        &["sk-canary-123".to_string()],
    );
    assert!(!out.contains("sk-canary-123"), "{out}");
    assert!(out.contains("•••"), "{out}");
}

#[test]
fn scrub_strips_url_query_string() {
    // The lethal case: a reqwest error with the credential in the URL query.
    let msg = "error sending request for url (https://api.browserbase.com/mcp?projectId=pid&apiKey=qp-canary) failed";
    let out = scrub(msg, &[]);
    assert!(
        !out.contains("qp-canary"),
        "query-carried secret leaked: {out}"
    );
    assert!(!out.contains("projectId"), "{out}");
    assert!(out.contains("https://api.browserbase.com/mcp"), "{out}");
}

#[test]
fn scrub_strips_query_and_replaces_secret_together() {
    let msg = "url https://host/mcp?apiKey=qp-canary and bearer sk-canary";
    let out = scrub(msg, &["qp-canary".to_string(), "sk-canary".to_string()]);
    assert!(!out.contains("qp-canary"), "{out}");
    assert!(!out.contains("sk-canary"), "{out}");
}

#[test]
fn scrub_truncates_utf8_safely() {
    let long = "é".repeat(400); // 800 bytes
    let out = scrub(&long, &[]);
    assert!(out.len() <= SCRUB_MAX_BYTES + "…".len());
    assert!(out.ends_with('…'));
}
