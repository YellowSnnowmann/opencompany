//! Shared fixtures for the content-validation test split: the repo-root
//! and bundle-directory helpers, and the roster loader that drops the
//! global baseline (split out of `content_tests.rs`).

use std::path::{Path, PathBuf};

use super::CompanyManifest;

pub(super) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every bundle directory under `dir`. An underscore-prefixed entry is not a
/// bundle: `companies/_globals` is the global baseline, which has no
/// `company.toml` and is covered by the `globals` tests instead.
pub(super) fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read {}: {err}", dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('_'))
        })
        .collect();
    dirs.sort();
    dirs
}

pub(super) fn toml_files(dir: &Path) -> Vec<PathBuf> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read {}: {err}", dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .collect();
    files.sort();
    files
}

pub(super) fn load_company(name: &str) -> CompanyManifest {
    let dir = repo_root().join("companies").join(name);
    let mut manifest =
        CompanyManifest::from_path(&dir).unwrap_or_else(|err| panic!("{}: {err}", dir.display()));
    // These tests are about what a bundle's author declared, so the global
    // baseline appended to every roster is dropped: a global teammate's tool
    // request is not this company's search posture, and holding a shipped
    // bundle responsible for one would make every company look like it granted
    // whatever the baseline asks for.
    manifest.agents.retain(|agent| !agent.global);
    manifest
}
