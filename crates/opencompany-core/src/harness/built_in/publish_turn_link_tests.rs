use super::publish_turn_helpers_tests::NoopHost;

use super::publish_turn_helpers_tests::*;
use crate::ports::artifacts::ArtifactStore;
use crate::ports::brain::Brain;
use crate::ports::tasks::{COLUMN_IN_PROGRESS, TaskStore};

// ---------------------------------------------------------------------------
// Issue #339: the card's link to what the run produced
// ---------------------------------------------------------------------------
//
// Epic #183 §6. A task that finished used to end as a chat message — prose in a
// conversation with nothing durable to open, share, or hand to somebody else.
// These drive the same real dispatch as the tests above, under a real attempt
// row, and assert the stamp the card comes back carrying.

/// The headline: a run that published a file leaves the card pointing at that
/// file **at the version this run wrote**, under the attempt that wrote it.
#[tokio::test]
async fn a_published_card_links_to_the_version_its_run_wrote() {
    let (base_url, _script) = spawn_script(vec![
        write("launch.md", "# Launch spec\nShip on Friday."),
        publish("launch.md"),
        Turn::Say("Drafted the launch spec and published it."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();
    mint_run(&ops, "run-1", "t-1").await;

    brain
        .run_cycle(dispatch_as("t-1", "run-1"), &NoopHost)
        .await
        .expect("cycle runs");

    let output = card_after(&ops, "t-1")
        .await
        .output
        .expect("a successful run stamps its card");
    assert_eq!(output.source.run_id(), Some("run-1"));
    assert_eq!(
        output.source.attempt(),
        Some(1),
        "the first attempt at this card"
    );
    assert_eq!(output.artifacts.len(), 1, "got {:?}", output.artifacts);

    let linked = &output.artifacts[0];
    assert_eq!(linked.version, 1, "pinned at what this run wrote");
    assert_eq!(linked.title, "launch.md");
    assert_eq!(linked.kind, crate::ports::artifacts::ArtifactKind::Markdown);
    // The link resolves: the id names a record that is actually on the card.
    let stored = artifacts_on(&ops, "t-1").await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, linked.artifact_id);
    assert!(stored[0].version(linked.version).is_some());
}

/// *"Every card has a link including tasks that produced no file"* — the
/// acceptance criterion most easily lost, because the obvious implementation
/// stamps nothing when there is no artifact.
///
/// The agent here writes nothing at all (so no nudge fires) and simply answers.
/// The trace **is** the deliverable, and the stamp says so.
#[tokio::test]
async fn a_task_that_produced_no_file_still_links_to_its_trace() {
    let (base_url, script) =
        spawn_script(vec![Turn::Say("Checked it — nothing needed changing.")]).await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();
    mint_run(&ops, "run-1", "t-1").await;

    brain
        .run_cycle(dispatch_as("t-1", "run-1"), &NoopHost)
        .await
        .expect("cycle runs");

    assert_eq!(
        nudge_turns(&script),
        0,
        "nothing was written to nudge about"
    );
    assert!(artifacts_on(&ops, "t-1").await.is_empty(), "no deliverable");

    let output = card_after(&ops, "t-1")
        .await
        .output
        .expect("no artifact must not mean no link");
    assert_eq!(output.source.run_id(), Some("run-1"));
    assert!(output.artifacts.is_empty());
    assert!(output.workflows.is_empty());
}

/// *"The link is pinned to the attempt that produced it, and a retried task
/// links to the successful one."*
///
/// Two real dispatches under two real attempts. The second republishes, so the
/// stamp moves wholesale to the second attempt and the second version — nothing
/// merges, and no read-time query has to decide which attempt wins.
#[tokio::test]
async fn a_re_run_repins_the_link_to_the_attempt_that_last_succeeded() {
    let (base_url, _script) = spawn_script(vec![
        write("launch.md", "# Launch spec\nDraft."),
        publish("launch.md"),
        Turn::Say("First draft published."),
        write("launch.md", "# Launch spec\nRevised."),
        publish("launch.md"),
        Turn::Say("Revised and republished."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();
    mint_run(&ops, "run-1", "t-1").await;
    brain
        .run_cycle(dispatch_as("t-1", "run-1"), &NoopHost)
        .await
        .expect("run 1");

    let first = card_after(&ops, "t-1").await.output.expect("stamped");
    assert_eq!(first.artifacts[0].version, 1);

    // Re-dispatch the way a re-run starts: the card comes back to In Progress.
    let mut again = card_after(&ops, "t-1").await;
    again.column = COLUMN_IN_PROGRESS.to_string();
    TaskStore::upsert(&*ops, &company(), &again).await.unwrap();
    mint_run(&ops, "run-2", "t-1").await;
    brain
        .run_cycle(dispatch_as("t-1", "run-2"), &NoopHost)
        .await
        .expect("run 2");

    let second = card_after(&ops, "t-1").await.output.expect("re-stamped");
    assert_eq!(second.source.run_id(), Some("run-2"));
    assert_eq!(second.source.attempt(), Some(2));
    assert_eq!(second.artifacts.len(), 1, "one path, one record, one link");
    assert_eq!(
        second.artifacts[0].version, 2,
        "the link must name what THIS run wrote, not the record's first draft"
    );
    assert_eq!(
        second.artifacts[0].artifact_id, first.artifacts[0].artifact_id,
        "republishing one path extends one record"
    );
    // The earlier attempt is not erased — it is still reachable as its own run.
    assert_ne!(second.source.run_id(), first.source.run_id());
}

/// *"A retried task links to the successful one."* The failing half: a later
/// attempt that fails must leave the previous success's link standing, because
/// a card whose link vanished on a retry is worse than a card with no link at
/// all — the deliverable still exists and the operator can no longer find it.
///
/// The second dispatch runs against a poisoned endpoint, so the turn errors:
/// the real `TaskRunEnd::Failed` path, not a simulated one.
#[tokio::test]
async fn a_failed_retry_does_not_erase_the_link_to_the_success_before_it() {
    let (base_url, _script) = spawn_script(vec![
        write("launch.md", "# Launch spec\nShipped."),
        publish("launch.md"),
        Turn::Say("Published."),
        // Attempt 2 falls over before it produces anything.
        Turn::Boom,
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();
    mint_run(&ops, "run-1", "t-1").await;
    brain
        .run_cycle(dispatch_as("t-1", "run-1"), &NoopHost)
        .await
        .expect("run 1");
    let succeeded = card_after(&ops, "t-1").await.output.expect("stamped");

    let mut again = card_after(&ops, "t-1").await;
    again.column = COLUMN_IN_PROGRESS.to_string();
    TaskStore::upsert(&*ops, &company(), &again).await.unwrap();
    mint_run(&ops, "run-2", "t-1").await;
    brain
        .run_cycle(dispatch_as("t-1", "run-2"), &NoopHost)
        .await
        .expect("a failed attempt still completes its cycle");

    let after = card_after(&ops, "t-1").await;
    assert_eq!(
        after.column,
        crate::ports::tasks::COLUMN_TODO,
        "a failed attempt returns the card to To-do"
    );
    let still = after
        .output
        .expect("the success's link survives the failure");
    assert_eq!(still.source.run_id(), succeeded.source.run_id());
    assert_eq!(still.artifacts, succeeded.artifacts);
}

/// *"A task whose output was later edited by a human still resolves, and says
/// so."*
///
/// The host half of that promise: the pin does not move when an operator
/// appends a version, so the card still names the revision the run produced
/// while the record grows past it. The console reads exactly this difference —
/// pinned version versus latest — to say a human edited it since.
#[tokio::test]
async fn an_operator_edit_leaves_the_pinned_link_naming_what_the_run_wrote() {
    use crate::ports::artifacts::ArtifactAuthor;

    let (base_url, _script) = spawn_script(vec![
        write("launch.md", "# Launch spec\nAgent draft."),
        publish("launch.md"),
        Turn::Say("Published."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();
    mint_run(&ops, "run-1", "t-1").await;
    brain
        .run_cycle(dispatch_as("t-1", "run-1"), &NoopHost)
        .await
        .expect("cycle runs");

    let pinned = card_after(&ops, "t-1").await.output.expect("stamped");
    assert_eq!(pinned.artifacts[0].version, 1);

    // A human edits the deliverable after the run — the console's "Edit as
    // operator" path, which appends rather than overwriting.
    let mut record = artifacts_on(&ops, "t-1").await.remove(0);
    record.push_version(
        "# Launch spec\nOperator's wording.",
        ArtifactAuthor::Operator,
        "operator",
        99,
        Some("operator edit before approval".to_string()),
    );
    ArtifactStore::upsert(&*ops, &company(), &record)
        .await
        .unwrap();

    // The card is untouched: nothing about an edit rewrites the run's stamp.
    let after = card_after(&ops, "t-1").await.output.expect("still stamped");
    assert_eq!(
        after.artifacts[0].version, 1,
        "the pin does not follow edits"
    );

    // And it still resolves — to the agent's revision, not the human's.
    let stored = artifacts_on(&ops, "t-1").await.remove(0);
    let resolved = stored
        .version(after.artifacts[0].version)
        .expect("a pinned version can never dangle: versions are append-only");
    assert_eq!(resolved.author, ArtifactAuthor::Agent);
    assert_eq!(resolved.body, "# Launch spec\nAgent draft.");
    // The newer version is what the console names in its "edited since" line.
    assert_eq!(stored.latest().unwrap().version, 2);
    assert_eq!(stored.latest().unwrap().author, ArtifactAuthor::Operator);
}

/// A dispatch with no attempt row has nothing addressable to point at, so it
/// stamps nothing rather than inventing an id. Fail silent, not closed: the
/// card lands exactly as it did before #339, and the artifact is still
/// recorded.
#[tokio::test]
async fn a_dispatch_with_no_attempt_row_stamps_nothing() {
    let (base_url, _script) = spawn_script(vec![
        write("launch.md", "# Launch spec"),
        publish("launch.md"),
        Turn::Say("Published."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();

    // `run_id: None` — the choke point minted no row.
    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("cycle runs");

    let after = card_after(&ops, "t-1").await;
    assert!(after.output.is_none(), "no attempt, no link to it");
    assert_eq!(
        artifacts_on(&ops, "t-1").await.len(),
        1,
        "the deliverable is still recorded — only the card's link is missing"
    );
    assert_eq!(after.column, crate::ports::tasks::COLUMN_IN_REVIEW);
}
