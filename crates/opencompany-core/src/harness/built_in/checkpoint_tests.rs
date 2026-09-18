use super::*;
use serde_json::json;
use tempfile::TempDir;

struct WriteTool(PathBuf);

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write_fixture"
    }
    fn description(&self) -> &str {
        "writes a fixture"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        std::fs::write(&self.0, args["body"].as_str().unwrap_or_default())?;
        Ok(ToolResult::success("written"))
    }
}

fn log(workspace: &Path) -> String {
    String::from_utf8(
        Command::new("git")
            .args(["log", "--format=%s"])
            .current_dir(workspace)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
}

#[tokio::test]
async fn initializes_out_of_band_and_checkpoints_a_tool_write() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    let checkpointer = WorkspaceCheckpointer::initialize(&workspace).unwrap();
    let mut tools = CheckpointingTool::wrap_all(
        vec![Box::new(WriteTool(workspace.join("answer.txt")))],
        checkpointer,
    );

    let result = tools
        .remove(0)
        .execute(json!({"body": "42"}))
        .await
        .unwrap();

    assert_eq!(result.output(), "written");
    assert!(workspace.join(".git").is_file());
    assert!(dir.path().join("workspace.git/HEAD").is_file());
    let history = log(&workspace);
    assert!(
        history.contains("checkpoint: after write_fixture"),
        "{history}"
    );
    assert!(
        history.contains("checkpoint: initialize workspace"),
        "{history}"
    );
}

#[tokio::test]
async fn an_unchanged_tool_call_creates_no_checkpoint() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    let checkpointer = WorkspaceCheckpointer::initialize(&workspace).unwrap();
    checkpointer.checkpoint("read_only").await.unwrap();
    assert_eq!(log(&workspace).lines().count(), 1);
}

#[tokio::test]
async fn a_failed_checkpoint_preserves_the_successful_tool_result() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    let checkpointer = WorkspaceCheckpointer::initialize(&workspace).unwrap();
    std::fs::remove_file(checkpointer.git_dir.join("HEAD")).unwrap();
    let mut tools = CheckpointingTool::wrap_all(
        vec![Box::new(WriteTool(workspace.join("answer.txt")))],
        checkpointer,
    );

    let result = tools
        .remove(0)
        .execute(json!({"body": "still written"}))
        .await
        .unwrap();

    assert_eq!(result.output(), "written");
    assert_eq!(
        std::fs::read_to_string(workspace.join("answer.txt")).unwrap(),
        "still written"
    );
}

#[tokio::test]
async fn concurrent_tool_calls_serialize_the_git_index() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    let checkpointer = WorkspaceCheckpointer::initialize(&workspace).unwrap();
    let mut tools = CheckpointingTool::wrap_all(
        vec![
            Box::new(WriteTool(workspace.join("one.txt"))),
            Box::new(WriteTool(workspace.join("two.txt"))),
        ],
        checkpointer,
    );
    let one = tools.remove(0);
    let two = tools.remove(0);

    let (one_result, two_result) = tokio::join!(
        one.execute(json!({"body": "one"})),
        two.execute(json!({"body": "two"}))
    );

    assert!(!one_result.unwrap().is_error);
    assert!(!two_result.unwrap().is_error);
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&workspace)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(String::from_utf8(status.stdout).unwrap().trim().is_empty());
    assert!(!dir.path().join("workspace.git/index.lock").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn an_agent_written_git_pointer_cannot_redirect_checkpoints() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    // A decoy "repository" the agent's pointer names, bearing a hook that
    // would expose host execution were the checkpointer to commit inside
    // it.
    let decoy = dir.path().join("decoy");
    let decoy_hooks = decoy.join("hooks");
    std::fs::create_dir_all(&decoy_hooks).unwrap();
    std::fs::write(
        decoy_hooks.join("post-commit"),
        "#!/bin/sh\ntouch host-executed\n",
    )
    .unwrap();

    // The agent plants its own pointer before the checkpointer initializes.
    std::fs::write(
        workspace.join(".git"),
        format!("gitdir: {}\n", decoy.display()),
    )
    .unwrap();

    let checkpointer = WorkspaceCheckpointer::initialize(&workspace).unwrap();
    let mut tools = CheckpointingTool::wrap_all(
        vec![Box::new(WriteTool(workspace.join("answer.txt")))],
        checkpointer,
    );
    tools
        .remove(0)
        .execute(json!({"body": "42"}))
        .await
        .unwrap();

    // Checkpoints land in the out-of-band repository, discovered normally
    // through the sanitized pointer...
    assert!(dir.path().join("workspace.git/HEAD").is_file());
    let history = log(&workspace);
    assert!(
        history.contains("checkpoint: after write_fixture"),
        "{history}"
    );
    // ...the planted pointer was overwritten with the real one...
    assert_eq!(
        std::fs::read_to_string(workspace.join(".git")).unwrap(),
        format!("gitdir: {}\n", dir.path().join("workspace.git").display())
    );
    // ...the decoy repository was never initialized...
    assert!(!decoy.join("HEAD").is_file());
    // ...and its hook never ran in the host process.
    assert!(!dir.path().join("host-executed").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_commits_never_run_repository_hooks() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    let checkpointer = WorkspaceCheckpointer::initialize(&workspace).unwrap();

    // Even a hook dropped straight into the out-of-band repository's own
    // hooks directory must never execute: `isolate_git` pins `core.hooksPath`
    // so a checkpoint commit cannot run agent-supplied code in the host
    // process, however the repository was poisoned.
    let hooks = checkpointer.git_dir.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join("post-commit"), "#!/bin/sh\ntouch hook-ran\n").unwrap();

    let mut tools = CheckpointingTool::wrap_all(
        vec![Box::new(WriteTool(workspace.join("answer.txt")))],
        checkpointer,
    );
    tools
        .remove(0)
        .execute(json!({"body": "42"}))
        .await
        .unwrap();

    assert!(
        !dir.path().join("hook-ran").exists(),
        "a checkpoint commit must not run repository hooks"
    );
    assert!(
        log(&workspace).contains("checkpoint: after write_fixture"),
        "the checkpoint still committed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_blocks_behind_an_in_flight_checkpoint_lock() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    WorkspaceCheckpointer::initialize(&workspace).unwrap();

    // Hold the per-git-dir checkpoint lock, then re-initialize on a thread
    // associated with the runtime: `initialize` must serialize behind the
    // same lock the per-call checkpoint path holds rather than racing it
    // into the Git index.
    let lock = path_lock(&dir.path().join("workspace.git"));
    let _guard = lock.lock().await;

    let entered = tokio::runtime::Handle::current();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _enter = entered.enter();
        let _ = tx.send(WorkspaceCheckpointer::initialize(&workspace).is_ok());
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        rx.try_recv().is_err(),
        "re-initialize raced past the in-flight checkpoint lock"
    );

    drop(_guard);
    assert!(
        rx.recv().expect("re-initialize completes"),
        "re-initialize must succeed once the checkpoint lock is released"
    );
    thread.join().expect("lock thread");
}

#[test]
fn current_thread_initialization_fails_instead_of_deadlocking_on_contention() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    WorkspaceCheckpointer::initialize(&workspace).unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let lock = path_lock(&dir.path().join("workspace.git"));
        let _guard = lock.lock().await;
        let started = std::time::Instant::now();

        let error = WorkspaceCheckpointer::initialize_off_worker(&workspace)
            .expect_err("current-thread initialization must not wait on a Tokio mutex");

        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "contended initialization blocked the current-thread runtime"
        );
        assert!(error.to_string().contains("contending"), "{error}");
    });
}
