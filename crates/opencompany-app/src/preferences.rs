//! The user's own settings that outlive a launch: `<data_dir>/preferences.json`.
//!
//! Today that is one switch, `{"analytics": bool}`. A missing file, a missing
//! key and an unreadable file all mean "the user never chose", which resolves to
//! the default (analytics on). The unreadable case is logged, because a file the
//! user *did* write that we cannot read is a choice we are not honouring.
//!
//! Written atomically — temp file in the same directory, fsync, rename — the
//! way `local.rs` writes `instances.json`, so a crash mid-write leaves the old
//! choice rather than half a file.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file name under the shell's data root.
pub const PREFERENCES_FILE: &str = "preferences.json";

/// What the user has chosen. Every field is `None` until they choose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    /// `Some(false)` is an opt-out, `Some(true)` an explicit opt-in, `None`
    /// "never asked" (which is on, by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analytics: Option<bool>,
}

fn path_of(data_dir: &Path) -> PathBuf {
    data_dir.join(PREFERENCES_FILE)
}

impl Preferences {
    /// Reads the saved preferences; never fails.
    pub fn load(data_dir: &Path) -> Self {
        let path = path_of(data_dir);
        let body = match std::fs::read(&path) {
            Ok(body) => body,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "could not read preferences; using defaults");
                return Self::default();
            }
        };
        serde_json::from_slice(&body).unwrap_or_else(|error| {
            tracing::warn!(%error, path = %path.display(), "preferences are not valid; using defaults");
            Self::default()
        })
    }

    /// Whether analytics is on once the default is applied.
    pub fn analytics_enabled(&self) -> bool {
        self.analytics.unwrap_or(true)
    }

    /// Writes these preferences atomically.
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let path = path_of(data_dir);
        let write = std::fs::create_dir_all(data_dir).and_then(|()| {
            let body = serde_json::to_vec_pretty(self)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let mut temporary = tempfile::Builder::new()
                .prefix(".preferences.json.")
                .tempfile_in(data_dir)?;
            temporary.as_file_mut().write_all(&body)?;
            temporary.as_file().sync_all()?;
            temporary.persist(&path).map_err(|error| error.error)?;
            #[cfg(unix)]
            std::fs::File::open(data_dir)?.sync_all()?;
            Ok(())
        });
        write.map_err(|error| format!("could not write {}: {error}", path.display()))
    }
}

#[cfg(test)]
#[path = "preferences_tests.rs"]
mod tests;
