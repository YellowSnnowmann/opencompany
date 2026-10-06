use std::path::{Path, PathBuf};

use super::*;
use tinyskills::document_digest;

fn companies_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../companies")
}

fn collect_skill_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read a skills directory")
        .map(|entry| entry.expect("read a skills entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_skill_files(&path, out);
        } else if path.file_name().is_some_and(|name| name == "SKILL.md") {
            out.push(path);
        }
    }
}

fn shipped_skill_files() -> Vec<PathBuf> {
    let root = companies_dir();
    let mut bundles: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("read companies/")
        .map(|entry| entry.expect("read a bundle entry").path())
        .filter(|path| path.join("skills").is_dir())
        .collect();
    bundles.sort();
    let mut files = Vec::new();
    for bundle in bundles {
        collect_skill_files(&bundle.join("skills"), &mut files);
    }
    files
}

/// Every shipped `SKILL.md`, parsed and re-rendered, digested and pinned.
///
/// `render` is the digest of `render_skill_md(parse_skill_md(src))`, `source`
/// is `document_digest(src)` — the value a registry install pins — and `extra` is
/// the digest of the frontmatter lines the parser keeps aside. A parser or
/// renderer swap that moves any of the three for any shipped skill would shift
/// the pin an existing install was recorded against.
///
/// Re-bless with `BLESS_SKILL_PINS=1`, then read the diff.
#[test]
fn every_shipped_skill_parses_and_renders_to_its_pinned_digest() {
    let root = companies_dir();
    let files = shipped_skill_files();
    let mut rows = Vec::with_capacity(files.len());
    for file in &files {
        let src = std::fs::read_to_string(file).expect("read a shipped SKILL.md");
        let slug = file
            .parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            .expect("a SKILL.md sits in a slug directory");
        let doc = parse_skill_md(slug, &src)
            .unwrap_or_else(|err| panic!("{} does not parse: {err}", file.display()));
        let rel = file
            .strip_prefix(&root)
            .expect("under companies/")
            .to_string_lossy()
            .replace('\\', "/");
        rows.push(format!(
            "{rel} render={} source={} extra={}",
            document_digest(&render_skill_md(&doc)),
            document_digest(&src),
            document_digest(&doc.extra_frontmatter.join("\n")),
        ));
    }
    rows.sort_unstable();
    let rendered = format!("{}\n", rows.join("\n"));

    let globals = rows
        .iter()
        .filter(|row| row.starts_with("_globals/"))
        .count();
    assert_eq!(globals, 3, "the global baseline ships three skills");
    assert_eq!(rows.len(), 111, "every shipped SKILL.md is pinned");

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/skill-pins.txt");
    if std::env::var_os("BLESS_SKILL_PINS").is_some() {
        std::fs::write(&path, &rendered).expect("write the skill pin snapshot");
        return;
    }
    let committed = std::fs::read_to_string(&path).expect("read the skill pin snapshot");
    assert_eq!(
        rendered, committed,
        "a shipped skill's parse/render output or source digest moved. If that is intended, \
         re-bless with BLESS_SKILL_PINS=1 and say why: an install pinned against the old \
         digest now reports as modified"
    );
}
