//! What may join the registry, and what the built-ins promise.

use serde_json::json;

use super::*;
use crate::ledger::spec::parse;

fn declared(slug: &str) -> LedgerSpec {
    parse(
        &json!({
            "slug": slug,
            "title": slug,
            "derived": format!("derived/{slug}.md"),
            "fields": [
                { "name": "id", "role": "id" },
                { "name": "what", "role": "title" },
                { "name": "status", "role": "status" }
            ],
            "statuses": [{ "name": "open" }]
        }),
        false,
    )
    .expect("valid")
}

/// Every built-in parses. A malformed one would be this crate's bug, and it
/// would otherwise surface as a ledger quietly missing from every listing.
#[test]
fn builtins_are_valid() {
    let (specs, faults) = builtins();
    assert!(faults.is_empty(), "{faults:?}");
    assert_eq!(
        specs
            .iter()
            .map(|spec| spec.slug.as_str())
            .collect::<Vec<_>>(),
        ["tasks", "goals", "decisions"]
    );
    for spec in &specs {
        assert!(spec.builtin);
        assert!(!spec.purpose.is_empty(), "`{}` has no purpose", spec.slug);
        assert!(
            !spec.written_by.is_empty(),
            "`{}` does not say how it is written",
            spec.slug
        );
        assert!(spec.derived.starts_with("derived/"), "{}", spec.derived);
    }
}

/// The board keeps its own store and its own dispatch edge, so it must not
/// advertise a write path that would refuse its caller a second time.
#[test]
fn the_board_is_native_and_says_it_is_not_written_with_record_entry() {
    let registry = Registry::build([]);
    let tasks = registry.find("tasks").expect("built in");
    assert_eq!(tasks.source, LedgerSource::Native);
    assert!(
        !tasks.written_by.contains("`record_entry` to add"),
        "{}",
        tasks.written_by
    );
    assert!(
        tasks.written_by.contains("spawn_task"),
        "{}",
        tasks.written_by
    );
}

/// The board's statuses are its three phases, and only Done ends a card's life.
/// A card in review or paused is stopped, not finished — and both read as
/// `working`, which is the whole of issue #1512.
#[test]
fn the_boards_statuses_are_its_three_phases() {
    let registry = Registry::build([]);
    let tasks = registry.find("tasks").expect("built in");
    let names: Vec<&str> = tasks.statuses.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        crate::ledger::board::phase_ids().to_vec(),
        "the ledger's statuses must be the board's phases, in board order"
    );
    assert_eq!(tasks.closing_statuses(), [crate::ledger::board::PHASE_DONE]);
}

/// The cap that makes this issue's point on every ledger at once: nothing a
/// company reads or writes is asked to hold more than three states.
#[test]
fn no_built_in_ledger_declares_more_than_three_statuses() {
    let registry = Registry::build([]);
    for spec in registry.specs() {
        assert!(
            spec.statuses.len() <= 3,
            "`{}` declares {} statuses; three is the ceiling — a fourth is a \
             question being answered in the wrong place",
            spec.slug,
            spec.statuses.len()
        );
    }
}

/// The point of the table: one edit adds a phase to the ledger's statuses, the
/// rendered file's sections and the console's labels. This asserts the
/// host-side halves agree, item for item — the check the hand-maintained lists
/// could never make.
#[test]
fn the_board_ledger_is_built_from_the_phase_table() {
    let registry = Registry::build([]);
    let tasks = registry.find("tasks").expect("built in");
    assert_eq!(tasks.statuses.len(), crate::ledger::board::PHASES.len());
    for (status, phase) in tasks.statuses.iter().zip(crate::ledger::board::PHASES) {
        assert_eq!(status.name, phase.id);
        assert_eq!(status.closed, phase.closed);
        // The label is what let the console delete its own copy of this table.
        assert_eq!(status.label, phase.label, "`{}` lost its label", phase.id);
        assert!(
            !status.label.is_empty(),
            "`{}` reaches the console as a wire word",
            phase.id
        );
    }
}

/// Every stage renders somewhere in `derived/tasks.md` — through its phase. A
/// stage whose phase no section carried would leave its cards out of the file
/// entirely: the silent disappearance the column vocabulary exists to prevent,
/// arrived at from the file's side.
#[test]
fn every_stage_lands_in_a_rendered_section() {
    let registry = Registry::build([]);
    let tasks = registry.find("tasks").expect("built in");
    for column in crate::ledger::board::COLUMNS {
        assert!(
            tasks
                .sections
                .iter()
                .any(|section| section.statuses.iter().any(|name| name == column.phase)),
            "`{}` renders in no section of the file",
            column.id
        );
    }
    // And every section says what it is for, since a heading alone tells a
    // reader nothing about which of two similar lists they are looking at.
    for section in &tasks.sections {
        assert!(
            !section.blurb.is_empty(),
            "`{}` has no blurb",
            section.heading
        );
    }
}

/// The archive is the one bounded section, and it must stay bounded: unbounded,
/// "recently done" becomes the largest thing in the file and is re-read on every
/// turn that carries it.
#[test]
fn the_boards_archive_is_the_one_capped_section() {
    let registry = Registry::build([]);
    let tasks = registry.find("tasks").expect("built in");
    let done = tasks
        .sections
        .iter()
        .find(|section| {
            section
                .statuses
                .iter()
                .any(|name| name == crate::ports::tasks::COLUMN_DONE)
        })
        .expect("the archive section exists");
    assert_eq!(done.cap, 5);
    for section in tasks
        .sections
        .iter()
        .filter(|held| held.heading != done.heading)
    {
        assert_eq!(section.cap, crate::ledger::budget::MAX_LISTED);
    }
}

#[test]
fn a_declared_ledger_joins_the_built_ins() {
    let registry = Registry::build([declared("risks")]);
    assert!(registry.faults().is_empty(), "{:?}", registry.faults());
    assert_eq!(registry.slugs(), ["tasks", "goals", "decisions", "risks"]);
    assert!(!registry.find("risks").expect("declared").builtin);
}

#[test]
fn a_declared_ledger_may_not_shadow_a_built_in() {
    let mut clash = declared("risks");
    clash.slug = "tasks".to_string();
    let registry = Registry::build([clash.clone()]);
    let faults = registry.faults().join("\n");
    assert!(faults.contains("built-in"), "{faults}");
    // The board itself is untouched — a bad declaration must not cost a company
    // the ledger it already had.
    assert_eq!(registry.find("tasks").expect("still there").title, "Tasks");
    assert!(Registry::build([]).admits(&clash).is_err());
}

#[test]
fn two_ledgers_may_not_write_one_derived_file() {
    let mut clash = declared("risks");
    clash.derived = "derived/goals.md".to_string();
    let registry = Registry::build([clash.clone()]);
    let faults = registry.faults().join("\n");
    assert!(faults.contains("goals"), "{faults}");
    assert!(faults.contains("disappears"), "{faults}");
    assert!(Registry::build([]).admits(&clash).is_err());
}

/// `native` is for ledgers this runtime renders in Rust. A company that could
/// declare one would have a ledger the engine cannot read and nothing else
/// writes.
#[test]
fn a_company_may_not_declare_a_native_ledger() {
    let mut spec = declared("risks");
    spec.source = LedgerSource::Native;
    let error = Registry::build([]).admits(&spec).expect_err("refused");
    assert!(format!("{error}").contains("events"));
}

#[test]
fn the_declared_count_is_capped_and_says_what_it_dropped() {
    let specs: Vec<LedgerSpec> = (0..MAX_DECLARED + 3)
        .map(|n| declared(&format!("axis-{n}")))
        .collect();
    let registry = Registry::build(specs);
    assert_eq!(
        registry.specs().iter().filter(|spec| !spec.builtin).count(),
        MAX_DECLARED
    );
    assert_eq!(registry.faults().len(), 3, "{:?}", registry.faults());
    assert!(registry.faults()[0].contains("cap"));
}

/// A company that wrote one bad declaration must still reach every good one.
#[test]
fn one_bad_declaration_does_not_cost_the_others() {
    let mut broken = declared("broken");
    broken.statuses.clear();
    let registry = Registry::build([broken, declared("risks")]);
    assert_eq!(registry.faults().len(), 1);
    assert!(registry.find("risks").is_some());
    assert!(registry.find("tasks").is_some());
    assert!(registry.find("broken").is_none());
}

/// The discovery path a model actually follows: guess, then learn the real
/// names from the failure, in one turn.
#[test]
fn an_unknown_slug_comes_back_with_the_real_ones() {
    let registry = Registry::build([]);
    let error = registry.require("taks").expect_err("unknown");
    let message = format!("{error}");
    assert!(message.contains("taks"), "{message}");
    assert!(message.contains("tasks"), "{message}");
    assert!(message.contains("goals"), "{message}");
}

#[test]
fn the_registry_names_the_owner_of_a_derived_file() {
    let registry = Registry::build([declared("risks")]);
    assert_eq!(
        registry
            .owner_of_derived("derived/risks.md")
            .map(|s| s.slug.as_str()),
        Some("risks")
    );
    assert_eq!(
        registry
            .owner_of_derived("/derived/tasks.md")
            .map(|s| s.slug.as_str()),
        Some("tasks")
    );
    assert!(registry.owner_of_derived("derived/NOBODY.md").is_none());
}

/// Every closing status on a built-in demands a reason. A row that closed
/// without one is worth nothing to whoever reads it next, and these three are
/// exactly the ledgers read for *have we already ruled this out*.
#[test]
fn a_declared_ledgers_closing_statuses_demand_a_reason() {
    let registry = Registry::build([]);
    for slug in ["goals", "decisions"] {
        let spec = registry.find(slug).expect("built in");
        for status in spec.statuses.iter().filter(|status| status.closed) {
            assert!(
                status.needs_reason,
                "`{slug}`'s `{}` closes without demanding a reason",
                status.name
            );
        }
    }
}

/// Every status the built-ins retired in issue #1512 still resolves, and
/// resolves to a status that renders. A row recorded last quarter as `at_risk`
/// or `superseded` is a row somebody wrote; a narrowed vocabulary that dropped
/// it on the floor would take it out of the rendered file with nothing saying
/// so — the silent disappearance the derived file exists to prevent, reached
/// from the declaration's side.
#[test]
fn every_retired_status_still_resolves_to_one_that_renders() {
    let registry = Registry::build([]);
    for (slug, retired, lands_on) in [
        ("goals", "proposed", "active"),
        ("goals", "at_risk", "active"),
        ("goals", "missed", "dropped"),
        ("decisions", "superseded", "retired"),
        ("decisions", "reversed", "retired"),
    ] {
        let spec = registry.find(slug).expect("built in");
        assert_eq!(
            spec.canonical_status(retired),
            lands_on,
            "`{slug}`'s `{retired}` resolves nowhere"
        );
        assert!(
            spec.sections
                .iter()
                .any(|section| section.statuses.iter().any(|name| name == lands_on)),
            "`{slug}`'s `{lands_on}` renders in no section, so `{retired}` still vanishes"
        );
        // Healed on read, refused on write: the client learns the surviving
        // vocabulary once rather than being kept on the retired one.
        assert!(spec.knows_status(retired), "{slug}/{retired}");
        assert!(!spec.declares_status(retired), "{slug}/{retired}");
    }
}
