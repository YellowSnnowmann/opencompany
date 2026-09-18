use std::{fs, path::PathBuf, process::ExitStatus, time::SystemTime};

use serde::Serialize;
use tokio::process::Command;

use crate::{OpenCompanyError, Result};

/// OpenHuman target to launch from a sibling checkout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum LaunchMode {
    /// Launch the core Rust binary (`openhuman-core`).
    Core,
    /// Launch the Tauri desktop host, driving `cargo tauri` directly.
    Desktop,
}

/// Desktop windowing backend. CEF is OpenHuman's primary surface on macOS;
/// `wry` is the cross-platform fallback that runs on Linux/Windows.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum DesktopBackend {
    /// Chromium Embedded Framework — the `dev:app`/`macos:build:*` scripts.
    /// macOS-only: they assume the Keychain, `APPLE_SIGNING_IDENTITY`, and the
    /// vendored CEF-aware `tauri-cli`.
    Cef,
    /// Native system webview (WebKitGTK on Linux, WebView2 on Windows) — the
    /// `dev:wry`/`tauri:build:ui` scripts, selected on every non-macOS host.
    Wry,
}

impl DesktopBackend {
    /// The backend OpenHuman's own scripts expect on this host: CEF on macOS,
    /// `wry` everywhere else.
    pub fn for_host() -> Self {
        if cfg!(target_os = "macos") {
            Self::Cef
        } else {
            Self::Wry
        }
    }
}

/// Describes an OpenHuman launch request.
#[derive(Clone, Debug)]
pub struct OpenHumanLaunch {
    root: PathBuf,
    mode: LaunchMode,
    release: bool,
    args: Vec<String>,
}

impl OpenHumanLaunch {
    /// Creates a launch request for the OpenHuman core binary.
    pub fn core(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            mode: LaunchMode::Core,
            release: false,
            args: Vec::new(),
        }
    }

    /// Creates a launch request for the OpenHuman desktop host.
    pub fn desktop(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            mode: LaunchMode::Desktop,
            release: false,
            args: Vec::new(),
        }
    }

    /// Switch from a dev launch to a release build (a bundled `.app`/dmg on
    /// macOS, a deb/AppImage elsewhere). For Core this adds `--release` to the
    /// `cargo run`; for Desktop it selects OpenHuman's `*build*` invocation.
    pub fn release(mut self) -> Self {
        self.release = true;
        self
    }

    /// Adds passthrough arguments forwarded after `--` to the OpenHuman core
    /// binary. Desktop mode ignores these — it drives a fixed `cargo tauri`
    /// invocation, which does not accept passthrough args.
    pub fn with_args(mut self, args: impl IntoIterator<Item = String>) -> Self {
        self.args = args.into_iter().collect();
        self
    }

    /// The `cargo tauri` subcommand arguments Desktop mode drives, picked from
    /// the host backend and whether this is a dev or release build. Copied
    /// verbatim from OpenHuman's own `dev:app`/`dev:wry`/`macos:build:release`/
    /// `tauri:build:ui` pnpm scripts so the host matches what OpenHuman expects.
    fn desktop_tauri_args(&self) -> Vec<String> {
        // OPENHUMAN_DEV_PORT becomes a numeric devUrl in the --config JSON; a
        // non-numeric value would emit usable-but-wrong JSON, and quotes or
        // backslashes would make it malformed. Parse as u16 and fall back to
        // Tauri/Vite's default 1420.
        let port = desktop_dev_port(std::env::var("OPENHUMAN_DEV_PORT").ok().as_deref());
        self.desktop_tauri_args_for(DesktopBackend::for_host(), port)
    }

    /// The backend-specific `cargo tauri` arguments. Split from
    /// [`desktop_tauri_args`](Self::desktop_tauri_args) so both the CEF and Wry
    /// branches are directly testable from any host rather than only via
    /// [`DesktopBackend::for_host`].
    fn desktop_tauri_args_for(&self, backend: DesktopBackend, port: u16) -> Vec<String> {
        match (self.release, backend) {
            (false, DesktopBackend::Cef) => {
                let config = format!("{{\"build\":{{\"devUrl\":\"http://localhost:{port}\"}}}}");
                vec!["dev".into(), "--config".into(), config]
            }
            (false, DesktopBackend::Wry) => {
                vec![
                    "dev".into(),
                    "--no-default-features".into(),
                    "--features".into(),
                    "wry".into(),
                ]
            }
            (true, DesktopBackend::Cef) => vec![
                "build".into(),
                "--bundles".into(),
                "app".into(),
                "dmg".into(),
                "--".into(),
                "--bin".into(),
                "OpenHuman".into(),
            ],
            (true, DesktopBackend::Wry) => {
                vec![
                    "build".into(),
                    "--".into(),
                    "--bin".into(),
                    "OpenHuman".into(),
                ]
            }
        }
    }

    /// Returns the command OpenCompany will spawn, without spawning it.
    ///
    /// Desktop mode calls `cargo tauri dev`/`build` directly. The preflight
    /// (installing the vendored CEF-aware `cargo-tauri`, pinning `CEF_PATH`,
    /// loading `<root>/.env`, and on macOS seeding the Chromium keychain +
    /// the signing identity) is performed by [`run`](Self::run) before this
    /// command is spawned — `cargo run --bin OpenHuman` alone opens a blank
    /// window or panics inside `cef::library_loader`.
    pub fn command_preview(&self) -> Vec<String> {
        match self.mode {
            LaunchMode::Core => {
                let mut command = vec!["cargo".to_string(), "run".to_string()];
                if self.release {
                    command.push("--release".to_string());
                }
                command.extend([
                    "--manifest-path".to_string(),
                    self.root.join("Cargo.toml").display().to_string(),
                    "--bin".to_string(),
                    "openhuman-core".to_string(),
                    "--".to_string(),
                ]);
                command.extend(self.args.clone());
                command
            }
            LaunchMode::Desktop => {
                let mut command = vec!["cargo".to_string(), "tauri".to_string()];
                command.extend(self.desktop_tauri_args());
                command
            }
        }
    }

    /// The single line a `--dry-run` prints: the working directory followed by
    /// the command. Desktop runs from the checkout root (like [`run`](Self::run)
    /// passes `current_dir`), so the preview is copy-paste executable from any
    /// cwd; Core carries an absolute `--manifest-path`, so it needs no `cd`.
    pub fn dry_run_preview(&self) -> String {
        match self.mode {
            LaunchMode::Desktop => format!(
                "cd {} && {}",
                self.root.display(),
                self.command_preview().join(" ")
            ),
            _ => self.command_preview().join(" "),
        }
    }

    /// Rejects passthrough args in Desktop mode, which drives a fixed
    /// `cargo tauri` invocation that does not forward them. Called by
    /// [`run`](Self::run) and by the CLI's dry-run path so execution and
    /// `--dry-run` agree on what is launchable.
    pub fn validate(&self) -> Result<()> {
        if matches!(self.mode, LaunchMode::Desktop) && !self.args.is_empty() {
            return Err(OpenCompanyError::OpenHuman {
                code: 400,
                message: "desktop mode drives a fixed `cargo tauri` invocation and does not \
                     accept passthrough args; use --mode core for binary args"
                    .to_string(),
            });
        }
        Ok(())
    }

    /// Starts OpenHuman and waits for it to exit.
    pub async fn run(self) -> Result<ExitStatus> {
        if !self.root.exists() {
            return Err(OpenCompanyError::MissingOpenHumanRoot(self.root));
        }

        self.validate()?;

        match self.mode {
            LaunchMode::Core => {
                let preview = self.command_preview();
                let status = Command::new(&preview[0])
                    .args(&preview[1..])
                    .status()
                    .await?;
                Ok(status)
            }
            LaunchMode::Desktop => {
                // `.env` is loaded by the preflight below; seed it from the
                // vendored example so a fresh checkout launches out-of-the-box.
                self.ensure_env()?;

                // Install the vendored CEF-aware cargo-tauri if missing or stale.
                // Stock tauri-cli cannot bundle the CEF runtime, so this is the
                // step that makes `cargo tauri build` produce a working `.app`.
                self.ensure_tauri_cli().await?;

                // macOS CEF dev also pre-seeds the Chromium Safe Storage keychain
                // entry so CEF reads it without a prompt (no-op off macOS).
                if !self.release && DesktopBackend::for_host() == DesktopBackend::Cef {
                    setup_chromium_safe_storage();
                }

                let preview = self.command_preview();
                let mut command = Command::new(&preview[0]);
                command.args(&preview[1..]).current_dir(&self.root);
                self.apply_desktop_env(&mut command)?;
                let status = command.status().await?;
                Ok(status)
            }
        }
    }

    /// Copy `<root>/.env.example` to `<root>/.env` when the latter is missing,
    /// so the preflight env loader does not skip. No-op once `.env` exists or
    /// when no example is present.
    fn ensure_env(&self) -> Result<()> {
        let env = self.root.join(".env");
        if env.exists() {
            return Ok(());
        }
        let example = self.root.join(".env.example");
        if !example.exists() {
            return Ok(());
        }
        fs::copy(&example, &env).map_err(OpenCompanyError::from)?;
        Ok(())
    }

    /// Where the vendored CEF-aware `cargo-tauri` is installed. Mirrors
    /// `ensure-tauri-cli.sh`: `OPENHUMAN_CARGO_INSTALL_ROOT` or
    /// `<root>/.cache/cargo-install`.
    fn tauri_install_root(&self) -> PathBuf {
        std::env::var("OPENHUMAN_CARGO_INSTALL_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| self.root.join(".cache").join("cargo-install"))
    }

    /// The CEF binary distribution location. Mirrors `ensure-tauri-cli.sh`:
    /// Wry hosts (Linux/Windows) have **no** CEF distribution, so `None` —
    /// even when a `CEF_PATH` is set in the caller's environment, which must
    /// not leak into a non-CEF run — and no macOS-shaped cache directory or
    /// `$HOME` requirement is created there. On the CEF backend the `CEF_PATH`
    /// override wins, otherwise the default `$HOME/Library/Caches/tauri-cef`.
    fn cef_path() -> Option<PathBuf> {
        cef_path_for(
            DesktopBackend::for_host(),
            std::env::var("CEF_PATH").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        )
    }

    /// Install the vendored CEF-aware `tauri-cli` as `cargo-tauri` under
    /// [`tauri_install_root`](Self::tauri_install_root) when it is missing or
    /// stale. A port of `vendor/openhuman/scripts/ensure-tauri-cli.sh`.
    async fn ensure_tauri_cli(&self) -> Result<()> {
        let vendored_cef = self
            .root
            .join("app")
            .join("src-tauri")
            .join("vendor")
            .join("tauri-cef");
        let vendored_cli = vendored_cef.join("crates").join("tauri-cli");
        if !vendored_cli.join("Cargo.toml").exists() {
            return Err(OpenCompanyError::OpenHuman {
                code: 500,
                message: format!(
                    "vendored CEF-aware tauri-cli not found at {}; run \
                     `git submodule update --init --recursive` in the OpenHuman \
                     checkout",
                    vendored_cli.display()
                ),
            });
        }

        let install_root = self.tauri_install_root();
        let bin = install_root.join("bin").join("cargo-tauri");
        fs::create_dir_all(&install_root).map_err(OpenCompanyError::from)?;
        // Only the CEF backend keeps a dedicated distribution directory; Wry
        // hosts skip it so a launch never needs `HOME` or creates a fake
        // macOS cache tree on Linux/Windows.
        if let Some(cef) = Self::cef_path() {
            fs::create_dir_all(&cef).map_err(OpenCompanyError::from)?;
        }

        let crates_toml = install_root.join(".crates.toml");
        let from_vendored = fs::read_to_string(&crates_toml)
            .map(|content| installed_from_vendored(&content, &vendored_cli))
            .unwrap_or(false);
        let fresh = from_vendored && tauri_cli_is_fresh(&bin, &vendored_cef);
        if fresh {
            return Ok(());
        }

        eprintln!(
            "[ensure-tauri-cli] installing vendored CEF-aware tauri-cli from {}",
            vendored_cli.display()
        );
        eprintln!(
            "[ensure-tauri-cli] (first install only — takes a few minutes; \
             subsequent runs are instant)"
        );

        let mut install = Command::new("cargo");
        install.args([
            "install",
            "--root",
            &install_root.to_string_lossy(),
            "--locked",
            "--path",
            &vendored_cli.to_string_lossy(),
        ]);
        if let Some(cef) = Self::cef_path() {
            install.env("CEF_PATH", cef);
        }
        // Put the install root's bin first so the install itself resolves a
        // CEF-aware toolchain if it shells out; `~/.cargo/bin` is appended
        // only when HOME is set, since HOME is what locates that directory.
        let inherited = std::env::var("PATH").unwrap_or_default();
        let path = desktop_path_env(
            &install_root,
            std::env::var("HOME").ok().as_deref(),
            &inherited,
        );
        install.env("PATH", path);
        let status = install.status().await?;
        if !status.success() {
            return Err(OpenCompanyError::OpenHuman {
                code: 500,
                message: "cargo install of the vendored CEF-aware tauri-cli failed".into(),
            });
        }
        Ok(())
    }

    /// Apply the Desktop preflight environment to a `cargo tauri` command:
    /// `CEF_PATH`, a `PATH` that prefers the vendored `cargo-tauri`, the
    /// loaded `<root>/.env` vars, and (for macOS CEF dev) the signing identity.
    fn apply_desktop_env(&self, command: &mut Command) -> Result<()> {
        match Self::cef_path() {
            Some(cef) => command.env("CEF_PATH", &cef),
            // Wry backend: no CEF distribution to point at, and a `CEF_PATH`
            // inherited from the caller's environment must not leak into a
            // non-CEF run.
            None => command.env_remove("CEF_PATH"),
        };

        // Prefer the CEF-aware cargo-tauri over any stock install on PATH.
        // `<install_root>/bin` is always prepended — it is where
        // `ensure_tauri_cli` just installed the vendored binary, and cargo
        // resolves the `tauri` subcommand through the child's PATH. Only the
        // `~/.cargo/bin` segment depends on HOME, which is often unset on
        // Windows.
        let install_root = self.tauri_install_root();
        let inherited = std::env::var("PATH").unwrap_or_default();
        // Home may be unset on Windows; `~/.cargo/bin` is only appended then.
        let path = desktop_path_env(
            &install_root,
            std::env::var("HOME").ok().as_deref(),
            &inherited,
        );
        command.env("PATH", path);

        // Load <root>/.env (set after CEF_PATH so a .env override wins, matching
        // the scripts: `export CEF_PATH=...; source load-dotenv.sh`).
        let env_file = self.root.join(".env");
        if env_file.exists() {
            for (key, value) in load_dotenv(&env_file)? {
                command.env(key, value);
            }
        }

        // macOS CEF dev sets the signing identity inline after .env, so it wins.
        if !self.release && cfg!(target_os = "macos") {
            command.env("APPLE_SIGNING_IDENTITY", "OpenHuman Dev Signer");
        }
        Ok(())
    }
}

/// The CEF distribution path for a given backend: `None` for Wry hosts even
/// when `override_path` is set (a caller's `CEF_PATH` must not leak into a
/// non-CEF run); on CEF the `CEF_PATH` override wins, otherwise
/// `$HOME/Library/Caches/tauri-cef` is derived from `home`. Pure so the Wry
/// branch is testable from any host rather than only via
/// [`DesktopBackend::for_host`].
fn cef_path_for(
    backend: DesktopBackend,
    override_path: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    if backend != DesktopBackend::Cef {
        return None;
    }
    match override_path {
        Some(path) => Some(PathBuf::from(path)),
        None => home.map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Caches")
                .join("tauri-cef")
        }),
    }
}

/// The PATH for a Desktop launch or install: `<install_root>/bin` first (where
/// `ensure_tauri_cli` puts the vendored CEF-aware `cargo-tauri`), then
/// `~/.cargo/bin` when `home` is known — HOME is what locates that directory
/// and is often unset on Windows — then the inherited PATH.
fn desktop_path_env(install_root: &std::path::Path, home: Option<&str>, inherited: &str) -> String {
    let mut parts = vec![install_root.join("bin")];
    if let Some(home) = home {
        parts.push(PathBuf::from(home).join(".cargo").join("bin"));
    }
    parts.extend(std::env::split_paths(inherited));
    // join_paths uses the platform PATH separator (`:` on Unix, `;` on Windows)
    // and understands drive-letter colons, so the vendored bin stays resolvable
    // on every host. It only errors on a NUL or a bare separator inside a
    // component, which checkout-derived paths cannot contain.
    std::env::join_paths(parts)
        .expect("install root, home cargo dir, and inherited paths contain no separator")
        .to_string_lossy()
        .into_owned()
}

/// The `OPENHUMAN_DEV_PORT` value, parsed as a `u16` and falling back to
/// Tauri/Vite's default 1420 on an unset, non-numeric, out-of-range, or zero
/// value. Port zero is reserved and must not be baked into the `devUrl` JSON.
fn desktop_dev_port(raw: Option<&str>) -> u16 {
    raw.and_then(|p| p.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(1420)
}

/// Whether the installed `cargo-tauri` (per `.crates.toml`) came from the
/// vendored CEF-aware path. A port of the `grep -q "tauri-cli.*$VENDOR_CLI"`
/// check in `ensure-tauri-cli.sh`.
fn installed_from_vendored(crates_toml: &str, vendored_cli: &std::path::Path) -> bool {
    let path = vendored_cli.to_string_lossy();
    crates_toml
        .lines()
        .any(|line| line.contains("tauri-cli") && line.contains(path.as_ref()))
}

/// Whether the installed `cargo-tauri` binary is newer than every file under
/// the vendored `tauri-cef` tree. A port of `find … -newer` in
/// `ensure-tauri-cli.sh`; returns false (stale) when the binary is absent.
fn tauri_cli_is_fresh(bin: &std::path::Path, source_root: &std::path::Path) -> bool {
    let Ok(bin_meta) = fs::metadata(bin) else {
        return false;
    };
    let Ok(bin_mtime) = bin_meta.modified() else {
        return false;
    };
    !any_file_newer_than(source_root, bin_mtime)
}

/// True if any regular file under `dir` (recursively) was modified after
/// `threshold`. Early-exits on the first match.
fn any_file_newer_than(dir: &std::path::Path, threshold: SystemTime) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `symlink_metadata` (lstat) so a symlinked directory is neither
        // followed (a link to an ancestor would recurse without bound) nor its
        // target's mtimes attributed to the vendored tree.
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_file() {
            if let Ok(mtime) = meta.modified()
                && mtime > threshold
            {
                return true;
            }
        } else if meta.is_dir() && any_file_newer_than(&path, threshold) {
            return true;
        }
    }
    false
}

/// Parse a `.env` file into `(key, value)` pairs. A port of
/// `vendor/openhuman/scripts/load-dotenv.sh`: strip a line at the first `#`,
/// trim whitespace, drop a leading `export `, split on the first `=`, and
/// strip one surrounding pair of quotes from the value.
fn load_dotenv(path: &std::path::Path) -> Result<Vec<(String, String)>> {
    let content = fs::read_to_string(path).map_err(OpenCompanyError::from)?;
    Ok(content.lines().filter_map(parse_env_line).collect())
}

/// Parse one `.env` line into `(key, value)`, or `None` for blanks/comments.
fn parse_env_line(line: &str) -> Option<(String, String)> {
    // Strip from the first `#` (comment) — matches load-dotenv.sh.
    let line = line.split('#').next().unwrap_or("");
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // Drop a leading `export `.
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let mut value = value.trim().to_string();
    // Strip one surrounding pair of quotes (either kind), in the order the
    // script does: leading/trailing `"` then leading/trailing `'`.
    if value.starts_with('"') {
        value.remove(0);
    }
    if value.ends_with('"') {
        value.pop();
    }
    if value.starts_with('\'') {
        value.remove(0);
    }
    if value.ends_with('\'') {
        value.pop();
    }
    Some((key.to_string(), value))
}

/// Pre-seed the macOS "Chromium Safe Storage" keychain entry with a permissive
/// ACL so CEF/Chromium reads it without prompting. A port of
/// `vendor/openhuman/scripts/setup-chromium-safe-storage.sh`; no-op off macOS
/// and best-effort (never fatal) on macOS. Called only for the macOS CEF dev
/// path, as in `dev:app`.
#[cfg(target_os = "macos")]
fn setup_chromium_safe_storage() {
    use std::process::Command as StdCommand;
    let svc = "Chromium Safe Storage";
    let acct = "Chromium";
    let keychain = format!(
        "{}/Library/Keychains/login.keychain-db",
        std::env::var("HOME").unwrap_or_default()
    );
    let exists = StdCommand::new("security")
        .args(["find-generic-password", "-s", svc, "-a", acct, &keychain])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if exists {
        // Refresh the ACL only; leave the encryption key intact.
        let _ = StdCommand::new("security")
            .args([
                "set-generic-password-partition-list",
                "-S",
                "apple-tool:,apple:,unsigned:",
                "-s",
                svc,
                "-a",
                acct,
                "-k",
                "",
                &keychain,
            ])
            .status();
    } else {
        // Seed a random key with a permissive ACL.
        let key = StdCommand::new("openssl")
            .args(["rand", "-base64", "16"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        // Never store an empty Safe Storage key: if `openssl` is missing or
        // produced no usable output, seed nothing rather than a weak blank
        // password. Seeding is best-effort anyway, so a skipped key is fine.
        if !key.is_empty() {
            let _ = StdCommand::new("security")
                .args([
                    "add-generic-password",
                    "-s",
                    svc,
                    "-a",
                    acct,
                    "-w",
                    &key,
                    "-A",
                    &keychain,
                ])
                .status();
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn setup_chromium_safe_storage() {}

#[cfg(test)]
#[path = "launcher_tests.rs"]
mod tests;
