//! **An operator DM, driven end to end.**
//!
//! Everything else about DMs is asserted a decision at a time: the hive is
//! built right, the chat resolves to a room, the owner answers rather than a
//! ranker. None of that proves the parts meet. This drives the real path --
//! real `dm_hives`, real `surface_of`, real dispatcher, real `conducted::run`,
//! real `HostedRunner`, real seats on the pool's own agents -- and stubs one
//! thing, at the one boundary that would need a credential: the model's
//! choices, via a scripted OpenAI-compatible endpoint on loopback.
//!
//! That is the shape `gated_tool_turn_tests` established, reused here because
//! an episode is exactly the kind of wiring a unit test stays green through.

use std::sync::Arc;

use crate::harness::HarnessPool;
use crate::hive::test_support::{MemoryLog, TWO_DESKS, record};
use crate::ports::events::EventLog;
use crate::ports::types::EventSeq;
use crate::workflows::gated_tool_turn_tests::{Turn, deps, spawn_script_recording};

/// An operator's message in a teammate's DM runs an episode, and the teammate
/// whose DM it is answers it.
///
/// The two halves are separately fragile. A DM that never becomes a room
/// silently falls back to the pooled turn -- the old behaviour, no error. A DM
/// that becomes a room but routes by rank is answered by whoever the ranker
/// preferred, which is also not an error. Both look like working software.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_operator_dm_runs_an_episode_answered_by_its_own_teammate() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // A reply that calls no tool records nothing, so a seat that only *says*
    // something leaves the episode open until it hits the turn wall. The
    // script completes, which is what a seat answering its operator does.
    let (base_url, script) = spawn_script_recording(vec![Turn::Call {
        tool: "desk_complete_episode",
        args: serde_json::json!({
            "message": "passkeys land next sprint",
            "chat": "dm:ceo",
            "parent": null
        }),
    }])
    .await;
    let (deps, _journal) = deps(base_url, dir.path());
    let record = record(TWO_DESKS);

    let pool = HarnessPool::new();
    pool.ensure(&record, &deps).await.expect("the roster boots");

    // The hives a DM chat resolves through, built the way `hives_for` builds
    // them when the flag is on.
    let (hives, errors) = crate::hive::graph::dm_hives(&record, 3, &|id| {
        futures::executor::block_on(pool.agent(&record.id, id))
            .map(|agent| agent.runtime_agent().clone())
    });
    assert!(errors.is_empty(), "{errors:?}");

    let surface = crate::hive::dispatch::surface_of(&record, &hives, Some("dm:ceo"));
    let crate::hive::dispatch::Surface::Room { desk_id } = surface else {
        panic!("an operator DM with a hive is a room, not a pooled turn");
    };
    assert_eq!(desk_id, "dm:ceo");

    let log = Arc::new(MemoryLog::default());
    let events: Arc<dyn EventLog> = log.clone();
    let dispatcher = crate::hive::dispatch::dispatcher(
        Arc::new(record.clone()),
        Arc::clone(&events),
        hives,
        Arc::new(deps),
        Arc::new(pool),
        None,
    );

    let report: crate::Result<_> = dispatcher
        .run_desk_message(
            &desk_id,
            crate::hive::conducted::Trigger {
                seq: EventSeq::new(1),
                text: "ship passkeys next sprint?".to_owned(),
                parent: None,
                mentions: Vec::new(),
            },
        )
        .await;
    {
        let seen = script.seen.lock().unwrap();
        eprintln!("[dm-test] requests seen: {}", seen.len());
        if let Some(first) = seen.first() {
            let names: Vec<String> = first["tools"]
                .as_array()
                .map(|t| {
                    t.iter()
                        .filter_map(|x| x["function"]["name"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            eprintln!("[dm-test] tools offered: {names:?}");
        }
        if let Some(last) = seen.last() {
            for m in last["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .take(4)
            {
                eprintln!(
                    "[dm-test] msg role={} content={:?}",
                    m["role"], m["content"]
                );
            }
        }
    }
    let report = report.expect("the episode runs");

    assert!(report.turns > 0, "a seat took a turn: {report:?}");
    assert!(
        !script.seen.lock().unwrap().is_empty(),
        "the seat actually reached the model"
    );

    let replies = log.replies("dm:ceo");
    assert!(
        replies.iter().any(|(who, _)| who == "ceo"),
        "the teammate whose DM it is answered: {replies:?}"
    );
    assert!(
        !replies.iter().any(|(who, _)| who != "ceo"),
        "and nobody else did -- the roster is bound to be asked, not to reply: {replies:?}"
    );
}

/// A question handed on lands in the other teammate's own line, and the
/// episode that answers it is theirs.
///
/// This is the move `ask` cannot make. `ask` resolves its target inside one
/// episode's membership and the answer comes back to the asker; a hand-off
/// crosses into a different conversation and leaves the answer there. The
/// operator's correspondent changes, which is the whole point and also the
/// cost -- so the notice has to name where the reply will appear.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hand_off_opens_the_other_teammates_own_line() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let (base_url, _script) = spawn_script_recording(vec![Turn::Call {
        tool: "desk_complete_episode",
        args: serde_json::json!({
            "message": "webauthn is two sprints, not one",
            "chat": "dm:engineer",
            "parent": null
        }),
    }])
    .await;
    let (deps, _journal) = deps(base_url, dir.path());
    let record = record(TWO_DESKS);

    let pool = HarnessPool::new();
    pool.ensure(&record, &deps).await.expect("the roster boots");

    let log = Arc::new(MemoryLog::default());
    let events: Arc<dyn EventLog> = log.clone();

    // The CEO hands the question to the engineer.
    let (chat, seq) = crate::hive::dispatch::hand_off_to_dm(
        events.as_ref(),
        &record.id,
        "engineer",
        "how long is webauthn really?",
    )
    .await
    .expect("the question is delivered");
    assert_eq!(chat, "dm:engineer", "it lands in their line, not the asker's");

    // And what the operator is told points at that line rather than promising
    // a reply the mechanism cannot guarantee.
    let notice = crate::hive::dispatch::hand_off_notice("engineer", &chat);
    assert!(notice.contains("dm:engineer"), "the notice says where: {notice}");
    assert!(
        !notice.to_lowercase().contains("this turn"),
        "and promises no timing it cannot keep: {notice}"
    );

    // The delivered question opens the engineer's own episode.
    let (hives, errors) = crate::hive::graph::dm_hives(&record, 3, &|id| {
        futures::executor::block_on(pool.agent(&record.id, id))
            .map(|agent| agent.runtime_agent().clone())
    });
    assert!(errors.is_empty(), "{errors:?}");
    let crate::hive::dispatch::Surface::Room { desk_id } =
        crate::hive::dispatch::surface_of(&record, &hives, Some(&chat))
    else {
        panic!("a handed-to DM is a room like any other");
    };

    let dispatcher = crate::hive::dispatch::dispatcher(
        Arc::new(record.clone()),
        Arc::clone(&events),
        hives,
        Arc::new(deps),
        Arc::new(pool),
        None,
    );
    let report = dispatcher
        .run_desk_message(
            &desk_id,
            crate::hive::conducted::Trigger {
                seq,
                text: "how long is webauthn really?".to_owned(),
                parent: None,
                mentions: Vec::new(),
            },
        )
        .await
        .expect("the engineer's episode runs");
    assert!(report.turns > 0, "{report:?}");

    let replies = log.replies(&chat);
    assert!(
        replies.iter().any(|(who, _)| who == "engineer"),
        "the teammate handed to answered, in their own line: {replies:?}"
    );
    assert!(
        log.replies("dm:ceo").is_empty(),
        "and nothing came back to the one who handed it over"
    );
}
