use super::tests_core::*;
use super::tests_core2::*;
use super::*;

/// The default scope is byte-identical to pre-#374 behaviour.
///
/// The existing suite passing untouched is the real proof; this pins the
/// negative the suite cannot state — that no number of ordinary approvals
/// ever *infers* a standing grant. A "we noticed you approve this a lot"
/// heuristic is the silent accumulation the issue forbids.
#[tokio::test]
async fn repeated_ordinary_approvals_never_infer_a_standing_grant() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let effect = grantable_effect("ops", "file_write", serde_json::json!({ "path": "a" }));

    let rt = Arc::new(
        RuntimeBuilder::new(home, manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain {
                effect: effect.clone(),
            }))
            .build()
            .await
            .unwrap(),
    );

    for _ in 0..5 {
        let report = rt
            .run_cycle(vec![CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                parent: None,
                text: "do it".into(),
                by: None,
                chat: None,
                deliverable: None,
                attachments: Vec::new(),
            }])
            .await
            .unwrap();
        let id = report.parked[0].clone();
        rt.resolve_approval(&id, Verdict::Approve, operator())
            .await
            .unwrap();
    }

    assert_eq!(
        rt.grants.standing_count(),
        0,
        "a standing grant is only ever asked for, never inferred"
    );
}

/// Standing grants survive a restart, and revoking one is durable too.
///
/// The reboots here take the path `serve` takes — `RuntimeBuilder::build`
/// and nothing else. `recover()` is not called, because no production
/// caller calls it.
#[tokio::test]
async fn a_standing_grant_replays_on_boot_and_a_revoked_one_does_not() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (rt, id) = park_one_blocked_tool_call(
        home.clone(),
        grantable_effect("ops", "file_write", serde_json::json!({ "path": "a" })),
    )
    .await;

    let (_, follow_up) = rt
        .resolve_approval_spawned(&id, Verdict::Approve, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;
    let grant_id = rt.standing_grants()[0].id.clone();

    // A fresh runtime over the same home rehydrates it.
    let rt2 = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("supervised"))
            .build()
            .await
            .unwrap(),
    );
    assert_eq!(rt2.grants.standing_count(), 1);
    assert_eq!(rt2.standing_grants()[0].id, grant_id);

    // Revoke, then boot again: it must stay gone.
    assert!(
        rt2.revoke_standing_grant(&grant_id, operator())
            .await
            .unwrap()
    );
    assert_eq!(rt2.grants.standing_count(), 0);
    assert!(
        !rt2.revoke_standing_grant(&grant_id, operator())
            .await
            .unwrap(),
        "revoking twice reports nothing to revoke"
    );

    let rt3 = Arc::new(
        RuntimeBuilder::new(home, manifest("supervised"))
            .build()
            .await
            .unwrap(),
    );
    assert_eq!(
        rt3.grants.standing_count(),
        0,
        "a restart must not hand back a permission the operator took away"
    );
}

/// The maintenance sweep retires a lapsed standing grant and journals it.
#[tokio::test]
async fn the_sweep_expires_a_lapsed_standing_grant() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect("ops", "file_write", serde_json::json!({})),
    )
    .await;

    // Already past its deadline the moment it is minted.
    let (_, follow_up) = rt
        .resolve_approval_spawned(
            &id,
            Verdict::Approve,
            operator(),
            GrantScope::Tool {
                expires_at_millis: 1,
            },
        )
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;
    assert_eq!(rt.grants.standing_count(), 1);

    rt.sweep_expired_grants().await.unwrap();
    assert_eq!(rt.grants.standing_count(), 0);
}

/// The summary carries the flag only where the control is actually
/// offerable — and what the tool can reach is what decides it.
#[tokio::test]
async fn the_summary_marks_only_broadly_grantable_cards() {
    let home_dir = tmp_home();
    let (rt, _) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect("ops", "file_write", serde_json::json!({})),
    )
    .await;
    assert!(rt.pending_approvals()[0].broadly_grantable);

    // A Composio call with no action slug the classifier recognises reads
    // as a send, so no scope control is offered (issue #441's cautious
    // direction — before it, *every* Composio call landed here, including
    // the reads).
    let home_dir = tmp_home();
    let (rt, _) = park_one(
        home_dir.path().to_path_buf(),
        harness_effect("finance", "composio_execute", serde_json::json!({})),
    )
    .await;
    assert!(!rt.pending_approvals()[0].broadly_grantable);

    // Issue #444: `workspace_write` used to be marked grantable, because
    // its name carries no consequence word. It overwrites guidance the
    // operator wrote, so it stays a per-call decision — the same answer
    // the parking side of the gate has always given for it.
    let home_dir = tmp_home();
    let (rt, _) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect("ops", "workspace_write", serde_json::json!({ "path": "a" })),
    )
    .await;
    assert!(
        !rt.pending_approvals()[0].broadly_grantable,
        "overwriting operator-owned guidance is not a week-long permission"
    );

    // And neither is running an arbitrary command, which is where an
    // operator on staging *could* get a standing grant before #444.
    let home_dir = tmp_home();
    let (rt, _) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect("ops", "shell", serde_json::json!({ "command": "ls" })),
    )
    .await;
    assert!(!rt.pending_approvals()[0].broadly_grantable);
}

/// Issue #441, from the mint side: the same tool, two different answers,
/// decided by the action in the arguments rather than the name they share.
///
/// This is the whole shape of the bug — an operator could grant a standing
/// scope on running arbitrary terminal commands, and could not grant one on
/// reading a repository's pull requests.
#[tokio::test]
#[cfg(feature = "openhuman")]
async fn a_composio_read_is_offerable_and_a_composio_send_is_not() {
    let home_dir = tmp_home();
    let (rt, _) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect(
            "ops",
            "composio_execute",
            serde_json::json!({ "tool": "GITHUB_LIST_PULL_REQUESTS" }),
        ),
    )
    .await;
    assert!(
        rt.pending_approvals()[0].broadly_grantable,
        "a repository read scoped to a connected account is grantable"
    );

    let home_dir = tmp_home();
    let (rt, _) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect(
            "ops",
            "composio_execute",
            serde_json::json!({ "tool": "GMAIL_SEND_EMAIL" }),
        ),
    )
    .await;
    assert!(
        !rt.pending_approvals()[0].broadly_grantable,
        "sending mail stays a per-call decision"
    );
}

// ── Issue #983: the pre-journaled entry point ────────────────────────────

/// Every input of a cycle appears in the journal **exactly once**, whichever
/// entry point drove it.
///
/// This is the pin the plumbing change is worth having. `run_cycle` is what
/// every other trigger in the tree uses — the scheduler, cron, webhooks, the
/// telegram poller, delegation, approval follow-ups — so the append it does
/// must stay exactly one per input; and `run_journaled_cycle`, which exists
/// so the chat route can append at accept time instead, must do none. Either
/// half getting it wrong is invisible at the call site and shows up as a
/// duplicated (or missing) message in somebody's transcript.
///
/// It also pins `CycleReport::input_seqs`, and therefore the chat response's
/// `messageId`: the pre-journaled path reports back the seqs it was handed,
/// not seqs of its own.
#[tokio::test]
async fn each_input_is_journaled_exactly_once_by_either_entry_point() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let rt = RuntimeBuilder::new(home, manifest("full"))
        .with_brain(Arc::new(CapturingBrain {
            seen: Arc::clone(&seen),
        }))
        .build()
        .await
        .unwrap();

    let ask = |text: &str| CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        text: text.to_string(),
        by: None,
        chat: None,
        parent: None,
        deliverable: None,
        attachments: Vec::new(),
    };
    let messages = |stored: &[crate::ports::types::StoredEvent]| -> Vec<String> {
        stored
            .iter()
            .filter_map(|s| match &s.event {
                CompanyEvent::OperatorMessage { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };

    // The appending wrapper: one line per input, and the report names them.
    let appended = rt.run_cycle(vec![ask("first")]).await.unwrap();
    let stored = rt
        .events()
        .read_from(rt.id(), EventSeq::new(0), 1_000)
        .await
        .unwrap();
    assert_eq!(
        messages(&stored),
        ["first"],
        "the appending entry point wrote its input exactly once"
    );
    assert_eq!(appended.input_seqs.len(), 1);
    assert_eq!(
        stored
            .iter()
            .find(|s| matches!(&s.event, CompanyEvent::OperatorMessage { text, .. } if text == "first"))
            .map(|s| s.seq),
        appended.input_seqs.first().copied(),
        "the reported seq is the one the message was appended under"
    );

    // The pre-journaled entry point: the caller's append is the only one.
    let pre = rt.events().append(rt.id(), ask("second")).await.unwrap();
    let journaled = rt
        .run_journaled_cycle(vec![(pre, ask("second"))], None)
        .await
        .unwrap();
    let stored = rt
        .events()
        .read_from(rt.id(), EventSeq::new(0), 1_000)
        .await
        .unwrap();
    assert_eq!(
        messages(&stored),
        ["first", "second"],
        "the pre-journaled entry point appended its input a second time"
    );
    assert_eq!(
        journaled.input_seqs,
        vec![pre],
        "the report carries the seq the caller supplied, not one of its own"
    );

    // And the brain saw both, so skipping the append did not skip the
    // input.
    //
    // By prefix, not equality: this is an identity check — did each input
    // reach the brain — and the brain's copy is where the cycle's in-memory
    // briefings land. Both messages here are unaddressed, which is the
    // General desk, so the second one arrives carrying the thread index for
    // the first (#1890 E). Asserting the exact bytes would make every
    // briefing this file adds a failure of a test about append counts.
    let seen = seen.lock().expect("seen").clone();
    assert_eq!(seen.len(), 2, "both inputs reached the brain: {seen:?}");
    assert!(seen[0].starts_with("first"), "{seen:?}");
    assert!(seen[1].starts_with("second"), "{seen:?}");
}

/// A pre-journaled cycle moves the caller's run row to `Running` **inside**
/// the serial lock, and leaves settling it to the caller.
///
/// Both halves matter. Starting the row outside the lock would make
/// `Running` mean "accepted" rather than "owns the lock", which is exactly
/// the queued-behind-another-turn wait an operator needs to see. And letting
/// the cycle's terminality backstop settle it would close the row while the
/// task that journals the turn's replies is still running.
#[tokio::test]
async fn a_journaled_cycle_starts_the_callers_run_and_leaves_it_running() {
    use crate::ports::runs::{NewRun, RunStatus};

    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home, manifest("full"))
        .build()
        .await
        .unwrap();
    rt.runs()
        .create_run(rt.id(), NewRun::for_chat("turn-1", "general", "ceo"))
        .await
        .unwrap();

    let seq = rt
        .events()
        .append(
            rt.id(),
            CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                text: "hello".into(),
                by: None,
                chat: None,
                parent: None,
                deliverable: None,
                attachments: Vec::new(),
            },
        )
        .await
        .unwrap();
    rt.run_journaled_cycle(
        vec![(
            seq,
            CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                text: "hello".into(),
                by: None,
                chat: None,
                parent: None,
                deliverable: None,
                attachments: Vec::new(),
            },
        )],
        Some("turn-1".to_string()),
    )
    .await
    .unwrap();

    let row = rt
        .runs()
        .get_run(rt.id(), "turn-1")
        .await
        .unwrap()
        .expect("the row survives the cycle");
    assert_eq!(
        row.status,
        RunStatus::Running,
        "the cycle started the row and must not have settled it"
    );
    assert_eq!(
        row.trigger_event_seq,
        Some(seq),
        "the row is stamped with the seq the caller supplied"
    );
}

/// **Issue #1739.** A cycle reports one `turn_finished`, and the operator's
/// own words are not in it.
///
/// The message text here is the thing the payload must never carry, so it is
/// deliberately distinctive: the assertion is a substring search over the
/// whole rendered event, which fails if any field ever starts holding
/// free-form text.
#[tokio::test]
async fn a_cycle_reports_its_shape_and_not_the_operators_words() {
    let home_dir = tmp_home();
    let recorder = Arc::new(crate::analytics::RecordingTracker::new());
    let rt = RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("full"))
        .with_analytics(recorder.clone())
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        text: "acquire Northwind Traders for 4.2 million".into(),
        by: None,
        chat: None,
        parent: None,
        deliverable: None,
        mentions: Vec::new(),
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    let turns: Vec<_> = recorder
        .events()
        .into_iter()
        .filter(|event| matches!(event, crate::analytics::Event::TurnFinished { .. }))
        .collect();
    assert_eq!(turns.len(), 1, "one cycle, one event: {turns:?}");

    match turns[0] {
        crate::analytics::Event::TurnFinished {
            trigger,
            outcome,
            failure,
            ..
        } => {
            assert_eq!(trigger, crate::analytics::Trigger::OperatorMessage);
            assert_eq!(outcome, crate::analytics::Outcome::Ok);
            assert_eq!(failure, None);
        }
        ref other => panic!("{other:?}"),
    }

    let rendered = format!("{:?}", turns[0]);
    assert!(
        !rendered.contains("Northwind"),
        "the operator's message reached the payload: {rendered}"
    );
}

/// `turn_finished` counts the effects the cycle actually performed.
///
/// These two numbers used to be read off `CycleReport`, which exists only
/// on the success path — so every failed cycle reported zero effects and
/// zero parked approvals, including one that executed an irreversible
/// effect and *then* hit an adapter error on the way out. That is a
/// systematic undercount of exactly the turns worth looking at.
///
/// They now come from the host, read before the fallible tail of
/// `run_locked` rather than after it. This covers the reading being
/// faithful: a cycle whose brain emits one effect reports one. The failure
/// case is covered by where the read happens — `*effects = host.counts()`
/// sits above `let result = result?;` and above every `?` that follows, so
/// no later error can reach the tracker with the counts unset.
#[tokio::test]
async fn a_cycle_reports_the_effects_it_actually_performed() {
    let home_dir = tmp_home();
    let recorder = Arc::new(crate::analytics::RecordingTracker::new());
    let effect = Effect {
        kind: "noop".into(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    };
    let rt = RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("full"))
        .with_brain(Arc::new(EffectBrain { effect }))
        .with_analytics(recorder.clone())
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        text: "do the thing".into(),
        by: None,
        chat: None,
        parent: None,
        deliverable: None,
        mentions: Vec::new(),
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    let turns: Vec<_> = recorder
        .events()
        .into_iter()
        .filter(|event| matches!(event, crate::analytics::Event::TurnFinished { .. }))
        .collect();
    assert_eq!(turns.len(), 1, "one cycle, one event: {turns:?}");

    match turns[0] {
        crate::analytics::Event::TurnFinished {
            effects_executed,
            approvals_parked,
            ..
        } => {
            assert_eq!(
                effects_executed + approvals_parked,
                1,
                "the cycle's one effect must be counted, executed or parked: {:?}",
                turns[0]
            );
        }
        ref other => panic!("{other:?}"),
    }
}

/// A bounced card states **why**, and the landing label comes from the
/// ledger rather than a fourth transcription of the column names.
#[test]
fn a_settled_line_names_the_landing_and_a_bounce_names_its_reason() {
    let mut card = TaskRecord {
        id: "t-1".to_string(),
        title: TaskTitle::authored("Draft the investor update"),
        note: None,
        column: crate::ports::tasks::COLUMN_IN_REVIEW.to_string(),
        priority: "medium".to_string(),
        assignee: "writer".to_string(),
        updated_at_millis: 0,
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
    };
    assert_eq!(
        settled_briefing_line(&card),
        "- Draft the investor update — finished → In review"
    );

    card.column = COLUMN_TODO.to_string();
    card.bounced = Some("the dispatch failed: provider timeout".to_string());
    assert_eq!(
        settled_briefing_line(&card),
        "- Draft the investor update — finished → To-do (the dispatch failed: provider \
timeout)",
        "without the reason, 'finished → To-do' reads as merely queued"
    );
}

/// The whole of what C repairs, end to end through the injector.
///
/// A card raised in a thread settles; the operator asks in that same
/// thread; the turn is handed the fact. And — the half that makes it worth
/// having — a card raised in a *sibling* thread of the same channel is not,
/// because a briefing that leaked across threads would undo sub-issue A one
/// message later.
#[tokio::test]
async fn a_settled_card_briefs_the_thread_that_raised_it_and_no_other() {
    let home_dir = tmp_home();
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
            .build()
            .await
            .unwrap(),
    );
    let id = rt.id().clone();
    let record = rt
        .store
        .load(&id)
        .await
        .unwrap()
        .expect("the company record");

    let mut mine = settled_card("t-mine", "Draft the launch email");
    mine.origin = TaskOrigin::new(Some("growth".to_string()), Some(EventSeq::new(41)));
    let mut sibling = settled_card("t-sibling", "Pull the Q3 CAC");
    sibling.origin = TaskOrigin::new(Some("growth".to_string()), Some(EventSeq::new(43)));
    // Raised in the same channel, but at channel level rather than in a
    // thread. `None` is a conversation of its own, not a wildcard.
    let mut channel_level = settled_card("t-channel", "Renew the domain");
    channel_level.origin = TaskOrigin::new(Some("growth".to_string()), None);
    for card in [&mine, &sibling, &channel_level] {
        rt.tasks().upsert(&id, card).await.unwrap();
    }

    let mut events = vec![operator_in_thread("growth", Some(41), "make it shorter")];
    CycleRunner::new(&rt)
        .inject_handed_task_awareness(
            &record,
            &mut events,
            &rt.tasks().list(&id).await.expect("list"),
        )
        .await;
    let text = message_text(&events[0]);

    assert!(
        text.contains(SETTLED_WORK_ANNOTATION),
        "the thread's own settled work is briefed: {text}"
    );
    assert!(text.contains("Draft the launch email"), "{text}");
    assert!(
        !text.contains("Pull the Q3 CAC"),
        "a sibling thread's work must not leak into this one: {text}"
    );
    assert!(
        !text.contains("Renew the domain"),
        "nor the channel-level conversation's: {text}"
    );
    // And the operator's own words survive the append, which is the whole
    // reason `operator_words` cuts on this marker.
    assert!(text.starts_with("make it shorter"), "{text}");
}
