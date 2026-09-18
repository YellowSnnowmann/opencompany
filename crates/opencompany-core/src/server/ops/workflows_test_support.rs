//! Shared fixtures for the `workflows` ops test-file cluster.
//!
//! This module holds the const/fn/struct fixtures that used to live in a
//! single inline `#[cfg(test)] mod tests { ... }` block in `workflows.rs`
//! before it was mechanically split into the sibling `workflows_*_tests.rs`
//! files declared at the tail of `workflows.rs`. Splitting scattered the
//! shared helpers across files that can no longer see each other, so they
//! are collected back here for every split file to import.
//!
//! Layout mirrors the original nesting: fixtures shared by every test file
//! live at the top level (`pub(super)`, reachable from any sibling test
//! file one level up via `super::workflows_test_support::*`); fixtures used
//! only by the hosted-mode cluster or only by the running cluster live in
//! their own nested module (`pub(crate)`, since a `pub(super)` two levels
//! down would only reach back up to this module, not out to the sibling
//! test files that need it).
#![cfg(test)]

/// The listed rows this company itself has, with the global baseline
/// filtered out. Every company lists the baseline graphs; these tests are
/// about what this one created, deleted, or declared.
///
/// This is an **id heuristic**, not provenance: `WorkflowSummary` carries
/// no `global` flag, so a row is classified as "the baseline's" purely by
/// id membership in `crate::globals::workflows()`. A company definition of
/// the *same* id supersedes the global one and would be wrongly excluded
/// here — none of the fixtures below give a company workflow a colliding
/// id, so the gap does not fire in this suite; see
/// `write_test::workflow_create_of_an_id_matching_a_global_wins_by_content`
/// for that case asserted directly, without this helper.
pub(super) fn own_rows(listed: &serde_json::Value) -> Vec<&serde_json::Value> {
    listed
        .as_array()
        .expect("array response")
        .iter()
        .filter(|row| {
            let id = row["id"].as_str().unwrap_or_default();
            !crate::globals::workflows().iter().any(|w| w.id == id)
        })
        .collect()
}

pub(super) const DEMO: &str = r#"
        id = "demo"
        name = "Demo flow"
        description = "A tiny trigger → agent → output graph."
        [[node]]
        id = "start"
        kind = "trigger"
        name = "Start"
        summary = "Kicks it off."
        [[node]]
        id = "worker"
        kind = "agent"
        name = "Worker"
        summary = "Does the thing."
        agent = "assistant"
        [[node]]
        id = "done"
        kind = "output"
        name = "Report"
        [[edge]]
        from = "start"
        to = "worker"
        [[edge]]
        from = "worker"
        to = "done"
        label = "ok"
    "#;

/// Writes `DEMO` to `<dir>/workflows/demo.toml` and returns `dir`.
pub(super) fn seed_demo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let workflows = dir.path().join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("demo.toml"), DEMO).unwrap();
    dir
}

#[path = "workflows_test_support_hosted.rs"]
pub(crate) mod hosted_mode;

#[path = "workflows_test_support_running.rs"]
pub(crate) mod running;
