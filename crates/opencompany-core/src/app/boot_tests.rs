use super::*;

/// An unparseable `auth_mode` aborts the boot rather than being dropped.
///
/// The same refusal `serve` makes, and for the same reason: "the sign-in you
/// configured is not the one you got" is invisible from a running host. It
/// has to be made *here* rather than by each embedder, because an embedder
/// that read this field leniently would turn a typo into a silently
/// different security posture — on the desktop, into a host with no sign-in
/// where the operator asked for one.
#[tokio::test]
async fn an_unparseable_auth_mode_refuses_to_start() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "auth_mode = \"emial\"\n").unwrap();

    let error = prepare_instance(Some(dir.path().to_path_buf()))
        .await
        .expect_err("a mode nobody can parse must not be ignored");
    assert!(
        error.to_string().contains("emial"),
        "the refusal has to name the value: {error}"
    );
}

/// A file naming no mode leaves the decision to the embedder, which is not
/// the same as naming `email` — that default lives one layer further down,
/// on each company's own `[users].mode`.
#[tokio::test]
async fn an_absent_auth_mode_stays_absent() {
    let dir = tempfile::tempdir().unwrap();
    let instance = prepare_instance(Some(dir.path().to_path_buf()))
        .await
        .unwrap();
    assert_eq!(instance.auth_mode(), None);
}

#[tokio::test]
async fn a_prepared_root_is_locked_against_a_second_instance() {
    let dir = tempfile::tempdir().unwrap();
    let first = prepare_instance(Some(dir.path().to_path_buf()))
        .await
        .expect("the first instance prepares");
    assert_eq!(first.home(), dir.path());

    let refused = prepare_instance(Some(dir.path().to_path_buf()))
        .await
        .expect_err("a second instance on one root must be refused");
    assert!(
        refused
            .to_string()
            .contains("already using the data directory"),
        "unexpected refusal: {refused}"
    );
}

/// How long [`releasing_the_instance_frees_the_root`] waits for a released
/// root to become takeable, and how long it sleeps between attempts.
///
/// The window this rides out is measured in microseconds (see the test's
/// own comment), so two seconds is several orders of magnitude of headroom
/// — generous enough that only a genuinely stuck lock exhausts it, short
/// enough that a real regression fails the suite promptly rather than
/// looking like a hang.
const RELEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const RELEASE_POLL: std::time::Duration = std::time::Duration::from_millis(10);

#[tokio::test]
async fn releasing_the_instance_frees_the_root() {
    // A desktop app restarted after a clean quit must be able to start.
    let dir = tempfile::tempdir().unwrap();
    drop(
        prepare_instance(Some(dir.path().to_path_buf()))
            .await
            .unwrap(),
    );

    // Retried rather than asserted in the same instant, because an instant
    // re-acquire is stricter than the lock promises. `store::lock`'s "The
    // fork window" section states it outright: the lock belongs to the open
    // file description, so between `fork()` and `exec()` a child transiently
    // shares every descriptor its parent held. This binary spawns
    // subprocesses in sibling tests (`app::journal`'s vendored-seam and
    // keyring-pin children), and one landing in this test's release window
    // keeps the just-released root locked for the microseconds until the
    // child `exec`s and its `O_CLOEXEC` copy closes.
    //
    // So this asserts what the lock actually guarantees — that the root
    // *becomes* takeable — and not that it is takeable within one
    // instruction. A lock that never releases still fails: the loop reports
    // the last refusal after the timeout rather than passing quietly, which
    // is what stops the retry being a blindfold over a real regression.
    let deadline = std::time::Instant::now() + RELEASE_TIMEOUT;
    let mut attempts = 0;
    let last = loop {
        attempts += 1;
        match prepare_instance(Some(dir.path().to_path_buf())).await {
            Ok(_) => return,
            Err(e) if std::time::Instant::now() >= deadline => break e,
            Err(_) => tokio::time::sleep(RELEASE_POLL).await,
        }
    };
    panic!(
        "a released root must become takeable, but {attempts} attempts over \
         {RELEASE_TIMEOUT:?} were all refused; last error: {last}"
    );
}

/// The gap this closed: `serve` materialized the workspace layout and the
/// desktop did not, so an embedded instance never had `memory/`, `store/`,
/// `files/`, `logs/` or `tmp/` and never cleared stale scratch on restart.
#[tokio::test]
async fn preparing_materializes_the_workspace_layout() {
    let dir = tempfile::tempdir().unwrap();
    let instance = prepare_instance(Some(dir.path().to_path_buf()))
        .await
        .unwrap();

    let layout = DataLayout::new(instance.home());
    for expected in [
        layout.memory_dir(),
        layout.store_dir(),
        layout.files_dir(),
        layout.logs_dir(),
        layout.tmp_dir(),
    ] {
        assert!(
            expected.is_dir(),
            "{} must exist after prepare_instance",
            expected.display()
        );
    }
}

#[tokio::test]
async fn preparing_clears_tmp_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let stale = DataLayout::new(dir.path()).tmp_dir().join("stale.txt");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "from a previous run").unwrap();

    drop(
        prepare_instance(Some(dir.path().to_path_buf()))
            .await
            .unwrap(),
    );

    assert!(
        !stale.exists(),
        "the ephemeral tmp/ scratch must not survive a restart"
    );
}

/// `[workspace]` in the root's `config.toml` is honoured by the embedded
/// path, not only by `serve` — both the layout knob and the two fields the
/// embedder has to put on its own `AppConfig`.
#[tokio::test]
async fn preparing_reads_the_workspace_section_from_config_toml() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[workspace]\nclear_tmp_on_startup = false\ngit_enabled = true\nmax_blob_mb = 8.0\n",
    )
    .unwrap();
    let keep = DataLayout::new(dir.path()).tmp_dir().join("keep.txt");
    std::fs::create_dir_all(keep.parent().unwrap()).unwrap();
    std::fs::write(&keep, "kept").unwrap();

    let instance = prepare_instance(Some(dir.path().to_path_buf()))
        .await
        .unwrap();

    assert!(
        keep.exists(),
        "clear_tmp_on_startup = false must be honoured"
    );
    assert!(instance.workspace().git_enabled);
    assert_eq!(
        instance.workspace().quota.max_blob_bytes,
        8 * 1024 * 1024,
        "the embedder must see the configured blob cap, not the default"
    );
}

#[tokio::test]
async fn preparing_reports_the_journal_without_exporting_it() {
    // The whole point of the embedded path: no `set_var`, because in a
    // desktop process other threads are already running and racing a
    // concurrent `getenv` is undefined behaviour rather than a stale read.
    let dir = tempfile::tempdir().unwrap();
    let before = std::env::var("OPENHUMAN_WORKSPACE").ok();

    let instance = prepare_instance(Some(dir.path().to_path_buf()))
        .await
        .unwrap();

    assert_eq!(std::env::var("OPENHUMAN_WORKSPACE").ok(), before);
    let (name, value) = instance.journal_env();
    assert_eq!(name, "OPENHUMAN_WORKSPACE");
    assert!(
        value.starts_with(dir.path()),
        "the journal must sit under the instance root: {}",
        value.display()
    );
}
