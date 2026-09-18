use super::*;

/// The ids are the dispatch edge's, so they are pinned against a literal:
/// a rename here would silently stop `in_progress` dispatching.
#[test]
fn the_table_carries_the_boards_ids_in_board_order() {
    assert_eq!(
        ids(),
        [
            "todo",
            "planning",
            "in_progress",
            "paused",
            "in_review",
            "done"
        ]
    );
}

/// The three phases, pinned. This is the vocabulary every reader of the
/// board is shown, and the whole of it.
#[test]
fn there_are_exactly_three_phases_and_these_are_they() {
    assert_eq!(phase_ids(), ["pending", "working", "done"]);
    let labels: Vec<&str> = PHASES.iter().map(|phase| phase.label).collect();
    assert_eq!(labels, ["Pending", "Working", "Done"]);
}

/// Every stage names a phase that exists, and the four middle stages all
/// name the same one — which is the collapse, asserted rather than assumed.
#[test]
fn every_stage_files_under_a_real_phase() {
    for column in &COLUMNS {
        assert!(
            phase(column.phase).is_some(),
            "`{}` files under `{}`, which is not a phase",
            column.id,
            column.phase
        );
    }
    let working: Vec<&str> = COLUMNS
        .iter()
        .filter(|column| column.phase == PHASE_WORKING)
        .map(|column| column.id)
        .collect();
    assert_eq!(
        working,
        [
            COLUMN_PLANNING,
            COLUMN_IN_PROGRESS,
            COLUMN_PAUSED,
            COLUMN_IN_REVIEW
        ]
    );
}

/// A phase word resolves to a stage; a stage word does not resolve to
/// itself. That asymmetry is what lets the write boundary tell them apart.
#[test]
fn a_phase_resolves_to_the_stage_a_drop_writes() {
    assert_eq!(entry_stage(PHASE_PENDING), Some(COLUMN_TODO));
    assert_eq!(entry_stage(PHASE_WORKING), Some(COLUMN_IN_PROGRESS));
    assert_eq!(entry_stage(PHASE_DONE), Some(COLUMN_DONE));
    assert_eq!(entry_stage(COLUMN_IN_REVIEW), None);
    assert_eq!(entry_stage(""), None);
}

/// An unknown stage reports itself rather than being filed as pending.
#[test]
fn an_unknown_stage_is_its_own_phase() {
    assert_eq!(phase_of(COLUMN_IN_REVIEW), PHASE_WORKING);
    assert_eq!(phase_of("teleported"), "teleported");
}

/// The stage labels, pinned. These are what a card's badge and an exported
/// record read, now that they are no longer column headings.
#[test]
fn the_labels_are_the_ones_every_surface_renders() {
    let labels: Vec<&str> = COLUMNS.iter().map(|column| column.label).collect();
    assert_eq!(
        labels,
        [
            "To-do",
            "Planning",
            "In progress",
            "Paused",
            "In review",
            "Done"
        ]
    );
}

#[test]
fn every_column_has_a_label_that_is_not_its_wire_word() {
    for column in &COLUMNS {
        assert!(!column.label.is_empty(), "{} has no label", column.id);
        assert!(
            !column.label.contains('_'),
            "{} still reads as a wire word: {}",
            column.id,
            column.label
        );
    }
}

/// Done is the only finished phase, and it is reached only by a person's
/// verdict. Calling review or paused closed would make "what is still
/// outstanding" answer wrong on every surface at once.
#[test]
fn exactly_one_phase_is_closed_and_it_is_done() {
    let closed: Vec<&str> = PHASES
        .iter()
        .filter(|phase| phase.closed)
        .map(|phase| phase.id)
        .collect();
    assert_eq!(closed, [PHASE_DONE]);
}

#[test]
fn every_phase_says_what_it_is_for() {
    for phase in &PHASES {
        assert!(!phase.blurb.is_empty(), "{} has no blurb", phase.id);
        assert!(!phase.label.is_empty(), "{} has no label", phase.id);
    }
}

#[test]
fn a_column_is_found_by_id_and_an_invented_one_is_not() {
    assert_eq!(column("done").map(|held| held.label), Some("Done"));
    assert!(column("in-progress").is_none());
    assert!(column("").is_none());
}

#[test]
fn a_phase_is_found_by_id_and_an_invented_one_is_not() {
    assert_eq!(phase("working").map(|held| held.label), Some("Working"));
    assert!(phase("in_progress").is_none());
    assert!(phase("").is_none());
}
