//! The board's column vocabulary, declared once.
//!
//! The task board's columns existed in three places that could not check each
//! other: a `[&str; 6]` on the port, a `match` mapping each id to a label
//! beside it, and a hand-maintained `TASK_COLUMNS` in the console — whose own
//! comment admitted the drift it could not prevent, since *"a Rust test cannot
//! see the TS list, so a column added on one side and not the other keeps this
//! green."*
//!
//! This is that one place. [`COLUMNS`] carries every column's id, its human
//! label, whether it ends a card's life, and where it renders in the ledger
//! file; [`crate::ports::tasks`] derives its list and its labels from it, the
//! `tasks` ledger declaration builds its statuses and sections from it, and the
//! console reads the labels off the ledger rather than keeping a copy. Adding a
//! column is one edit here.
//!
//! # What is deliberately still a plain const
//!
//! The individual ids — [`COLUMN_IN_PROGRESS`](crate::ports::tasks::COLUMN_IN_PROGRESS)
//! and its siblings — stay leaf constants on the port, and this table refers to
//! them. They are load-bearing in a way a label is not: entering `in_progress`
//! *dispatches the card*, and the edge keys off that exact literal. So the
//! **identity** of a column is a compile-time constant a `match` arm can name,
//! and only its presentation and its grouping live here.
//!
//! # And why the table is not itself declarable
//!
//! A company may declare any ledger it likes, but not this one: a stage here
//! is a lifecycle state that spends money. `planning` fires a model call,
//! `in_progress` opens an attempt, and `done` is reachable only through a human
//! verdict. A company that could add a seventh stage from a JSON file would
//! have a state the runtime has no edge for, and a card that entered it would
//! sit there forever with nothing to say why.
//!
//! # Stages are the runtime's; phases are everybody else's (issue #1512)
//!
//! Six lifecycle states is the right number for the *runtime*, and the wrong
//! number for a reader. The board asked every agent and every operator to hold
//! a six-word vocabulary in which four of the words mean some shade of "a
//! teammate has started this", and told them apart by *which* machine is
//! currently owed something — a distinction the runtime needs and nobody else
//! does. What that bought in practice was agents moving cards to `in_review`
//! when they meant paused, filing work as `planning` because a plan existed,
//! and reading a rendered board they could not summarise.
//!
//! So the table now carries two vocabularies over one set of rows:
//!
//! * a **stage** ([`BoardColumn::id`]) — the six lifecycle states, unchanged.
//!   Persisted, matched on by the dispatch edge, and never widened.
//! * a **phase** ([`BoardColumn::phase`], and [`PHASES`]) — the three states
//!   everything that reads the board is shown: pending, working, done.
//!
//! [`PHASES`] is what the `tasks` ledger declares as its statuses, what
//! `derived/tasks.md` groups under, and what the console renders as columns.
//! A stage is still on the card — `Task.stage` on the wire, a badge on the
//! card — so *waiting on your verdict* is still visible where it matters, as a
//! property of a working card rather than as a fourth thing to file it under.

use crate::ports::tasks::{
    COLUMN_DONE, COLUMN_IN_PROGRESS, COLUMN_IN_REVIEW, COLUMN_PAUSED, COLUMN_PLANNING, COLUMN_TODO,
};

/// One **stage** of the board: a lifecycle state the runtime can be in the
/// middle of, and everything any surface needs in order to name it.
///
/// The type keeps its name because every caller's `column` field does. What
/// changed in #1512 is what a stage *is for*: it is the runtime's state, not
/// the board's column. The column a reader sees is this stage's
/// [`phase`](BoardColumn::phase).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardColumn {
    /// The wire word a card stores and the dispatch edge matches on.
    pub id: &'static str,
    /// What a person reads when the stage itself is shown — on the card's
    /// badge, in an exported record, in a detail view. Never derived from the
    /// id: `in_review` humanises to "In review" by luck and `todo` does not.
    pub label: &'static str,
    /// Which of the three [`PHASES`] a card in this stage is filed under.
    ///
    /// This is the whole of the collapse. `planning`, `in_progress`, `paused`
    /// and `in_review` all name work that has started and has not finished, so
    /// all four are [`PHASE_WORKING`]; the differences between them are the
    /// runtime's business and stay on the card.
    pub phase: &'static str,
}

/// One **phase**: a column of the board as everything that reads it sees one.
///
/// Three, and the set is closed. A phase answers the only question a reader of
/// a board actually asks — *has this started, and is it finished* — and adding
/// a fourth would mean answering a second question in the same place, which is
/// the mistake the six-column board made four times over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardPhase {
    /// The wire word the `tasks` ledger declares as a status, the console
    /// renders as a column, and a drop writes back.
    pub id: &'static str,
    /// What a person reads.
    pub label: &'static str,
    /// Whether a card here is finished.
    ///
    /// Exactly one phase is, and it is not a presentation detail: it decides
    /// which cards an index files as archive and which the ledger counts as
    /// outstanding. A card in review or paused is **stopped, not finished** —
    /// which is why both are [`PHASE_WORKING`] and neither is closed.
    pub closed: bool,
    /// The sentence under this phase's heading in `derived/tasks.md`.
    pub blurb: &'static str,
    /// The stage a card enters when something drops it into this phase.
    ///
    /// The translation the write boundary performs, declared here rather than
    /// at the boundary so there is one answer to *what does dropping a card in
    /// Working actually do* — it dispatches, because [`COLUMN_IN_PROGRESS`] is
    /// what `working` resolves to.
    pub entry_stage: &'static str,
}

/// Nothing has started yet.
pub const PHASE_PENDING: &str = "pending";
/// Started, not finished — whoever or whatever it is currently waiting on.
pub const PHASE_WORKING: &str = "working";
/// Finished, by a person's verdict.
pub const PHASE_DONE: &str = "done";

/// Every phase, in board order.
pub const PHASES: [BoardPhase; 3] = [
    BoardPhase {
        id: PHASE_PENDING,
        label: "Pending",
        closed: false,
        blurb: "Waiting for a person to say the work should start — entered by hand, or returned \
                by a pass that could not finish. Most recently added or returned first.",
        entry_stage: COLUMN_TODO,
    },
    BoardPhase {
        id: PHASE_WORKING,
        label: "Working",
        closed: false,
        blurb: "Started and not finished: being planned, being worked, or stopped waiting on a \
                person. Each card says which on its own line.",
        entry_stage: COLUMN_IN_PROGRESS,
    },
    BoardPhase {
        id: PHASE_DONE,
        label: "Done",
        closed: true,
        blurb: "The most recently finished. Kept, because a company that cannot see what it \
                already did repeats it.",
        entry_stage: COLUMN_DONE,
    },
];

/// Every stage, in lifecycle order.
///
/// The order is the reading order and the drag order, and it is asserted
/// against a literal in [`crate::ports::tasks`] so a reorder is a deliberate
/// two-line change rather than a side effect of editing this table.
pub const COLUMNS: [BoardColumn; 6] = [
    BoardColumn {
        id: COLUMN_TODO,
        label: "To-do",
        phase: PHASE_PENDING,
    },
    BoardColumn {
        id: COLUMN_PLANNING,
        label: "Planning",
        phase: PHASE_WORKING,
    },
    BoardColumn {
        id: COLUMN_IN_PROGRESS,
        label: "In progress",
        phase: PHASE_WORKING,
    },
    BoardColumn {
        id: COLUMN_PAUSED,
        label: "Paused",
        phase: PHASE_WORKING,
    },
    BoardColumn {
        id: COLUMN_IN_REVIEW,
        label: "In review",
        phase: PHASE_WORKING,
    },
    BoardColumn {
        id: COLUMN_DONE,
        label: "Done",
        phase: PHASE_DONE,
    },
];

/// Every column id, in board order.
///
/// A `const fn` so [`crate::ports::tasks::BOARD_COLUMNS`] stays a genuine
/// constant — usable in a `match`, in an array length, and in a `const` context
/// — rather than becoming a lazily-built `Vec` every caller has to unwrap. The
/// alternative was a second literal list, which is the thing this module
/// exists to delete.
pub const fn ids() -> [&'static str; COLUMNS.len()] {
    let mut out = [""; COLUMNS.len()];
    let mut index = 0;
    while index < COLUMNS.len() {
        out[index] = COLUMNS[index].id;
        index += 1;
    }
    out
}

/// The column `id` names, if it names one.
pub fn column(id: &str) -> Option<&'static BoardColumn> {
    COLUMNS.iter().find(|column| column.id == id)
}

/// The phase `id` names, if it names one.
pub fn phase(id: &str) -> Option<&'static BoardPhase> {
    PHASES.iter().find(|phase| phase.id == id)
}

/// The phase a stage is filed under, or the stage itself when this build has
/// never heard of it.
///
/// Falling back to the input rather than to [`PHASE_PENDING`] is deliberate: a
/// card carrying a stage from a newer build must not be silently reported as
/// not-started, which would make *what is outstanding* answer wrong in the
/// safest-looking direction. An unknown word renders as itself and is visibly
/// odd.
pub fn phase_of(stage: &str) -> &str {
    column(stage).map_or(stage, |column| column.phase)
}

/// The stage a card enters when something drops it into `phase`.
///
/// `None` when `phase` is not one of the three — which is how the write
/// boundary tells a phase word from a stage word without keeping a second list
/// of either.
pub fn entry_stage(phase: &str) -> Option<&'static str> {
    self::phase(phase).map(|phase| phase.entry_stage)
}

/// Every phase id, in board order.
pub const fn phase_ids() -> [&'static str; PHASES.len()] {
    let mut out = [""; PHASES.len()];
    let mut index = 0;
    while index < PHASES.len() {
        out[index] = PHASES[index].id;
        index += 1;
    }
    out
}

#[cfg(test)]
mod test {
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
}
