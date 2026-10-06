//! Reading an uploaded skill: a bare `SKILL.md`, or an archive that carries
//! one.
//!
//! This module answers one question — what document did the operator upload,
//! and under which slug — and refuses everything it cannot answer that for. It
//! deliberately stops there: [`skill_validate`](super::skill_validate) decides
//! whether the document is acceptable and [`tinyskills::scan_skill`]
//! decides whether its text is safe, so an upload passes the same two gates a
//! registry install and a console-authored skill do.
//!
//! ## The archive is the attack surface
//!
//! A `.zip` is a list of paths and byte counts supplied by whoever built it,
//! and every one of those is hostile input. [`tinyskills::read_skill_archive`]
//! reads it under [`tinyskills::MAX_ARCHIVE_ENTRIES`] and
//! [`tinyskills::MAX_ARCHIVE_BYTES`], checking every entry's path, link bit,
//! nesting and declared size before anything is decompressed, and reading
//! content through a bound. This module turns each refusal into the sentence
//! the upload dialog shows against the file's row.
//!
//! ## Bundled resource files are refused, not dropped
//!
//! `SkillState.custom_doc` is a single document, so there is nowhere to put a
//! script or a reference file an archive carries. An archive with extras is
//! therefore refused with its file names in the message. Storing the `SKILL.md`
//! and silently discarding the rest would hand the operator a skill whose
//! procedure references files no agent will ever find.

use tinyskills::{ArchiveError, ArchiveFormat, ArchiveLimits, SKILL_MD, read_skill_archive};

use super::skill_validate::{slugify, validate_slug};

/// A document read off an upload, ready for validation and the scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadedSkill {
    /// The slug the document will be stored under — the archive's top
    /// directory when it has one, otherwise derived from the frontmatter name.
    pub slug: String,
    /// The `SKILL.md` source, verbatim.
    pub doc: String,
}

/// Reads one uploaded file into the document it carries.
///
/// `filename` picks the reader: `.md` is the document itself, `.zip` and
/// `.skill` are archives containing one. Anything else is refused by extension
/// rather than by sniffing, so an operator is told which formats the route
/// takes instead of watching a `.tar.gz` fail as a malformed archive.
///
/// The refusal is a plain sentence because it is rendered against the file's
/// own row in the upload dialog: several files are uploaded at once and each
/// gets its own outcome, so this cannot be a whole-request error.
pub fn read_upload(filename: &str, bytes: &[u8]) -> Result<UploadedSkill, String> {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".md") {
        return read_markdown(bytes);
    }
    if lower.ends_with(".zip") || lower.ends_with(".skill") {
        return read_archive(bytes);
    }
    Err("only `.md`, `.zip` and `.skill` files can be uploaded as skills.".to_string())
}

/// Reads a bare `SKILL.md`, whose slug comes from its own frontmatter name.
///
/// There is no directory to take a slug from, so the document names itself the
/// same way console authoring does — through [`slugify`] over the display name.
fn read_markdown(bytes: &[u8]) -> Result<UploadedSkill, String> {
    let doc = String::from_utf8(bytes.to_vec()).map_err(|_| NOT_UTF8.to_string())?;
    let name = frontmatter_name(&doc).ok_or(
        "that file has no `name` in its frontmatter, so there is nothing to store it under. A \
         skill starts with a `---` block carrying `name` and `description`.",
    )?;
    Ok(UploadedSkill {
        slug: slugify(&name),
        doc,
    })
}

/// Reads an archive that carries exactly one `SKILL.md` and nothing else.
fn read_archive(bytes: &[u8]) -> Result<UploadedSkill, String> {
    let archive = read_skill_archive(ArchiveFormat::Zip, bytes, &ArchiveLimits::default())
        .map_err(|error| archive_problem(bytes, error))?;

    if !archive.resources.is_empty() {
        let prefix = archive
            .root
            .as_ref()
            .map(|dir| format!("{dir}/"))
            .unwrap_or_default();
        let extras: Vec<String> = archive
            .resources
            .iter()
            .map(|file| format!("{prefix}{}", file.path))
            .collect();
        return Err(format!(
            "that archive also carries {}. A skill stores one document, so there is nowhere to \
             keep bundled files — upload a `{SKILL_MD}` on its own rather than have them \
             dropped.",
            join_names(&extras)
        ));
    }

    let slug = match archive.root {
        Some(dir) => {
            validate_slug(&dir).map_err(|problem| {
                format!("that archive's directory is not a usable skill name. {problem}")
            })?;
            dir
        }
        None => slugify(&frontmatter_name(&archive.document).ok_or_else(|| {
            format!(
                "that archive's `{SKILL_MD}` has no `name` in its frontmatter, so there is \
                 nothing to store it under."
            )
        })?),
    };

    Ok(UploadedSkill {
        slug,
        doc: archive.document,
    })
}

const NOT_UTF8: &str = "that file is not UTF-8 text, so it is not a `SKILL.md`.";

/// The upload dialog's sentence for an archive [`read_skill_archive`] refused.
fn archive_problem(bytes: &[u8], error: ArchiveError) -> String {
    let inside = "An uploaded skill is read as a directory of its own, so every entry has to sit \
                  inside it.";
    let no_single = "A skill archive carries one at the top level, or inside a single directory.";
    match error {
        ArchiveError::Unreadable(reason) => format!("that archive could not be read: {reason}."),
        ArchiveError::TooManyEntries { max } => {
            let held = zip::ZipArchive::new(std::io::Cursor::new(bytes))
                .map(|archive| archive.len())
                .unwrap_or(max + 1);
            format!("that archive holds {held} entries — an uploaded skill may hold {max}.")
        }
        ArchiveError::TooLarge { max_bytes } => format!(
            "that archive expands to more than {} KB, which is more than an uploaded skill may \
             hold.",
            max_bytes / 1024
        ),
        ArchiveError::EmptyPath => "that archive holds an entry with no name.".to_string(),
        ArchiveError::BackslashPath(path) => format!(
            "`{path}` in that archive is not a relative path. Archive entries separate \
             directories with `/`."
        ),
        ArchiveError::AbsolutePath(path) => {
            format!("`{path}` in that archive is an absolute path. {inside}")
        }
        ArchiveError::Traversal(path) => {
            format!("`{path}` in that archive points outside it. {inside}")
        }
        ArchiveError::PathTooLong => {
            "that archive holds an entry with an unreasonably long path.".to_string()
        }
        ArchiveError::Link(path) => format!(
            "`{path}` in that archive is a symbolic link. An uploaded skill is read as files, and \
             a link is how an archive reaches a path it never names."
        ),
        ArchiveError::NestedArchive(path) => format!(
            "`{path}` in that archive is itself an archive. An uploaded skill is read one level \
             deep."
        ),
        ArchiveError::MultipleRoots => {
            format!("that archive has no single `{SKILL_MD}`. {no_single}")
        }
        ArchiveError::NoSkillDocument => format!("that archive has no `{SKILL_MD}`. {no_single}"),
        ArchiveError::NotUtf8 => NOT_UTF8.to_string(),
    }
}

/// The `name` scalar from a document's frontmatter, when it has one.
fn frontmatter_name(doc: &str) -> Option<String> {
    let (frontmatter, _) = tinyskills::split_frontmatter(doc)?;
    frontmatter.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(':')?;
        (key.trim().eq_ignore_ascii_case("name") && !value.trim().is_empty())
            .then(|| value.trim().to_string())
    })
}

/// Renders a handful of file names as a readable list.
fn join_names(names: &[String]) -> String {
    const SHOWN: usize = 3;
    let shown: Vec<String> = names
        .iter()
        .take(SHOWN)
        .map(|name| format!("`{name}`"))
        .collect();
    match names.len().checked_sub(SHOWN) {
        Some(rest) if rest > 0 => format!("{} and {rest} more", shown.join(", ")),
        _ => shown.join(", "),
    }
}

#[cfg(test)]
#[path = "skill_upload_tests.rs"]
mod tests;
