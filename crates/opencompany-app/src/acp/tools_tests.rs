use super::*;
use crate::acp::discovery::HARNESSES;

fn claude() -> &'static Harness {
    HARNESSES.iter().find(|h| h.id == "claude").unwrap()
}

#[test]
fn the_tools_directory_sits_under_this_app_and_not_the_users_npm_prefix() {
    let dir = tools_dir();
    assert!(dir.ends_with("acp-tools"));
    assert!(
        dir.starts_with(crate::default_data_dir()),
        "adapters this app installed must be removable with this app"
    );
}

#[test]
fn nothing_is_reported_installed_in_an_empty_root() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(installed_adapter_in(root.path(), claude()), None);
    assert_eq!(installed_version_in(root.path(), claude()), None);
    assert!(!is_pinned_version_in(root.path(), claude()));
}

/// The layout is read the way `npm --prefix` really writes it, confirmed
/// against the registry rather than assumed: the executable is linked into
/// `node_modules/.bin`, and the version lives in the package's own
/// manifest.
#[test]
fn an_installed_adapter_is_found_by_its_binary_and_dated_by_its_manifest() {
    let root = tempfile::tempdir().unwrap();
    let harness = claude();

    let bin = root.path().join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join(harness.command), "#!/usr/bin/env node\n").unwrap();

    let pkg = root.path().join("node_modules").join(harness.package);
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("package.json"),
        format!(
            r#"{{"name":"{}","version":"{}"}}"#,
            harness.package, harness.version
        ),
    )
    .unwrap();

    assert_eq!(
        installed_adapter_in(root.path(), harness),
        Some(bin.join(harness.command))
    );
    assert_eq!(
        installed_version_in(root.path(), harness).as_deref(),
        Some(harness.version)
    );
    assert!(is_pinned_version_in(root.path(), harness));
}

/// An older install is a different state from no install: one needs an
/// update, the other needs an install, and the operator is told which.
#[test]
fn an_install_behind_the_pin_is_not_mistaken_for_the_pinned_one() {
    let root = tempfile::tempdir().unwrap();
    let harness = claude();
    let pkg = root.path().join("node_modules").join(harness.package);
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("package.json"), r#"{"version":"0.0.1-ancient"}"#).unwrap();

    assert_eq!(
        installed_version_in(root.path(), harness).as_deref(),
        Some("0.0.1-ancient")
    );
    assert!(!is_pinned_version_in(root.path(), harness));
}

/// A manifest that is present but unreadable must not read as a version.
/// Answering `Some(garbage)` would compare unequal to the pin and put the
/// row into a permanent "update available" that updating cannot clear.
#[test]
fn an_unparseable_manifest_reads_as_no_version_at_all() {
    let root = tempfile::tempdir().unwrap();
    let harness = claude();
    let pkg = root.path().join("node_modules").join(harness.package);
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("package.json"), "{ truncated").unwrap();
    assert_eq!(installed_version_in(root.path(), harness), None);
}
