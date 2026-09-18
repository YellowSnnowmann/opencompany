use super::*;

async fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "t@t.test"],
        vec!["config", "user.name", "T"],
    ] {
        git(&repo, &args).await.unwrap();
    }
    std::fs::write(repo.join("README.md"), "hello").unwrap();
    git(&repo, &["add", "."]).await.unwrap();
    git(&repo, &["commit", "-q", "-m", "first"]).await.unwrap();
    (dir, repo)
}

#[tokio::test]
async fn two_tasks_get_directories_that_cannot_collide() {
    // The whole reason this exists. Shared-directory designs put both of
    // these in one place and let whoever writes last win.
    let (dir, repo) = repo().await;
    let root = dir.path().join("work");

    let a = acquire(&root, &repo, "task-a").await.unwrap();
    let b = acquire(&root, &repo, "task-b").await.unwrap();

    assert_ne!(a.path, b.path);
    assert_eq!(a.isolation, Isolation::Worktree);
    assert_eq!(b.isolation, Isolation::Worktree);
    // A branch each, so each task's work is a reviewable diff.
    assert_ne!(a.branch, b.branch);

    // Genuinely independent trees: a write in one is invisible in the other.
    std::fs::write(a.path.join("only-a.txt"), "x").unwrap();
    assert!(!b.path.join("only-a.txt").exists());
}

#[tokio::test]
async fn a_clean_worktree_is_removed_on_release() {
    let (dir, repo) = repo().await;
    let root = dir.path().join("work");
    let workspace = acquire(&root, &repo, "task-clean").await.unwrap();
    assert!(workspace.path.exists());

    release(&repo, &workspace)
        .await
        .expect("a clean tree is removed");
    assert!(!workspace.path.exists());
}

#[tokio::test]
async fn uncommitted_work_is_never_destroyed() {
    // A task that failed half way has left the only copy of whatever it
    // did. Disk is cheaper than that.
    let (dir, repo) = repo().await;
    let root = dir.path().join("work");
    let workspace = acquire(&root, &repo, "task-dirty").await.unwrap();
    std::fs::write(
        workspace.path.join("README.md"),
        "edited but never committed",
    )
    .unwrap();

    let refused = release(&repo, &workspace).await;
    assert!(
        matches!(refused, Err(WorktreeError::Dirty { .. })),
        "{refused:?}"
    );
    assert!(workspace.path.exists(), "the work must still be there");
}

#[tokio::test]
async fn a_brand_new_untracked_file_counts_as_work() {
    // A task whose whole output is one new file shows up only as an
    // untracked entry. A dirty check that ignored those would delete
    // exactly the tasks that succeeded.
    let (dir, repo) = repo().await;
    let root = dir.path().join("work");
    let workspace = acquire(&root, &repo, "task-new").await.unwrap();
    std::fs::write(workspace.path.join("result.md"), "the answer").unwrap();

    assert!(matches!(
        release(&repo, &workspace).await,
        Err(WorktreeError::Dirty { .. })
    ));
}

#[tokio::test]
async fn a_target_that_is_not_a_repository_still_gets_a_directory() {
    // Plenty of real work is not in git, and refusing it would make this
    // unavailable where someone is most likely to be experimenting. The
    // weaker guarantee is reported rather than hidden.
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("not-a-repo");
    std::fs::create_dir_all(&plain).unwrap();

    let workspace = acquire(&dir.path().join("work"), &plain, "task-plain")
        .await
        .unwrap();
    assert_eq!(workspace.isolation, Isolation::PlainDirectory);
    assert!(workspace.branch.is_none());
    assert!(workspace.path.is_dir());

    // And releasing it removes nothing, because nothing here tracked what
    // was in it.
    release(&plain, &workspace).await.unwrap();
    assert!(workspace.path.exists());
}

#[tokio::test]
async fn retrying_a_task_reuses_its_workspace() {
    // A retry must not fail because the previous attempt's directory is
    // still there — and must not silently start somewhere else either.
    let (dir, repo) = repo().await;
    let root = dir.path().join("work");
    let first = acquire(&root, &repo, "task-retry").await.unwrap();
    std::fs::write(first.path.join("progress.txt"), "half done").unwrap();

    let second = acquire(&root, &repo, "task-retry").await.unwrap();
    assert_eq!(first.path, second.path);
    assert!(second.path.join("progress.txt").exists());
}
