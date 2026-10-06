use super::*;

use std::process::Command;

use oh::mcp::registry::types::{CommandKind, Transport};

const NODE_STUB: &str = r#"
const readline = require('node:readline');
const rl = readline.createInterface({ input: process.stdin });
const send = (value) => process.stdout.write(JSON.stringify(value) + '\n');
rl.on('line', (line) => {
  const request = JSON.parse(line);
  if (!request.id) return;
  if (request.method === 'initialize') {
send({ jsonrpc: '2.0', id: request.id, result: { protocolVersion: '2024-11-05', capabilities: { tools: {} }, serverInfo: { name: 'test', version: '1' } } });
  } else if (request.method === 'tools/list') {
send({ jsonrpc: '2.0', id: request.id, result: { tools: [{ name: 'echo', description: 'Echo text', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] } }] } });
  } else if (request.method === 'tools/call') {
send({ jsonrpc: '2.0', id: request.id, result: { content: [{ type: 'text', text: 'echo: ' + request.params.arguments.text }] } });
  }
});
"#;

#[tokio::test]
async fn install_connect_call_disconnect_round_trip() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("skipping MCP runtime test because node is unavailable");
        return;
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let script = temp.path().join("mcp-stub.cjs");
    std::fs::write(&script, NODE_STUB).expect("write node stub");
    let runtime = McpRuntime::new(temp.path().join("workspace"));
    let server = InstalledServer {
        server_id: uuid::Uuid::new_v4().to_string(),
        qualified_name: "test-node-echo".to_string(),
        display_name: "Test Node Echo".to_string(),
        description: None,
        icon_url: None,
        command_kind: CommandKind::Binary,
        command: "node".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        env_keys: vec![],
        config: None,
        installed_at: 0,
        last_connected_at: None,
        transport: Transport::Stdio,
        enabled: true,
    };

    runtime.install(&server, &HashMap::new()).expect("install");
    let tools = runtime.connect(&server.server_id).await.expect("connect");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let result = runtime
        .call_tool(
            &server.server_id,
            "echo",
            serde_json::json!({"text": "hello"}),
        )
        .await
        .expect("call");
    assert_eq!(result["content"][0]["text"], "echo: hello");

    assert!(
        runtime
            .disconnect(&server.server_id)
            .await
            .expect("disconnect")
    );
    assert!(
        runtime
            .uninstall(&server.server_id)
            .await
            .expect("uninstall")
    );
    assert!(runtime.list().expect("list").is_empty());
}

/// `get` on an install that was never persisted reports `McpServerNotFound`
/// — the "genuinely absent" half of the store-error split.
#[test]
fn get_on_an_absent_server_reports_not_found() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = McpRuntime::new(temp.path().join("workspace"));
    let error = runtime
        .get("no-such-server")
        .expect_err("an absent install must not resolve");
    assert!(
        matches!(error, OpenCompanyError::McpServerNotFound(ref id) if id == "no-such-server"),
        "absent install must be McpServerNotFound, got: {error:?}"
    );
}

/// `get` on a store that fails to read must NOT be reported as a missing
/// server — the caller would be told to reinstall something that is there.
/// Truncating the SQLite file beneath the runtime's open connection forces
/// the next `get_server` read to fail, and the error must surface as a
/// `Store` error rather than the blanket `McpServerNotFound` the pre-split
/// code produced for every failure.
#[test]
fn get_on_a_store_that_fails_to_read_reports_store_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let runtime = McpRuntime::new(workspace.clone());
    // Open the host so the store and its schema exist on disk.
    runtime.list().expect("open the mcp store");
    // Corrupt the file beneath the open connection: the query can no longer
    // be satisfied, so `get_server` must fail with a store error.
    let db = workspace.join("mcp_clients").join("mcp_clients.db");
    std::fs::write(&db, b"this is not a sqlite database").expect("corrupt the store file");
    let error = runtime
        .get("some-server")
        .expect_err("a store that cannot read must not resolve");
    assert!(
        matches!(error, OpenCompanyError::Store(_)),
        "a failing store read must surface as Store, got: {error:?}"
    );
}
