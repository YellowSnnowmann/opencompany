use super::*;

fn parse(src: &str) -> (Vec<TaskSeed>, Vec<String>) {
    parse_tasks(TASKS_FILE, src)
}

#[test]
fn reads_a_card_and_defaults_what_an_author_left_out() {
    let (tasks, problems) = parse(
        r#"
        [[task]]
        id = "set-up-the-book-flow"
        title = "Set up the book flow"
        "#,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(tasks.len(), 1);

    let card = tasks[0].to_record(1_000);
    assert_eq!(card.id, "set-up-the-book-flow");
    assert_eq!(card.priority, "medium");
    // Unassigned, not a guessed name — the seeder is below the resolver.
    assert_eq!(card.assignee, "");
    assert_eq!(card.updated_at_millis, 1_000);
}

#[test]
fn a_seeded_card_is_always_todo_and_carries_no_invented_provenance() {
    let (tasks, _) = parse(
        r#"
        [[task]]
        id = "a"
        title = "A"
        "#,
    );
    let card = tasks[0].to_record(0);
    // The whole safety property: nothing seeded can enter the dispatching
    // or the billing column.
    assert_eq!(card.column, COLUMN_TODO);
    assert!(card.origin_chat_id().is_none());
    assert!(card.parent_task_id.is_none());
    assert!(card.origin_run_id.is_none());
    assert!(card.origin_workflow_id.is_none());
    assert!(card.plan.is_none());
    assert!(card.output.is_none());
}

#[test]
fn a_column_key_is_refused_rather_than_honoured() {
    // `deny_unknown_fields` is what enforces "a card cannot name a column".
    let (tasks, problems) = parse(
        r#"
        [[task]]
        id = "a"
        title = "A"
        column = "in_progress"
        "#,
    );
    assert!(tasks.is_empty());
    assert!(!problems.is_empty(), "a column key must be refused");
}

#[test]
fn refuses_a_duplicate_id() {
    let (tasks, problems) = parse(
        r#"
        [[task]]
        id = "a"
        title = "First"

        [[task]]
        id = "a"
        title = "Second"
        "#,
    );
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "First");
    assert!(
        problems.iter().any(|p| p.contains("repeats an `id`")),
        "{problems:?}"
    );
}

#[test]
fn refuses_an_unrenderable_priority() {
    let (tasks, problems) = parse(
        r#"
        [[task]]
        id = "a"
        title = "A"
        priority = "urgent"
        "#,
    );
    assert!(tasks.is_empty());
    assert!(
        problems.iter().any(|p| p.contains("priority")),
        "{problems:?}"
    );
}

#[test]
fn refuses_a_title_that_says_nothing() {
    let (tasks, problems) = parse(
        r#"
        [[task]]
        id = "a"
        title = "   "
        "#,
    );
    assert!(tasks.is_empty());
    assert!(
        problems.iter().any(|p| p.contains("`title`")),
        "{problems:?}"
    );
}

#[test]
fn refuses_an_id_that_is_not_lowercase_and_dashed() {
    for bad in ["Set_Up", "-leading", "trailing-", "has space"] {
        let (tasks, problems) = parse(&format!("[[task]]\nid = \"{bad}\"\ntitle = \"A\"\n"));
        assert!(tasks.is_empty(), "`{bad}` should be refused");
        assert!(!problems.is_empty(), "`{bad}` should report a problem");
    }
}

#[test]
fn one_bad_card_does_not_cost_the_others() {
    let (tasks, problems) = parse(
        r#"
        [[task]]
        id = "good-one"
        title = "Good"

        [[task]]
        id = "bad one"
        title = "Bad"

        [[task]]
        id = "good-two"
        title = "Also good"
        "#,
    );
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["good-one", "good-two"]);
    assert_eq!(problems.len(), 1);
}

#[test]
fn a_malformed_file_is_one_problem_and_no_cards() {
    let (tasks, problems) = parse("[[task]\nid = ");
    assert!(tasks.is_empty());
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("not valid TOML"), "{problems:?}");
}

#[test]
fn a_missing_file_is_not_a_problem() {
    let dir = std::env::temp_dir().join("oc-task-file-absent");
    std::fs::create_dir_all(&dir).expect("temp dir");
    assert!(!has_task_file(&dir));
    assert!(load_dir_tasks(&dir).expect("absent is fine").is_empty());
}
