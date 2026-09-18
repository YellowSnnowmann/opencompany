use super::toolbelt_test_helpers_tests::*;
use super::*;
use serde_json::json;

// --- FAIL-axis: what the sandbox belt does when its dependency fails -----

/// `read_workspace_state` is wired beside the shell tool against a
/// workspace path that `build_agent` creates only on a best-effort basis —
/// a failed `ensure_agent_workspace` is logged, not fatal, and the belt is
/// still built over the absent directory. Reading a workspace that is not
/// there must therefore degrade to a reported result, never a panic or a
/// hang, or the first turn of a company whose disk was full dies inside a
/// tool instead of telling the operator.
#[tokio::test]
async fn workspace_state_survives_a_workspace_that_was_never_created() {
    let parent = tempfile::Builder::new()
        .prefix("oc-toolbelt-nows-")
        .tempdir()
        .expect("tempdir");
    let missing = parent.path().join("never-created");
    assert!(!missing.exists(), "the premise is an absent workspace");

    let tool = WorkspaceStateTool::new(missing.clone());
    let result = tool.execute(json!({})).await;

    let result = result.expect("a missing workspace must not abort the tool call");
    assert!(
        !result.output().is_empty(),
        "the tool must say something about the workspace it could not read"
    );
    assert!(
        !missing.exists(),
        "a read-only state probe must not create the workspace as a side effect"
    );
}

/// A multi-edit patch whose LAST hunk is unappliable must leave the file
/// exactly as it was. `apply_patch` takes a batch, and a batch that writes
/// the hunks it liked before discovering the one it cannot is a partial
/// write: the agent is told the patch failed while the file on disk holds
/// half of it, and the next read disagrees with the last report.
#[tokio::test]
async fn apply_patch_leaves_no_partial_write_when_a_later_hunk_fails() {
    let ws_dir = tempfile::Builder::new()
        .prefix("oc-toolbelt-partial-")
        .tempdir()
        .expect("tempdir");
    let ws = ws_dir.path();
    let target = ws.join("notes.txt");
    let original = "alpha\nbravo\ncharlie\n";
    std::fs::write(&target, original).expect("seed file");

    let security = test_security(ws, PolicyMode::Full);
    let tool = ApplyPatchTool::new(security);

    // Hunk 1 matches; hunk 2 names a string that is not in the file.
    let result = tool
        .execute(json!({
            "edits": [
                { "path": "notes.txt", "old_string": "alpha", "new_string": "ALPHA" },
                { "path": "notes.txt", "old_string": "no-such-anchor", "new_string": "x" }
            ]
        }))
        .await
        .expect("tool call completes");

    assert!(
        result.is_error,
        "a batch with an unappliable hunk must be reported as failed: {}",
        result.output()
    );
    let on_disk = std::fs::read_to_string(&target).expect("read back");
    assert_eq!(
        on_disk, original,
        "a failed patch batch must not leave the earlier hunk written to disk"
    );
}

/// `git_operations` is wired for every `code`-granted agent against the
/// agent's own workspace, which is an ordinary directory — `build_agent`
/// creates it and nothing initialises a repository in it. Every operation
/// must therefore report the missing repository rather than panicking or
/// walking up to whatever repository happens to contain the workspace.
#[tokio::test]
async fn git_operations_refuses_a_workspace_that_is_not_a_repository() {
    let ws_dir = tempfile::Builder::new()
        .prefix("oc-toolbelt-norepo-")
        .tempdir()
        .expect("tempdir");
    let ws = ws_dir.path();
    assert!(!ws.join(".git").exists(), "the premise is a bare directory");

    let security = test_security(ws, PolicyMode::Full);
    let tool = GitOperationsTool::new(security, ws.to_path_buf());

    for operation in ["status", "log", "diff"] {
        let result = tool
            .execute(json!({ "operation": operation }))
            .await
            .expect("a missing repository must not abort the tool call");
        assert!(
            result.is_error,
            "`{operation}` on a non-repository must be reported as an error: {}",
            result.output()
        );
    }
}

/// A corrupted repository is the same contract one step further in: `.git`
/// exists, so the operation is attempted, and the failure comes back from
/// git itself. It must still arrive as a reported error.
#[tokio::test]
async fn git_operations_reports_a_corrupted_repository_rather_than_panicking() {
    let ws_dir = tempfile::Builder::new()
        .prefix("oc-toolbelt-badrepo-")
        .tempdir()
        .expect("tempdir");
    let ws = ws_dir.path();
    // A `.git` that is a file of garbage: present enough to be found, not a
    // repository by any reading.
    std::fs::write(ws.join(".git"), "not a git directory").expect("seed .git");

    let security = test_security(ws, PolicyMode::Full);
    let tool = GitOperationsTool::new(security, ws.to_path_buf());

    let result = tool
        .execute(json!({ "operation": "status" }))
        .await
        .expect("a corrupted repository must not abort the tool call");
    assert!(
        result.is_error,
        "a corrupted repository must be reported as an error: {}",
        result.output()
    );
}

/// Exports above the row ceiling are refused before any workspace write.
#[tokio::test]
async fn csv_export_refuses_an_unbounded_row_count() {
    let ws_dir = tempfile::Builder::new()
        .prefix("oc-toolbelt-csvcap-")
        .tempdir()
        .expect("tempdir");
    let ws = ws_dir.path();
    let security = test_security(ws, PolicyMode::Full);
    let tool = CsvExportTool::new(security);

    let rows: Vec<serde_json::Value> = (0..200_000)
        .map(|i| json!({ "id": i, "note": "padding padding padding padding" }))
        .collect();
    let data = serde_json::to_string(&rows).expect("serialise rows");

    let result = tool
        .execute(json!({ "data": data, "filename": "huge.csv" }))
        .await
        .expect("tool call completes");

    assert!(
        result.is_error,
        "an export of {} rows must be refused by a stated ceiling, not written: {}",
        rows.len(),
        result.output()
    );
}

#[tokio::test]
async fn csv_factory_caps_rendered_bytes_on_every_execution_path() {
    let ws = tempfile::tempdir().unwrap();
    let tools = code_tools(test_security(ws.path(), PolicyMode::Full), ws.path());
    let tool = tools
        .iter()
        .find(|tool| tool.name() == "csv_export")
        .unwrap();
    let args = json!({
        "data": serde_json::to_string(&json!([{ "value": "x".repeat(MAX_CSV_BYTES / 2) }])).unwrap(),
        "columns": ["value", "value"],
        "filename": "oversized.csv"
    });
    for result in [
        tool.execute(args.clone()).await.unwrap(),
        tool.execute_with_options(args.clone(), ToolCallOptions::default())
            .await
            .unwrap(),
        tool.execute_with_context(args, ToolCallOptions::default(), None)
            .await
            .unwrap(),
    ] {
        assert!(result.is_error, "{}", result.output());
        assert!(result.output().contains("bytes"), "{}", result.output());
    }
    assert!(!ws.path().join("exports").exists());

    let result = tool
        .execute(json!({
            "data": r#"[{"value":"comma, quote\" and newline\n"}]"#,
            "filename": "small.csv"
        }))
        .await
        .unwrap();
    assert!(!result.is_error, "{}", result.output());
    assert_eq!(
        std::fs::read_to_string(ws.path().join("exports/small.csv")).unwrap(),
        "value\n\"comma, quote\"\" and newline\n\"\n"
    );
}

#[test]
fn csv_limits_admit_the_exact_row_and_byte_boundaries() {
    let row_args =
        |count| json!({ "data": serde_json::to_string(&vec![json!({}); count]).unwrap() });
    assert!(CsvLimits.refusal(&row_args(MAX_CSV_ROWS)).is_none());
    assert!(CsvLimits.refusal(&row_args(MAX_CSV_ROWS + 1)).is_some());
    let byte_args = |count| json!({ "data": serde_json::to_string(&json!([{ "x": "y".repeat(count) }])).unwrap() });
    assert!(CsvLimits.refusal(&byte_args(MAX_CSV_BYTES - 3)).is_none());
    assert!(CsvLimits.refusal(&byte_args(MAX_CSV_BYTES - 2)).is_some());
    assert!(
        CsvLimits
            .refusal(&json!({ "data": " ".repeat(MAX_CSV_INPUT_BYTES + 1) }))
            .is_some()
    );
    assert_eq!(csv_cell_bytes("a,\"\n\r"), 8);
}

/// The SSRF allowlist is a per-company setting, and `http_request` — the
/// arbitrary-method tool, the one that can POST — is constructed from the
/// same `allowed_domains` vector as `web_fetch` in [`web_tools`]. A strict
/// (non-empty) list must therefore bind it just as tightly: a public host
/// the company did not list is refused before any request leaves.
#[tokio::test]
async fn a_strict_domain_allowlist_binds_http_request_as_it_binds_web_fetch() {
    let ws = std::env::temp_dir();
    let security = test_security(&ws, PolicyMode::Full);
    let tools = web_tools(security, vec!["allowed.example.com".to_string()], &ws);

    for tool in &tools {
        if !matches!(tool.name(), "web_fetch" | "http_request") {
            continue;
        }
        let result = tool
            .execute(json!({ "url": "https://not-listed.example.org/" }))
            .await
            .expect("tool call completes");
        assert!(
            result.is_error,
            "{} must refuse a host outside a strict allowlist: {}",
            tool.name(),
            result.output()
        );
    }
}

/// `image_info` parses dimensions out of a file the agent names, so the
/// input it is handed is attacker-shaped in the ordinary case (a download
/// the agent just made with `curl`, into the same workspace). It must bound
/// the file by size *before* parsing, so an oversized file is refused
/// rather than read into memory and walked.
#[tokio::test]
async fn image_info_refuses_an_oversized_file_before_parsing_it() {
    let ws_dir = tempfile::Builder::new()
        .prefix("oc-toolbelt-bigimg-")
        .tempdir()
        .expect("tempdir");
    let ws = ws_dir.path();
    let path = ws.join("bomb.png");

    // A PNG magic header followed by padding past any sane ceiling. The
    // header makes it parseable-looking; the size is the thing under test.
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.resize(12 * 1024 * 1024, 0u8);
    std::fs::write(&path, &bytes).expect("seed oversized image");

    let security = test_security(ws, PolicyMode::Full);
    let tool = ImageInfoTool::new(security);

    let result = tool
        .execute(json!({ "path": path.to_string_lossy() }))
        .await
        .expect("tool call completes");

    assert!(
        result.is_error,
        "a {}-byte image must be refused on size before it is parsed: {}",
        bytes.len(),
        result.output()
    );
}
