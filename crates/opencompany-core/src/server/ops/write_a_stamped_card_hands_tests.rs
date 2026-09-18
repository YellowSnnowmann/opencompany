//! Integration tests for the `ops` write plane: tasks, memory, workspace,
//! skills, team, inbox-read, and desk chat — exercised end-to-end over the
//! router against a real fs-backed company.

use axum::http::StatusCode;
use serde_json::json;

use super::write_test_support::*;
use crate::company::steer::{InflightEntry, InflightKind};
use crate::ports::facts::{FactKind, FactRecord};
use crate::ports::tasks::{TaskRecord, TaskTitle};
use crate::ports::types::{CompanyId, CompressedTrace};

/// Issue #339: the card's output link reaches the console on **both** reads.
///
/// The board read matters as much as task detail here, and that is the whole
/// point: the link is rendered on the card, so a board that had to open every
/// card to discover what it produced would cost N reads per four-second poll.
///
/// A card that never succeeded omits the key entirely rather than sending
/// `null`, so the pre-#339 wire shape is unchanged for every card the board
/// created — which is also what the console reads as "link to the card itself".
#[tokio::test]
async fn a_stamped_card_hands_its_output_link_to_both_reads() {
    use crate::ports::artifacts::ArtifactKind;
    use crate::ports::tasks::{
        TaskOutput, TaskOutputAction, TaskOutputArtifact, TaskOutputSource, TaskOutputWorkflow,
    };

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    // A card the board created: no attempt has run, so no link.
    let (_, plain) = send(
        &state,
        "POST",
        "/api/v1/company/tasks",
        Some(json!({"title": "Typed on the board"})),
    )
    .await;
    assert!(
        plain.get("output").is_none(),
        "a card that never succeeded must not grow the key: {plain}"
    );
    let id = plain["id"].as_str().unwrap().to_string();

    // Stamp it the way a successful settle does, through the plain store port.
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).unwrap();
    let mut card = runtime
        .tasks()
        .list(&company)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.id == id)
        .expect("card");
    card.output = Some(TaskOutput {
        source: TaskOutputSource::Run {
            run_id: "run-2".to_string(),
            attempt: Some(2),
        },
        at_millis: 42,
        artifacts: vec![TaskOutputArtifact {
            artifact_id: "a-1".to_string(),
            version: 3,
            title: "Launch spec".to_string(),
            kind: ArtifactKind::Markdown,
        }],
        workflows: vec![TaskOutputWorkflow {
            workflow_id: "digest".to_string(),
            run_id: Some("wf-1".to_string()),
            action: TaskOutputAction::Ran,
        }],
    });
    runtime.tasks().upsert(&company, &card).await.unwrap();

    // The board read — the one the card's own link is rendered from.
    let (_, board) = send(&state, "GET", "/api/v1/company/tasks", None).await;
    let listed = board
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == json!(id))
        .expect("the card is on the board");
    assert_eq!(listed["output"]["runId"], "run-2");
    assert_eq!(listed["output"]["attempt"], 2);
    assert_eq!(listed["output"]["artifacts"][0]["artifactId"], "a-1");
    assert_eq!(
        listed["output"]["artifacts"][0]["version"], 3,
        "the link must carry the version the run wrote, not just the record"
    );
    assert_eq!(listed["output"]["artifacts"][0]["kind"], "markdown");
    assert_eq!(listed["output"]["workflows"][0]["workflowId"], "digest");
    assert_eq!(listed["output"]["workflows"][0]["action"], "ran");

    // …and task detail, where the operator opens what the link points at.
    let (status, detail) = send(&state, "GET", &format!("/api/v1/company/tasks/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["task"]["output"]["runId"], "run-2");
    assert_eq!(detail["task"]["output"]["artifacts"][0]["version"], 3);
}

/// Issue #246 spend gate, at the HTTP boundary. The transcript's "Add to
/// board" action omits `column` on purpose so the *server* decides where a
/// chat-created card lands — and the one thing that must never happen is that
/// it lands on the dispatch trigger, which spends an agent turn nobody
/// approved. `dispatch_task` is a no-op in this build (no harness attached),
/// so the load-bearing assertion is the landing column itself; the journal
/// check is the belt to that braces, and would catch a create that started
/// journaling a dispatch of its own.
#[tokio::test]
async fn a_chat_created_card_lands_off_the_dispatch_trigger() {
    use crate::ports::tasks::{COLUMN_IN_PROGRESS, COLUMN_TODO};
    use crate::ports::types::CompanyEvent;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();

    let (status, created) = send(
        &state,
        "POST",
        "/api/v1/company/tasks",
        Some(json!({"title": "Draft the announcement", "originChatId": "main"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Asserted on `stage`, not on `column`: since issue #1512 the DTO's
    // `column` is the phase, and `working` covers both the dispatched stage and
    // three that are not — so a phase assertion could not tell "dispatched"
    // from "being planned", which is the only thing this test is about.
    assert_ne!(
        created["stage"], COLUMN_IN_PROGRESS,
        "a chat-created card must never arrive already dispatched"
    );
    assert_eq!(
        created["column"],
        crate::ledger::board::PHASE_PENDING,
        "it lands in the board's intake lane, where the human drag is the gate"
    );
    assert_eq!(created["stage"], serde_json::Value::Null, "{created}");
    let _ = COLUMN_TODO;

    // Issue #301 added a second pre-dispatch column and made To-do the only
    // intake lane, so the spend gate is re-checked across every creation shape
    // the board can produce: the bare board `+` (which now sends nothing but a
    // prompt-derived title) and an explicit `planning`. Neither may dispatch —
    // `planning` in particular is *not* a dispatch trigger, which is the whole
    // reason it can ship inert ahead of §4's auto-advance.
    for body in [
        json!({"title": "Typed on the board"}),
        json!({"title": "Being planned", "column": "planning"}),
    ] {
        let (status, created) = send(&state, "POST", "/api/v1/company/tasks", Some(body)).await;
        assert_eq!(status, StatusCode::OK);
        assert_ne!(created["stage"], COLUMN_IN_PROGRESS, "{created}");
    }

    let journal = runtime
        .events()
        .read_from(
            runtime.id(),
            crate::ports::types::EventSeq::new(0),
            usize::MAX,
        )
        .await
        .unwrap();
    assert!(
        !journal
            .iter()
            .any(|e| matches!(e.event, CompanyEvent::TaskDispatched { .. })),
        "creating a card must not dispatch it, whichever pre-dispatch column it lands in"
    );
}

#[tokio::test]
async fn steer_task_validates_statuses_and_journals_acceptance() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).unwrap();
    let endpoint = |key: &str| format!("/api/v1/company/tasks/{key}/steer");

    for body in [
        json!({"action": "unknown"}),
        json!({"action": "cancel"}),
        json!({"action": "redirect", "instruction": "   "}),
    ] {
        let (status, _) = send(&state, "POST", &endpoint("missing"), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    runtime
        .tasks()
        .upsert(
            &company,
            &TaskRecord {
                id: "idle".into(),
                title: TaskTitle::authored("Idle"),
                note: None,
                column: crate::ports::tasks::COLUMN_TODO.into(),
                priority: "medium".into(),
                assignee: String::new(),
                updated_at_millis: 1,
                origin: None,
                parent_task_id: None,
                output: None,
                plan: None,
                planning_attempts: Vec::new(),
                deliverable: crate::ports::tasks::TaskDeliverable::Once,
                workflow_proposal: None,
                origin_run_id: None,
                origin_workflow_id: None,
                origin_message_seq: None,
                bounced: None,
            },
        )
        .await
        .unwrap();
    let (status, _) = send(
        &state,
        "POST",
        &endpoint("idle"),
        Some(json!({"action": "pause"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = send(
        &state,
        "POST",
        &endpoint("missing"),
        Some(json!({"action": "pause"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let _delegation = runtime.steer().register(
        &company,
        InflightEntry {
            key: "delegation".into(),
            task_id: None,
            kind: InflightKind::Delegation,
            title: "Engineering".into(),
            agent_id: "ceo".into(),
            started_at_millis: 1,
            pending_action: None,
        },
    );
    let (status, _) = send(
        &state,
        "POST",
        &endpoint("delegation"),
        Some(json!({"action": "pause"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let _task = runtime.steer().register(
        &company,
        InflightEntry {
            key: "active".into(),
            task_id: Some("active".into()),
            kind: InflightKind::Task,
            title: "Active".into(),
            agent_id: "ceo".into(),
            started_at_millis: 2,
            pending_action: None,
        },
    );
    let (status, _) = send(
        &state,
        "POST",
        &endpoint("active"),
        Some(json!({"action": "redirect", "instruction": "focus on the API"})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let events = runtime
        .events()
        .read_from(&company, crate::ports::types::EventSeq::new(0), usize::MAX)
        .await
        .unwrap();
    assert!(events.iter().any(|stored| matches!(
        &stored.event,
        crate::ports::types::CompanyEvent::TaskSteered {
            task_id,
            action,
            instruction: Some(instruction),
            ..
        } if task_id == "active" && action == "redirect" && instruction == "focus on the API"
    )));
}

#[tokio::test]
async fn memory_create_and_delete_journals_event() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, fact) = send(
        &state,
        "POST",
        "/api/v1/company/memory",
        Some(json!({"kind": "preference", "title": "Tone", "body": "Warm"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fact["kind"], "preference");
    let id = fact["id"].as_str().unwrap().to_string();

    let (status, _) = send(
        &state,
        "DELETE",
        &format!("/api/v1/company/memory/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn memory_traces_are_inspectable_newest_last() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();

    for (cycle_id, summary, at_millis) in [
        ("cycle-1", "first completed cycle", 1_000),
        ("cycle-2", "second completed cycle", 2_000),
    ] {
        runtime
            .memory
            .save_trace(
                runtime.id(),
                CompressedTrace {
                    cycle_id: cycle_id.into(),
                    summary: summary.into(),
                    at_millis,
                },
            )
            .await
            .unwrap();
    }

    let (status, traces) = send(&state, "GET", "/api/v1/company/memory/traces", None).await;
    assert_eq!(status, StatusCode::OK);
    let traces = traces.as_array().unwrap();
    assert_eq!(traces.len(), 2);
    assert_eq!(traces[0]["cycleId"], "cycle-1");
    assert_eq!(traces[0]["summary"], "first completed cycle");
    assert_eq!(traces[0]["atMillis"], 1_000);
    assert_eq!(traces[1]["cycleId"], "cycle-2");
}

#[tokio::test]
async fn memory_list_filters_stats_and_dual_write() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();

    // Seed three facts with controlled, distinct timestamps so newest-first is
    // deterministic (the HTTP create path stamps `now_millis`, which can tie
    // across rapid inserts). Seeding straight into the FactStore also means
    // these do NOT create ContextStore mirrors — only the HTTP create path does.
    let seed = [
        ("f-old", FactKind::Fact, "Alpha channel report", 1_000u64),
        ("f-mid", FactKind::Preference, "Warm tone", 2_000),
        ("f-new", FactKind::Person, "Priya contact", 3_000),
    ];
    for (id, kind, title, ts) in seed {
        runtime
            .facts()
            .upsert(
                runtime.id(),
                &FactRecord {
                    id: id.into(),
                    kind,
                    title: title.into(),
                    body: "detail".into(),
                    source: "Seed".into(),
                    updated_at_millis: ts,
                },
            )
            .await
            .unwrap();
    }

    // List reflects the store, newest-first.
    let (status, rows) = send(&state, "GET", "/api/v1/company/memory", None).await;
    assert_eq!(status, StatusCode::OK);
    let rows = rows["items"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["id"], "f-new");
    assert_eq!(rows[2]["id"], "f-old");

    // `?kind=` narrows to one taxonomy.
    let (status, pref) = send(
        &state,
        "GET",
        "/api/v1/company/memory?kind=preference",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pref = pref["items"].as_array().unwrap();
    assert_eq!(pref.len(), 1);
    assert_eq!(pref[0]["id"], "f-mid");

    // `?query=` is a case-insensitive substring over title + body.
    let (status, hit) = send(&state, "GET", "/api/v1/company/memory?query=priya", None).await;
    assert_eq!(status, StatusCode::OK);
    let hit = hit["items"].as_array().unwrap();
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0]["id"], "f-new");

    // Stats over the seeded facts: 3 display items, freshest timestamp, no
    // teammate memory yet (seeding bypassed the mirror), and 0 task outcomes.
    let (status, stats) = send(&state, "GET", "/api/v1/company/memory/stats", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats["facts"], 3);
    assert_eq!(stats["factsUpdatedAtMillis"], 3_000);
    assert_eq!(stats["totalItems"], 3);
    assert_eq!(stats["teammateMemory"], 0);
    assert_eq!(stats["taskOutcomes"], 0);
    assert_eq!(stats["documentMemory"], 0);
    // Nothing but facts so far, so "Last updated" tracks the newest fact.
    assert_eq!(stats["lastUpdatedAtMillis"], 3_000);

    // Dual-write: the HTTP create path mirrors the fact into the ContextStore so
    // the agent can recall it. A direct search finds the mirrored text — the
    // fix that closes the operator manual-ingest loop.
    let (status, _) = send(
        &state,
        "POST",
        "/api/v1/company/memory",
        Some(json!({"kind": "fact", "title": "Launch date", "body": "ships on Friday"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let hits = runtime
        .context
        .search(runtime.id(), "Friday", 5)
        .await
        .unwrap();
    assert!(
        hits.iter().any(|h| h.snippet.contains("ships on Friday")),
        "an operator fact must be mirrored into the ContextStore for agent recall"
    );

    // The mirror stays agent-recallable but is not a display item of teammate
    // memory — the fact is the one row the operator sees.
    let (status, stats) = send(&state, "GET", "/api/v1/company/memory/stats", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stats["facts"], 4);
    assert_eq!(stats["totalItems"], 4);
    assert_eq!(stats["teammateMemory"], 0);
    assert_eq!(stats["taskOutcomes"], 0);
    assert_eq!(stats["documentMemory"], 0);
}
