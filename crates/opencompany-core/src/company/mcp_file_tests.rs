use super::*;

fn parse(src: &str) -> (Vec<McpServer>, Vec<String>) {
    parse_mcp_file(MCP_FILE, src)
}

#[test]
fn reads_a_server_and_takes_its_name_from_the_key() {
    let (servers, problems) = parse(
        r#"{"mcpServers": {"deepwiki": {
            "url": "https://mcp.deepwiki.com/mcp",
            "description": "Docs for public repos."
        }}}"#,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "deepwiki");
    assert_eq!(servers[0].endpoint, "https://mcp.deepwiki.com/mcp");
    assert_eq!(
        servers[0].description.as_deref(),
        Some("Docs for public repos.")
    );
    // Defaults an author did not have to write.
    assert!(servers[0].enabled);
    assert_eq!(
        servers[0].timeout_secs,
        super::super::mcp::DEFAULT_TIMEOUT_SECS
    );
}

#[test]
fn accepts_endpoint_as_an_alias_for_url() {
    let (servers, problems) =
        parse(r#"{"mcpServers": {"a": {"endpoint": "https://example.test/mcp"}}}"#);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(servers[0].endpoint, "https://example.test/mcp");
}

#[test]
fn refuses_url_and_endpoint_that_disagree() {
    let (servers, problems) = parse(
        r#"{"mcpServers": {"a": {
            "url": "https://one.test/mcp",
            "endpoint": "https://two.test/mcp"
        }}}"#,
    );
    assert!(servers.is_empty());
    assert!(
        problems
            .iter()
            .any(|p| p.contains("both `url` and `endpoint`")),
        "{problems:?}"
    );
}

#[test]
fn drops_a_stdio_server_and_names_the_real_problem() {
    // The shape a vendor README hands you. It must not cost the sibling
    // entry, and the message must say `command`, not "missing endpoint".
    let (servers, problems) = parse(
        r#"{"mcpServers": {
            "local": {"command": "npx some-mcp"},
            "remote": {"url": "https://example.test/mcp"}
        }}"#,
    );
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "remote");
    assert!(
        problems.iter().any(|p| p.contains("stdio `command`")),
        "{problems:?}"
    );
}

#[test]
fn drops_a_non_http_endpoint() {
    let (servers, problems) = parse(r#"{"mcpServers": {"a": {"url": "ftp://x.test/mcp"}}}"#);
    assert!(servers.is_empty());
    assert!(
        problems.iter().any(|p| p.contains("`http://`")),
        "{problems:?}"
    );
}

#[test]
fn refuses_a_credential_in_the_query_string() {
    let (servers, problems) =
        parse(r#"{"mcpServers": {"a": {"url": "https://x.test/mcp?apiKey=sk-live-1"}}}"#);
    assert!(servers.is_empty());
    assert!(
        problems.iter().any(|p| p.contains("credential")),
        "{problems:?}"
    );
}

#[test]
fn allows_an_auth_secret_which_names_a_key_rather_than_a_token() {
    let (servers, problems) = parse(
        r#"{"mcpServers": {"a": {
            "url": "https://x.test/mcp",
            "enabled": false,
            "authSecret": "mcp/a/auth"
        }}}"#,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert!(!servers[0].enabled);
    assert_eq!(servers[0].auth_secret.as_deref(), Some("mcp/a/auth"));
}

#[test]
fn ignores_comments_but_still_refuses_a_typo() {
    let (servers, problems) = parse(
        r#"{"$comment": "why these", "mcpServers": {"a": {
            "$comment": "why this one",
            "url": "https://x.test/mcp"
        }}}"#,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(servers.len(), 1);

    let (_, problems) = parse(r#"{"mcpServers": {"a": {"urll": "https://x.test/mcp"}}}"#);
    assert!(!problems.is_empty(), "a misspelled key must be reported");
}

#[test]
fn a_malformed_file_is_one_problem_and_no_servers() {
    let (servers, problems) = parse("{not json");
    assert!(servers.is_empty());
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("not valid JSON"), "{problems:?}");
}

#[test]
fn servers_come_back_sorted_by_name() {
    let (servers, problems) = parse(
        r#"{"mcpServers": {
            "zulu": {"url": "https://z.test/mcp"},
            "alpha": {"url": "https://a.test/mcp"}
        }}"#,
    );
    assert!(problems.is_empty(), "{problems:?}");
    let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["alpha", "zulu"]);
}

#[test]
fn a_missing_file_is_not_a_problem() {
    let dir = std::env::temp_dir().join("oc-mcp-file-absent");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (servers, problems) = load_dir_mcp_servers(&dir);
    assert!(servers.is_empty());
    assert!(problems.is_empty(), "{problems:?}");
}
