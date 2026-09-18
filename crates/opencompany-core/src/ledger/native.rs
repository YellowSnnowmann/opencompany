//! Reading a runtime-owned ledger through the ledger surface.
//!
//! The task board keeps its own store, its own routes and its own dispatch
//! edge — see [`registry::builtins`](super::registry::builtins) for why. What
//! it does not get to keep is a second way of being read: a discovery surface
//! that names every ledger and then cannot open the one the company has had
//! since day one is a surface an agent stops trusting.
//!
//! So this projects [`TaskRecord`]s into the same [`Entry`] the engine folds an
//! event log into. One renderer, one index, one read shape, whichever side a
//! ledger's rows actually live on.

use std::collections::BTreeMap;

use super::engine::{Entries, Entry};
use super::types::{AuthorKind, LedgerAuthor};
use crate::ports::tasks::TaskRecord;

/// Projects the board into ledger entries, in the order the store returned
/// them.
///
/// `touched` is the **reverse** index rather than the position, because the
/// board's list is most-recently-updated first while the engine's `Recent`
/// order sorts descending on `touched`. Getting that backwards would render the
/// stalest card at the top of a section whose blurb says most recent first,
/// which is precisely the failure the ordering exists to fix.
pub fn entries_from_tasks(tasks: &[TaskRecord]) -> Entries {
    let total = tasks.len();
    let entries = tasks
        .iter()
        .enumerate()
        .map(|(position, task)| {
            let mut fields: BTreeMap<String, String> = BTreeMap::new();
            fields.insert("title".to_string(), task.title.to_string());
            // The row's **status** is the phase, because that is the ledger's
            // declared vocabulary and the console's column list. The stage
            // rides alongside as prose, so a reader still learns that a working
            // card is specifically waiting on their verdict — without the board
            // growing a fourth pile to put it in. See `super::board`.
            fields.insert(
                "column".to_string(),
                super::board::phase_of(&task.column).to_string(),
            );
            if let Some(column) = super::board::column(&task.column)
                && column.phase == super::board::PHASE_WORKING
            {
                fields.insert("stage".to_string(), column.label.to_string());
            }
            fields.insert("priority".to_string(), task.priority.clone());
            if !task.assignee.trim().is_empty() {
                fields.insert("assignee".to_string(), task.assignee.clone());
            }
            if let Some(note) = task.note.as_ref().filter(|note| !note.trim().is_empty()) {
                fields.insert("note".to_string(), note.clone());
            }
            // Lineage is provenance a reader asks about often enough to be
            // worth a line, and it costs nothing when absent.
            if let Some(parent) = &task.parent_task_id {
                fields.insert("parent".to_string(), parent.clone());
            }
            let author = if task.assignee.trim().is_empty() {
                LedgerAuthor {
                    kind: AuthorKind::System,
                    id: "board".to_string(),
                    label: "the board".to_string(),
                }
            } else {
                LedgerAuthor::agent(task.assignee.clone())
            };
            Entry {
                id: task.id.clone(),
                fields,
                opened_by: author.clone(),
                updated_by: author,
                opened_at_millis: task.updated_at_millis,
                updated_at_millis: task.updated_at_millis,
                // A card's history is on the card, not in an event count. One
                // is the honest answer: this projection saw it once.
                events: 1,
                touched: total.saturating_sub(position + 1),
            }
        })
        .collect();
    Entries {
        entries,
        faults: Vec::new(),
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
