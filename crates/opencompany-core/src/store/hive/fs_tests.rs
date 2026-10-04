//! [`FsHiveStore`]: the shared suite, plus what a crash leaves in the files.

use std::io::Write;
use std::sync::Arc;

use super::*;
use crate::store::hive::conformance;

fn store(root: &std::path::Path) -> Arc<dyn HiveStore> {
    Arc::new(FsHiveStore::new(root))
}

fn messages_path(root: &std::path::Path, company: &CompanyId) -> std::path::PathBuf {
    Bundle::new(root, company).dir().join("hive").join("messages.jsonl")
}

fn append_raw(path: &std::path::Path, bytes: &[u8]) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open messages")
        .write_all(bytes)
        .expect("append");
}

#[tokio::test]
async fn conformance_hive_store() {
    let root = tempfile::tempdir().unwrap();
    conformance::assert_hive_store(store(root.path())).await;
}

#[tokio::test]
async fn conformance_hive_commit_race() {
    let root = tempfile::tempdir().unwrap();
    conformance::assert_hive_commit_race(store(root.path())).await;
}

#[tokio::test]
async fn the_state_and_log_live_under_the_company_hive_directory() {
    let root = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let hive = store(root.path());
    conformance::seed_committed(&hive, &company).await;

    let dir = Bundle::new(root.path(), &company).dir().join("hive");
    assert!(dir.join("state.json").is_file());
    let log = std::fs::read_to_string(dir.join("messages.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 3, "one line per row: {log}");
}

/// A crash after the rows were flushed and before the state rename: whole
/// orphan lines past the committed length, then a torn one with no newline.
#[tokio::test]
async fn rows_past_the_committed_length_are_ignored_and_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let hive = store(root.path());
    conformance::seed_committed(&hive, &company).await;

    let path = messages_path(root.path(), &company);
    for sequence in 3..6 {
        let line = serde_json::to_string(&conformance::orphan_row(sequence)).unwrap();
        append_raw(&path, format!("{line}\n").as_bytes());
    }
    append_raw(&path, br#"{"sequence":6,"bo"#);

    conformance::assert_orphans_ignored(hive, &company).await;
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(!log.contains("orphan"), "the orphan tail was not truncated: {log}");
    assert!(log.ends_with('\n'));
}

/// A log shorter than the length the state document committed is lost data,
/// not an empty hive: the load says so rather than returning a short history.
#[tokio::test]
async fn a_log_shorter_than_its_committed_length_fails_the_load() {
    let root = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let hive = store(root.path());
    conformance::seed_committed(&hive, &company).await;

    let path = messages_path(root.path(), &company);
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(4).unwrap();

    let error = hive.load_hive(&company, None).await.unwrap_err();
    assert!(error.to_string().contains("shorter"), "{error}");
}
