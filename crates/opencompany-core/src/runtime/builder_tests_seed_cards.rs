use super::tests_core::*;

fn bundle(body: &str) -> tempfile::TempDir {
    let dir = tmp_home("opencompany-seed-cards-");
    std::fs::write(dir.path().join("tasks.toml"), body).expect("write tasks.toml");
    dir
}

fn resolve(disable: &[&str], dir: Option<&std::path::Path>) -> Vec<String> {
    let disable: Vec<String> = disable.iter().map(|d| (*d).to_string()).collect();
    resolve_seed_cards(&disable, dir, |err| {
        panic!("unexpected load failure: {err}")
    })
    .into_iter()
    .map(|seed| seed.id)
    .collect()
}

/// A company with no bundle still gets the baseline: a
/// platform-provisioned tenant carries no `companies/<name>` directory
/// and is still a company somebody has to start using.
#[test]
fn a_company_with_no_bundle_gets_the_baseline() {
    let ids = resolve(&[], None);
    assert!(!ids.is_empty(), "the baseline must seed something");
    let baseline: Vec<String> = crate::globals::tasks()
        .iter()
        .map(|seed| seed.id.clone())
        .collect();
    assert_eq!(ids, baseline);
}

/// The bundle's cards land after the baseline's, so the setup work every
/// company shares is read first.
#[test]
fn a_bundle_appends_its_own_cards_after_the_baseline() {
    let dir = bundle("[[task]]\nid = \"set-up-the-thing\"\ntitle = \"Set up the thing\"\n");
    let ids = resolve(&[], Some(dir.path()));
    assert_eq!(
        ids.last().map(String::as_str),
        Some("set-up-the-thing"),
        "{ids:?}"
    );
    assert_eq!(ids.len(), crate::globals::tasks().len() + 1);
}

/// A bundle card of the same id **replaces** the baseline's rather than
/// duplicating it — the precedence every other global surface uses.
#[test]
fn a_bundle_card_supersedes_the_baseline_card_of_the_same_id() {
    let shared = &crate::globals::tasks()[0].id;
    let dir = bundle(&format!(
        "[[task]]\nid = \"{shared}\"\ntitle = \"Ours instead\"\n"
    ));
    let seeds = resolve_seed_cards(&[], Some(dir.path()), |err| panic!("{err}"));
    let matching: Vec<&crate::company::TaskSeed> =
        seeds.iter().filter(|s| &s.id == shared).collect();
    assert_eq!(matching.len(), 1, "the id must not appear twice");
    assert_eq!(matching[0].title, "Ours instead");
    assert_eq!(seeds.len(), crate::globals::tasks().len());
}

/// `[globals].disable` drops a baseline card, using the same
/// `<kind>:<id>` vocabulary that already drops a baseline agent,
/// workflow, skill or ledger.
#[test]
fn disable_drops_one_baseline_card_and_keeps_the_rest() {
    let dropped = crate::globals::tasks()[0].id.clone();
    let ids = resolve(&[&format!("task:{dropped}")], None);
    assert!(!ids.contains(&dropped), "{ids:?}");
    assert_eq!(ids.len(), crate::globals::tasks().len() - 1);
}

/// A bundle file that will not load costs its own cards and nothing
/// else. Refusing the boot would strand a hand-edited bundle where the
/// console that could fix it is unreachable.
#[test]
fn a_malformed_bundle_file_still_leaves_the_baseline() {
    let dir = bundle("[[task]\nid = ");
    let mut reported = None;
    let seeds = resolve_seed_cards(&[], Some(dir.path()), |err| {
        reported = Some(err.to_string());
    });
    assert!(
        reported.is_some(),
        "the failure must be reported, not swallowed"
    );
    assert_eq!(seeds.len(), crate::globals::tasks().len());
}

/// Every seeded card is To-do, whatever it came from. `in_progress`
/// dispatches a run and `planning` bills a pass, so this is the property
/// that keeps a freshly provisioned company from spending money at boot.
#[test]
fn every_seeded_card_is_todo() {
    let dir = bundle("[[task]]\nid = \"ours\"\ntitle = \"Ours\"\n");
    for seed in resolve_seed_cards(&[], Some(dir.path()), |err| panic!("{err}")) {
        let card = seed.to_record(0);
        assert_eq!(card.column, crate::ports::tasks::COLUMN_TODO, "{}", seed.id);
    }
}

/// Seeding is opt-in. `tests/one_card_per_message.rs` asserts exact board
/// sizes against a company built straight from this builder, so a
/// baseline that arrived unasked would quietly turn those assertions into
/// statements about the baseline.
#[test]
fn task_seeding_is_off_unless_a_caller_asks_for_it() {
    let home = tmp_home("opencompany-seed-flag-");
    let manifest: CompanyManifest =
        toml::from_str("[company]\nname = \"Acme\"\n").expect("manifest");
    let builder = RuntimeBuilder::new(home.path().to_path_buf(), manifest);
    assert!(!builder.seed_tasks, "board seeding must default to off");
    assert!(builder.with_task_seeding(true).seed_tasks);
}
