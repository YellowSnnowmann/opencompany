//! The hosts this machine runs, as a set rather than as a singleton.
//!
//! [`crate::embedded`] knows how to start *one* host over *one* data root, and
//! deliberately says nothing about which roots exist. This module is the layer
//! above: the roster of local instances an operator has asked for, where each
//! one keeps its data, and which of them are listening right now.
//!
//! ## Why a roster on disk
//!
//! An instance is only interesting because it survives a quit. The port does
//! not (it is ephemeral by design — see `embedded.rs`), and neither does the
//! process, so the durable thing about "the Acme instance" is its data root and
//! the name someone gave it. That has to be written down somewhere the next
//! launch reads, and `<data-dir>/instances.json` is it.
//!
//! ## Why the first instance is the data root itself
//!
//! Every install that predates this module keeps its company under
//! `<data-dir>` directly. Moving it under `<data-dir>/instances/default/`
//! would be a migration whose failure mode is "my company is gone", to buy
//! nothing but symmetry. So the default instance's root *is* the data dir, and
//! only instances created after it get a subdirectory.
//!
//! ## Failure is a state, not an error
//!
//! A root can be held by another process — a second window, or an
//! `opencompany serve` in a terminal. That instance simply does not start, and
//! the rest do; the console renders the reason on its row. Refusing to launch
//! because one of N roots is busy would make the multi-instance case *worse*
//! than the single one it replaces.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::embedded::{self, EmbeddedHost, FirstRun};

/// The id of the instance rooted at the data dir itself.
pub const DEFAULT_INSTANCE_ID: &str = "default";

/// What the console calls that instance before anyone renames it.
pub const DEFAULT_INSTANCE_LABEL: &str = "This computer";

/// The roster file, under the data dir.
const ROSTER_FILE: &str = "instances.json";

/// Where new instances put their data, under the data dir.
const INSTANCES_DIR: &str = "instances";

/// One instance's durable record. No port, no pid: neither survives a quit.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RosterEntry {
    id: String,
    label: String,
    /// The data root, relative to the data dir. `None` means the data dir
    /// itself, which is what the default instance has always used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<String>,
    /// Whether to start this instance on launch. An operator who stopped an
    /// instance means it to stay stopped across a relaunch — otherwise the
    /// stop button is undone by every quit.
    #[serde(default = "yes")]
    autostart: bool,
    /// Set only when this application provisions the root. A path shaped like
    /// one of ours is not proof that the application owns its contents.
    #[serde(default, skip_serializing_if = "is_false")]
    desktop_created: bool,
}

fn yes() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Roster {
    #[serde(default)]
    instances: Vec<RosterEntry>,
}

/// One instance as the console sees it: what it is, and where it got to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalInstanceInfo {
    pub id: String,
    pub label: String,
    pub data_dir: String,
    pub running: bool,
    /// Present exactly when `running`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The host's own durable identity, which is what the console keys its
    /// connection row on — the address changes every launch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    pub companies: Vec<String>,
    /// Why it is not running, in the operator's words. Most often the root
    /// being held by another process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct Instance {
    entry: RosterEntry,
    host: Option<EmbeddedHost>,
    error: Option<String>,
}

impl Instance {
    fn info(&self, data_dir: &Path) -> LocalInstanceInfo {
        LocalInstanceInfo {
            id: self.entry.id.clone(),
            label: self.entry.label.clone(),
            data_dir: root_of(data_dir, &self.entry).display().to_string(),
            running: self.host.is_some(),
            base_url: self.host.as_ref().map(EmbeddedHost::base_url),
            // Read off disk when nothing is running, because the console
            // prunes its remembered profiles against this. A stopped instance
            // that reported no identity would be indistinguishable from a data
            // root this application no longer serves, and the console would
            // forget its connection id — which is what every browser-local key
            // for that instance is scoped by.
            instance_id: match self.host.as_ref() {
                Some(host) => Some(host.instance_id().to_string()),
                None => opencompany::app::instance::peek(&root_of(data_dir, &self.entry)),
            },
            companies: self
                .host
                .as_ref()
                .map(|host| host.companies().to_vec())
                .unwrap_or_default(),
            error: self.error.clone(),
        }
    }
}

/// Resolves an entry's data root against the application's data dir.
fn root_of(data_dir: &Path, entry: &RosterEntry) -> PathBuf {
    match entry.root.as_deref() {
        None => data_dir.to_path_buf(),
        Some(relative) => data_dir.join(relative),
    }
}

/// Every local instance, and the ones currently listening.
pub struct LocalHosts {
    data_dir: PathBuf,
    instances: Vec<Instance>,
}

impl LocalHosts {
    /// Reads the roster and starts everything marked for autostart.
    ///
    /// Never fails as a whole: an unreadable roster falls back to the single
    /// default instance, and an instance that cannot start becomes a row
    /// carrying its reason. A desktop that refused to open because one data
    /// root was busy is the bug this shape exists to avoid.
    pub async fn load(data_dir: PathBuf) -> Self {
        let roster = read_roster(&data_dir);
        let mut hosts = Self {
            data_dir,
            instances: roster
                .instances
                .into_iter()
                .map(|entry| Instance {
                    entry,
                    host: None,
                    error: None,
                })
                .collect(),
        };
        for index in 0..hosts.instances.len() {
            if hosts.instances[index].entry.autostart {
                hosts.start_at(index).await;
            }
        }
        hosts
    }

    /// The roster, in listing order.
    pub fn list(&self) -> Vec<LocalInstanceInfo> {
        self.instances
            .iter()
            .map(|instance| instance.info(&self.data_dir))
            .collect()
    }

    /// The instance rooted at the data dir itself, when it is running.
    ///
    /// What `oc_embedded` answers with, so a console (or a shell) that predates
    /// the roster keeps seeing exactly what it saw before.
    pub fn default_instance(&self) -> Option<LocalInstanceInfo> {
        self.instances
            .iter()
            .find(|instance| instance.entry.id == DEFAULT_INSTANCE_ID)
            .filter(|instance| instance.host.is_some())
            .map(|instance| instance.info(&self.data_dir))
    }

    /// Adds an instance over a fresh data root and starts it.
    ///
    /// The root is derived from the id, never from the label: a label is free
    /// text an operator retypes, and a renamed instance must not become a
    /// second empty one.
    pub async fn create(&mut self, label: &str) -> Result<LocalInstanceInfo, String> {
        let label = label.trim();
        if label.is_empty() {
            return Err("an instance needs a name".to_string());
        }
        let id = self.mint_id(label);
        let entry = RosterEntry {
            root: Some(format!("{INSTANCES_DIR}/{id}")),
            id,
            label: label.to_string(),
            autostart: true,
            desktop_created: true,
        };
        self.instances.push(Instance {
            entry,
            host: None,
            error: None,
        });
        let index = self.instances.len() - 1;
        self.start_at(index).await;
        self.persist();
        let instance = &self.instances[index];
        match &instance.error {
            // A create that could not start is reported as an error rather than
            // as a stopped row: the operator asked for a running instance and
            // nothing on screen would otherwise say why they did not get one.
            Some(error) => Err(error.clone()),
            None => Ok(instance.info(&self.data_dir)),
        }
    }

    /// Starts a stopped instance, or reports why it will not start.
    pub async fn start(&mut self, id: &str) -> Result<LocalInstanceInfo, String> {
        let index = self.index_of(id)?;
        if self.instances[index].host.is_none() {
            self.start_at(index).await;
        }
        // Autostart follows the operator's last explicit choice, so a started
        // instance comes back on the next launch.
        self.instances[index].entry.autostart = true;
        self.persist();
        let instance = &self.instances[index];
        match &instance.error {
            Some(error) => Err(error.clone()),
            None => Ok(instance.info(&self.data_dir)),
        }
    }

    /// Stops an instance, freeing its port and its data root.
    ///
    /// The row stays: an instance is its data, and stopping is not forgetting.
    pub fn stop(&mut self, id: &str) -> Result<LocalInstanceInfo, String> {
        let index = self.index_of(id)?;
        // Dropping the host aborts its server task and releases the root lock.
        self.instances[index].host = None;
        self.instances[index].error = None;
        self.instances[index].entry.autostart = false;
        self.persist();
        Ok(self.instances[index].info(&self.data_dir))
    }

    /// Stops every listening host without recording that anyone stopped it.
    ///
    /// [`Self::stop`] is an operator's decision and writes it down — it clears
    /// `autostart`, so the next launch leaves that instance down. This is not
    /// that. It is for the moment the shell is about to be replaced on disk and
    /// relaunched: `restart` spawns the successor and *then* exits, so every
    /// data root has to be unlocked before the new process reaches for it, or
    /// the application comes back with each company reading "held by another
    /// process". The roster is left exactly as it was, so what comes back up is
    /// what was running.
    ///
    /// Answers with the ids it stopped, so a caller whose install then failed
    /// can put them back.
    pub fn quiesce(&mut self) -> Vec<String> {
        self.instances
            .iter_mut()
            .filter(|instance| instance.host.is_some())
            .map(|instance| {
                // Dropping the host aborts its server task and releases the
                // root lock — the same mechanism as `stop`, without the
                // roster edit.
                instance.host = None;
                instance.entry.id.clone()
            })
            .collect()
    }

    /// Removes an instance from the roster, leaving its data on disk.
    ///
    /// Deliberately not a delete. The roster is a list of things to run; the
    /// data root is someone's company. Removing the first is reversible by
    /// re-adding the root, and destroying the second is not reversible at all,
    /// so this does only the reversible half.
    ///
    /// The default instance cannot be removed: its root is the data dir, so a
    /// roster without it is a roster this application cannot rebuild.
    pub fn forget(&mut self, id: &str) -> Result<(), String> {
        if id == DEFAULT_INSTANCE_ID {
            return Err("the instance on this computer cannot be removed".to_string());
        }
        let index = self.index_of(id)?;
        self.instances.remove(index);
        self.persist();
        Ok(())
    }

    /// Permanently removes a desktop-created instance and its data root.
    ///
    /// The default instance is deliberately excluded: its root is the
    /// application's data directory, which also owns the roster and every
    /// desktop-created instance below it. Only roots minted under
    /// `instances/` are eligible for recursive deletion.
    pub async fn delete(&mut self, id: &str) -> Result<(), String> {
        if id == DEFAULT_INSTANCE_ID {
            return Err("the instance on this computer cannot be deleted".to_string());
        }
        let index = self.index_of(id)?;
        let Some(relative_root) = self.instances[index].entry.root.as_deref() else {
            return Err("only desktop-created instances can be deleted".to_string());
        };
        if relative_root != format!("{INSTANCES_DIR}/{id}") {
            return Err("only desktop-created instances can be deleted".to_string());
        }
        if !self.instances[index].entry.desktop_created {
            return Err("only desktop-created instances can be deleted".to_string());
        }
        let root = self.data_dir.join(relative_root);

        // Release the server and root lock, then durably disable autostart
        // before removing anything beneath it. If the final roster write
        // fails, the stopped row remains safe to retry: a relaunch cannot
        // recreate an empty root over data that was already deleted.
        self.instances[index].host = None;
        let was_autostart = self.instances[index].entry.autostart;
        self.instances[index].entry.autostart = false;
        if let Err(error) = self.try_persist() {
            self.instances[index].entry.autostart = was_autostart;
            return Err(error);
        }
        match tokio::fs::remove_dir_all(&root).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("could not delete {}: {error}", root.display()));
            }
        }
        let removed = self.instances.remove(index);
        if let Err(error) = self.try_persist() {
            self.instances.insert(index, removed);
            return Err(error);
        }
        Ok(())
    }

    /// Renames an instance. Its id, and therefore its data root, is untouched.
    pub fn rename(&mut self, id: &str, label: &str) -> Result<LocalInstanceInfo, String> {
        let label = label.trim();
        if label.is_empty() {
            return Err("an instance needs a name".to_string());
        }
        let index = self.index_of(id)?;
        self.instances[index].entry.label = label.to_string();
        self.persist();
        Ok(self.instances[index].info(&self.data_dir))
    }

    fn index_of(&self, id: &str) -> Result<usize, String> {
        self.instances
            .iter()
            .position(|instance| instance.entry.id == id)
            .ok_or_else(|| format!("no such instance: {id}"))
    }

    async fn start_at(&mut self, index: usize) {
        let entry = &self.instances[index].entry;
        let root = root_of(&self.data_dir, entry);
        // Every instance adopts what its root already holds and seeds nothing
        // — the one at the data root included, which used to be the exception.
        //
        // That exception was an ordering artifact rather than a decision. #632
        // seeded a starter company because a double-clicked application had no
        // other way in: there was no setup wizard yet, and an empty registry is
        // a login form addressing a company that does not exist. The wizard
        // arrived later and this arm was never revisited — so the one install
        // that most needs onboarding became the one install that could never
        // reach it. Seeding is not merely a head start: it makes `/spec` report
        // `setup_complete` (`stamp || !registry.is_empty()`), `ConnectionConsole`
        // enters its setup phase from that field and nothing else, and no
        // settings link re-opens the wizard. One silent answer, permanently.
        //
        // Note what does **not** change, because it is the whole risk of
        // touching this: an install that already has a company still boots
        // straight into it, with no wizard. Seeding was only ever the fallback
        // half of `desktop::bootstrap_companies`, reached when adoption came
        // back empty — so the arm dropped here could only ever fire on a root
        // holding no companies at all, which on the default instance means a
        // genuinely fresh install. Everything else is the adopt half, which
        // stays.
        //
        // #632's guarantee survives too, as a guarantee about *reachability*
        // rather than about seeding: the wizard is anonymous on loopback while
        // the registry is empty (`server::setup::authorize`), its model step is
        // skippable onto a curated roster, and finishing it seeds a template
        // through `desktop::seed_company`. Still enterable with no terminal, no
        // mail server and no credential — it asks first, which is the point.
        let first_run = FirstRun::RunSetupWizard;
        match embedded::start_with(root, first_run).await {
            Ok(host) => {
                tracing::info!(
                    id = %self.instances[index].entry.id,
                    address = %host.address(),
                    "local instance listening"
                );
                self.instances[index].host = Some(host);
                self.instances[index].error = None;
            }
            Err(error) => {
                tracing::warn!(
                    id = %self.instances[index].entry.id,
                    %error,
                    "local instance did not start"
                );
                self.instances[index].host = None;
                self.instances[index].error = Some(error.to_string());
            }
        }
    }

    /// A filesystem-safe id that no existing instance already uses.
    fn mint_id(&self, label: &str) -> String {
        let taken: HashSet<&str> = self
            .instances
            .iter()
            .map(|instance| instance.entry.id.as_str())
            .collect();
        let base = slugify(label);
        if !taken.contains(base.as_str()) {
            return base;
        }
        // Suffix rather than fail: two instances called "Acme" is an ordinary
        // thing to want, and the label is what the operator reads anyway.
        for n in 2.. {
            let candidate = format!("{base}-{n}");
            if !taken.contains(candidate.as_str()) {
                return candidate;
            }
        }
        unreachable!("an unbounded range always yields a free id")
    }

    fn persist(&self) {
        if let Err(error) = self.try_persist() {
            let path = self.data_dir.join(ROSTER_FILE);
            // Not fatal for reversible state changes: the instances running
            // right now keep running, and the roster stays at its last
            // successful write. Permanent deletion uses `try_persist`
            // directly because it cannot safely suppress this failure.
            tracing::error!(%error, path = %path.display(), "could not write the instance roster");
        }
    }

    fn try_persist(&self) -> Result<(), String> {
        self.try_persist_with(|| Ok(()))
    }

    fn try_persist_with<F>(&self, before_replace: F) -> Result<(), String>
    where
        F: FnOnce() -> std::io::Result<()>,
    {
        let roster = Roster {
            instances: self
                .instances
                .iter()
                .map(|instance| instance.entry.clone())
                .collect(),
        };
        let path = self.data_dir.join(ROSTER_FILE);
        let write = std::fs::create_dir_all(&self.data_dir).and_then(|()| {
            let body = serde_json::to_vec_pretty(&roster)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let mut temporary = tempfile::Builder::new()
                .prefix(".instances.json.")
                .tempfile_in(&self.data_dir)?;
            temporary.as_file_mut().write_all(&body)?;
            temporary.as_file().sync_all()?;
            before_replace()?;
            temporary.persist(&path).map_err(|error| error.error)?;
            #[cfg(unix)]
            std::fs::File::open(&self.data_dir)?.sync_all()?;
            Ok(())
        });
        write.map_err(|error| {
            format!(
                "could not write the instance roster at {}: {error}",
                path.display()
            )
        })
    }
}

/// Reads the roster, or invents the one every install has always had.
fn read_roster(data_dir: &Path) -> Roster {
    let path = data_dir.join(ROSTER_FILE);
    let parsed = std::fs::read(&path)
        .ok()
        .and_then(|body| serde_json::from_slice::<Roster>(&body).ok());
    let mut roster = parsed.unwrap_or_else(|| Roster {
        instances: Vec::new(),
    });

    // Drop anything a hand-edit or a partial write left unusable, and anything
    // whose root escapes the data dir. The roster is a plain file in a
    // directory an operator can open, and a `root` of `../../..` would point a
    // host — and its lock — at somewhere this application never chose.
    roster.instances.retain(|entry| {
        !entry.id.is_empty()
            && entry.id == slugify(&entry.id)
            && entry.root.as_deref().is_none_or(is_contained)
    });
    // By id across the whole file, not `dedup_by`, which only compares
    // neighbours. A roster holding `acme`, `other`, `acme` would keep both
    // `acme` rows, and two entries sharing an id share a *root*: the second
    // cannot start because the first holds its lock, so it becomes a permanent
    // failed row — and `index_of` resolves the first, so `rename`, `stop` and
    // `forget` could never reach it. Retaining the first occurrence keeps
    // listing order.
    let mut seen = HashSet::new();
    roster
        .instances
        .retain(|entry| seen.insert(entry.id.clone()));

    // The default is always present, and always first: it is the root every
    // pre-roster install already keeps its company in, and the one the console
    // opens on.
    if !roster
        .instances
        .iter()
        .any(|entry| entry.id == DEFAULT_INSTANCE_ID)
    {
        roster.instances.insert(
            0,
            RosterEntry {
                id: DEFAULT_INSTANCE_ID.to_string(),
                label: DEFAULT_INSTANCE_LABEL.to_string(),
                root: None,
                autostart: true,
                desktop_created: false,
            },
        );
    }
    roster
}

/// Whether a recorded root stays under the data dir.
fn is_contained(root: &str) -> bool {
    let path = Path::new(root);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

/// A lowercase, filesystem- and URL-safe form of a label.
///
/// Deliberately strict rather than clever: the result becomes a directory name
/// under the data dir, so everything outside `[a-z0-9-]` is folded to `-`
/// rather than transliterated. A label with nothing usable in it — every
/// non-Latin script, for instance — falls back to a fixed stem, and
/// [`LocalHosts::mint_id`] makes it unique.
fn slugify(label: &str) -> String {
    let mut out = String::new();
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        return "instance".to_string();
    }
    // Long enough to stay readable, short enough to stay inside every
    // filesystem's component limit once the data dir is prepended.
    trimmed.chars().take(48).collect()
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
