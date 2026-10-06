//! The host's shared skill library: what an operator browses and installs by
//! slug, and what a degenerate registry install is healed from.
//!
//! The library is a directory of company bundles read from disk — a checkout's
//! `companies/`, the copy the desktop bundle ships beside the app, or one a
//! container is pointed at — and never compiled into the binary.
//! [`for_host`] picks which one a host serves.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::company::{SkillDoc, load_catalog_skills};
use crate::error::{OpenCompanyError, Result};

/// The environment variable naming a host's library directory.
pub const SKILL_LIBRARY_ENV: &str = "OPENCOMPANY_SKILL_LIBRARY";

/// Where a host's library was resolved from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryOrigin {
    /// Named by the host itself, e.g. `serve`'s `--company` checkout.
    Explicit(PathBuf),
    /// Named by [`SKILL_LIBRARY_ENV`].
    Env(PathBuf),
    /// The copy packaged beside the application.
    Packaged(PathBuf),
    /// The host serves no library.
    None,
}

/// A host's shared skill library.
pub trait SkillLibrary: Send + Sync + std::fmt::Debug {
    /// Every document the library serves, one per slug, ordered by slug.
    ///
    /// Empty only when the host serves no library. A configured library that
    /// cannot load is an error, never an empty answer: an empty library is
    /// what lets an install fall back to client-authored metadata.
    ///
    /// # Errors
    ///
    /// [`OpenCompanyError::Config`] when a configured directory is missing or
    /// any of its documents fails to parse.
    fn snapshot(&self) -> Result<Arc<[SkillDoc]>>;

    /// Where this library was resolved from.
    fn origin(&self) -> LibraryOrigin;
}

/// A library read from a directory of bundles, loaded once and cached.
#[derive(Debug)]
pub struct DirLibrary {
    dir: PathBuf,
    origin: LibraryOrigin,
    loaded: OnceLock<Arc<[SkillDoc]>>,
}

impl DirLibrary {
    fn at(dir: PathBuf, origin: LibraryOrigin) -> Self {
        Self {
            dir,
            origin,
            loaded: OnceLock::new(),
        }
    }

    /// A library over the `companies/` directory the host named itself.
    pub fn explicit(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self::at(dir.clone(), LibraryOrigin::Explicit(dir))
    }

    /// A library over the directory [`SKILL_LIBRARY_ENV`] named.
    pub fn env(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self::at(dir.clone(), LibraryOrigin::Env(dir))
    }

    /// A library over the copy packaged beside the application.
    pub fn packaged(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self::at(dir.clone(), LibraryOrigin::Packaged(dir))
    }
}

impl SkillLibrary for DirLibrary {
    fn snapshot(&self) -> Result<Arc<[SkillDoc]>> {
        if let Some(cached) = self.loaded.get() {
            return Ok(cached.clone());
        }
        let dir = self.dir.as_path();
        // `load_catalog_skills` answers `Ok(empty)` for a missing directory,
        // which would quietly turn a configured library into "no library".
        if !dir.is_dir() {
            return Err(OpenCompanyError::Config(format!(
                "shared skill library at {} is not a directory",
                dir.display()
            )));
        }
        // A parse failure surfaces as `DataParse`/`DataInvalid`, which the HTTP
        // layer maps to the caller's bad input. The library is host
        // configuration, so it is recast as `Config`.
        let docs: Arc<[SkillDoc]> = load_catalog_skills(dir)
            .map_err(|error| {
                OpenCompanyError::Config(format!(
                    "shared skill library at {} failed to load: {error}",
                    dir.display()
                ))
            })?
            .into();
        tracing::debug!(
            "[skills] loaded {} library skills from {}",
            docs.len(),
            dir.display()
        );
        let _ = self.loaded.set(docs.clone());
        Ok(self.loaded.get().cloned().unwrap_or(docs))
    }

    fn origin(&self) -> LibraryOrigin {
        self.origin.clone()
    }
}

/// The library of a host that serves none.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoLibrary;

impl SkillLibrary for NoLibrary {
    fn snapshot(&self) -> Result<Arc<[SkillDoc]>> {
        Ok(Arc::from([]))
    }

    fn origin(&self) -> LibraryOrigin {
        LibraryOrigin::None
    }
}

/// The library a host serves, in order: `explicit`, then `env` (the value of
/// [`SKILL_LIBRARY_ENV`]), then `packaged`, then none.
///
/// A blank `env` counts as unset. `explicit` and `env` are configuration and
/// are served as named, so a wrong path fails at boot. `packaged` is where the
/// application expects its own copy, and is served only when that directory
/// exists: a build that carries no copy serves no library rather than failing
/// to start.
pub fn for_host(
    explicit: Option<PathBuf>,
    env: Option<OsString>,
    packaged: Option<PathBuf>,
) -> Arc<dyn SkillLibrary> {
    let library = if let Some(dir) = explicit {
        DirLibrary::explicit(dir)
    } else if let Some(dir) = env.filter(|value| !value.to_string_lossy().trim().is_empty()) {
        DirLibrary::env(dir)
    } else if let Some(dir) = packaged.filter(|dir| dir.is_dir()) {
        DirLibrary::packaged(dir)
    } else {
        tracing::debug!("[skills] this host serves no shared skill library");
        return Arc::new(NoLibrary);
    };
    tracing::debug!(
        "[skills] shared skill library resolved from {:?}",
        library.origin
    );
    Arc::new(library)
}

/// [`for_host`] with `env` read from the process environment.
pub fn for_host_from_env(
    explicit: Option<PathBuf>,
    packaged: Option<PathBuf>,
) -> Arc<dyn SkillLibrary> {
    for_host(explicit, std::env::var_os(SKILL_LIBRARY_ENV), packaged)
}

#[cfg(test)]
#[path = "skill_library_tests.rs"]
mod tests;
