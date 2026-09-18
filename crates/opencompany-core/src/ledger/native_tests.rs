use super::*;
use crate::ledger::registry::Registry;
use crate::ports::tasks::{
    COLUMN_DONE, COLUMN_IN_PROGRESS, COLUMN_IN_REVIEW, COLUMN_PAUSED, COLUMN_TODO, TaskDeliverable,
    TaskTitle,
};

fn card(id: &str, column: &str, updated: u64) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        title: TaskTitle::authored(&format!("card {id}")),
        note: None,
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: "ops".to_string(),
        updated_at_millis: updated,
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    }
}

#[test]
fn a_card_projects_onto_the_declared_field_names() {
    let registry = Registry::build([]);
    let spec = registry.find("tasks").expect("tasks is built in");
    let entries = entries_from_tasks(&[card("t1", COLUMN_IN_PROGRESS, 10)]);
    let entry = entries.find("t1").expect("the card projects");
    assert_eq!(entry.title(spec), "card t1");
    assert_eq!(entry.status(spec), crate::ledger::board::PHASE_WORKING);
    assert_eq!(entry.get("assignee"), "ops");
}

/// The four working stages project onto one status, and each says which
/// one it is on the row rather than in the pile it landed in (issue #1512).
#[test]
fn every_working_stage_reads_as_one_status_and_keeps_its_own_name() {
    use crate::ledger::board::PHASE_WORKING;
    use crate::ports::tasks::COLUMN_PLANNING;

    let registry = Registry::build([]);
    let spec = registry.find("tasks").expect("tasks is built in");
    let entries = entries_from_tasks(&[
        card("planning", COLUMN_PLANNING, 40),
        card("running", COLUMN_IN_PROGRESS, 30),
        card("parked", crate::ports::tasks::COLUMN_PAUSED, 20),
        card("waiting", crate::ports::tasks::COLUMN_IN_REVIEW, 10),
    ]);
    for id in ["planning", "running", "parked", "waiting"] {
        let entry = entries.find(id).expect("the card projects");
        assert_eq!(entry.status(spec), PHASE_WORKING, "{id}");
    }
    assert_eq!(entries.find("waiting").unwrap().get("stage"), "In review");
    assert_eq!(entries.find("planning").unwrap().get("stage"), "Planning");
}

/// Pending and done carry no stage: there is only one way to be either, so
/// a line saying which would be a line saying nothing.
#[test]
fn a_pending_or_done_card_carries_no_stage_line() {
    let entries = entries_from_tasks(&[
        card("waiting", COLUMN_TODO, 20),
        card("shipped", COLUMN_DONE, 10),
    ]);
    assert_eq!(entries.find("waiting").unwrap().get("stage"), "");
    assert_eq!(entries.find("shipped").unwrap().get("stage"), "");
}

/// The board lists most-recently-updated first; `Recent` sorts descending
/// on `touched`. The two must agree, or the section's blurb lies.
#[test]
fn the_boards_order_survives_the_projection() {
    let entries = entries_from_tasks(&[
        card("newest", COLUMN_TODO, 30),
        card("middle", COLUMN_TODO, 20),
        card("oldest", COLUMN_TODO, 10),
    ]);
    let ordered =
        super::super::engine::ordered(&entries.entries, super::super::spec::Order::Recent);
    let ids: Vec<&str> = ordered.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(ids, ["newest", "middle", "oldest"]);
}

/// Done is the board's one closed phase, so it is the one that lands in an
/// index's archive rather than in its open list.
#[test]
fn only_done_reads_as_closed() {
    let registry = Registry::build([]);
    let spec = registry.find("tasks").expect("tasks is built in");
    let entries = entries_from_tasks(&[
        card("a", COLUMN_DONE, 30),
        card("b", COLUMN_IN_PROGRESS, 20),
        card("c", COLUMN_TODO, 10),
    ]);
    assert_eq!(entries.closed_count(spec), 1);
    assert_eq!(entries.open_count(spec), 2);
}

/// The file an agent actually reads: three headings, and a working card that
/// still says what it is waiting on (issue #1512).
#[test]
fn the_rendered_board_has_three_headings_and_keeps_the_stage_on_the_row() {
    let registry = Registry::build([]);
    let spec = registry.find("tasks").expect("tasks is built in");
    let entries = entries_from_tasks(&[
        card("a", COLUMN_TODO, 40),
        card("b", COLUMN_PAUSED, 30),
        card("c", COLUMN_IN_REVIEW, 20),
        card("d", COLUMN_DONE, 10),
    ]);
    let rendered = crate::ledger::engine::render(spec, &entries);
    let headings: Vec<&str> = rendered
        .lines()
        .filter(|line| line.starts_with("## "))
        .collect();
    assert_eq!(
        headings,
        ["## Pending", "## Working", "## Done"],
        "{rendered}"
    );
    assert!(rendered.contains("In review"), "{rendered}");
    assert!(rendered.contains("Paused"), "{rendered}");
}
