use super::publish_turn_helpers_tests::NoopHost;

use super::publish_turn_helpers_tests::*;
use crate::harness::publish::PUBLISH_ARTIFACT_TOOL;
use crate::ports::brain::Brain;
use crate::ports::tasks::{COLUMN_IN_PROGRESS, TaskStore};

// ---------------------------------------------------------------------------
// The headline
// ---------------------------------------------------------------------------

/// A model, driving a real dispatch, writes a file and publishes it — and the
/// record lands with the file's content and its path as identity.
///
/// Nothing hands the tool the path out of band: the model emits the same
/// relative path it wrote to, which is what proves the two tools share one
/// sandbox view.
#[tokio::test]
#[ignore = "TODO(Phase 3): drives one of this crate's own tools (workspace/publish/tasks/search/approval/ledger) through a turn. Since plan hive-desks Phase 2 a turn runs on the embedded OpenHuman runtime, which has no seam for a host-built tool; the belt is served to the agent over MCP in Phase 3, where this test is re-homed."]
async fn a_real_dispatch_publishes_a_file_the_agent_wrote() {
    let (base_url, script) = spawn_script(vec![
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

    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("cycle runs");

    // The tool reached the wire under its real name.
    let advertised = advertised_tools(&script);
    assert!(
        advertised.contains(&PUBLISH_ARTIFACT_TOOL.to_string()),
        "publish_artifact was never advertised: {advertised:?}"
    );

    let artifacts = artifacts_on(&ops, "t-1").await;
    assert_eq!(artifacts.len(), 1, "one published file, one record");
    let record = &artifacts[0];
    assert_eq!(record.source.as_deref(), Some("launch.md"));
    assert_eq!(record.title, "launch.md");
    assert_eq!(record.versions.len(), 1);
    assert_eq!(
        record.versions[0].body, "# Launch spec\nShip on Friday.",
        "the stored body must be the file, not the reply"
    );
    // The reply is emphatically NOT the artifact — that is the whole change.
    assert!(
        !record.versions[0].body.contains("Drafted the launch spec"),
        "the chat reply must not be captured as a deliverable"
    );

    // No nudge: everything that changed was published.
    assert_eq!(nudge_turns(&script), 0, "nothing was left unpublished");
}

/// A re-run that republishes the same path appends a **version** to the same
/// record rather than opening a second one — the identity contract, proven
/// through two real dispatches.
#[tokio::test]
#[ignore = "TODO(Phase 3): drives one of this crate's own tools (workspace/publish/tasks/search/approval/ledger) through a turn. Since plan hive-desks Phase 2 a turn runs on the embedded OpenHuman runtime, which has no seam for a host-built tool; the belt is served to the agent over MCP in Phase 3, where this test is re-homed."]
async fn a_re_run_republishing_the_same_path_adds_a_version() {
    let (base_url, script) = spawn_script(vec![
        // Run 1.
        write("launch.md", "# Launch spec\nDraft."),
        publish("launch.md"),
        Turn::Say("First draft published."),
        // Run 2.
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

    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("run 1");
    // Re-dispatch: the card comes back to In Progress the way a re-run starts.
    let mut again = card_after(&ops, "t-1").await;
    again.column = COLUMN_IN_PROGRESS.to_string();
    TaskStore::upsert(&*ops, &company(), &again).await.unwrap();
    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("run 2");

    let artifacts = artifacts_on(&ops, "t-1").await;
    assert_eq!(
        artifacts.len(),
        1,
        "one path, one record — never a duplicate"
    );
    let record = &artifacts[0];
    assert_eq!(record.versions.len(), 2);
    assert_eq!(record.versions[0].body, "# Launch spec\nDraft.");
    assert_eq!(record.versions[1].body, "# Launch spec\nRevised.");
    assert_eq!(nudge_turns(&script), 0);
}

/// An agent with no file grant is never offered the tool — and naming it anyway
/// does not publish anything. Advertisement is a hint; the grant is the control.
#[tokio::test]
async fn an_ungranted_agent_is_never_offered_the_publish_tool() {
    let (base_url, script) = spawn_script(vec![
        publish("launch.md"),
        Turn::Say("I could not publish anything."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"web\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();

    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("cycle runs");

    let advertised = advertised_tools(&script);
    assert!(
        !advertised.contains(&PUBLISH_ARTIFACT_TOOL.to_string()),
        "an agent with no file grant must not be offered it: {advertised:?}"
    );
    assert!(
        artifacts_on(&ops, "t-1").await.is_empty(),
        "naming an ungranted tool must publish nothing"
    );
}

// ---------------------------------------------------------------------------
// The nudge
// ---------------------------------------------------------------------------

/// **The nudge lands.** Turn one writes a file and forgets to publish it; the
/// follow-up turn publishes it and the record appears.
///
/// Also pins the two properties the design turns on: **exactly one** nudge, and
/// both turns recorded against the one dispatch rather than a second attempt.
#[tokio::test]
#[ignore = "TODO(Phase 3): drives one of this crate's own tools (workspace/publish/tasks/search/approval/ledger) through a turn. Since plan hive-desks Phase 2 a turn runs on the embedded OpenHuman runtime, which has no seam for a host-built tool; the belt is served to the agent over MCP in Phase 3, where this test is re-homed."]
async fn the_nudge_recovers_a_deliverable_the_agent_forgot_to_publish() {
    let (base_url, script) = spawn_script(vec![
        // Turn 1: writes, does not publish.
        write("launch.md", "# Launch spec\nShip on Friday."),
        Turn::Say("I've drafted the launch spec."),
        // The nudge turn.
        publish("launch.md"),
        Turn::Say("Published it — that one is the deliverable."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();

    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("cycle runs");

    assert_eq!(nudge_turns(&script), 1, "exactly one nudge, never two");

    // The nudge carried the context it needs, because turns share none.
    let nudge = nudge_text(&script).expect("the nudge was sent");
    assert!(nudge.contains("launch.md"), "{nudge}");
    assert!(
        nudge.contains("Draft the launch spec"),
        "the brief: {nudge}"
    );
    assert!(
        nudge.contains("I've drafted the launch spec."),
        "the agent's own reply: {nudge}"
    );

    let artifacts = artifacts_on(&ops, "t-1").await;
    assert_eq!(artifacts.len(), 1, "the nudge recovered the deliverable");
    assert_eq!(artifacts[0].source.as_deref(), Some("launch.md"));
    assert_eq!(
        artifacts[0].versions[0].body,
        "# Launch spec\nShip on Friday."
    );

    // The card still reports the *primary* reply — the nudge is bookkeeping and
    // must not overwrite what the agent actually answered.
    let after = card_after(&ops, "t-1").await;
    let note = after.note.expect("note");
    assert!(note.contains("I've drafted the launch spec."), "{note}");
    // And because it published, there is no decline line.
    assert!(!note.contains("unpublished:"), "{note}");
}

/// **A clean decline.** The agent says the files were scratch, publishes
/// nothing, and that is the end of it: no artifact, no error, and the reason
/// kept on the card where somebody can read it.
#[tokio::test]
async fn a_declined_nudge_records_no_artifact_and_keeps_the_reason() {
    let (base_url, script) = spawn_script(vec![
        write("scratch.txt", "half-finished thinking"),
        Turn::Say("I looked into it; the answer is Friday."),
        // The nudge turn declines in prose.
        Turn::Say("That was just working notes, not a deliverable."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();

    // A decline must not fail the cycle.
    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("a decline is a clean outcome, not an error");

    assert_eq!(nudge_turns(&script), 1);
    assert!(
        artifacts_on(&ops, "t-1").await.is_empty(),
        "a decline produces no deliverable"
    );

    let note = card_after(&ops, "t-1").await.note.expect("note");
    assert!(
        note.contains("unpublished: scratch.txt"),
        "the files must be named on the card: {note}"
    );
    assert!(
        note.contains("That was just working notes"),
        "the *reason* is the point — it must be addressable: {note}"
    );
    // The primary reply is still there; the decline was appended, not swapped in.
    assert!(note.contains("the answer is Friday"), "{note}");
}

/// **The one-nudge bound, under the condition that would break a counter.** The
/// nudge turn writes *more* files and still publishes nothing. There is no
/// second nudge, and the warning's file list is the **pre-nudge** diff — a
/// scratch file written while answering must not become evidence against the
/// agent.
#[tokio::test]
async fn a_nudge_that_writes_more_files_does_not_trigger_a_second_nudge() {
    let (base_url, script) = spawn_script(vec![
        write("scratch.txt", "half-finished thinking"),
        Turn::Say("Looked into it."),
        // The nudge turn writes another file and still publishes nothing.
        write("more-scratch.txt", "notes about the notes"),
        Turn::Say("Both of those are working notes."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();

    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("cycle runs");

    assert_eq!(
        nudge_turns(&script),
        1,
        "a second nudge must be impossible, not merely unlikely"
    );
    assert!(artifacts_on(&ops, "t-1").await.is_empty());

    let note = card_after(&ops, "t-1").await.note.expect("note");
    assert!(note.contains("unpublished: scratch.txt"), "{note}");
    assert!(
        !note.contains("more-scratch.txt"),
        "a file written while answering the nudge must not pollute the record: {note}"
    );
}

/// No nudge when there is nothing to nudge about: the agent wrote nothing at
/// all. Asserted on the scripted endpoint's own call count, so an extra turn
/// cannot hide.
#[tokio::test]
async fn a_run_that_wrote_nothing_is_never_nudged() {
    let (base_url, script) = spawn_script(vec![Turn::Say("The answer is Friday.")]).await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();

    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("cycle runs");

    assert_eq!(nudge_turns(&script), 0);
    assert_eq!(
        script.seen.lock().unwrap().len(),
        1,
        "exactly one model call: the turn itself"
    );
    assert!(artifacts_on(&ops, "t-1").await.is_empty());
    // …and no phantom decline line on the card.
    let note = card_after(&ops, "t-1").await.note.expect("note");
    assert!(!note.contains("unpublished:"), "{note}");
}

/// No nudge when the agent published everything it wrote — the gate is
/// `changed − staged`, not "did anything change".
#[tokio::test]
#[ignore = "TODO(Phase 3): drives one of this crate's own tools (workspace/publish/tasks/search/approval/ledger) through a turn. Since plan hive-desks Phase 2 a turn runs on the embedded OpenHuman runtime, which has no seam for a host-built tool; the belt is served to the agent over MCP in Phase 3, where this test is re-homed."]
async fn a_run_that_published_everything_is_never_nudged() {
    let (base_url, script) = spawn_script(vec![
        write("a.md", "one"),
        write("b.md", "two"),
        publish("a.md"),
        publish("b.md"),
        Turn::Say("Both published."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();

    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("cycle runs");

    assert_eq!(nudge_turns(&script), 0);
    assert_eq!(artifacts_on(&ops, "t-1").await.len(), 2);
}

/// **A nudge that cannot run is contained.** The provider fails the follow-up
/// turn; the run still completes cleanly, its card still lands, and the
/// fallback warning still describes the pre-nudge state.
///
/// This is the property that makes the nudge safe to add at all: it is
/// bookkeeping bolted onto a run whose work is already done, and it must never
/// be able to fail or delay that run.
#[tokio::test]
async fn a_nudge_turn_that_fails_never_fails_the_run() {
    let (base_url, script) = spawn_script(vec![
        write("scratch.txt", "thinking"),
        Turn::Say("Looked into it."),
        // The nudge turn's very first call falls over.
        Turn::Boom,
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (brain, ops) = brain(base_url, "\"*\"", dir.path());
    TaskStore::upsert(&*ops, &company(), &card("t-1"))
        .await
        .unwrap();

    brain
        .run_cycle(dispatch("t-1"), &NoopHost)
        .await
        .expect("a failed nudge must not fail the run");

    // Attempted, not asserted to a count: the provider retries a failed request
    // internally, so one *nudge turn* can be several requests. The exact-one
    // bound is pinned by the tests above, where the turn succeeds and the count
    // is unambiguous; what matters here is only that the failure was contained.
    assert!(nudge_turns(&script) >= 1, "the nudge was attempted");
    assert!(artifacts_on(&ops, "t-1").await.is_empty());

    let after = card_after(&ops, "t-1").await;
    // The run still landed where a completed run lands.
    assert_eq!(after.column, crate::ports::tasks::COLUMN_IN_REVIEW);
    let note = after.note.expect("note");
    assert!(
        note.contains("Looked into it."),
        "the primary reply survives a failed nudge: {note}"
    );
    // No decline line, because the agent never got to answer.
    assert!(
        !note.contains("unpublished:"),
        "a nudge that never ran cannot have been declined: {note}"
    );
}
