use std::path::Path;

use super::*;

fn seed(companies: &Path, bundle: &str, slug: &str) {
    let dir = companies.join(bundle).join("skills").join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {slug}\ndescription: {slug} from {bundle}\n---\n# {slug}\n"),
    )
    .unwrap();
}

#[test]
fn explicit_wins_over_env_and_packaged() {
    let library = for_host(
        Some(PathBuf::from("/explicit")),
        Some(OsString::from("/env")),
        Some(std::env::temp_dir()),
    );
    assert_eq!(
        library.origin(),
        LibraryOrigin::Explicit(PathBuf::from("/explicit"))
    );
}

#[test]
fn env_wins_over_packaged() {
    let library = for_host(
        None,
        Some(OsString::from("/env")),
        Some(std::env::temp_dir()),
    );
    assert_eq!(library.origin(), LibraryOrigin::Env(PathBuf::from("/env")));
}

#[test]
fn a_blank_env_counts_as_unset() {
    let packaged = tempfile::tempdir().unwrap();
    let library = for_host(
        None,
        Some(OsString::from("  ")),
        Some(packaged.path().to_path_buf()),
    );
    assert_eq!(
        library.origin(),
        LibraryOrigin::Packaged(packaged.path().to_path_buf())
    );
}

#[test]
fn a_packaged_copy_is_served_only_when_present() {
    let missing = tempfile::tempdir().unwrap().path().join("absent");
    let library = for_host(None, None, Some(missing));
    assert_eq!(library.origin(), LibraryOrigin::None);
    assert!(library.snapshot().unwrap().is_empty());
}

#[test]
fn nothing_named_serves_no_library() {
    let library = for_host(None, None, None);
    assert_eq!(library.origin(), LibraryOrigin::None);
    assert!(library.snapshot().unwrap().is_empty());
}

#[test]
fn a_named_directory_that_is_missing_fails_rather_than_serving_nothing() {
    let missing = tempfile::tempdir().unwrap().path().join("absent");
    for library in [
        for_host(Some(missing.clone()), None, None),
        for_host(None, Some(missing.clone().into_os_string()), None),
    ] {
        let err = library.snapshot().unwrap_err();
        assert_eq!(err.code(), "config_error", "{err}");
    }
}

#[test]
fn a_malformed_document_fails_the_load_as_configuration() {
    let companies = tempfile::tempdir().unwrap();
    let dir = companies.path().join("bundle/skills/broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "no frontmatter\n").unwrap();

    let err = DirLibrary::explicit(companies.path())
        .snapshot()
        .unwrap_err();
    assert_eq!(err.code(), "config_error", "{err}");
}

#[test]
fn a_directory_library_reads_every_bundle_and_caches_the_first_load() {
    let companies = tempfile::tempdir().unwrap();
    seed(companies.path(), "_globals", "shared");
    seed(companies.path(), "zeta", "shared");
    seed(companies.path(), "alpha", "only-alpha");
    let library = DirLibrary::packaged(companies.path());

    let first = library.snapshot().unwrap();
    let slugs: Vec<&str> = first.iter().map(|doc| doc.slug.as_str()).collect();
    assert_eq!(slugs, ["only-alpha", "shared"]);
    assert_eq!(
        first[1].description, "shared from _globals",
        "the baseline bundle keeps a slug another bundle also ships"
    );

    std::fs::remove_dir_all(companies.path().join("alpha")).unwrap();
    let second = library.snapshot().unwrap();
    assert!(
        Arc::ptr_eq(&first, &second),
        "a later read is the cached load"
    );
}

#[test]
fn the_shipped_companies_directory_loads_as_a_library() {
    let companies = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../companies");
    let docs = DirLibrary::explicit(companies).snapshot().unwrap();
    assert!(docs.len() > 50, "the shipped library serves its bundles");
    for doc in crate::globals::skills() {
        assert!(
            docs.iter().any(|served| served.slug == doc.slug),
            "the library serves the global `{}`",
            doc.slug
        );
    }
}
