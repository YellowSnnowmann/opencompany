use super::*;

#[test]
fn the_whole_folder_is_guarded_not_the_files_in_it() {
    assert!(is_derived_path("derived/goals.md"));
    assert!(is_derived_path("/derived/goals.md"));
    assert!(is_derived_path("derived"));
    // A ledger declared next week renders a file no guard has heard of.
    assert!(is_derived_path("derived/SOMETHING-NOBODY-HAS-DECLARED.md"));
    // Case is not a way around it.
    assert!(is_derived_path("Derived/goals.md"));
    assert!(!is_derived_path("notes/derived/goals.md"));
    assert!(!is_derived_path("derivedish/goals.md"));
    assert!(!is_derived_path("goals.md"));
}

/// The refusal has to name a remedy the caller can actually follow. The
/// board's is not `record_entry`, and saying it was would send them to a
/// tool that refuses them a second time.
#[test]
fn the_refusal_names_the_owning_ledgers_real_write_path() {
    let registry = Registry::build([]);
    let board = refusal(&registry, "derived/tasks.md");
    assert!(board.contains("tasks"), "{board}");
    assert!(
        !board.contains("`record_entry` to add"),
        "the board does not take record_entry: {board}"
    );
    let goals = refusal(&registry, "derived/goals.md");
    assert!(goals.contains("record_entry"), "{goals}");
}

#[test]
fn an_unclaimed_derived_file_is_still_refused() {
    let registry = Registry::build([]);
    let message = refusal(&registry, "derived/LEFTOVER.md");
    assert!(message.contains("list_ledgers"), "{message}");
    assert!(guard(&registry, "derived/LEFTOVER.md").is_err());
    assert!(guard(&registry, "notes/plan.md").is_ok());
}
