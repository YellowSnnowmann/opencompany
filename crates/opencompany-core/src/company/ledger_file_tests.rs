//! Unit tests for the ledger declaration reader.

use super::*;

/// A complete, well-formed declaration, as a template author writes one.
const PIPELINE: &str = r#"
title = "Deal pipeline"
purpose = "Every deal in flight, what stage it is at, and why a lost one was lost."
written_by = "`record_entry` to open or move a deal, `close_entry` to win or lose it"

[[field]]
name = "deal"
role = "id"
description = "Short slug for the deal, e.g. acme-renewal."
required = true

[[field]]
name = "summary"
role = "title"

[[field]]
name = "stage"
role = "status"

[[status]]
name = "qualifying"

[[status]]
name = "won"
closed = true
needs_reason = true

[[section]]
heading = "In flight"
statuses = ["qualifying"]
order = "recent"
cap = 20
"#;

fn reader(
    files: Vec<(&'static str, &'static str)>,
) -> impl Fn(&str) -> std::result::Result<String, std::io::ErrorKind> {
    move |name: &str| {
        files
            .iter()
            .find(|(file, _)| *file == name)
            .map(|(_, body)| (*body).to_string())
            .ok_or(std::io::ErrorKind::NotFound)
    }
}

#[test]
fn a_declaration_parses_into_the_same_spec_a_run_time_declaration_would() {
    let spec = parse_ledger_file("pipeline.toml", PIPELINE).expect("parses");
    assert_eq!(spec.slug, "pipeline");
    assert_eq!(spec.title, "Deal pipeline");
    assert!(!spec.builtin, "a bundle never declares a built-in");
    assert_eq!(spec.source, crate::ledger::LedgerSource::Events);
    // Not authored, derived: an author who never writes the folder convention
    // cannot get it wrong.
    assert_eq!(spec.derived, "derived/pipeline.md");
    assert_eq!(spec.id_field().expect("an id field").name, "deal");
    assert_eq!(spec.closing_statuses(), vec!["won"]);
    assert_eq!(spec.default_order(), crate::ledger::Order::Recent);
}

#[test]
fn the_filename_is_the_slug_and_a_body_key_that_disagrees_is_refused() {
    let problems = parse_ledger_file("pipeline.toml", &format!("slug = \"deals\"\n{PIPELINE}"))
        .expect_err("refused");
    assert!(
        problems[0].contains("a ledger's slug is its filename"),
        "{problems:?}"
    );

    // Agreeing is fine — the file may read as a complete declaration.
    let spec = parse_ledger_file("pipeline.toml", &format!("slug = \"pipeline\"\n{PIPELINE}"))
        .expect("parses");
    assert_eq!(spec.slug, "pipeline");
}

/// The bound every declaration is held to lives in `LedgerSpec::normalize`, and
/// this reader must not be a second door around it.
#[test]
fn a_declaration_with_no_id_field_is_refused_by_the_shared_validator() {
    let src = "title = \"Broken\"\n[[field]]\nname = \"summary\"\nrole = \"title\"\n\
               [[status]]\nname = \"open\"\n";
    let problems = parse_ledger_file("broken.toml", src).expect_err("refused");
    assert!(
        problems[0].contains("exactly one field with role `id`"),
        "{problems:?}"
    );
}

#[test]
fn an_unknown_key_is_refused_rather_than_silently_dropped() {
    let problems = parse_ledger_file("pipeline.toml", &format!("owner = \"sales\"\n{PIPELINE}"))
        .expect_err("refused");
    assert!(problems[0].contains("not valid TOML"), "{problems:?}");
}

#[test]
fn a_bundle_may_not_shadow_a_built_in() {
    let names = vec!["tasks.toml".to_string()];
    let (specs, problems) = parse_ledgers(&names, &reader(vec![("tasks.toml", PIPELINE)]));
    assert!(specs.is_empty());
    assert!(
        problems[0].contains("collides with the built-in `tasks`"),
        "{problems:?}"
    );
}

#[test]
fn two_declarations_may_not_claim_one_derived_file() {
    let other = format!("derived = \"derived/pipeline.md\"\n{PIPELINE}");
    let names = vec!["pipeline.toml".to_string(), "renewals.toml".to_string()];
    let files = vec![
        ("pipeline.toml", PIPELINE),
        ("renewals.toml", Box::leak(other.into_boxed_str()) as &str),
    ];
    let (specs, problems) = parse_ledgers(&names, &reader(files));
    assert_eq!(specs.len(), 1, "the first one keeps the file");
    assert!(
        problems[0].contains("two writers on one derived file"),
        "{problems:?}"
    );
}

/// One malformed declaration costs itself and nothing else — the property the
/// global baseline relies on.
#[test]
fn a_malformed_declaration_does_not_cost_the_ones_beside_it() {
    let names = vec!["broken.toml".to_string(), "pipeline.toml".to_string()];
    let (specs, problems) = parse_ledgers(
        &names,
        &reader(vec![
            ("broken.toml", "title = \"Broken\"\nnope ="),
            ("pipeline.toml", PIPELINE),
        ]),
    );
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].slug, "pipeline");
    assert_eq!(problems.len(), 1);
}

#[test]
fn a_bundle_may_not_declare_more_than_the_cap() {
    let bodies: Vec<(String, String)> = (0..MAX_DECLARED + 2)
        .map(|index| {
            (
                format!("axis-{index:02}.toml"),
                PIPELINE.replace("Deal pipeline", &format!("Axis {index}")),
            )
        })
        .collect();
    let names: Vec<String> = bodies.iter().map(|(name, _)| name.clone()).collect();
    let (specs, problems) = parse_ledgers(&names, &|rel| {
        bodies
            .iter()
            .find(|(name, _)| name == rel)
            .map(|(_, body)| body.clone())
            .ok_or(std::io::ErrorKind::NotFound)
    });
    assert_eq!(specs.len(), MAX_DECLARED);
    assert_eq!(problems.len(), 2);
    assert!(problems[0].contains("past the"), "{problems:?}");
}

#[test]
fn a_bundle_with_no_ledgers_directory_loads_nothing_and_reports_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(!has_ledger_files(dir.path()));
    assert!(
        load_dir_ledgers(dir.path())
            .expect("no directory is not a problem")
            .is_empty()
    );
}

#[test]
fn a_directory_of_declarations_loads_sorted_by_filename() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledgers = dir.path().join(LEDGERS_DIR);
    std::fs::create_dir_all(&ledgers).expect("mkdir");
    std::fs::write(ledgers.join("pipeline.toml"), PIPELINE).expect("write");
    std::fs::write(
        ledgers.join("accounts.toml"),
        PIPELINE.replace("Deal pipeline", "Accounts"),
    )
    .expect("write");

    assert!(has_ledger_files(dir.path()));
    let specs = load_dir_ledgers(dir.path()).expect("loads");
    let slugs: Vec<&str> = specs.iter().map(|spec| spec.slug.as_str()).collect();
    assert_eq!(slugs, vec!["accounts", "pipeline"]);
}

#[test]
fn one_bad_file_fails_the_whole_company_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledgers = dir.path().join(LEDGERS_DIR);
    std::fs::create_dir_all(&ledgers).expect("mkdir");
    std::fs::write(ledgers.join("pipeline.toml"), PIPELINE).expect("write");
    std::fs::write(ledgers.join("broken.toml"), "title = \"Broken\"\n").expect("write");

    let err = load_dir_ledgers(dir.path()).expect_err("refused");
    assert!(
        format!("{err}").contains("broken.toml"),
        "the message names the file: {err}"
    );
}

#[test]
fn only_immediate_toml_files_are_declarations() {
    let names = embedded_ledger_names(&[
        ("pipeline.toml", ""),
        ("notes/reference.toml", ""),
        ("README.md", ""),
        ("accounts.toml", ""),
    ]);
    assert_eq!(names, vec!["accounts.toml", "pipeline.toml"]);
}
