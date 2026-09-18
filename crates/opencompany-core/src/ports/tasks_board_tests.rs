use super::*;

/// Pins the **Rust** list's ids and their order against a literal, so a
/// reorder or a rename is a deliberate two-place edit rather than a
/// side effect.
///
/// It does **not** protect against drift from the console's mirror in
/// `frontend/src/lib/tasks-sample.ts` — a Rust test cannot see the TS list,
/// so a column added on one side and not the other keeps this green.
/// Closing that gap means generating one list from the other (a build step
/// this crate does not have, across a separate npm build), so for now the
/// mirror is maintained by hand and the two lists are reviewed together.
#[test]
fn columns_are_ordered_and_unique() {
    assert_eq!(
        BOARD_COLUMNS,
        [
            "todo",
            "planning",
            "in_progress",
            "paused",
            "in_review",
            "done"
        ]
    );
    let mut sorted = BOARD_COLUMNS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        BOARD_COLUMNS.len(),
        "column ids must be unique"
    );
}

#[test]
fn is_board_column_accepts_only_board_columns() {
    for column in BOARD_COLUMNS {
        assert!(is_board_column(column), "{column} is a board column");
    }
    // Issue #301: To-do is the one not-started column, Planning is new, and
    // the old Backlog pool is gone — a client still writing it gets a 400
    // rather than a card the board cannot render.
    assert!(is_board_column(COLUMN_TODO));
    assert!(is_board_column(COLUMN_PLANNING));
    assert!(!is_board_column(LEGACY_COLUMN_BACKLOG));
    // Near-misses a typo'd client might send.
    assert!(!is_board_column("to_do"));
    assert!(!is_board_column("To-do"));
    assert!(!is_board_column("inprogress"));
    assert!(!is_board_column(""));
}

/// Issue #352: every board column has a human label, and an unknown id
/// prints itself rather than a guess. The labels are the mirror of the
/// console's `TASK_COLUMNS`; asserting them against literals is what makes a
/// rename a deliberate two-place edit.
#[test]
fn every_column_has_a_human_label() {
    assert_eq!(column_label(COLUMN_TODO), "To-do");
    assert_eq!(column_label(COLUMN_PLANNING), "Planning");
    assert_eq!(column_label(COLUMN_IN_PROGRESS), "In progress");
    assert_eq!(column_label(COLUMN_PAUSED), "Paused");
    assert_eq!(column_label(COLUMN_IN_REVIEW), "In review");
    assert_eq!(column_label(COLUMN_DONE), "Done");
    for column in BOARD_COLUMNS {
        let label = column_label(column);
        assert!(!label.is_empty());
        assert!(
            !label.contains('_'),
            "{column} still reads as a wire word: {label}"
        );
    }
    assert_eq!(column_label("something_new"), "something_new");
}

/// Issue #337: the whole automatic edge, pinned status by status.
///
/// Every row is asserted against a literal rather than derived, because the
/// table *is* the decision — a mapping that computed itself from some other
/// property could drift without this failing.
#[test]
fn a_settled_run_lands_its_card_by_the_table() {
    assert_eq!(
        column_for_settled_run(RunStatus::Succeeded),
        Some(COLUMN_IN_REVIEW)
    );
    // Issue #465: a run parked on an approval stopped short of a result, so
    // it parks the card rather than presenting it as reviewable work.
    assert_eq!(
        column_for_settled_run(RunStatus::WaitingApproval),
        Some(COLUMN_PAUSED)
    );
    assert_eq!(
        column_for_settled_run(RunStatus::Paused),
        Some(COLUMN_PAUSED)
    );
    assert_eq!(column_for_settled_run(RunStatus::Failed), Some(COLUMN_TODO));
    assert_eq!(
        column_for_settled_run(RunStatus::Cancelled),
        Some(COLUMN_TODO)
    );
    // Issue #1809: a by-design decline returns the card to To-do, same as a
    // failure or cancel — the reason is on the note and the card becomes a
    // one-off, never a stuck column of its own.
    assert_eq!(
        column_for_settled_run(RunStatus::Declined),
        Some(COLUMN_TODO)
    );
    // Not settled — an in-flight attempt has no landing to write.
    assert_eq!(column_for_settled_run(RunStatus::Pending), None);
    assert_eq!(column_for_settled_run(RunStatus::Running), None);
}

/// The operator decision of 2026-08-05, pinned as its own test because it
/// supersedes the `Succeeded → Done` row epic #183 §4 originally wrote.
///
/// **Done is reached only by a person.** Nothing in the automatic edge may
/// write [`COLUMN_DONE`]; the only route there is
/// `review_landing_column(Approve)`. If a future change makes a settle land
/// in Done, it fails here first.
#[test]
fn the_automatic_edge_never_writes_done() {
    assert_eq!(
        column_for_settled_run(RunStatus::Succeeded),
        Some(COLUMN_IN_REVIEW),
        "a clean success stops for a person; Done is not automatic"
    );
    for status in [
        RunStatus::Pending,
        RunStatus::Running,
        RunStatus::WaitingApproval,
        RunStatus::Paused,
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Declined,
    ] {
        assert_ne!(
            column_for_settled_run(status),
            Some(COLUMN_DONE),
            "{status} must not auto-advance a card to Done"
        );
    }
}

/// **Issue #465, stated as the rule rather than the value.** Only a run that
/// produced a result may land in the column a review verdict consumes.
///
/// The teeth: `review_landing_column(Approve)` turns [`COLUMN_IN_REVIEW`]
/// into [`COLUMN_DONE`] in one gesture, and it is the only route to Done. A
/// run that stopped at an unauthorised call has produced nothing to accept —
/// in the reported case, nothing at all — so leaving it reviewable put
/// unstarted work one click from finished. #337 removed the automatic route
/// to that state; this removes the manual one.
///
/// Written as a loop over "did this run produce a result" rather than as an
/// equality on `WaitingApproval`, so a future status that also stops short
/// has to answer the same question instead of inheriting a column.
#[test]
fn only_a_run_that_produced_a_result_lands_where_review_can_approve_it() {
    for status in [
        RunStatus::WaitingApproval,
        RunStatus::Paused,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Declined,
    ] {
        assert_ne!(
            column_for_settled_run(status),
            Some(COLUMN_IN_REVIEW),
            "{status} produced nothing to review, so it must not present as \
             reviewable work a verdict could approve straight to Done"
        );
    }
    // The converse, so this cannot be satisfied by emptying the column: a
    // run that *did* produce a result still reaches the reviewer.
    assert_eq!(
        column_for_settled_run(RunStatus::Succeeded),
        Some(COLUMN_IN_REVIEW)
    );
}

/// Whatever the table says must be a column the board actually renders —
/// otherwise a settle writes a card straight off the board, which is the
/// silent disappearance [`BOARD_COLUMNS`] exists to prevent.
#[test]
fn every_landing_is_a_real_board_column() {
    for status in [
        RunStatus::Pending,
        RunStatus::Running,
        RunStatus::WaitingApproval,
        RunStatus::Paused,
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Declined,
    ] {
        if let Some(column) = column_for_settled_run(status) {
            assert!(is_board_column(column), "{status} lands in '{column}'");
        }
    }
}

/// Issue #335: a long paste is truncated on a character boundary, never on
/// a byte one — a message of multi-byte text must not panic the write path
/// or persist a split codepoint. Anything within the cap passes through
/// untouched.
#[test]
fn cap_discussion_is_codepoint_safe() {
    let long: String = "é".repeat(MAX_DISCUSSION_CHARS + 50);
    let capped = cap_discussion(&long);
    assert_eq!(capped.chars().count(), MAX_DISCUSSION_CHARS);
    // A clean multiple of 2 (é is 2 bytes) — no half-written character.
    assert_eq!(capped.len(), MAX_DISCUSSION_CHARS * 2);

    assert_eq!(cap_discussion("looks good to me"), "looks good to me");
    assert_eq!(cap_discussion(""), "");
}
