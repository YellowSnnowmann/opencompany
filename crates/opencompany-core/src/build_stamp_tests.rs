use super::*;

/// A full object id, as `git rev-parse HEAD` or `GITHUB_SHA` would give it.
const FULL_SHA: &str = "d31e532f7c8a4b19e0f6c25a1d8b7e3049fa62c1";

fn no_git() -> Option<String> {
    None
}

fn head(sha: &str) -> impl FnOnce() -> Option<String> {
    let sha = sha.to_string();
    move || Some(sha)
}

fn clean() -> bool {
    false
}

fn dirty() -> bool {
    true
}

#[test]
fn an_injected_commit_outranks_git() {
    // The escape hatch has to actually win, or a builder that knows better
    // than this script has no way to say so.
    let stamp = resolve_build_commit(
        Some("abc123abc123".into()),
        Some(FULL_SHA.into()),
        head("999999999999"),
        dirty,
    );
    assert_eq!(stamp, "abc123abc123");
}

#[test]
fn git_outranks_github_sha() {
    // `GITHUB_SHA` describes the ref CI meant to check out. When a
    // repository is present it is git, not CI, that knows what was built.
    let stamp = resolve_build_commit(None, Some(FULL_SHA.into()), head("999999999999"), clean);
    assert_eq!(stamp, "999999999999");
}

#[test]
fn a_dirty_tree_says_so() {
    let stamp = resolve_build_commit(None, None, head("999999999999"), dirty);
    assert_eq!(stamp, "999999999999-dirty");
}

#[test]
fn an_injected_commit_is_never_marked_dirty() {
    // The suffix is a claim about the tree the stamp names. An injected
    // value names someone else's tree, so measuring this one would be a
    // statement about the wrong thing.
    let stamp = resolve_build_commit(Some("abc123abc123".into()), None, head(FULL_SHA), dirty);
    assert_eq!(stamp, "abc123abc123");
}

/// The tarball / vendored-crate / no-`.git`-in-the-Docker-context case.
#[test]
fn github_sha_answers_when_there_is_no_git() {
    let stamp = resolve_build_commit(None, Some(FULL_SHA.into()), no_git, clean);
    assert_eq!(stamp, "d31e532f7c8a");
}

/// The whole-point case: no git binary, no `.git`, no CI. The build still
/// has to produce a stamp rather than fail.
#[test]
fn no_source_at_all_degrades_to_unknown() {
    let stamp = resolve_build_commit(None, None, no_git, clean);
    assert_eq!(stamp, "unknown");
    assert!(!stamp.is_empty(), "an empty stamp would break `env!`");
}

#[test]
fn a_source_that_is_present_but_empty_is_treated_as_absent() {
    // An exported-but-blank `GITHUB_SHA` and a `git` that exits zero with
    // nothing on stdout both reach here. Neither may win over a source
    // that actually knows something.
    let stamp = resolve_build_commit(
        Some("   ".into()),
        Some(FULL_SHA.into()),
        || Some("\n".into()),
        clean,
    );
    assert_eq!(stamp, "d31e532f7c8a");
}

#[test]
fn a_full_object_id_is_shortened_to_twelve() {
    assert_eq!(sanitize_commit(FULL_SHA).as_deref(), Some("d31e532f7c8a"));
}

#[test]
fn a_short_id_is_left_alone() {
    assert_eq!(sanitize_commit("d31e532f").as_deref(), Some("d31e532f"));
}

#[test]
fn a_non_hex_tag_is_not_shortened() {
    // Only an object id gets truncated. A branch or tag name someone
    // injected stays legible.
    assert_eq!(
        sanitize_commit("release-2026-08-25").as_deref(),
        Some("release-2026-08-25")
    );
}

#[test]
fn a_newline_cannot_forge_a_build_script_directive() {
    // The stamp is interpolated into `cargo:rustc-env=…`. A value carrying
    // a newline would otherwise inject a second directive into cargo's
    // stdout protocol.
    let stamp = sanitize_commit("abc123\ncargo:rustc-link-lib=evil").expect("a stamp");
    assert!(!stamp.contains('\n'), "{stamp:?} still spans lines");
    assert!(!stamp.contains(':'), "{stamp:?} kept a directive separator");
}

#[test]
fn a_stamp_is_bounded() {
    let stamp = sanitize_commit(&"z".repeat(500)).expect("a stamp");
    assert_eq!(stamp.len(), MAX_COMMIT_LEN);
}

#[test]
fn nothing_at_all_sanitizes_to_nothing() {
    for raw in ["", "   ", "\n\t", "///", "→→→"] {
        assert_eq!(sanitize_commit(raw), None, "{raw:?} named a commit");
    }
}

/// The stamp this very binary was compiled with. Not a test of the
/// resolver but of the wiring around it: an `env!` that never ran, or a
/// `build.rs` that emitted a malformed line, shows up here and nowhere
/// else.
#[test]
fn the_compiled_in_stamp_is_well_formed() {
    let stamp = crate::BUILD_COMMIT;
    assert!(!stamp.is_empty(), "the stamp must never be empty");
    assert!(
        stamp.len() <= MAX_COMMIT_LEN + "-dirty".len(),
        "{stamp:?} is longer than any source can produce"
    );
    assert!(
        stamp
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "{stamp:?} carries characters the sanitizer should have dropped"
    );
}
