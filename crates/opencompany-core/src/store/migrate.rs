//! One-shot boot migration off the legacy doubled home layout.
//!
//! Until the default home lost its `companies` leaf,
//! [`resolve_home`](crate::store::resolve_home) returned
//! `$HOME/.opencompany/companies` and [`Bundle`](crate::store::Bundle) appended a
//! `companies/` of its own, so a default local install's bundles sit one level
//! too deep:
//!
//! ```text
//! ~/.opencompany/companies/companies/<slug>/   ← legacy (doubled)
//! ~/.opencompany/companies/<slug>/             ← canonical
//! ```
//!
//! A local sqlite install is orphaned the same way: `serve` hands the resolved
//! home to [`open_storage`](crate::store::open_storage), so its database sits at
//! `~/.opencompany/companies/opencompany.db` rather than beside the workspace.
//!
//! Correcting the resolver without moving that data would silently orphan every
//! local company: the operator opens the console and their companies are gone,
//! which is worse than the wart itself. So `serve`, `export`, and `import` all
//! call [`migrate_legacy_nest_announced`] against the resolved home before
//! touching it. Running it for `export` and `import` too is not thoroughness for
//! its own sake: otherwise an un-migrated install's first post-upgrade command
//! fails to find its bundles.
//!
//! The harness workspace and the MCP registry are orphaned by the same shift:
//! both hang off the resolved home rather than off a bundle
//! (`runtime/builder.rs`), so an operator's installed MCP servers — and the
//! environment values stored with them — would be left behind at
//! `~/.opencompany/companies/mcp` exactly as the database was.
//!
//! The migration is a detect-and-move:
//!
//! - No `<home>/companies/companies` directory is a no-op. A hosted tenant, whose
//!   home resolves to `/data`, takes this branch on every boot: two `stat`s that
//!   find nothing.
//! - A nest that is itself **bundle-shaped** — holding any of the files or
//!   subdirectories only a company owns — *is* a real bundle whose slug happens
//!   to be `companies`, and is left alone. A manifest is deliberately not the
//!   test: ~20 sites create a bundle through
//!   [`Bundle::ensure_dirs`](crate::store::Bundle::ensure_dirs) with neither
//!   `company.toml` nor `meta.json`, and under a sqlite or mongodb store the
//!   manifest never reaches the filesystem at all while the keys, secrets and
//!   task board still do. Keying off the manifest would have dissolved exactly
//!   those installs — hosted ones included.
//! - Only entries that are **themselves bundle-shaped directories** are
//!   relocated. That is the same test one level down, and it is the second
//!   guard: even if the check above ever misread a bundle as a nest, that
//!   bundle's own `keys/`, `secrets/` and `tasks.json` are not bundle-shaped and
//!   would stay where they are. Anything unrecognised is left exactly where it
//!   is, silently — the legacy nest holds nothing but bundles, so an entry that
//!   does not look like one is not something this migration knows how to place.
//! - Each recognised entry is renamed up one level when the destination is free.
//! - A destination that already exists is **skipped with a warning naming both
//!   paths**, never merged: a user who ran the app both ways has two copies of
//!   one company, and interleaving two event logs and two signing keys is
//!   unrecoverable. Picking a winner is the same bet with the loser deleted.
//! - The nest directory is removed only once emptied, so a crash mid-loop simply
//!   resumes on the next boot. Idempotent by construction.
//! - The harness workspace (`<home>/harness`) and the MCP runtime registry
//!   (`<home>/mcp`) move up beside the workspace like the database, under the
//!   same shape guard: a company really can be slugged `harness`, and its
//!   canonical bundle is at exactly the path the legacy tree occupied.
//! - The database and its `-wal`/`-shm` siblings resume the same way: the set is
//!   detected from *any* surviving member, not from the database alone, so a run
//!   that moved the database and then died is finished by the next boot rather
//!   than being mistaken for a completed one.
//! - Only **regular files** are ever treated as the database. A company slugged
//!   `opencompany.db` owns the directory at that exact path, and moving it would
//!   delete the company.
//! - Files move by `link`+`unlink`, never by `rename`: a rename replaces a
//!   regular file silently, and a "is the destination free?" check taken
//!   beforehand is stale the instant it is read. Directories keep `rename`,
//!   which cannot replace a populated directory (`ENOTEMPTY`) or a regular file
//!   (`ENOTDIR`) at all.
//! - A source another process moved first is a success, not a failure: `serve`
//!   and a hand-run `export` against one home both migrate, and neither should
//!   abort because the other won the race.
//!
//! An install whose migration genuinely cannot complete — `EXDEV` because the
//! nest is a mount point, a root-owned or read-only `companies/` — still boots:
//! `--home ~/.opencompany/companies` resolves every bundle exactly where it
//! already sits and finds no nest beneath it to migrate. That shape warns about
//! the split workspace, correctly, and is the documented way to run an install
//! this migration cannot move.
//!
//! Moves are announced on stderr rather than through `warn!`: the default
//! `EnvFilter` drops warnings unless `RUST_LOG` is set, which would make the
//! announcement exactly as invisible as the bug it reports.

use std::path::{Path, PathBuf};

use crate::Result;
use crate::error::OpenCompanyError;

/// The local sqlite database — named by
/// [`open_storage`](crate::store::open_storage) — and its write-ahead-log
/// siblings.
///
/// These travel as a set: leaving a `-wal` behind either strands committed
/// transactions or pairs a moved database with a stale log. The whole slice is
/// also the *detector*, not just the payload — a run that moved the database and
/// then died is recognised by whichever member it left behind, which is what
/// makes the move resumable without a migration journal.
const LEGACY_SQLITE_FILES: &[&str] =
    &["opencompany.db", "opencompany.db-wal", "opencompany.db-shm"];

/// Top-level files only a company bundle has, from the layout
/// [`Bundle`](crate::store::Bundle) defines. These are the strong half of the
/// shape test: a company slug is always a *directory*, so no nest of bundles can
/// hold a `company.toml` or an `events.jsonl` of its own.
///
/// A manifest alone is not enough to identify a bundle — most of these files
/// exist in installs that never materialize one — which is why the whole list is
/// the marker rather than `company.toml`.
const BUNDLE_FILES: &[&str] = &[
    "company.toml",
    "meta.json",
    "events.jsonl",
    "ledger.jsonl",
    "journal.jsonl",
    "inbox.jsonl",
    "inbox-meta.json",
    "tasks.json",
    "facts.jsonl",
    "artifacts.jsonl",
    "workflow-revisions.jsonl",
    "users.json",
    "user-invites.json",
    "user-sessions.json",
    "login-codes.json",
    "usage.jsonl",
    "skills.json",
];

/// Top-level subdirectories a company bundle owns — the four
/// [`Bundle::ensure_dirs`](crate::store::Bundle::ensure_dirs) creates, plus the
/// two written on first use. This is the weaker half of the test: a company
/// *could* be slugged `keys` or `memory`, so a nest holding one of these reads
/// as a bundle. That bias is deliberate and one-directional — it can only ever
/// leave data where it is.
const BUNDLE_DIRS: &[&str] = &[
    "keys",
    "secrets",
    "memory",
    "context",
    "workspace",
    "feedback",
];

/// Runtime trees written under the *home* rather than under a bundle, so the
/// dropped `companies` leaf orphans them exactly as it orphaned the database.
///
/// `<home>/harness` holds every agent's working files
/// (`runtime/builder.rs`, `workspace_root`); `<home>/mcp` holds the MCP runtime
/// registry, whose persisted installs and stored environment values are
/// reconnected on boot.
const LEGACY_HOME_DIRS: &[(&str, Relocated)] = &[
    ("harness", Relocated::HarnessWorkspace),
    ("mcp", Relocated::McpRegistry),
];

/// What a migrated path holds, so the operator message names it accurately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relocated {
    /// A company bundle directory.
    Company,
    /// The local sqlite database or one of its `-wal`/`-shm` siblings.
    Database,
    /// The harness workspace tree holding every agent's working files.
    HarnessWorkspace,
    /// The MCP runtime registry: installed servers and their stored environment
    /// values.
    McpRegistry,
}

/// A same-named entry that already existed at its destination. Both copies are
/// left exactly where they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collision {
    /// What the two copies hold.
    pub what: Relocated,
    /// The copy still sitting in the legacy nested layout.
    pub legacy: PathBuf,
    /// The copy already at the canonical location.
    pub destination: PathBuf,
}

/// What [`migrate_legacy_nest`] did, so the caller can report it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NestMigration {
    /// Paths moved up one level, in their new canonical locations.
    pub moved: Vec<(Relocated, PathBuf)>,
    /// Entries skipped because the destination was occupied.
    pub collisions: Vec<Collision>,
}

impl NestMigration {
    /// True when nothing moved and nothing collided: the ordinary case on every
    /// boot after the first, and on every hosted boot.
    pub fn is_empty(&self) -> bool {
        self.moved.is_empty() && self.collisions.is_empty()
    }

    /// Operator-facing lines describing the migration, in report order. Empty
    /// when [`is_empty`](Self::is_empty), so a settled install stays silent.
    pub fn report(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (what, destination) in &self.moved {
            let noun = match what {
                Relocated::Company => "company bundle",
                Relocated::Database => "local database file",
                Relocated::HarnessWorkspace => "harness agent workspace",
                Relocated::McpRegistry => "MCP server registry",
            };
            lines.push(format!(
                "moved {noun} up out of the legacy nested layout: {}",
                destination.display()
            ));
        }
        for collision in &self.collisions {
            let advice = match collision.what {
                Relocated::Company => {
                    "Two copies of one company cannot be merged: interleaving two \
                     event logs and two signing keys is not recoverable. Keep one \
                     by hand and move or delete the other."
                }
                Relocated::Database => {
                    "Two databases hold two separate histories. Keep one by hand \
                     and move or delete the other, including its -wal and -shm \
                     siblings."
                }
                Relocated::HarnessWorkspace => {
                    "Two harness workspaces hold two sets of agent working \
                     files. Keep one by hand and move or delete the other."
                }
                Relocated::McpRegistry => {
                    "Two MCP registries hold two sets of installed servers and \
                     the environment values stored with them. Keep one by hand \
                     and move or delete the other."
                }
            };
            lines.push(format!(
                "left both copies in place: {} already exists, so the legacy copy \
                 stays at {}. {advice}",
                collision.destination.display(),
                collision.legacy.display(),
            ));
        }
        lines
    }
}

/// Moves a legacy `<home>/companies/companies/<slug>` install up one level, plus
/// any local sqlite database orphaned beside it, and announces what moved on
/// stderr.
///
/// This is the boot entry point; [`migrate_legacy_nest`] is the silent core the
/// tests drive.
pub fn migrate_legacy_nest_announced(home: &Path) -> Result<NestMigration> {
    let migration = migrate_legacy_nest(home)?;
    for line in migration.report() {
        eprintln!("opencompany: {line}");
    }
    Ok(migration)
}

/// Migrates the legacy doubled layout under `home`. See the [module docs](self)
/// for the rules; idempotent, and a no-op when there is no nest.
pub fn migrate_legacy_nest(home: &Path) -> Result<NestMigration> {
    let companies = home.join("companies");
    let mut migration = NestMigration::default();
    migrate_bundles(&companies, &mut migration)?;
    migrate_sqlite(home, &companies, &mut migration)?;
    migrate_home_dirs(home, &companies, &mut migration)?;
    Ok(migration)
}

/// Whether `dir` holds anything only a company bundle holds.
///
/// The manifest is not the test — see the [module docs](self) for why. Used
/// twice, and in opposite directions: a bundle-shaped *nest* is a company that
/// must not be dissolved, and a bundle-shaped *entry* is a company that must be
/// moved up. Both readings fail safe on a miss, because the only thing an
/// unrecognised path can do here is stay exactly where it already is.
fn is_bundle_shaped(dir: &Path) -> bool {
    BUNDLE_FILES
        .iter()
        .chain(BUNDLE_DIRS)
        .any(|name| exists(&dir.join(name)))
}

/// Renames every `<companies>/companies/<slug>` up into `<companies>/<slug>`.
fn migrate_bundles(companies: &Path, migration: &mut NestMigration) -> Result<()> {
    let nest = companies.join("companies");
    // The hosted no-op: one `stat` that finds nothing.
    if !nest.is_dir() {
        return Ok(());
    }
    // A real bundle that happens to be slugged `companies`. Dissolving it would
    // scatter its event log, task board and signing key across the companies
    // directory — and a bundle needs no manifest at all to be one, which is what
    // makes the shape rather than a marker file the test.
    if is_bundle_shaped(&nest) {
        return Ok(());
    }

    let mut names = read_dir_names(&nest)?;
    names.sort();
    for name in names {
        let legacy = nest.join(&name);
        // The second guard. The legacy nest holds nothing but bundles, so an
        // entry that does not look like one is either a stray or evidence that
        // this nest is a bundle after all. Either way it is not something this
        // migration knows where to put, and leaving it costs nothing.
        if !legacy.is_dir() || !is_bundle_shaped(&legacy) {
            continue;
        }
        let destination = companies.join(&name);
        if exists(&destination) {
            // Another process migrating this same bundle takes the legacy entry
            // with it, so a destination that appeared while the source vanished
            // is that move — not two different bundles contending for one name.
            if !exists(&legacy) {
                continue;
            }
            migration.collisions.push(Collision {
                what: Relocated::Company,
                legacy,
                destination,
            });
            continue;
        }
        if rename_or_already_moved(&legacy, &destination)? {
            migration.moved.push((Relocated::Company, destination));
        }
    }

    // Only once emptied. A skipped collision keeps the nest, and the next boot
    // picks up where this one stopped.
    if read_dir_names(&nest)?.is_empty() {
        // A failure here leaves an empty directory, not lost data, and the next
        // boot retries. Never worth aborting a boot for.
        let _ = std::fs::remove_dir(&nest);
    }
    Ok(())
}

/// Moves a local sqlite database orphaned at `<home>/companies/opencompany.db`
/// beside the workspace at `<home>/opencompany.db`, with its `-wal`/`-shm`
/// siblings.
///
/// A hosted tenant never has one: it resolves its home to the data root, so its
/// database is written at `<root>/opencompany.db` to begin with.
fn migrate_sqlite(home: &Path, companies: &Path, migration: &mut NestMigration) -> Result<()> {
    // **Regular files only.** A company whose slug happens to be
    // `opencompany.db` has its canonical bundle at exactly
    // `<home>/companies/opencompany.db`, and a bare existence check accepts that
    // directory. Moving it up would delete the company from every bundle lookup
    // and hand sqlite a directory to open. `slug` permits `.`, so this is a name
    // an operator can really have.
    let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
    for name in LEGACY_SQLITE_FILES {
        let legacy = companies.join(name);
        if is_regular_file(&legacy)? {
            moves.push((legacy, home.join(name)));
        }
    }
    // Detection deliberately does **not** hinge on the database file alone. A
    // previous run that moved `opencompany.db` and then died would leave the
    // `-wal` behind, and keying off the database would read that as "already
    // migrated" — pairing a relocated database with a stranded write-ahead log,
    // which loses committed transactions. Any surviving member of the set is a
    // migration to resume, so the next boot finishes what the last one started.
    if moves.is_empty() {
        return Ok(());
    }
    // An occupied destination among the files still to move means two databases,
    // i.e. two histories. Skip the whole set and say so.
    //
    // A resumed half-move is not this case: there the database is already at its
    // destination and is no longer among `moves`, so only the stranded siblings
    // are checked and they find their destinations free. The one shape that
    // cannot be told apart — a legacy `-wal` with no legacy `.db`, beside an
    // unrelated canonical database — is not one sqlite produces.
    //
    // A destination that is the *same file* as its source is not this case
    // either: it is the link half of a move that died before unlinking the
    // source, and reporting that as two databases would send the operator to
    // resolve a collision between a file and itself.
    for (legacy, destination) in &moves {
        if exists(destination) && !same_file(legacy, destination)? {
            migration.collisions.push(Collision {
                what: Relocated::Database,
                legacy: legacy.clone(),
                destination: destination.clone(),
            });
            return Ok(());
        }
    }
    for (legacy, destination) in moves {
        let outcome = move_file_no_replace(&legacy, &destination)?;
        match outcome {
            Moved::Here => migration.moved.push((Relocated::Database, destination)),
            Moved::Already => {}
            // The scan above found this destination free and something claimed
            // it in between. Stop the set rather than move the rest around it:
            // whatever is there now is live, and the next boot resumes.
            Moved::Occupied => {
                migration.collisions.push(Collision {
                    what: Relocated::Database,
                    legacy,
                    destination,
                });
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Moves the runtime trees that hang off the *home* rather than off a bundle —
/// the harness workspace and the MCP registry — up beside the workspace.
///
/// Guarded by the same shape test as the nest: `<home>/companies/harness` is
/// both where the legacy harness tree sat and where a company slugged `harness`
/// has its canonical bundle, and only one of the two is bundle-shaped.
fn migrate_home_dirs(home: &Path, companies: &Path, migration: &mut NestMigration) -> Result<()> {
    for (name, what) in LEGACY_HOME_DIRS {
        let legacy = companies.join(name);
        if !legacy.is_dir() || is_bundle_shaped(&legacy) {
            continue;
        }
        let destination = home.join(name);
        if exists(&destination) {
            migration.collisions.push(Collision {
                what: *what,
                legacy,
                destination,
            });
            continue;
        }
        if rename_or_already_moved(&legacy, &destination)? {
            migration.moved.push((*what, destination));
        }
    }
    Ok(())
}

/// Existence that a dangling symlink still counts as occupying: plain
/// [`Path::exists`] follows the link and reports `false`, and a rename onto a
/// dangling symlink would silently replace it.
fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Directory entry names, or an empty vec when the directory is gone.
fn read_dir_names(dir: &Path) -> Result<Vec<std::ffi::OsString>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(OpenCompanyError::StoreIo {
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(Ok(entry.file_name())),
            // The directory went away mid-scan: another process finished the
            // same migration and removed it. Nothing left to enumerate, which
            // is the same answer an absent directory gives above.
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => Some(Err(OpenCompanyError::StoreIo {
                path: dir.to_path_buf(),
                source,
            })),
        })
        .collect()
}

/// Whether `path` is a regular file, following no symlink. Absent is `false`;
/// an unreadable path is an error rather than a silent `false`.
fn is_regular_file(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(OpenCompanyError::StoreIo {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Renames within one directory tree, returning whether *this* call performed
/// the move.
///
/// A source that vanished between enumeration and rename, whose destination now
/// exists, is another process having migrated it first — `Ok(false)`, not a
/// failure. Running `opencompany export` against a home a `serve` process is
/// booting is ordinary, and both commands migrate; aborting one of them on a
/// `NotFound` that means "already done" would be a boot failure with nothing
/// wrong behind it.
///
/// Every other failure aborts rather than continuing with half an install: a
/// runtime that silently comes up missing companies is the exact symptom this
/// migration exists to prevent. A `NotFound` with the destination *also* absent
/// is such a failure (a missing destination parent, say) and propagates.
fn rename_or_already_moved(from: &Path, to: &Path) -> Result<bool> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound && exists(to) => Ok(false),
        Err(source) => Err(OpenCompanyError::StoreIo {
            path: from.to_path_buf(),
            source,
        }),
    }
}

/// What one file move did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Moved {
    /// This call performed the move.
    Here,
    /// The file was already at its destination — a previous run, or another
    /// process, got there first. Nothing left to do and nothing to report.
    Already,
    /// Something else occupies the destination. Nothing was touched.
    Occupied,
}

/// Moves a regular file, never replacing whatever is at the destination.
///
/// [`std::fs::rename`] replaces a regular file silently, and a separate "is the
/// destination free?" check cannot close that: the answer is stale the moment it
/// is read. The window is narrow and the loss is total — a `serve` that has
/// already migrated and opened the database is writing a live
/// `opencompany.db-wal` at the destination, and a rename over it drops every
/// committed transaction that log still holds.
///
/// [`std::fs::hard_link`] fails when the destination exists, so the check and
/// the move are one indivisible step and no interleaving can produce a
/// replacement. The source is unlinked once the link is in place; a crash
/// between the two leaves one file reachable under both names, which the next
/// run recognises by device and inode and finishes rather than reporting as two
/// databases.
///
/// Directories keep [`rename_or_already_moved`]: `rename` cannot replace a
/// populated directory (`ENOTEMPTY`) or a regular file (`ENOTDIR`), so the only
/// thing it can overwrite there is an empty directory, which holds nothing.
fn move_file_no_replace(from: &Path, to: &Path) -> Result<Moved> {
    match std::fs::hard_link(from, to) {
        Ok(()) => {
            remove_file(from)?;
            Ok(Moved::Here)
        }
        // Another process moved this file between the scan and here.
        Err(source) if source.kind() == std::io::ErrorKind::NotFound && exists(to) => {
            Ok(Moved::Already)
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            if same_file(from, to)? {
                // Our own link, from a run that died before the unlink.
                remove_file(from)?;
                return Ok(Moved::Already);
            }
            Ok(Moved::Occupied)
        }
        Err(source) => Err(OpenCompanyError::StoreIo {
            path: from.to_path_buf(),
            source,
        }),
    }
}

/// Whether two paths name one file — two links to a single inode rather than two
/// copies.
///
/// Always `false` off unix, where a crash between `link` and `unlink` degrades
/// to a reported collision the operator resolves by hand instead of resolving
/// itself. Both paths must exist; every caller has just established that.
fn same_file(a: &Path, b: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let left = std::fs::symlink_metadata(a).map_err(|source| OpenCompanyError::StoreIo {
            path: a.to_path_buf(),
            source,
        })?;
        let right = std::fs::symlink_metadata(b).map_err(|source| OpenCompanyError::StoreIo {
            path: b.to_path_buf(),
            source,
        })?;
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        Ok(false)
    }
}

/// Removes a file, naming it in the error.
fn remove_file(path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(|source| OpenCompanyError::StoreIo {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
#[path = "migrate_tests.rs"]
mod tests;
