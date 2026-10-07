//! SKILL.md documents: `skills/<slug>/SKILL.md` (repo-level and per-company).
//!
//! A skill is a Markdown file with a small `---`-fenced frontmatter block
//! carrying `name`, `description`, and an optional `category` and `version`.
//! The frontmatter is read by [`tinyskills::parse_flat`], line by line rather
//! than as YAML, and the Markdown body is preserved verbatim.

use std::path::{Path, PathBuf};

use tinyskills::{FlatError, FlatSkill};

use crate::error::{OpenCompanyError, Result};

/// A parsed SKILL.md document.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillDoc {
    /// The skill's directory name (its slug).
    pub slug: String,
    /// Display name, from frontmatter.
    pub name: String,
    /// One-line description, from frontmatter.
    pub description: String,
    /// Optional grouping category, from frontmatter.
    pub category: Option<String>,
    /// Optional publisher version, from frontmatter (e.g. `1.0.0`).
    ///
    /// Purely descriptive: nothing compares or orders it yet. It rides inside a
    /// registry install's snapshotted `SKILL.md`, so an installed skill records
    /// which revision it pinned and a later "update available" affordance can
    /// diff the installed snapshot against the live registry.
    pub version: Option<String>,
    /// The Markdown body after the frontmatter, preserved verbatim.
    pub body: String,
    /// Frontmatter lines this parser does not keep as a scalar field, verbatim:
    /// an unrecognised key, or a recognised key repeated after its first
    /// occurrence.
    ///
    /// The parser drops these, but an uploaded document is stored and
    /// materialized as its own source, so whatever those lines say is what the
    /// agent reads. Keeping them means the scan can see the whole of what will
    /// be stored rather than only the keys this struct names.
    pub extra_frontmatter: Vec<String>,
}

/// Parses one SKILL.md document for the given `slug` (its directory name).
///
/// The frontmatter must be a `---`-fenced block of `key: value` lines at the
/// very top; `name` and `description` are required, `category` and `version`
/// are optional, and any other keys are kept in
/// [`SkillDoc::extra_frontmatter`]. The body after the closing fence is kept
/// verbatim. The parsing is [`tinyskills::parse_flat`]'s; the errors name the
/// slug.
pub fn parse_skill_md(slug: &str, src: &str) -> Result<SkillDoc> {
    let path = PathBuf::from(format!("{slug}/SKILL.md"));
    match tinyskills::parse_flat(src) {
        Ok(flat) => Ok(SkillDoc::from_flat(slug, flat)),
        Err(FlatError::MissingKeys { keys }) => Err(OpenCompanyError::DataInvalid {
            path,
            problems: keys
                .iter()
                .map(|key| format!("skill `{slug}` is missing a `{key}` in its frontmatter."))
                .collect(),
        }),
        Err(_) => Err(OpenCompanyError::DataParse {
            path,
            message: "missing a `---` frontmatter block at the top of the file.".to_string(),
        }),
    }
}

/// Renders a [`SkillDoc`] back to `SKILL.md` source with
/// [`tinyskills::render_flat`]: a `---`-fenced frontmatter block of `name`,
/// `description`, then `category` and `version` when non-empty, then the
/// [`SkillDoc::extra_frontmatter`] lines, followed by the body verbatim.
///
/// `parse → render → parse` is a fixed point on the doc, but the output is the
/// canonical form rather than a copy of the original source: each scalar and
/// extra line is collapsed to one trimmed line, so a value cannot inject a key
/// or close the block early, and an extra line that would claim a recognised
/// key is left out.
pub fn render_skill_md(doc: &SkillDoc) -> String {
    tinyskills::render_flat(&doc.to_flat())
}

impl SkillDoc {
    /// The doc for `slug` from a document [`tinyskills::parse_flat`] read.
    pub fn from_flat(slug: &str, flat: FlatSkill) -> Self {
        Self {
            slug: slug.to_string(),
            name: flat.name,
            description: flat.description,
            category: flat.category,
            version: flat.version,
            body: flat.body,
            extra_frontmatter: flat.extra_frontmatter,
        }
    }

    /// The doc as a [`FlatSkill`], without its slug.
    pub fn to_flat(&self) -> FlatSkill {
        FlatSkill {
            name: self.name.clone(),
            description: self.description.clone(),
            category: self.category.clone(),
            version: self.version.clone(),
            body: self.body.clone(),
            extra_frontmatter: self.extra_frontmatter.clone(),
        }
    }

    /// The doc's text surfaces, borrowed for [`tinyskills::scan_skill`].
    pub fn scan_document(&self) -> tinyskills::ScanDocument<'_> {
        tinyskills::ScanDocument {
            name: &self.name,
            description: &self.description,
            category: self.category.as_deref(),
            version: self.version.as_deref(),
            body: &self.body,
            extra_frontmatter: &self.extra_frontmatter,
        }
    }
}

/// The bundle directory the skill registry lists first.
///
/// The global baseline (`companies/_globals`) is not a company, but its
/// `skills/` are what `[skills].always` installs everywhere, so they head the
/// registry and win any slug a vertical also ships.
const BASELINE_BUNDLE: &str = "_globals";

/// Loads the skill registry: every `<bundle>/skills/<slug>/SKILL.md` under a
/// `companies/` directory, one document per slug, sorted by slug.
///
/// There is no separate shared library — a skill lives in the bundle it
/// belongs to, and the registry is the union. When two bundles ship the same
/// slug the first in registry order keeps it: the baseline, then every other
/// bundle in name order. Each company still materializes its *own* bundle's
/// copy (`harness::built_in::skills`); this only decides what the registry
/// offers under that slug.
///
/// A missing `companies_dir` yields an empty list, exactly like
/// [`load_dir_skills`]; a bundle without a `skills/` directory contributes
/// nothing. A malformed `SKILL.md` anywhere fails the whole load, because a
/// registry that silently dropped a document would serve a different catalog
/// from the one on disk.
pub fn load_catalog_skills(companies_dir: &Path) -> Result<Vec<SkillDoc>> {
    if !companies_dir.exists() {
        return Ok(Vec::new());
    }
    let entries =
        std::fs::read_dir(companies_dir).map_err(|source| OpenCompanyError::DataRead {
            path: companies_dir.to_path_buf(),
            source,
        })?;
    let mut bundles = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| OpenCompanyError::DataRead {
            path: companies_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir()
            && let Some(name) = path.file_name().and_then(|name| name.to_str())
        {
            bundles.push((name.to_string(), path));
        }
    }
    // Baseline first, then name order — the precedence the doc above promises.
    bundles.sort_by(|a, b| {
        (a.0 != BASELINE_BUNDLE)
            .cmp(&(b.0 != BASELINE_BUNDLE))
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut by_slug: std::collections::BTreeMap<String, SkillDoc> =
        std::collections::BTreeMap::new();
    for (_, bundle) in bundles {
        for doc in load_dir_skills(&bundle.join("skills"))? {
            by_slug.entry(doc.slug.clone()).or_insert(doc);
        }
    }
    Ok(by_slug.into_values().collect())
}

/// Loads every `<slug>/SKILL.md` under a directory, sorted by slug.
///
/// A missing directory yields an empty list; a subdirectory without a
/// `SKILL.md` is skipped. [`load_catalog_skills`] composes this per bundle.
pub fn load_dir_skills(dir: &Path) -> Result<Vec<SkillDoc>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut slugs = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|source| OpenCompanyError::DataRead {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| OpenCompanyError::DataRead {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("SKILL.md").is_file()
            && let Some(slug) = path.file_name().and_then(|name| name.to_str())
        {
            slugs.push(slug.to_string());
        }
    }
    slugs.sort();

    let mut out = Vec::with_capacity(slugs.len());
    for slug in slugs {
        let file = dir.join(&slug).join("SKILL.md");
        let text = std::fs::read_to_string(&file).map_err(|source| OpenCompanyError::DataRead {
            path: file.clone(),
            source,
        })?;
        // Re-label parse/validation errors with the real on-disk path.
        let doc = match parse_skill_md(&slug, &text) {
            Ok(doc) => doc,
            Err(OpenCompanyError::DataInvalid { problems, .. }) => {
                return Err(OpenCompanyError::DataInvalid {
                    path: file,
                    problems,
                });
            }
            Err(OpenCompanyError::DataParse { message, .. }) => {
                return Err(OpenCompanyError::DataParse {
                    path: file,
                    message,
                });
            }
            Err(other) => return Err(other),
        };
        out.push(doc);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "skill_file_pins_tests.rs"]
mod pins_tests;
#[cfg(test)]
#[path = "skill_file_tests.rs"]
mod tests;
