use super::*;

#[test]
fn lowercases_and_dashes_the_shapes_the_seeds_actually_carry() {
    assert_eq!(kebab_name("Agents"), "agents");
    assert_eq!(kebab_name("Playbooks"), "playbooks");
    assert_eq!(kebab_name("Close checklist.md"), "close-checklist.md");
    assert_eq!(kebab_name("Q2 close.md"), "q2-close.md");
    assert_eq!(kebab_name("LiveOps calendar.md"), "liveops-calendar.md");
    assert_eq!(kebab_name("README.md"), "readme.md");
    assert_eq!(kebab_name("Page.tsx"), "page.tsx");
    assert_eq!(kebab_name("Page.compiled.mjs"), "page.compiled.mjs");
    assert_eq!(kebab_name("page_builder"), "page-builder");
}

#[test]
fn collapses_separator_runs_and_trims_the_ends() {
    assert_eq!(kebab_name("  Spring   launch  "), "spring-launch");
    assert_eq!(kebab_name("a---b"), "a-b");
    assert_eq!(kebab_name("Q3 report -.md"), "q3-report.md");
    assert_eq!(kebab_name("trailing-"), "trailing");
}

#[test]
fn never_produces_a_segment_that_is_not_addressable() {
    // A path separator is a separator, not a name.
    assert_eq!(kebab_name("a/b"), "a-b");
    // No hidden files, no `.`/`..`, no empty segment.
    assert_eq!(kebab_name(".hidden"), "hidden");
    assert_eq!(kebab_name("."), FALLBACK_NAME);
    assert_eq!(kebab_name(".."), FALLBACK_NAME);
    assert_eq!(kebab_name(""), FALLBACK_NAME);
    assert_eq!(kebab_name("🎉"), FALLBACK_NAME);
}

#[test]
fn the_fallback_is_itself_normalized() {
    assert_eq!(kebab_name_or("🎉", "Task 42"), "task-42");
    // A caller cannot smuggle a name past the rule through the fallback.
    assert_eq!(kebab_name_or("", "Not A Slug"), "not-a-slug");
    // And a fallback that is itself unnameable still yields a name.
    assert_eq!(kebab_name_or("", "🎉"), FALLBACK_NAME);
}

#[test]
fn is_kebab_name_is_the_fixed_point_of_kebab_name() {
    for raw in [
        "Agents",
        "Close checklist.md",
        "page.toml",
        "readme.md",
        "a-b-c",
        "🎉",
        "",
        "UPPER",
        "under_score",
    ] {
        let once = kebab_name(raw);
        assert_eq!(kebab_name(&once), once, "{raw}: not idempotent");
        assert!(is_kebab_name(&once), "{raw}: normalized form rejected");
    }
    assert!(!is_kebab_name("Agents"));
    assert!(!is_kebab_name("Close checklist.md"));
    assert!(is_kebab_name("close-checklist.md"));
}

#[test]
fn bounds_the_length_without_leaving_a_dangling_separator() {
    let long = "Very Long Title ".repeat(40);
    let name = kebab_name(&long);
    assert!(name.len() <= MAX_NAME_BYTES, "{} bytes", name.len());
    assert!(!name.ends_with('-'));
    assert!(is_kebab_name(&name));
}

/// A name long enough to hit the cap keeps the extension that identifies
/// its format: the stored name keys both mime inference and
/// `ingest::extract` dispatch, so a `.docx` shed at the cap would make the
/// upload unreadable at recall time (codex review finding on #1682).
#[test]
fn a_long_name_keeps_the_extension_that_identifies_its_format() {
    let raw = format!(
        "Quarterly Financial Review & Board Deck {} .docx",
        "FINAL ".repeat(20)
    );
    let name = kebab_name(&raw);
    assert!(name.len() <= MAX_NAME_BYTES, "{} bytes", name.len());
    assert!(name.ends_with(".docx"), "{name}");
    assert!(is_kebab_name(&name), "{name}");
}

/// The extension reserve only engages on truncation — a name that fits is
/// byte-for-byte the same as before the reserve existed.
#[test]
fn the_extension_reserve_does_not_rewrite_names_that_fit() {
    assert_eq!(kebab_name("README.md"), "readme.md");
    assert_eq!(kebab_name("Page.compiled.mjs"), "page.compiled.mjs");
    assert_eq!(kebab_name("Close checklist.md"), "close-checklist.md");
}

#[test]
fn paths_normalize_segment_by_segment() {
    assert_eq!(kebab_path("specs/Launch Plan.md"), "specs/launch-plan.md");
    assert_eq!(kebab_path("/Docs//A B/"), "docs/a-b");
}
