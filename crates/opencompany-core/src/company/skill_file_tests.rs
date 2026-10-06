use super::*;

const WEB_RESEARCH: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../companies/_globals/skills/web-research/SKILL.md"
));
const WEEKLY_REPORT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../companies/_globals/skills/weekly-report/SKILL.md"
));

#[test]
fn parses_both_shipped_repo_skills() {
    let web = parse_skill_md("web-research", WEB_RESEARCH).expect("web-research is valid");
    assert_eq!(web.slug, "web-research");
    assert_eq!(web.name, "Web Research");
    assert!(web.description.starts_with("Answer a question"));
    assert_eq!(web.category, None);
    // Body is preserved verbatim, including its heading.
    assert!(web.body.contains("# Web Research"));
    assert!(web.body.contains("## When to use"));

    let weekly = parse_skill_md("weekly-report", WEEKLY_REPORT).expect("weekly-report is valid");
    assert_eq!(weekly.name, "Weekly Report");
}

#[test]
fn missing_frontmatter_is_a_parse_error() {
    let err = parse_skill_md("demo", "# No frontmatter here\n").unwrap_err();
    assert_eq!(err.code(), "data_parse");
}

#[test]
fn unterminated_frontmatter_is_a_parse_error() {
    let err = parse_skill_md("demo", "---\nname: Demo\n").unwrap_err();
    assert_eq!(err.code(), "data_parse");
}

#[test]
fn missing_required_keys_is_a_validation_error() {
    let err = parse_skill_md("demo", "---\ncategory: research\n---\nbody\n").unwrap_err();
    assert_eq!(err.code(), "data_invalid");
    let message = err.to_string();
    assert!(message.contains("`name`"), "{message}");
    assert!(message.contains("`description`"), "{message}");
}

#[test]
fn every_baseline_skill_carries_a_version() {
    // The baseline's skills are installed in every company, so an install
    // of one must be pinnable to the revision it was made from.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../companies")
        .join(BASELINE_BUNDLE)
        .join("skills");
    let docs = load_dir_skills(&dir).expect("the baseline skills parse");
    assert!(!docs.is_empty(), "the baseline ships skills");
    for doc in &docs {
        assert!(
            doc.version.is_some(),
            "baseline skill `{}` is missing `version` in its frontmatter",
            doc.slug
        );
    }
}

#[test]
fn catalog_is_the_union_of_bundles_with_the_baseline_winning_a_slug() {
    let root = tempfile::tempdir().expect("tempdir");
    let write = |bundle: &str, slug: &str, name: &str| {
        let dir = root.path().join(bundle).join("skills").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: d\n---\n# {name}\n"),
        )
        .unwrap();
    };
    // `zeta` sorts after `alpha` but before nothing else; `_globals` must
    // still win despite `_` sorting before letters only by accident.
    write("zeta", "shared", "Zeta Shared");
    write("alpha", "shared", "Alpha Shared");
    write("_globals", "shared", "Baseline Shared");
    write("alpha", "only-alpha", "Only Alpha");
    // A bundle with no skills/ at all, and a stray file, contribute nothing.
    std::fs::create_dir_all(root.path().join("bare")).unwrap();
    std::fs::write(root.path().join("README.md"), "x").unwrap();

    let docs = load_catalog_skills(root.path()).expect("catalog loads");
    let slugs: Vec<&str> = docs.iter().map(|d| d.slug.as_str()).collect();
    assert_eq!(slugs, ["only-alpha", "shared"]);
    assert_eq!(docs[1].name, "Baseline Shared");

    // Without a baseline copy, bundle name order decides.
    std::fs::remove_dir_all(root.path().join("_globals")).unwrap();
    let docs = load_catalog_skills(root.path()).expect("catalog loads");
    assert_eq!(docs[1].name, "Alpha Shared");

    // Nothing there at all is an empty catalog, not an error.
    assert!(
        load_catalog_skills(&root.path().join("missing"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn every_shipped_bundle_skill_renders_to_its_own_source() {
    // Installing a registry skill persists `render_skill_md` output, so for
    // an install to be a faithful copy the committed file must already be in
    // canonical form. Pinning that here turns the one lossy case — an
    // unknown frontmatter key, which the parser tolerates but the renderer
    // drops — into a CI failure instead of a silent loss at install time.
    // Every bundle's skills are in the registry, so every bundle is walked.
    let companies = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../companies");
    let mut walked = 0;
    for bundle in std::fs::read_dir(&companies).expect("companies/ is readable") {
        let dir = bundle.expect("entry").path().join("skills");
        for doc in load_dir_skills(&dir).expect("the bundle's skills parse") {
            walked += 1;
            let file = dir.join(&doc.slug).join("SKILL.md");
            let src = std::fs::read_to_string(&file).expect("readable");
            assert_eq!(
                render_skill_md(&doc),
                src,
                "{} is not in canonical frontmatter form, so installing it \
             would not persist it verbatim. Frontmatter must be exactly \
             `name`, `description`, optional `category`, optional `version`, in that \
             order — any other key is dropped by the renderer. Either drop the extra \
             key or teach `SkillDoc`/`render_skill_md` about it.",
                file.display()
            );
        }
    }
    assert!(
        walked > 14,
        "sanity: the bundles ship skills ({walked} walked)"
    );
}

#[test]
fn parse_errors_name_the_slug_and_its_document() {
    match parse_skill_md("demo", "---\nname:\n---\nbody\n").unwrap_err() {
        OpenCompanyError::DataInvalid { path, problems } => {
            assert_eq!(path, PathBuf::from("demo/SKILL.md"));
            assert_eq!(
                problems,
                [
                    "skill `demo` is missing a `name` in its frontmatter.",
                    "skill `demo` is missing a `description` in its frontmatter.",
                ]
            );
        }
        other => panic!("expected DataInvalid, got {other:?}"),
    }
    match parse_skill_md("demo", "# no fence\n").unwrap_err() {
        OpenCompanyError::DataParse { path, message } => {
            assert_eq!(path, PathBuf::from("demo/SKILL.md"));
            assert_eq!(
                message,
                "missing a `---` frontmatter block at the top of the file."
            );
        }
        other => panic!("expected DataParse, got {other:?}"),
    }
}

#[test]
fn a_doc_converts_to_and_from_its_flat_form() {
    let src = "---\nname: Demo\ndescription: D\ncategory: c\nversion: 1\nowner: eve\n---\nbody\n";
    let doc = parse_skill_md("demo", src).expect("valid");
    assert_eq!(doc.extra_frontmatter, ["owner: eve"]);
    assert_eq!(SkillDoc::from_flat("demo", doc.to_flat()), doc);
    let scan = doc.scan_document();
    assert_eq!(scan.name, "Demo");
    assert_eq!(scan.body, "body\n");
    assert_eq!(scan.extra_frontmatter, doc.extra_frontmatter.as_slice());
}
