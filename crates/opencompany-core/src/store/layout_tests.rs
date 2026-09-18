use super::*;

fn scratch_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("oc-layout-{}-{tag}", std::process::id()))
}

#[test]
fn subdirs_hang_off_the_root() {
    let layout = DataLayout::new("/data");
    assert_eq!(layout.root(), Path::new("/data"));
    assert_eq!(layout.companies_dir(), Path::new("/data/companies"));
    assert_eq!(layout.tmp_dir(), Path::new("/data/tmp"));
    assert_eq!(layout.memory_dir(), Path::new("/data/memory"));
    assert_eq!(
        layout.company_repos_dir("acme"),
        Path::new("/data/companies/acme/repos"),
    );
    assert_eq!(
        layout.agent_audit_dir("acme", "ceo"),
        Path::new("/data/companies/acme/audit/ceo"),
    );
}

/// The whole point of issue #775: the audit sink must not be reachable from
/// the agent workspace subtree, which is the `workspace_only` policy root
/// the file tools sandbox to. Pinned as a *path* property here, and proven
/// against the real file tools in `crate::harness::audit`'s tests.
#[test]
fn the_audit_sink_is_outside_every_agent_workspace() {
    let layout = DataLayout::new("/data");
    let audit = layout.agent_audit_dir("acme", "ceo");
    // The harness roots every agent workspace at `<root>/harness/...`.
    let workspaces = Path::new("/data/harness");
    assert!(
        !audit.starts_with(workspaces),
        "{} must not sit under the agent-workspace tree {}",
        audit.display(),
        workspaces.display(),
    );
    // Two agents in one company never share a directory — the vendored
    // logger registry caches per directory with first-config-wins, so a
    // shared directory would hand agent B agent A's file.
    assert_ne!(
        layout.agent_audit_dir("acme", "ceo"),
        layout.agent_audit_dir("acme", "cto"),
    );
}

#[tokio::test]
async fn ensure_creates_the_shared_subdirs() {
    let root = scratch_root("create");
    let layout = DataLayout::new(&root);
    layout.ensure(true).await.unwrap();
    for dir in layout.shared_dirs() {
        assert!(dir.is_dir(), "{} should exist", dir.display());
    }
    tokio::fs::remove_dir_all(&root).await.ok();
}

#[tokio::test]
async fn ensure_clears_tmp_but_keeps_it_when_asked() {
    let root = scratch_root("tmp");
    let layout = DataLayout::new(&root);
    layout.ensure(true).await.unwrap();

    let scratch = layout.tmp_dir().join("scratch.txt");
    tokio::fs::write(&scratch, b"stale").await.unwrap();

    // clear_tmp = false keeps the scratch file.
    layout.ensure(false).await.unwrap();
    assert!(scratch.exists(), "clear_tmp=false must keep tmp contents");

    // clear_tmp = true wipes it (but tmp/ itself is recreated).
    layout.ensure(true).await.unwrap();
    assert!(!scratch.exists(), "clear_tmp=true must empty tmp");
    assert!(
        layout.tmp_dir().is_dir(),
        "tmp/ is recreated after clearing"
    );

    tokio::fs::remove_dir_all(&root).await.ok();
}

#[tokio::test]
async fn usage_bytes_sums_files_recursively() {
    let root = scratch_root("usage");
    let layout = DataLayout::new(&root);
    layout.ensure(true).await.unwrap();
    tokio::fs::write(layout.files_dir().join("a.bin"), vec![0u8; 1000])
        .await
        .unwrap();
    tokio::fs::write(layout.tmp_dir().join("scratch.bin"), vec![0u8; 500])
        .await
        .unwrap();

    assert_eq!(
        layout.usage_bytes().await.unwrap(),
        1500,
        "root sums all files"
    );
    assert_eq!(layout.tmp_bytes().await.unwrap(), 500, "tmp/ subtree only");

    // A missing workspace measures zero, not an error.
    let absent = DataLayout::new(scratch_root("absent"));
    assert_eq!(absent.usage_bytes().await.unwrap(), 0);

    tokio::fs::remove_dir_all(&root).await.ok();
}

/// The repo mirror cache joins the boot quota measurement for free — it is
/// a subtree of the root, and `usage_bytes` walks the whole root. Asserted
/// rather than assumed: a cache that measured zero would let one bad clone
/// fill a tenant volume with the quota check reporting all clear.
#[tokio::test]
async fn usage_bytes_counts_the_repo_cache() {
    let root = scratch_root("repos-usage");
    let layout = DataLayout::new(&root);
    layout.ensure(true).await.unwrap();

    let repos = layout.company_repos_dir("acme");
    // `companies/<slug>/` does not exist yet on a mongodb tenant, so the
    // cache creates its own parents. That is the case measured here.
    tokio::fs::create_dir_all(repos.join("acme-widgets.git/objects"))
        .await
        .unwrap();
    tokio::fs::write(
        repos.join("acme-widgets.git/objects/pack.bin"),
        vec![0u8; 4096],
    )
    .await
    .unwrap();

    assert_eq!(
        layout.usage_bytes().await.unwrap(),
        4096,
        "the repo mirror cache must be inside the measured root"
    );

    tokio::fs::remove_dir_all(&root).await.ok();
}

/// The shell audit sink joins the soft quota for free, for the same reason
/// the repo cache does — it hangs off the measured root. Asserted rather
/// than assumed: a sink that measured zero would let a runaway command loop
/// fill a tenant volume with the quota check reporting all clear.
#[tokio::test]
async fn usage_bytes_counts_the_agent_audit_sink() {
    let root = scratch_root("audit-usage");
    let layout = DataLayout::new(&root);
    layout.ensure(true).await.unwrap();

    let audit = layout.agent_audit_dir("acme", "ceo");
    // `companies/<slug>/` does not exist yet on a mongodb tenant, so the
    // sink creates its own parents. That is the case measured here.
    tokio::fs::create_dir_all(&audit).await.unwrap();
    tokio::fs::write(audit.join("audit.log"), vec![0u8; 2048])
        .await
        .unwrap();

    assert_eq!(
        layout.usage_bytes().await.unwrap(),
        2048,
        "the audit sink must be inside the measured root"
    );

    tokio::fs::remove_dir_all(&root).await.ok();
}
