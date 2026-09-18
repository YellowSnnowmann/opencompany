use super::*;

#[test]
fn core_preview_points_to_openhuman_core() {
    let preview = OpenHumanLaunch::core("vendor/openhuman")
        .with_args(["status".to_string()])
        .command_preview();

    assert!(preview.contains(&"openhuman-core".to_string()));
    assert!(preview.contains(&"vendor/openhuman/Cargo.toml".to_string()));
    assert_eq!(preview.last(), Some(&"status".to_string()));
}

#[test]
fn core_release_adds_release_flag_before_manifest() {
    let preview = OpenHumanLaunch::core("vendor/openhuman")
        .release()
        .command_preview();

    let run = preview.iter().position(|p| p == "run").unwrap();
    assert_eq!(preview[run + 1], "--release");
    assert!(preview.contains(&"openhuman-core".to_string()));
}

#[test]
fn desktop_preview_calls_cargo_tauri_not_pnpm_or_cargo_run() {
    let preview = OpenHumanLaunch::desktop("vendor/openhuman").command_preview();

    assert_eq!(preview.first().map(String::as_str), Some("cargo"));
    assert_eq!(preview.get(1).map(String::as_str), Some("tauri"));
    // Not a raw `cargo run --bin OpenHuman` (blank window / CEF panic) and
    // not a pnpm script delegation.
    assert!(
        !preview.contains(&"run".to_string()) || preview.iter().any(|p| p == "dev"),
        "desktop must call `cargo tauri dev`, not `cargo run`: {preview:?}"
    );
}

#[test]
fn desktop_wry_dev_args_match_openhuman_script() {
    // On Linux/Windows the dev args are exactly `dev --no-default-features
    // --features wry` — copied verbatim from OpenHuman's `dev:wry`.
    if cfg!(target_os = "macos") {
        return;
    }
    let preview = OpenHumanLaunch::desktop("vendor/openhuman").command_preview();
    let tail = &preview[2..];
    assert_eq!(
        tail,
        &[
            "dev".to_string(),
            "--no-default-features".into(),
            "--features".into(),
            "wry".into()
        ]
    );
}

#[test]
fn desktop_release_switches_to_build() {
    let dev = OpenHumanLaunch::desktop("vendor/openhuman").command_preview();
    let rel = OpenHumanLaunch::desktop("vendor/openhuman")
        .release()
        .command_preview();

    assert_ne!(dev.get(2), rel.get(2));
    assert_eq!(rel.get(2).map(String::as_str), Some("build"));
    assert!(rel.contains(&"--bin".to_string()));
    assert!(rel.contains(&"OpenHuman".to_string()));
}

#[test]
fn desktop_cef_args_build_app_dmg_in_release() {
    // The `--bundles app dmg` segment is what makes `cargo tauri build`
    // produce a working macOS .app/dmg rather than a bare binary — it is
    // only reachable on macOS via `for_host()`, so exercise the CEF branch
    // directly from any host.
    let launch = OpenHumanLaunch::desktop("vendor/openhuman").release();
    assert_eq!(
        launch.desktop_tauri_args_for(DesktopBackend::Cef, 1420),
        vec![
            "build".to_string(),
            "--bundles".into(),
            "app".into(),
            "dmg".into(),
            "--".into(),
            "--bin".into(),
            "OpenHuman".into()
        ]
    );
}

#[test]
fn desktop_cef_dev_args_interpolate_the_dev_port() {
    // The CEF dev script drives `dev --config {"build":{"devUrl":"…"}}`; the
    // JSON payload is the whole macOS dev surface, and it must carry the
    // resolved OPENHUMAN_DEV_PORT.
    let launch = OpenHumanLaunch::desktop("vendor/openhuman");
    assert_eq!(
        launch.desktop_tauri_args_for(DesktopBackend::Cef, 4242),
        vec![
            "dev".to_string(),
            "--config".into(),
            r#"{"build":{"devUrl":"http://localhost:4242"}}"#.to_string(),
        ]
    );
}

#[test]
fn desktop_dev_port_parses_numbers_and_falls_back() {
    assert_eq!(desktop_dev_port(None), 1420);
    assert_eq!(desktop_dev_port(Some("3000")), 3000);
    assert_eq!(desktop_dev_port(Some("  8080  ")), 8080);
    // Non-numeric, out-of-range, and reserved zero values fall back to the
    // default 1420 rather than emitting a devUrl JSON that could never
    // parse or that points at an unconnectable port 0.
    assert_eq!(desktop_dev_port(Some("not-a-port")), 1420);
    assert_eq!(desktop_dev_port(Some("70000")), 1420);
    assert_eq!(desktop_dev_port(Some("0")), 1420);
    assert_eq!(desktop_dev_port(Some("")), 1420);
}

#[test]
fn desktop_backend_matches_host_os() {
    assert_eq!(
        DesktopBackend::for_host(),
        if cfg!(target_os = "macos") {
            DesktopBackend::Cef
        } else {
            DesktopBackend::Wry
        }
    );
}

#[test]
fn parse_env_line_handles_comments_quotes_and_export() {
    assert_eq!(
        parse_env_line("FOO=bar"),
        Some(("FOO".into(), "bar".into()))
    );
    assert_eq!(
        parse_env_line("export FOO=bar"),
        Some(("FOO".into(), "bar".into()))
    );
    assert_eq!(
        parse_env_line(r#"FOO="bar baz""#),
        Some(("FOO".into(), "bar baz".into()))
    );
    assert_eq!(
        parse_env_line("FOO='bar'"),
        Some(("FOO".into(), "bar".into()))
    );
    assert_eq!(parse_env_line("# a comment"), None);
    assert_eq!(parse_env_line("   "), None);
    assert_eq!(
        parse_env_line("FOO=bar # inline"),
        Some(("FOO".into(), "bar".into()))
    );
    assert_eq!(parse_env_line("=novalue"), None);
}

#[test]
fn installed_from_vendored_detects_origin() {
    let vendored = std::path::Path::new(
        "/repo/vendor/openhuman/app/src-tauri/vendor/tauri-cef/crates/tauri-cli",
    );
    let yes = format!(
        "[[..]]\nname = \"tauri-cli\"\nversion = \"0.0.0\"\nsource = \"{}\"\n",
        vendored.display()
    );
    assert!(installed_from_vendored(&yes, vendored));
    let no = "[[..]]\nname = \"tauri-cli\"\nsource = \"registry+https://crates.io\"\n";
    assert!(!installed_from_vendored(no, vendored));
}

#[test]
fn tauri_cli_is_fresh_when_binary_is_newer_than_sources() {
    let tmp = tempfile_dir("fresh");
    let bin = tmp.join("bin").join("cargo-tauri");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(&bin, b"#!/bin/sh\n").unwrap();
    let src = tmp.join("tauri-cef");
    fs::create_dir_all(src.join("crates/tauri-cli")).unwrap();
    fs::write(src.join("crates/tauri-cli/Cargo.toml"), b"[]").unwrap();

    // Pin the binary's mtime strictly later than the sources instead of
    // relying on write ordering: on nanosecond-resolution filesystems a
    // source written *after* the binary is strictly newer than it, which
    // correctly reports stale and flips the assertion.
    let bin_mtime = SystemTime::now() + std::time::Duration::from_secs(3600);
    fs::File::open(&bin)
        .unwrap()
        .set_modified(bin_mtime)
        .unwrap();
    // Binary is pinned newer than the source → fresh.
    assert!(tauri_cli_is_fresh(&bin, &src));

    // Pin a source file strictly newer than the binary → stale.
    fs::File::open(src.join("crates/tauri-cli/Cargo.toml"))
        .unwrap()
        .set_modified(SystemTime::now() + std::time::Duration::from_secs(7200))
        .unwrap();
    assert!(!tauri_cli_is_fresh(&bin, &src));
}

#[test]
fn tauri_cli_is_fresh_false_when_binary_missing() {
    let tmp = tempfile_dir("missing");
    let src = tmp.join("tauri-cef");
    fs::create_dir_all(&src).unwrap();
    assert!(!tauri_cli_is_fresh(&tmp.join("missing"), &src));
}

#[test]
fn desktop_validate_rejects_passthrough_args() {
    let tmp = tempfile_dir("desktop-args");
    let launch = OpenHumanLaunch::desktop(&tmp).with_args(["--flag".to_string()]);
    let err = launch.validate().unwrap_err();
    assert!(matches!(
        &err,
        OpenCompanyError::OpenHuman { code: 400, .. }
    ));
    // Core mode still accepts passthrough args.
    let core = OpenHumanLaunch::core(&tmp).with_args(["--flag".to_string()]);
    assert!(core.validate().is_ok());
}

#[tokio::test]
async fn desktop_run_rejects_passthrough_args_before_spawning() {
    let tmp = tempfile_dir("desktop-run-args");
    let launch = OpenHumanLaunch::desktop(&tmp).with_args(["--flag".to_string()]);
    let err = launch.run().await.unwrap_err();
    assert!(matches!(
        &err,
        OpenCompanyError::OpenHuman { code: 400, .. }
    ));
}

#[test]
fn dry_run_preview_names_desktop_working_directory() {
    let tmp = tempfile_dir("desktop-preview-cwd");
    let preview = OpenHumanLaunch::desktop(&tmp).dry_run_preview();
    assert!(preview.starts_with(&format!("cd {} && cargo tauri", tmp.display())));
    // Core preview has an absolute --manifest-path and needs no cd.
    let core = OpenHumanLaunch::core(&tmp).dry_run_preview();
    assert!(!core.starts_with("cd "));
}

#[test]
fn ensure_env_copies_example_when_env_absent() {
    let tmp = tempfile_dir("ensure-env-copy");
    fs::write(tmp.join(".env.example"), b"PORT=4242\n").unwrap();
    OpenHumanLaunch::desktop(&tmp).ensure_env().unwrap();
    assert_eq!(fs::read_to_string(tmp.join(".env")).unwrap(), "PORT=4242\n");
}

#[test]
fn ensure_env_preserves_existing_env() {
    let tmp = tempfile_dir("ensure-env-existing");
    fs::write(tmp.join(".env"), b"PORT=9999\n").unwrap();
    fs::write(tmp.join(".env.example"), b"PORT=4242\n").unwrap();
    OpenHumanLaunch::desktop(&tmp).ensure_env().unwrap();
    assert_eq!(fs::read_to_string(tmp.join(".env")).unwrap(), "PORT=9999\n");
}

#[tokio::test]
async fn ensure_tauri_cli_errors_when_vendored_cli_missing() {
    // A checkout without the vendored `tauri-cef` (e.g. submodules not
    // initialized) must fail fast with the guidance message before any
    // `cargo install` is spawned.
    let tmp = tempfile_dir("ensure-cli-missing");
    let err = OpenHumanLaunch::desktop(&tmp)
        .ensure_tauri_cli()
        .await
        .unwrap_err();
    let OpenCompanyError::OpenHuman { code, message } = &err else {
        panic!("expected OpenHuman error, got {err:?}");
    };
    assert_eq!(*code, 500);
    assert!(
        message.contains("vendored CEF-aware tauri-cli not found"),
        "error should name the missing vendored cli: {message}"
    );
}

#[test]
fn ensure_env_is_noop_without_example() {
    let tmp = tempfile_dir("ensure-env-no-example");
    OpenHumanLaunch::desktop(&tmp).ensure_env().unwrap();
    assert!(!tmp.join(".env").exists());
}

#[test]
fn desktop_path_env_prefers_install_bin_and_gates_cargo_bin_on_home() {
    // Build the inputs and expectations from PathBuf/join_paths so the
    // test passes on Windows too, where the PATH separator is `;`.
    let install_root = PathBuf::from("root").join(".cache").join("cargo-install");
    let first_inherited = PathBuf::from("maybe").join("one");
    let second_inherited = PathBuf::from("perhaps").join("two");
    let inherited = std::env::join_paths([&first_inherited, &second_inherited])
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // HOME set: install bin first, then ~/.cargo/bin, then the inherited
    // PATH. Compare segment-by-segment, not against a `:`-joined string.
    let dev_home = PathBuf::from("home").join("dev");
    let with_home = desktop_path_env(&install_root, Some(&dev_home.to_string_lossy()), &inherited);
    let segments: Vec<_> = std::env::split_paths(&with_home).collect();
    assert_eq!(
        segments,
        vec![
            install_root.join("bin"),
            dev_home.join(".cargo").join("bin"),
            first_inherited.clone(),
            second_inherited.clone(),
        ]
    );
    // HOME unset: install bin is still prepended; no ~/.cargo/bin segment.
    let without_home = desktop_path_env(&install_root, None, &inherited);
    let segments: Vec<_> = std::env::split_paths(&without_home).collect();
    assert_eq!(
        segments,
        vec![install_root.join("bin"), first_inherited, second_inherited,]
    );
}

#[test]
fn apply_desktop_env_sets_path_dotenv_and_signing_identity() {
    let tmp = tempfile_dir("desktop-env");
    // `.env` is applied after CEF_PATH, so it overrides preflight vars.
    fs::write(tmp.join(".env"), b"CEF_PATH=/from-dotenv/orig\nFOO=bar\n").unwrap();

    let launch = OpenHumanLaunch::desktop(&tmp);
    let mut command = Command::new("true");
    launch.apply_desktop_env(&mut command).unwrap();

    let envs: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> = command
        .as_std()
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned())))
        .collect();
    let get = |key: &str| {
        envs.iter()
            .find(|(k, _)| k.as_os_str() == std::ffi::OsStr::new(key))
            .and_then(|(_, v)| v.as_ref())
            .map(|v| v.to_string_lossy().into_owned())
    };

    // PATH comes from `desktop_path_env` with the vendored install-root
    // bin first, the inferred ~/.cargo/bin when HOME is set, then the
    // inherited PATH. Compare via the helper on the platform separator so
    // the assertions hold on Windows too.
    let path = get("PATH").unwrap();
    let install_root = launch.tauri_install_root();
    let inherited = std::env::var("PATH").unwrap_or_default();
    let expected = desktop_path_env(
        &install_root,
        std::env::var("HOME").ok().as_deref(),
        &inherited,
    );
    assert_eq!(path, expected);
    let mut segments = std::env::split_paths(&expected);
    assert_eq!(
        segments.next(),
        Some(install_root.join("bin")),
        "PATH should prefer the vendored cargo-tauri bin: {path}"
    );

    // .env vars landed, and the CEF_PATH it names overrode any preflight value.
    assert_eq!(get("FOO").as_deref(), Some("bar"));
    assert_eq!(get("CEF_PATH").as_deref(), Some("/from-dotenv/orig"));

    // macOS dev sets the signing identity; everywhere else it stays absent.
    assert_eq!(
        get("APPLE_SIGNING_IDENTITY").as_deref(),
        if cfg!(target_os = "macos") && !launch.release {
            Some("OpenHuman Dev Signer")
        } else {
            None
        }
    );
}

fn tempfile_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "opencompany-launcher-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}
