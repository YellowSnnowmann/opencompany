//! Ledger declaration files: `companies/<name>/ledgers/<slug>.toml`, and the
//! same shape under `companies/_globals/ledgers/`.
//!
//! A company's axes were discoverable only at run time. The runtime ships three
//! ledgers (`tasks`, `goals`, `decisions`) and everything else a vertical
//! records — a deal pipeline, a matter list, an experiment log, the promises
//! made to a customer — had to be declared by an agent calling `define_ledger`
//! mid-run. That is the right *capability* (a company discovers which axes it
//! needs while it is running, and a declaration that needed a release would be
//! discovered and then not made) and the wrong *default*: a law firm that ships
//! with five agents, three skills and a workflow graph shipped with no matter
//! list, and got one only if some turn thought to invent it. Two runs of the
//! same template then disagreed about what the company even tracks.
//!
//! So a bundle may carry its axes the way it already carries its roster, its
//! workflows and its skills: one file per ledger, authored beside the company
//! it belongs to. [`load_dir_ledgers`] parses them; the runtime builder seeds
//! them into the store at first boot, exactly once, and never again — see
//! `runtime::builder`.
//!
//! # Why a wrapper shape rather than the wire type
//!
//! [`LedgerSpec`] is `rename_all = "camelCase"` because it is also what a
//! declaration round-trips through in every store backend and over the console
//! wire. TOML authored by hand is snake_case, and a template author should not
//! have to write `writtenBy` in a file that sits next to `needs_reason`. This
//! module is that seam and nothing else: every rule about what a ledger may be
//! stays in [`LedgerSpec::normalize`], which every parsed file is put through,
//! so a bundle ledger and one an agent declares at run time are held to exactly
//! one set of rules.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{OpenCompanyError, Result};
use crate::ledger::{Check, Field, LedgerSpec, MAX_DECLARED, Section, StatusSpec, normalize_slug};

/// The bundle subdirectory holding one TOML file per declared ledger.
pub const LEDGERS_DIR: &str = "ledgers";

/// The on-disk shape of one `ledgers/<slug>.toml`.
///
/// [`Field`], [`StatusSpec`] and [`Section`] are the runtime's own types,
/// reused rather than mirrored: none of the three renames its keys, so they
/// read as snake_case TOML already, and a field added to one of them is
/// available to a template author the same day. Only the top level needs a
/// wrapper, because [`LedgerSpec`] is camelCase on the wire.
///
/// `slug` is optional and comes from the filename. It is accepted in the body
/// only as a cross-check — see [`parse_ledger_file`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerFile {
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    purpose: String,
    /// How this ledger is actually written, in a sentence, shown to whoever is
    /// refused a hand-edit of its derived file. Defaulted by `normalize` when
    /// omitted.
    #[serde(default)]
    written_by: String,
    /// The rendered file under `derived/`. Omit it: `derived/<SLUG>.md` is what
    /// the author meant, and naming it is one more thing to get wrong.
    #[serde(default)]
    derived: String,
    #[serde(default)]
    writers: Vec<String>,
    #[serde(default)]
    checks: Vec<Check>,
    #[serde(default, rename = "field")]
    fields: Vec<Field>,
    #[serde(default, rename = "status")]
    statuses: Vec<StatusSpec>,
    #[serde(default, rename = "section")]
    sections: Vec<Section>,
}

/// Every `.toml` file directly inside `ledgers/`, sorted by path.
///
/// Sorted rather than readdir order, because the order declarations are seeded
/// in decides which one is refused first when a bundle overruns
/// [`MAX_DECLARED`] — and a cap that bites a different ledger depending on
/// which machine booted the company is not a cap anybody can author against.
///
/// Only the immediate directory is read; an unreadable one yields nothing,
/// which is what a bundle carrying no ledgers looks like.
fn ledger_file_paths(ledgers_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(ledgers_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();
    paths
}

/// Whether `dir` is a bundle carrying ledger declarations.
pub fn has_ledger_files(dir: &Path) -> bool {
    !ledger_file_paths(&dir.join(LEDGERS_DIR)).is_empty()
}

/// Parses one declaration file, named by `file_name` (`<slug>.toml`).
///
/// Every problem is reported in prosumer language and against the file that
/// carries it, matching [`super::agent_file`] and [`super::workflow_file`]: a
/// template author reads the message, not the serde path.
fn parse_ledger_file(file_name: &str, src: &str) -> std::result::Result<LedgerSpec, Vec<String>> {
    let stem = file_name.trim_end_matches(".toml");
    let file: LedgerFile = toml::from_str(src).map_err(|err| {
        vec![format!(
            "`{file_name}` is not valid TOML — {}",
            err.message()
        )]
    })?;

    // The filename is the identity. A body `slug` is allowed so the file reads
    // as a complete declaration, but a disagreement is refused rather than
    // resolved: either precedence rule silently ships a ledger under a slug its
    // author did not write, and every grant naming the other one then reads as
    // a ledger that does not exist.
    if let Some(declared) = file.slug.as_deref()
        && declared.trim() != stem
    {
        return Err(vec![format!(
            "`{file_name}` declares `slug = \"{}\"` but is named `{stem}.toml` — a ledger's slug \
             is its filename, so rename the file or drop the key.",
            declared.trim()
        )]);
    }

    let slug = normalize_slug(stem).map_err(|err| {
        vec![format!(
            "`{file_name}` is not a usable ledger filename — {err}"
        )]
    })?;

    let mut spec = LedgerSpec {
        slug,
        title: file.title,
        purpose: file.purpose,
        source: crate::ledger::LedgerSource::Events,
        derived: file.derived,
        fields: file.fields,
        statuses: file.statuses,
        sections: file.sections,
        checks: file.checks,
        writers: file.writers,
        written_by: file.written_by,
        builtin: false,
    };
    // One validator, shared with `define_ledger`: a bundle ledger is held to
    // every bound an agent-declared one is, including the clamps.
    spec.normalize()
        .map_err(|err| vec![format!("`{file_name}` is not a usable ledger — {err}")])?;
    Ok(spec)
}

/// Loads every ledger a bundle declares, from `<dir>/ledgers/*.toml`.
///
/// A missing directory is not a problem to report: most companies declare no
/// ledger of their own and get the built-ins, which is a complete answer.
///
/// # Errors
///
/// Returns [`OpenCompanyError::ManifestInvalid`] listing every problem across
/// every file — a malformed declaration, a slug that shadows a built-in, two
/// ledgers claiming one derived file, or more declarations than
/// [`MAX_DECLARED`]. All-or-nothing for a company's own bundle: shipping a
/// vertical silently short the axis it is about is the failure this exists to
/// prevent.
pub fn load_dir_ledgers(dir: &Path) -> Result<Vec<LedgerSpec>> {
    let ledgers_dir = dir.join(LEDGERS_DIR);
    let names: Vec<String> = ledger_file_paths(&ledgers_dir)
        .iter()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
        .collect();

    let (specs, problems) = parse_ledgers(&names, &|rel| {
        std::fs::read_to_string(ledgers_dir.join(rel)).map_err(|err| err.kind())
    });

    if problems.is_empty() {
        Ok(specs)
    } else {
        Err(OpenCompanyError::ManifestInvalid {
            path: ledgers_dir,
            problems,
        })
    }
}

/// Parses every named declaration, returning the ones that parsed alongside
/// every problem from the ones that did not.
///
/// [`load_dir_ledgers`] turns this into an all-or-nothing [`Result`] for a
/// company's own bundle. The global baseline (`crate::globals`) wants the
/// opposite — a malformed *global* must not cost every other global — so it
/// calls this directly and keeps what parsed, exactly as it does for agents.
///
/// `read` resolves a filename relative to `ledgers/`, so an embedded bundle and
/// an on-disk one go through one parser rather than two sets of rules.
pub(crate) fn parse_ledgers(
    names: &[String],
    read: &dyn Fn(&str) -> std::result::Result<String, std::io::ErrorKind>,
) -> (Vec<LedgerSpec>, Vec<String>) {
    let mut specs: Vec<LedgerSpec> = Vec::new();
    let mut problems = Vec::new();
    let (builtins, _) = crate::ledger::builtins();

    for name in names {
        let src = match read(name) {
            Ok(src) => src,
            Err(kind) => {
                problems.push(format!("`{name}` could not be read — {kind:?}"));
                continue;
            }
        };
        let spec = match parse_ledger_file(name, &src) {
            Ok(spec) => spec,
            Err(mut file_problems) => {
                problems.append(&mut file_problems);
                continue;
            }
        };

        // A built-in's slug and a built-in's derived path are both reserved.
        // `tasks` is the task board — a bundle file of that name would parse
        // perfectly and then be refused by the registry at boot, silently, on
        // every company that shipped it.
        if let Some(builtin) = builtins
            .iter()
            .find(|builtin| builtin.slug == spec.slug || builtin.derived == spec.derived)
        {
            problems.push(format!(
                "`{name}` collides with the built-in `{}`, which ships with the runtime and every \
                 prompt and route is written against — choose another slug.",
                builtin.slug
            ));
            continue;
        }
        if let Some(prior) = specs.iter().find(|prior| prior.derived == spec.derived) {
            problems.push(format!(
                "`{name}` renders into `{}`, which `{}` already claims — two writers on one \
                 derived file is how each one's rows disappear.",
                spec.derived, prior.slug
            ));
            continue;
        }
        if specs.len() >= MAX_DECLARED {
            problems.push(format!(
                "`{name}` is past the {MAX_DECLARED}-ledger cap; drop one of the declarations \
                 before it."
            ));
            continue;
        }
        specs.push(spec);
    }

    (specs, problems)
}

/// The declaration files of an embedded bundle, in the order `build.rs`
/// recorded them.
///
/// Only the immediate directory holds declarations, matching the on-disk walk.
pub(crate) fn embedded_ledger_names(files: &[(&str, &str)]) -> Vec<String> {
    let mut names: Vec<String> = files
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !name.contains('/') && name.ends_with(".toml"))
        .map(str::to_string)
        .collect();
    names.sort();
    names
}

#[cfg(test)]
#[path = "ledger_file_tests.rs"]
mod tests;
