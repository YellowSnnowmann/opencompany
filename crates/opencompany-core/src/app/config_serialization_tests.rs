use super::config_resolution_tests::*;
use super::*;

// -----------------------------------------------------------------
// resolve_serve_bind: the layers `serve` actually honours.
//
// Before issue #425 `serve` read only its `--bind` flag, so
// `OPENCOMPANY_BIND` moved `doctor`'s report but never the listener.
// `serve_bind_env_beats_config_toml` is the regression test for exactly
// that: it is red against the flag-only behaviour.
// -----------------------------------------------------------------

#[test]
pub(super) fn serve_bind_flag_beats_env_and_config_toml() {
    let env = MapEnv::new([("OPENCOMPANY_BIND", "127.0.0.1:2222")]);
    let (bind, source) = resolve_serve_bind(
        Some("127.0.0.1:1111".into()),
        &env,
        Some("127.0.0.1:3333".into()),
    );
    assert_eq!(bind, "127.0.0.1:1111");
    assert_eq!(source, "--bind");
}

#[test]
pub(super) fn serve_bind_env_beats_config_toml() {
    let env = MapEnv::new([("OPENCOMPANY_BIND", "127.0.0.1:2222")]);
    let (bind, source) = resolve_serve_bind(None, &env, Some("127.0.0.1:3333".into()));
    assert_eq!(bind, "127.0.0.1:2222");
    assert_eq!(source, "OPENCOMPANY_BIND");
}

#[test]
pub(super) fn serve_bind_empty_env_falls_through() {
    // Same empty-is-unset convention `empty_env_value_is_ignored` pins for
    // the `resolve` chain: an exported-but-blank variable must not shadow
    // the layer beneath it.
    let env = MapEnv::new([("OPENCOMPANY_BIND", "")]);
    let (bind, source) = resolve_serve_bind(None, &env, Some("127.0.0.1:3333".into()));
    assert_eq!(bind, "127.0.0.1:3333");
    assert_eq!(source, "config.toml");

    // With nothing under it either, an empty variable reaches the default.
    let (bind, source) = resolve_serve_bind(None, &env, None);
    assert_eq!(bind, DEFAULT_BIND);
    assert_eq!(source, "default");
}

#[test]
pub(super) fn serve_bind_config_toml_used_when_no_flag_or_env() {
    let env = MapEnv::default();
    let (bind, source) = resolve_serve_bind(None, &env, Some("127.0.0.1:3333".into()));
    assert_eq!(bind, "127.0.0.1:3333");
    assert_eq!(source, "config.toml");
}

#[test]
pub(super) fn serve_bind_defaults_to_loopback_when_nothing_set() {
    let env = MapEnv::default();
    let (bind, source) = resolve_serve_bind(None, &env, None);
    assert_eq!(bind, DEFAULT_BIND);
    assert_eq!(source, "default");
    // The default must stay loopback: a wildcard bind is only ever reached
    // by explicit operator intent (flag, variable, or config entry).
    assert!(
        bind.starts_with("127.0.0.1:"),
        "default bind must be loopback"
    );
}

#[test]
pub(super) fn serve_bind_honours_a_wildcard_only_from_an_explicit_layer() {
    // The hosted manager injects `OPENCOMPANY_BIND=0.0.0.0:8080`; that must
    // reach the listener, and be attributed to the variable.
    let env = MapEnv::new([("OPENCOMPANY_BIND", "0.0.0.0:8080")]);
    let (bind, source) = resolve_serve_bind(None, &env, None);
    assert_eq!(bind, "0.0.0.0:8080");
    assert_eq!(source, "OPENCOMPANY_BIND");
}

#[test]
pub(super) fn config_file_load_returns_none_when_absent() {
    let dir = std::env::temp_dir().join(format!("oc-cfg-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(ConfigFile::load(&dir).unwrap().is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
pub(super) fn config_file_load_parses_toml() {
    let dir = std::env::temp_dir().join(format!("oc-cfg-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(CONFIG_FILE),
        "brain_mode = \"sidecar\"\napi_url = \"https://x\"\n",
    )
    .unwrap();
    let file = ConfigFile::load(&dir).unwrap().unwrap();
    assert_eq!(file.brain_mode.as_deref(), Some("sidecar"));
    assert_eq!(file.api_url.as_deref(), Some("https://x"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
pub(super) fn workspace_section_defaults_to_clearing_tmp() {
    // Absent `[workspace]` → default (clear on startup).
    assert!(WorkspaceSection::default().resolve().clear_tmp_on_startup);
    // An explicit opt-out is honored.
    let section = WorkspaceSection {
        clear_tmp_on_startup: Some(false),
        ..WorkspaceSection::default()
    };
    assert!(!section.resolve().clear_tmp_on_startup);
}

#[test]
pub(super) fn workspace_section_parses_quotas() {
    let section = WorkspaceSection {
        storage_quota_gb: Some(2.0),
        tmp_quota_gb: Some(0.0), // non-positive → unlimited
        ..WorkspaceSection::default()
    };
    let cfg = section.resolve();
    assert_eq!(cfg.storage_quota_bytes, Some(2 * 1024 * 1024 * 1024));
    assert_eq!(cfg.tmp_quota_bytes, None);
    // Absent → unlimited.
    assert_eq!(
        WorkspaceSection::default().resolve().storage_quota_bytes,
        None
    );
}

#[test]
pub(super) fn config_file_parses_workspace_section() {
    let dir = std::env::temp_dir().join(format!("oc-cfg-ws-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(CONFIG_FILE),
        "[workspace]\ngit_enabled = true\nclear_tmp_on_startup = false\n",
    )
    .unwrap();
    let file = ConfigFile::load(&dir).unwrap().unwrap();
    assert_eq!(file.workspace.clear_tmp_on_startup, Some(false));
    assert_eq!(file.workspace.git_enabled, Some(true));
    assert!(!file.workspace.resolve().clear_tmp_on_startup);
    assert!(file.workspace.resolve().git_enabled);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
pub(super) fn malformed_config_file_is_a_config_error() {
    let dir = std::env::temp_dir().join(format!("oc-cfg-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(CONFIG_FILE), "not = = valid").unwrap();
    let err = ConfigFile::load(&dir).unwrap_err();
    assert_eq!(err.code(), "config_error");
    std::fs::remove_dir_all(&dir).ok();
}

// -----------------------------------------------------------------------
// write_config_toml
// -----------------------------------------------------------------------

pub(super) fn write_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("oc-write-{tag}-"))
        .tempdir()
        .expect("tempdir")
}

/// Writing into a data root that has no `config.toml` yet — the first-run
/// case — creates the file, and it parses back through the normal reader.
#[test]
pub(super) fn writing_creates_the_file_when_absent() {
    let dir = write_dir("new");
    let path = write_config_toml(
        dir.path(),
        &[
            ("bind", ConfigValue::Str("0.0.0.0:9000".into())),
            ("auth_mode", ConfigValue::Str("none".into())),
        ],
    )
    .unwrap();

    assert_eq!(path, dir.path().join(CONFIG_FILE));
    let file = ConfigFile::load(dir.path()).unwrap().unwrap();
    assert_eq!(file.bind.as_deref(), Some("0.0.0.0:9000"));
    assert_eq!(file.auth_mode.as_deref(), Some("none"));
}

/// The reason this goes through `toml_edit` at all: the shipped file's
/// commented `[[default_mcp_server]]` PLACEHOLDER block is documentation an
/// operator is meant to read and uncomment, and serializing a `ConfigFile`
/// back out would delete it along with every other comment. Untouched keys
/// must survive too.
#[test]
pub(super) fn writing_preserves_comments_and_untouched_keys() {
    let dir = write_dir("comments");
    std::fs::write(
        dir.path().join(CONFIG_FILE),
        "# The instance's bind address.\n\
         bind = \"127.0.0.1:8080\"\n\
         api_url = \"https://api.example.test\"\n\
         \n\
         # PLACEHOLDER — uncomment to ship a default tool server.\n\
         # [[default_mcp_server]]\n\
         # name = \"deepwiki\"\n",
    )
    .unwrap();

    write_config_toml(
        dir.path(),
        &[("bind", ConfigValue::Str("0.0.0.0:9000".into()))],
    )
    .unwrap();

    let text = std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
    assert!(text.contains("# The instance's bind address."));
    assert!(text.contains("# PLACEHOLDER — uncomment to ship a default tool server."));
    assert!(text.contains("# [[default_mcp_server]]"));
    assert!(text.contains("# name = \"deepwiki\""));
    assert!(
        text.contains("api_url = \"https://api.example.test\""),
        "an untouched key must survive verbatim"
    );
    assert!(text.contains("0.0.0.0:9000"), "the edit must land");
    assert!(
        !text.contains("127.0.0.1:8080"),
        "the old value must be replaced, not duplicated"
    );
}

/// A dotted key writes into `[workspace]`, creating the table when needed,
/// and the result resolves through `WorkspaceSection`.
#[test]
pub(super) fn writing_reaches_into_the_workspace_table() {
    let dir = write_dir("ws");
    write_config_toml(
        dir.path(),
        &[
            ("workspace.clear_tmp_on_startup", ConfigValue::Bool(false)),
            ("workspace.max_blob_mb", ConfigValue::Float(64.0)),
        ],
    )
    .unwrap();

    let text = std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
    assert!(
        text.contains("[workspace]"),
        "the table header must be explicit, not implicit: {text}"
    );

    let file = ConfigFile::load(dir.path()).unwrap().unwrap();
    assert_eq!(file.workspace.clear_tmp_on_startup, Some(false));
    assert_eq!(file.workspace.max_blob_mb, Some(64.0));
    assert!(!file.workspace.resolve().clear_tmp_on_startup);
}

/// `Unset` removes the key rather than writing `""`. The difference matters:
/// an absent key falls through to the next precedence layer, where a blank
/// string would be a set-but-empty value.
#[test]
pub(super) fn unset_removes_the_key_so_the_next_layer_applies() {
    let dir = write_dir("unset");
    std::fs::write(
        dir.path().join(CONFIG_FILE),
        "auth_mode = \"wallet\"\nbind = \"0.0.0.0:9000\"\n",
    )
    .unwrap();

    write_config_toml(dir.path(), &[("auth_mode", ConfigValue::Unset)]).unwrap();

    let text = std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
    assert!(!text.contains("auth_mode"), "the key must be gone: {text}");

    let file = ConfigFile::load(dir.path()).unwrap().unwrap();
    assert!(file.auth_mode.is_none());
    assert_eq!(file.bind.as_deref(), Some("0.0.0.0:9000"));

    // And with the key gone, resolution falls through to the manifest.
    let mut manifest = default_manifest();
    manifest.users.mode = "wallet".into();
    let (cfg, prov) = resolve(&MapEnv::default(), Some(&file), &manifest).unwrap();
    assert_eq!(cfg.auth_mode, AuthMode::Wallet);
    assert_eq!(prov.layer("auth_mode"), Some(ConfigLayer::Manifest));
}

/// Clearing a key out of a `[workspace]` table that does not exist must not
/// materialize an empty table just to delete nothing out of it.
#[test]
pub(super) fn unset_does_not_materialize_a_missing_table() {
    let dir = write_dir("unset-ws");
    write_config_toml(dir.path(), &[("workspace.max_blob_mb", ConfigValue::Unset)]).unwrap();
    let text = std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
    assert!(!text.contains("[workspace]"), "no empty table: {text}");
}

/// Merging into a document that could not be parsed would overwrite whatever
/// the operator actually had there, so a malformed file is refused — the
/// same contract `ConfigFile::load` holds.
#[test]
pub(super) fn writing_refuses_a_malformed_existing_file() {
    let dir = write_dir("bad");
    std::fs::write(dir.path().join(CONFIG_FILE), "not = = valid").unwrap();

    let err = write_config_toml(
        dir.path(),
        &[("bind", ConfigValue::Str("0.0.0.0:9000".into()))],
    )
    .unwrap_err();
    assert_eq!(err.code(), "config_error");

    let text = std::fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap();
    assert_eq!(text, "not = = valid", "the original must be left alone");
}

/// The write is atomic via a same-directory temp file and `rename`. Nothing
/// may be left behind for the next boot (or the next write) to trip over —
/// checked by name pattern rather than the old fixed `config.toml.tmp`,
/// since the temp name is now made unique per call.
#[test]
pub(super) fn writing_leaves_no_temp_file_behind() {
    let dir = write_dir("tmp");
    write_config_toml(
        dir.path(),
        &[("bind", ConfigValue::Str("0.0.0.0:9000".into()))],
    )
    .unwrap();
    let leftover: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp"))
        .collect();
    assert!(leftover.is_empty(), "left behind: {leftover:?}");
}

/// A write that fails after the temp file's directory disappears out from
/// under it must not leave anything behind, and must report the failure
/// rather than panic — the failure half of
/// `writing_leaves_no_temp_file_behind` above, forced deterministically
/// (absent-parent, like `store::fs::durable_append_reports_an_unwritable_path`)
/// rather than by injecting a permission failure, which behaves
/// differently depending on whether the test runs as root.
#[test]
pub(super) fn a_write_that_fails_leaves_no_temp_file_behind() {
    let dir = write_dir("write-fails");
    let root = dir.path().join("gone");
    // `existing` reads NotFound as "no config yet" and proceeds, so the
    // failure below comes from the temp file's own `std::fs::write`
    // rather than from the initial read.
    let err =
        write_config_toml(&root, &[("bind", ConfigValue::Str("0.0.0.0:9000".into()))]).unwrap_err();
    assert_eq!(err.code(), "config_error");
    assert!(
        !root.exists(),
        "a write into a missing directory must not create it or anything in it"
    );
}

/// Two writers racing the same directory must not clobber each other: each
/// call's edits land, and neither call's temp file collides with the
/// other's — the bug CodeRabbit flagged on #908 (`config.rs:549`).
#[test]
pub(super) fn concurrent_writes_to_the_same_directory_do_not_clobber_each_other() {
    let dir = write_dir("concurrent");
    let path = dir.path().to_path_buf();

    let a = std::thread::spawn({
        let path = path.clone();
        move || {
            for i in 0..25 {
                write_config_toml(
                    &path,
                    &[("bind", ConfigValue::Str(format!("0.0.0.0:{}", 9000 + i)))],
                )
                .unwrap();
            }
        }
    });
    let b = std::thread::spawn({
        let path = path.clone();
        move || {
            for i in 0..25 {
                write_config_toml(
                    &path,
                    &[(
                        "public_url",
                        ConfigValue::Str(format!("https://h{i}.example")),
                    )],
                )
                .unwrap();
            }
        }
    });
    a.join().unwrap();
    b.join().unwrap();

    // Both threads' edits target different keys, so a clobbered write would
    // show up as one key or the other missing from the final file — not as
    // a torn/unparseable file, which `ConfigFile::load` would already catch.
    let file = ConfigFile::load(&path).unwrap().unwrap();
    assert!(file.bind.is_some(), "the bind writer's edits went missing");
    assert!(
        file.public_url.is_some(),
        "the public_url writer's edits went missing"
    );

    // No stray temp file from either racer.
    let leftover: Vec<_> = std::fs::read_dir(&path)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp"))
        .collect();
    assert!(leftover.is_empty(), "left behind: {leftover:?}");
}

/// Setup completion is recorded in the file, so "has this instance been set
/// up" survives a new browser and travels with the data root.
#[test]
pub(super) fn setup_completion_round_trips() {
    let dir = write_dir("done");
    assert!(
        ConfigFile::load(dir.path()).unwrap().is_none(),
        "a fresh data root has no config at all"
    );

    write_config_toml(
        dir.path(),
        &[("setup_completed_at", ConfigValue::Int(1_755_000_000_000))],
    )
    .unwrap();

    let file = ConfigFile::load(dir.path()).unwrap().unwrap();
    assert_eq!(file.setup_completed_at, Some(1_755_000_000_000));
}
