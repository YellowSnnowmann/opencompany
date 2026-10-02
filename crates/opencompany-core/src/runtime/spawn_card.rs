//! The To-do card a `spawn_task` opens, built in one place.
//!
//! Two drains open these cards: the pooled turn's delegation drain and a
//! HiveMind seat's settle. Both must write the same shape, so both build it
//! here and differ only in the provenance they stamp.

use crate::ports::tasks::{COLUMN_TODO, TaskDeliverable, TaskOpener, TaskTitle};
use crate::ports::{TaskOrigin, TaskRecord, generate_id, now_millis};

/// One `spawn_task`, ready to become a board card.
#[derive(Clone, Debug, Default)]
pub struct SpawnCard {
    /// The card's headline as the model wrote it.
    pub title: String,
    /// The brief, when the model gave one.
    pub note: Option<String>,
    /// The canonical roster id that owns it, or empty for nobody.
    pub assignee: String,
    /// The conversation the card answers back into.
    pub origin: Option<TaskOrigin>,
    /// The dispatched card whose turn opened this one.
    pub parent_task_id: Option<String>,
    /// The machine act that opened it: a workflow run, or a chat turn.
    pub origin_run_id: Option<String>,
    /// The workflow graph behind `origin_run_id`, when there is one.
    pub origin_workflow_id: Option<String>,
    /// The teammate and episode that opened it from a HiveMind room.
    pub opened_by: Option<TaskOpener>,
}

impl SpawnCard {
    /// The card as written: To-do, medium priority, a one-off deliverable.
    #[must_use]
    pub fn into_record(self) -> TaskRecord {
        TaskRecord {
            id: generate_id(),
            title: TaskTitle::system(&self.title),
            note: self.note,
            column: COLUMN_TODO.to_string(),
            priority: "medium".to_string(),
            assignee: self.assignee,
            updated_at_millis: now_millis(),
            origin: self.origin,
            parent_task_id: self.parent_task_id,
            output: None,
            plan: None,
            planning_attempts: Vec::new(),
            deliverable: TaskDeliverable::Once,
            workflow_proposal: None,
            origin_run_id: self.origin_run_id,
            origin_message_seq: None,
            origin_workflow_id: self.origin_workflow_id,
            bounced: None,
            opened_by: self.opened_by,
        }
    }
}

#[cfg(test)]
#[path = "spawn_card_tests.rs"]
mod tests;
