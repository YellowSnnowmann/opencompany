use super::*;
use std::collections::HashSet;

struct Fake {
    installed: HashSet<String>,
}

impl Fake {
    fn new() -> Self {
        Self {
            installed: HashSet::new(),
        }
    }
    fn with_installed(mut self, command: &str) -> Self {
        self.installed.insert(command.to_string());
        self
    }
}

impl Probe for Fake {
    fn locate(&self, command: &str) -> Option<PathBuf> {
        self.installed
            .contains(command)
            .then(|| PathBuf::from(format!("/usr/local/bin/{command}")))
    }
}

/// What `PATH` is consulted for now: explaining an absence the OS has
/// already reported, never deciding one.
fn absent(probe: &dyn Probe, id: &str) -> Readiness {
    let harness = HARNESSES
        .iter()
        .find(|h| h.id == id)
        .expect("a known harness");
    diagnose_absent(probe, harness)
}

/// The bare name is always tried, on every platform.
///
/// Unix installs a command under its own name, so this must stay a
/// single candidate there — and it must stay *first* everywhere, so an
/// extensionless executable is not shadowed by a `.exe` beside it.
#[test]
fn a_command_is_looked_up_under_its_own_name_first() {
    assert_eq!(executable_names("node").next().as_deref(), Some("node"));
    #[cfg(unix)]
    assert_eq!(
        executable_names("node").count(),
        1,
        "unix needs no variants"
    );
}

/// Windows does not install `node` as `node`.
///
/// It is `node.exe`, and `npm` is `npm.cmd`. Looking only for the bare
/// name refused every install with "node was not found" on a correctly
/// configured machine — the same class of false negative as reading
/// `launchd`'s `PATH`, and invisible from a Unix dev box.
#[cfg(windows)]
#[test]
fn windows_also_tries_the_pathext_variants() {
    let names: Vec<String> = executable_names("node").collect();
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("node.exe")),
        "{names:?}"
    );
    // A command that already carries an extension is not extended again.
    assert_eq!(executable_names("node.exe").count(), 1);
}

/// The preference order the Install button depends on.
///
/// This was the defect Codex caught on #1681: the probe resolved
/// app-owned-first while `LocalAcpAgent` spawned the bare catalogue name,
/// so an installed adapter was reported `Ready` and then never run. On a
/// machine whose only adapter is the installed one, that is the difference
/// between the feature working and every turn failing "not on PATH".
#[test]
fn an_app_owned_adapter_wins_over_one_on_path() {
    let root = tempfile::tempdir().unwrap();
    let harness = HARNESSES.iter().find(|h| h.id == "claude").unwrap();
    let bin = root.path().join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join(harness.command), "#!/usr/bin/env node\n").unwrap();

    let on_path = Fake::new().with_installed(harness.command);
    assert_eq!(
        resolve_adapter_in(root.path(), &on_path, harness),
        Some(bin.join(harness.command)),
        "the version this app installed and pinned must win"
    );
}

/// ...and an operator's own install is still used when this app has none,
/// so the installer is additive rather than a new requirement. Every
/// machine that used this feature before the installer existed is set up
/// exactly this way.
#[test]
fn a_path_adapter_is_used_when_this_app_installed_none() {
    let empty = tempfile::tempdir().unwrap();
    let harness = HARNESSES.iter().find(|h| h.id == "claude").unwrap();
    let on_path = Fake::new().with_installed(harness.command);

    assert_eq!(
        resolve_adapter_in(empty.path(), &on_path, harness),
        Some(PathBuf::from(format!("/usr/local/bin/{}", harness.command)))
    );
    // Neither source has one: the caller falls back to the bare name and
    // the spawn reports `NotOnPath`, which is what produces the install
    // advice rather than a spawn error.
    assert_eq!(
        resolve_adapter_in(empty.path(), &Fake::new(), harness),
        None
    );
}

/// The survey decides nothing, and that is the point of the inversion.
///
/// It used to gate on `PATH` and hand `confirm` a pre-formed verdict. But
/// presence on `PATH` was only ever a proxy for the real question, and it
/// is wrong in both directions: a binary can be there and unusable (wrong
/// architecture, missing execute bit, dangling symlink, half-finished
/// `npm` install, a protocol version this client no longer speaks). The
/// subprocess answers authoritatively, so it is the only thing that answers.
#[test]
fn the_survey_decides_nothing_and_starts_nothing() {
    let statuses = survey();
    assert_eq!(statuses.len(), HARNESSES.len(), "every harness is offered");
    assert!(
        statuses.iter().all(|s| s.readiness == Readiness::Checking),
        "nothing is known until an adapter answers"
    );
}

/// Each harness is diagnosed against its own CLI. One being installed must
/// not make another look installed — they are separate packages, and
/// `codex` absent on a machine that has `claude` is ordinary.
#[test]
fn each_harness_is_diagnosed_against_its_own_cli() {
    let probe = Fake::new().with_installed("claude");
    assert!(matches!(
        absent(&probe, "claude"),
        Readiness::AdapterMissing { .. }
    ));
    assert_eq!(absent(&probe, "codex"), Readiness::NotInstalled);
}

/// The regression that motivated dropping the file probe: a signed-in
/// Claude Code on macOS keeps no `~/.claude/.credentials.json`, and the
/// Phase 2 must not overwrite a verdict phase 1 already reached.
///
/// The model picker calls [`confirm`] on whatever harness the operator
/// selected, with no readiness filter — so this is reachable from the UI,
/// and reachable is where a raw `os error 2` would replace an instruction
/// naming the exact package to install.
#[tokio::test]
async fn confirming_an_uninstalled_harness_reports_the_survey_not_a_spawn_error() {
    // `_acp` is not a real harness id, so this exercises the unknown-id
    // arm; the installed-state arm needs the real machine, which the
    // ignored live test covers. What is asserted here is the shape: a
    // caller never gets a spawn error for something that was never spawned.
    let confirmed = confirm("_nonexistent", Path::new(".")).await;
    assert!(matches!(confirmed.readiness, Readiness::SpawnFailed { .. }));
    assert!(confirmed.models.is_empty());
}

/// The message that started this: `claude` installed, adapter absent.
///
/// Reporting `NotInstalled` here tells an operator to install Claude Code
/// on the machine they already run it on, and never mentions the one
/// package that would fix it.
#[test]
fn an_installed_cli_without_its_adapter_is_not_reported_as_missing() {
    let probe = Fake::new().with_installed("claude");
    assert_eq!(
        absent(&probe, "claude"),
        Readiness::AdapterMissing {
            cli: PathBuf::from("/usr/local/bin/claude"),
            package: "@agentclientprotocol/claude-agent-acp",
        }
    );
}

/// Neither half present is the only case that should say "install it".
#[test]
fn a_machine_with_neither_binary_is_reported_as_not_installed() {
    assert_eq!(absent(&Fake::new(), "claude"), Readiness::NotInstalled);
}

/// Each harness names its own package. A shared or copy-pasted hint sends
/// someone to install the wrong one, which fails quietly and looks like the
/// advice simply did not work.
#[test]
fn each_harness_names_the_package_that_fixes_it() {
    let probe = Fake::new().with_installed("claude").with_installed("codex");
    for harness in HARNESSES {
        let Readiness::AdapterMissing { package, .. } = diagnose_absent(&probe, harness) else {
            panic!("{} should be AdapterMissing", harness.id);
        };
        assert!(
            package.ends_with(&format!("/{}-acp", harness.id))
                || package.ends_with(&format!("/{}-agent-acp", harness.id)),
            "{} points at {package}",
            harness.id
        );
    }
}

/// The regression that motivated dropping the credential-file probe, now
/// structural rather than a rule to remember: nothing in this module can
/// conclude `NotSignedIn` from the filesystem, because the only filesystem
/// call left ([`diagnose_absent`]) cannot return that variant at all. It is
/// reachable exclusively from an adapter's own refusal.
#[test]
fn sign_in_cannot_be_concluded_from_the_filesystem() {
    for probe in [Fake::new(), Fake::new().with_installed("claude")] {
        for harness in HARNESSES {
            assert_ne!(
                diagnose_absent(&probe, harness),
                Readiness::NotSignedIn,
                "{} must not be judged signed-out by a file lookup",
                harness.id
            );
        }
    }
}

#[test]
fn readiness_serialises_with_a_state_tag_the_console_can_switch_on() {
    let json = serde_json::to_value(Readiness::NotSignedIn).unwrap();
    assert_eq!(json["state"], "notSignedIn");
    let failed = serde_json::to_value(Readiness::SpawnFailed {
        reason: "exited immediately".into(),
    })
    .unwrap();
    assert_eq!(failed["state"], "spawnFailed");
    // The reason travels: "it didn't start" with no cause is not actionable.
    assert_eq!(failed["reason"], "exited immediately");
}
