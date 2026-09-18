use super::*;

/// Issue #301's whole migration, at the seam every persistence backend
/// shares. sqlite and mongodb store a card as a `task_json` string and the
/// fs bundle as a JSON array, so all three — plus export/import — parse
/// through this one `Deserialize`. A stored `backlog` card therefore heals
/// on read instead of failing `is_board_column` and vanishing from the
/// board.
#[test]
fn a_stored_backlog_card_reads_back_in_todo() {
    // A raw blob in exactly the shape a pre-#301 build persisted.
    let legacy = r#"{
        "id": "t-1",
        "title": "Bounced work",
        "note": "[operator] cancelled while in flight",
        "column": "backlog",
        "priority": "medium",
        "assignee": "maya",
        "updatedAtMillis": 7
    }"#;
    let migrated: TaskRecord = serde_json::from_str(legacy).expect("legacy card parses");
    assert_eq!(migrated.column, COLUMN_TODO);
    assert!(
        is_board_column(&migrated.column),
        "a migrated card must render on the board"
    );
    // The reason the collapse is lossless: the note survives untouched, so
    // "bounced back" is still readable on the card.
    assert_eq!(
        migrated.note.as_deref(),
        Some("[operator] cancelled while in flight")
    );

    // The next upsert persists the new literal — nothing re-writes it back.
    let round_tripped = serde_json::to_string(&migrated).unwrap();
    assert!(
        round_tripped.contains("\"column\":\"todo\""),
        "{round_tripped}"
    );

    // Migration is exactly one mapping; every live column is passed through
    // untouched, so a future column cannot be silently rewritten.
    for column in BOARD_COLUMNS {
        assert_eq!(migrate_column(column.to_string()), column);
    }
}

fn plain_card() -> TaskRecord {
    TaskRecord {
        id: "t-1".to_string(),
        title: TaskTitle::authored("Draft the spec"),
        note: None,
        column: COLUMN_IN_REVIEW.to_string(),
        priority: "medium".to_string(),
        assignee: "maya".to_string(),
        updated_at_millis: 7,
        origin: None,
        parent_task_id: None,
        output: None,
        // #339's baseline fixture stays baseline: it exists to prove the
        // output stamp round-trips against a card carrying nothing else.
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

/// Issue #1890 step 5: the origin is one value and **the same two keys**.
///
/// The whole claim of the type change is that no stored board migrates, so
/// this pins the bytes rather than the shape: a card raised in a thread
/// serializes to `originChatId` + `originParent` exactly as it did when
/// those were two loose fields, and a board card writes neither key.
#[test]
fn an_origin_serializes_to_the_two_keys_it_always_did() {
    let mut card = plain_card();
    card.origin = TaskOrigin::new(Some("engineering".to_string()), Some(EventSeq::new(41)));
    let json = serde_json::to_string(&card).expect("serializes");
    assert!(json.contains(r#""originChatId":"engineering""#), "{json}");
    assert!(json.contains(r#""originParent":41"#), "{json}");
    assert!(
        !json.contains(r#""origin":"#),
        "the value is flattened away"
    );
    assert_eq!(
        serde_json::from_str::<TaskRecord>(&json).expect("round trip"),
        card
    );

    // A channel-level card writes the desk and skips the thread, and a
    // board card writes neither — so an existing card's stored bytes are
    // unchanged rather than merely equivalent.
    card.origin = TaskOrigin::new(Some("engineering".to_string()), None);
    let channel = serde_json::to_string(&card).expect("serializes");
    assert!(
        channel.contains(r#""originChatId":"engineering""#),
        "{channel}"
    );
    assert!(!channel.contains("originParent"), "{channel}");

    card.origin = None;
    let board = serde_json::to_string(&card).expect("serializes");
    assert!(!board.contains("originChatId"), "{board}");
    assert!(!board.contains("originParent"), "{board}");
}

/// A stored card carrying the **drifted pair** loads with no origin at all.
///
/// A thread root beside no desk names no conversation. It was reachable
/// while these were two independent fields — #1890 B stamped the parent
/// from the raising message's own `parent` and D then changed what an
/// unparented message means — and a card in that state settled its marker
/// somewhere its thread could not see. `TaskOrigin` cannot represent it, so
/// the orphan is dropped on read instead of being carried forward.
#[test]
fn a_thread_root_without_a_desk_is_not_a_conversation() {
    let drifted = r#"{
        "id": "t-1",
        "title": "Draft the spec",
        "column": "in_review",
        "priority": "medium",
        "assignee": "maya",
        "updatedAtMillis": 7,
        "originParent": 41
    }"#;
    let card: TaskRecord = serde_json::from_str(drifted).expect("drifted card parses");
    assert!(card.origin.is_none(), "a parent alone is not an origin");
    assert_eq!(card.origin_chat_id(), None);
    assert_eq!(card.origin_parent(), None);
    assert!(
        !serde_json::to_string(&card)
            .expect("serializes")
            .contains("originParent"),
        "the orphan is dropped on read, not carried forward"
    );
}

/// Issue #661 (M5): the run reference round-trips as camelCase, and — the
/// part that matters — a card written before it existed still loads.
///
/// The legacy half is the load-bearing one. All three backends persist a card
/// as an opaque JSON blob, so `#[serde(default)]` is the entire migration:
/// a board written yesterday must deserialize today with both ids `None`,
/// not fail to load. And a card that carries no run reference must serialize
/// **without the keys at all**, so every existing card's stored bytes are
/// unchanged rather than merely equivalent.
#[test]
fn a_run_reference_round_trips_and_is_absent_on_a_legacy_card() {
    let mut card = plain_card();
    card.origin_run_id = Some("run-9".to_string());
    card.origin_workflow_id = Some("digest".to_string());

    let json = serde_json::to_string(&card).expect("serialize");
    assert!(json.contains("\"originRunId\":\"run-9\""), "{json}");
    assert!(json.contains("\"originWorkflowId\":\"digest\""), "{json}");
    let back: TaskRecord = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, card);

    // A card with no run reference keeps the pre-#661 wire shape exactly.
    let plain = plain_card();
    let json = serde_json::to_string(&plain).expect("serialize");
    assert!(!json.contains("originRunId"), "{json}");
    assert!(!json.contains("originWorkflowId"), "{json}");

    // And a payload written before the fields existed still loads.
    let legacy = serde_json::json!({
        "id": "t-legacy",
        "title": "Older card",
        "column": "todo",
        "priority": "medium",
        "assignee": "",
        "updatedAtMillis": 3
    });
    let loaded: TaskRecord = serde_json::from_value(legacy).expect("a pre-#661 card loads");
    assert_eq!(loaded.origin_run_id, None);
    assert_eq!(loaded.origin_workflow_id, None);
}

/// Issue #339: the whole stamp round-trips as camelCase, and — the part
/// that matters — a card written before it existed still loads, with
/// `None`. All three backends persist a card as an opaque JSON blob, so a
/// `#[serde(default)]` field is the entire migration.
#[test]
fn an_output_stamp_round_trips_and_is_absent_on_a_legacy_card() {
    let mut card = plain_card();
    card.output = Some(TaskOutput {
        source: TaskOutputSource::Run {
            run_id: "run-2".to_string(),
            attempt: Some(2),
        },
        at_millis: 99,
        artifacts: vec![TaskOutputArtifact {
            artifact_id: "a-1".to_string(),
            version: 3,
            title: "Launch spec".to_string(),
            kind: ArtifactKind::Markdown,
        }],
        workflows: vec![TaskOutputWorkflow {
            workflow_id: "digest".to_string(),
            run_id: Some("wf-run-1".to_string()),
            action: TaskOutputAction::Ran,
        }],
    });

    let json = serde_json::to_string(&card).expect("serialize");
    assert!(json.contains(r#""runId":"run-2""#), "{json}");
    assert!(json.contains(r#""artifactId":"a-1""#), "{json}");
    assert!(json.contains(r#""action":"ran""#), "{json}");
    assert_eq!(card, serde_json::from_str(&json).expect("round trip"));

    // Exactly the blob a pre-#339 build persisted: no `output` key at all.
    let legacy = r#"{
        "id": "t-1",
        "title": "Draft the spec",
        "column": "done",
        "priority": "medium",
        "assignee": "maya",
        "updatedAtMillis": 7
    }"#;
    let loaded: TaskRecord = serde_json::from_str(legacy).expect("a legacy card parses");
    assert_eq!(
        loaded.output, None,
        "a card that never recorded an attempt must not be given a synthesized one"
    );
    // And an unstamped card carries no empty scaffolding on the wire.
    let round_tripped = serde_json::to_string(&loaded).expect("serialize");
    assert!(!round_tripped.contains("output"), "{round_tripped}");
}

/// The trace case, pinned as its own test because it is the acceptance
/// criterion most easily lost: *"every card has a link including tasks that
/// produced no file."* An output with neither artifacts nor workflows is a
/// complete, valid stamp — the attempt is the deliverable — so `runId` must
/// survive a round trip on its own, with both lists omitted entirely.
#[test]
fn a_task_that_produced_no_file_still_carries_a_link() {
    let mut card = plain_card();
    card.output = Some(TaskOutput {
        source: TaskOutputSource::Run {
            run_id: "run-1".to_string(),
            attempt: Some(1),
        },
        at_millis: 5,
        artifacts: Vec::new(),
        workflows: Vec::new(),
    });

    let json = serde_json::to_string(&card).expect("serialize");
    assert!(json.contains(r#""runId":"run-1""#), "{json}");
    assert!(!json.contains("artifacts"), "{json}");
    assert!(!json.contains("workflows"), "{json}");

    let back: TaskRecord = serde_json::from_str(&json).expect("round trip");
    let output = back.output.expect("the stamp survives with no deliverable");
    assert_eq!(output.source.run_id(), Some("run-1"));
    assert!(output.artifacts.is_empty());
    assert!(output.workflows.is_empty());
}

/// The attempt ordinal is a label, not an identity: a stamp written when
/// the run row could not be read still addresses its attempt.
#[test]
fn an_unknown_attempt_ordinal_still_leaves_an_addressable_link() {
    let output = TaskOutput {
        source: TaskOutputSource::Run {
            run_id: "run-9".to_string(),
            attempt: None,
        },
        at_millis: 5,
        artifacts: Vec::new(),
        workflows: Vec::new(),
    };
    let json = serde_json::to_string(&output).expect("serialize");
    assert!(!json.contains("attempt"), "{json}");
    let back: TaskOutput = serde_json::from_str(&json).expect("round trip");
    assert_eq!(back.source.run_id(), Some("run-9"));
    assert_eq!(back.source.attempt(), None);
}

// --- issue #806: an output whose producer is not a run --------------------

/// **The migration guarantee.** Every card written before `TaskOutputSource`
/// existed carries a bare `runId` + `attempt` pair, and there is no dual-read
/// anywhere — so if this stops deserializing into `Run`, every stamped card
/// in every store silently loses its link.
///
/// The literal here is deliberately hand-written rather than produced by
/// serializing the current type: a test that round-trips today's shape would
/// still pass if both halves drifted together, which is exactly the failure
/// it is supposed to catch.
#[test]
fn a_stamp_written_before_the_source_union_still_reads_as_a_run() {
    let stored = r#"{"runId":"run-7","attempt":2,"atMillis":1234}"#;
    let output: TaskOutput =
        serde_json::from_str(stored).expect("a pre-#806 stamp must still load");
    assert_eq!(output.source.run_id(), Some("run-7"));
    assert_eq!(output.source.attempt(), Some(2));
    assert_eq!(output.at_millis, 1234);
}

/// And the other direction: a `Run` source must still *write* those exact
/// keys, so a card stamped by this build is readable by anything that has
/// not been updated — and by the console's `"runId" in output` discriminator.
#[test]
fn a_run_source_serializes_to_the_keys_it_always_did() {
    let output = TaskOutput {
        source: TaskOutputSource::Run {
            run_id: "run-7".to_string(),
            attempt: Some(2),
        },
        at_millis: 1234,
        artifacts: Vec::new(),
        workflows: Vec::new(),
    };
    let json = serde_json::to_string(&output).expect("serialize");
    assert!(json.contains(r#""runId":"run-7""#), "{json}");
    assert!(json.contains(r#""attempt":2"#), "{json}");
    assert!(
        !json.contains("chatId") && !json.contains("source"),
        "the union must be flattened, not nested or tagged: {json}"
    );
}

/// A chat turn's stamp carries the conversation and **no** run — the whole
/// point of #806. `run_id()` answering `None` is the truthful answer to a
/// question about runs, and is what stops a reader labelling a conversation
/// as an attempt.
#[test]
fn a_chat_turn_stamp_carries_a_conversation_and_no_run() {
    let output = TaskOutput {
        source: TaskOutputSource::ChatTurn {
            chat_id: "chat-3".to_string(),
        },
        at_millis: 9,
        artifacts: Vec::new(),
        workflows: vec![TaskOutputWorkflow {
            workflow_id: "wf-1".to_string(),
            run_id: None,
            action: TaskOutputAction::Created,
        }],
    };
    let json = serde_json::to_string(&output).expect("serialize");
    assert!(json.contains(r#""chatId":"chat-3""#), "{json}");
    assert!(
        !json.contains("runId\":\"chat"),
        "a chat turn must never be written as a run: {json}"
    );

    let back: TaskOutput = serde_json::from_str(&json).expect("round trip");
    assert_eq!(back.source.run_id(), None);
    assert_eq!(back.source.attempt(), None);
    assert_eq!(
        back.source,
        TaskOutputSource::ChatTurn {
            chat_id: "chat-3".to_string()
        }
    );
    assert_eq!(
        back.workflows.len(),
        1,
        "the deliverable it produced is what makes the link worth having"
    );
}

// --- The plan → workflow bridge (issue #580) -----------------------------

/// The deliverable enum's wire words are pinned — `once`/`workflow` are the
/// values the REST boundary validates and the operator's toggle sends. A
/// rename here is a deliberate edit, not an accident.
#[test]
fn the_deliverable_wire_words_are_stable() {
    assert_eq!(TaskDeliverable::default(), TaskDeliverable::Once);
    assert_eq!(TaskDeliverable::Once.as_str(), "once");
    assert_eq!(TaskDeliverable::Workflow.as_str(), "workflow");
    assert!(TaskDeliverable::Once.is_once());
    assert!(!TaskDeliverable::Workflow.is_once());
    // Serde uses the same lowercase words the operator's payload carries.
    assert_eq!(
        serde_json::to_string(&TaskDeliverable::Workflow).unwrap(),
        "\"workflow\""
    );
    let back: TaskDeliverable = serde_json::from_str("\"once\"").unwrap();
    assert_eq!(back, TaskDeliverable::Once);
}

/// The whole additive-wire contract for #580: a card written before it loads
/// as a one-off with no proposal, a one-off card stays byte-identical to a
/// pre-#580 card (no `deliverable` key), and a workflow card with a proposal
/// round-trips intact. This is what makes "no migration on any backend" true.
#[test]
fn the_deliverable_and_proposal_fields_are_additive_on_the_wire() {
    // A pre-#580 blob: no `deliverable`, no `workflowProposal`.
    let legacy = r#"{
        "id": "t-1",
        "title": "Unbridged work",
        "column": "todo",
        "priority": "medium",
        "assignee": "maya",
        "updatedAtMillis": 7
    }"#;
    let card: TaskRecord = serde_json::from_str(legacy).expect("a pre-#580 card parses");
    assert_eq!(card.deliverable, TaskDeliverable::Once);
    assert!(card.workflow_proposal.is_none());

    // A once card grows neither key — byte-identical to the pre-#580 shape.
    let round_tripped = serde_json::to_string(&card).unwrap();
    assert!(
        !round_tripped.contains("deliverable"),
        "a once card must not grow a deliverable key: {round_tripped}"
    );
    assert!(
        !round_tripped.contains("workflowProposal"),
        "an unproposed card must not grow a proposal key: {round_tripped}"
    );

    // A workflow card carrying a proposal round-trips whole.
    let proposed = TaskRecord {
        planning_attempts: Vec::new(),
        deliverable: TaskDeliverable::Workflow,
        workflow_proposal: Some(TaskWorkflowProposal {
            summary: "Email the weekly digest every Monday".to_string(),
            ops: serde_json::json!({
                "id": "weekly-digest",
                "name": "Weekly digest",
                "nodes": [{ "id": "t", "kind": "trigger" }],
                "edges": []
            }),
            generated_at_millis: 99,
            run_id: "run-7".to_string(),
        }),
        column: COLUMN_IN_REVIEW.to_string(),
        ..card
    };
    let json = serde_json::to_string(&proposed).unwrap();
    assert!(json.contains("\"deliverable\":\"workflow\""), "{json}");
    assert!(json.contains("\"workflowProposal\":"), "{json}");
    assert!(json.contains("\"runId\":\"run-7\""), "{json}");
    let back: TaskRecord = serde_json::from_str(&json).expect("round trip");
    assert_eq!(back, proposed);
}
