use super::tests_core::*;
use super::tests_core2::*;
use super::*;

/// Issue #243: resolving an approval that is already gone is a no-op, not a
/// second resolution.
///
/// A double-clicked approve, a retried request, or two operators on the same
/// queue all hit this. Before the outcome enum, the second call could not be
/// told apart from a deny: the gate returned `None` either way, so the
/// runner appended a second `ApprovalResolved` journal record AND ran a
/// second follow-up cycle — a whole model turn spent re-announcing a
/// resolution the brain had already been given.
#[tokio::test]
async fn resolving_an_already_resolved_approval_is_a_deterministic_no_op() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let sign_effect = Effect {
        kind: "filing.submit".into(),
        group: EffectGroup::Sign,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    };
    let rt = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain {
                effect: sign_effect,
            }))
            .build()
            .await
            .unwrap(),
    );

    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "file it".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();
    let approval_id = report.parked[0].clone();

    rt.resolve_approval(&approval_id, Verdict::Approve, operator())
        .await
        .unwrap();
    let events_after_first = rt
        .events
        .read_from(rt.id(), EventSeq::new(0), 1000)
        .await
        .unwrap()
        .len();

    // The second submit.
    let again = rt
        .resolve_approval(&approval_id, Verdict::Approve, operator())
        .await
        .unwrap();

    assert_eq!(again.responses.len(), 1);
    assert_eq!(
        again.responses[0].text, "This approval was already resolved.",
        "the operator gets a deterministic line, not an error and not a re-run"
    );
    assert!(again.executed_effects.is_empty());
    assert!(again.parked.is_empty());
    assert!(
        again.persisted_seq.is_none(),
        "a no-op must not claim to have persisted anything"
    );
    assert_eq!(
        rt.events
            .read_from(rt.id(), EventSeq::new(0), 1000)
            .await
            .unwrap()
            .len(),
        events_after_first,
        "no second ApprovalResolved event, and no follow-up cycle behind it"
    );
}

/// Issue #172: an already-decided approval request parks and reaches the
/// operator's queue **without** being re-evaluated.
///
/// The company runs `full` autonomy and the effect classifies as `Other` —
/// the two conditions under which `ApprovalGate::evaluate` returns `Allow`.
/// Had the request gone through `emit_effect` it would have been "executed"
/// as a no-op and vanished, which is exactly how a chat-gated tool call used
/// to disappear before ever reaching the Approvals page.
#[tokio::test]
async fn a_decided_request_parks_without_being_re_evaluated() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let tool_effect = Effect {
        kind: "composio_execute".into(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: crate::policy::test_support::composio_send_args(),
        agent: None,
        run_id: None,
    };
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(ParkingBrain {
            effect: tool_effect.clone(),
        }))
        .build()
        .await
        .unwrap();

    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "send that email".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();

    assert_eq!(report.parked.len(), 1, "the request parked");
    assert!(
        report.executed_effects.is_empty(),
        "a parked request must not execute"
    );

    // The Approvals page reads exactly this.
    let pending = rt.pending_approvals();
    assert_eq!(pending.len(), 1, "the operator sees the request");
    assert_eq!(pending[0].kind, "composio_execute");
    assert_eq!(pending[0].id, report.parked[0]);

    // And it is durable: a fresh runtime over the same home replays it, so a
    // restart does not lose what the operator still owes an answer to.
    let rt2 = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(ParkingBrain {
            effect: tool_effect,
        }))
        .build()
        .await
        .unwrap();
    assert_eq!(rt2.pending_approvals().len(), 1);
}

#[tokio::test]
async fn approval_survives_runtime_restart() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let sign_effect = Effect {
        kind: "filing.submit".into(),
        group: EffectGroup::Sign,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    };
    let approval_id = {
        let rt = RuntimeBuilder::new(home.clone(), manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain {
                effect: sign_effect.clone(),
            }))
            .build()
            .await
            .unwrap();
        let report = rt
            .run_cycle(vec![CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                parent: None,
                text: "file it".into(),
                by: None,
                chat: None,
                deliverable: None,
                attachments: Vec::new(),
            }])
            .await
            .unwrap();
        report.parked[0].clone()
    };

    // A fresh runtime over the same home rehydrates the parked approval and
    // can resolve it by its original id.
    let rt2 = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain {
                effect: sign_effect,
            }))
            .build()
            .await
            .unwrap(),
    );
    assert_eq!(rt2.pending_approvals().len(), 1);
    rt2.resolve_approval(&approval_id, Verdict::Deny, operator())
        .await
        .unwrap();
    assert!(rt2.pending_approvals().is_empty());
}

#[tokio::test]
async fn amend_then_approve_executes_edited_effect() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    // A parked Sign effect whose payload the operator will overwrite so the
    // executed effect routes an amended message to the operator channel.
    let sign_effect = Effect {
        kind: "filing.submit".into(),
        group: EffectGroup::Sign,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::json!({ "channel": "operator", "text": "ORIGINAL" }),
        agent: None,
        run_id: None,
    };
    // A recording operator channel we keep a handle to (Arc-shared buffer).
    let operator_channel = OperatorChannel::new();
    let channels: Vec<Arc<dyn ChannelAdapter>> = vec![Arc::new(operator_channel.clone())];
    let rt = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain {
                effect: sign_effect,
            }))
            .with_channels(channels)
            .build()
            .await
            .unwrap(),
    );

    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "file it".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();
    let approval_id = report.parked[0].clone();

    // Approve with an edited payload: only `text` changes.
    let follow_up = rt
        .resolve_approval_amended(
            &approval_id,
            serde_json::json!({ "text": "AMENDED" }),
            operator(),
        )
        .await
        .unwrap();
    assert!(follow_up.parked.is_empty());
    assert!(rt.pending_approvals().is_empty());

    // The amended effect executed: the operator channel saw "AMENDED",
    // never the original "ORIGINAL" text.
    let sent = operator_channel.sent();
    assert!(
        sent.iter().any(|m| m.text == "AMENDED"),
        "amended text was routed, got {sent:?}"
    );
    assert!(sent.iter().all(|m| m.text != "ORIGINAL"));

    // The immutable journal records both the original park and the amend.
    let raw = tokio::fs::read_to_string(Bundle::new(&home, rt.id()).journal_jsonl())
        .await
        .unwrap();
    assert!(raw.contains("ApprovalParked"));
    assert!(raw.contains("ApprovalAmended"));
    assert!(raw.contains("AMENDED"));
}

#[tokio::test]
async fn sweep_expires_parked_approval_to_deny() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let sign_effect = Effect {
        kind: "filing.submit".into(),
        group: EffectGroup::Sign,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    };
    // A zero-TTL gate: anything parked is immediately past its deadline.
    let gate = Arc::new(
        ManifestApprovalGate::new(manifest("supervised").policy.clone()).with_ttl_millis(0),
    );
    let rt = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("supervised"))
            .with_brain(Arc::new(EffectBrain {
                effect: sign_effect,
            }))
            .with_approvals(gate)
            .build()
            .await
            .unwrap(),
    );

    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "file it".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();
    let approval_id = report.parked[0].clone();
    assert_eq!(rt.pending_approvals().len(), 1);

    // The maintenance sweep resolves the silent approval to a default-deny.
    let expired = rt.sweep_expired_approvals().await.unwrap();
    assert_eq!(expired, vec![approval_id]);
    assert!(rt.pending_approvals().is_empty());

    let raw = tokio::fs::read_to_string(Bundle::new(&home, rt.id()).journal_jsonl())
        .await
        .unwrap();
    assert!(raw.contains("ApprovalExpired"));
}

#[tokio::test]
async fn an_expired_explicit_request_returns_a_system_denial_to_its_agent() {
    let home_dir = tmp_home();
    let gate = Arc::new(
        ManifestApprovalGate::new(manifest("supervised").policy.clone()).with_ttl_millis(0),
    );
    let brain = Arc::new(ExplicitExpiryBrain::default());
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
            .with_brain(brain.clone())
            .with_approvals(gate)
            .build()
            .await
            .unwrap(),
    );

    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "file it".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();
    assert_eq!(report.parked.len(), 1);
    assert_eq!(rt.continuations.outstanding(&report.cycle_id), 1);

    rt.sweep_expired_approvals().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while brain.decisions.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the expiry denial reaches the asking agent; outstanding={}, continuation_live={}",
            rt.continuations.outstanding(&report.cycle_id),
            rt.grants.peek_continuation(&report.parked[0]).is_some()
        )
    });
    assert_eq!(brain.decisions.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// `resolve_approval_spawned` checks `ensure_not_emergency_stopped` before
/// this runs, but that ask sits behind at least one `.await` before
/// `settle_approval` is actually reached. A stop engaged in that window
/// must still be caught here, before a native effect executes or a grant
/// is minted, and the approval must come back out exactly as parked as it
/// went in — not resolved with nothing to show for it.
#[tokio::test]
async fn settle_approval_refuses_a_native_effect_once_the_stop_is_engaged() {
    let home_dir = tmp_home();
    let sign_effect = Effect {
        kind: "filing.submit".into(),
        group: EffectGroup::Sign,
        amount_usd: Some(42.0),
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    };
    let gate = Arc::new(ManifestApprovalGate::new(
        manifest("supervised").policy.clone(),
    ));
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
            .with_brain(Arc::new(EffectBrain {
                effect: sign_effect,
            }))
            .with_approvals(gate)
            .build()
            .await
            .unwrap(),
    );

    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "file it".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();
    let approval_id = report.parked[0].clone();
    assert_eq!(rt.pending_approvals().len(), 1);

    rt.emergency_pause(operator(), None).await.expect("pause");

    let refused = CycleRunner::new(&rt)
        .settle_approval(&approval_id, Verdict::Approve, operator(), GrantScope::Once)
        .await;
    assert!(
        matches!(refused, Err(crate::OpenCompanyError::EmergencyStop(_))),
        "settle_approval must refuse while the stop is engaged, got {refused:?}"
    );
    assert_eq!(
        rt.pending_approvals().len(),
        1,
        "a refused settle must leave the approval exactly as parked as before"
    );
    assert!(
        rt.grants.peek(&approval_id).is_none(),
        "a refused settle must not have minted a grant"
    );

    let raw = tokio::fs::read_to_string(
        Bundle::new(home_dir.path().to_path_buf(), rt.id()).journal_jsonl(),
    )
    .await
    .unwrap();
    assert!(
        !raw.contains("ApprovalResolved"),
        "a refused settle must not journal a resolution"
    );
}

// ── Issue #243: the agent/native fork in `settle_approved_effect` ────────

/// `settle_approved_effect`'s whole job is a fork on [`Effect::agent`]: a
/// harness tool call (`Some`) is never executed, only granted; a native
/// effect (`None`) is never granted, only executed. Both directions are
/// driven off the same `resolve_approval` entry point that the operator
/// actually calls, for two different actors, so a regression that made
/// either fork perform the other's action is caught at the seam a real
/// approval goes through rather than by calling the private fork directly.
#[tokio::test]
async fn settle_approved_effect_mints_for_a_tool_call_and_executes_a_native_effect() {
    let home_dir = tmp_home();

    // A harness tool call: `agent` is `Some`, so approving it must mint a
    // single-use grant and must NOT run `execute_effect_once` — a real
    // `amount_usd` on the effect is what proves that: `perform_effect`
    // would ledger it if the native path ran by mistake.
    let tool_effect = harness_effect("finance", "composio_execute", serde_json::json!({}));
    let (rt, id) = park_one(home_dir.path().to_path_buf(), tool_effect).await;
    let board_member = Actor {
        kind: ActorKind::Operator,
        id: "board-member".into(),
    };
    rt.resolve_approval(&id, Verdict::Approve, board_member)
        .await
        .expect("approving a tool call succeeds");
    assert!(
        rt.grants.peek(&id).is_some(),
        "a harness tool call must be granted, not executed"
    );
    assert!(
        !rt.journal.is_executed(&format!("approval:{id}")),
        "the native execution path must never run for an agent-tagged effect"
    );
    assert_eq!(
        rt.store()
            .load(rt.id())
            .await
            .unwrap()
            .unwrap()
            .ledger
            .len(),
        0,
        "granting a tool call must not ledger the spend the tool itself would have"
    );

    // A native effect: `agent` is `None`, so approving it must execute it
    // exactly once and must NOT mint a grant nobody can ever redeem.
    let home_dir2 = tmp_home();
    let native_effect = Effect {
        kind: "filing.submit".into(),
        group: EffectGroup::Sign,
        amount_usd: Some(42.0),
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    };
    let (rt2, id2) = park_one(home_dir2.path().to_path_buf(), native_effect).await;
    let finance_lead = Actor {
        kind: ActorKind::Operator,
        id: "finance-lead".into(),
    };
    rt2.resolve_approval(&id2, Verdict::Approve, finance_lead)
        .await
        .expect("approving a native effect succeeds");
    assert!(
        rt2.grants.peek(&id2).is_none(),
        "a native effect has no agent to grant to and must not mint one"
    );
    assert!(
        rt2.journal.is_executed(&format!("approval:{id2}")),
        "a native effect must actually be executed once approved"
    );
}

/// Issue #1863: a parked blocker's effect is inert — it carries a question,
/// not a tool call — and `agent` is `None` on it exactly like a native
/// effect. Without the `is_blocker_effect` guard at the top of
/// `settle_approved_effect`, an approving verdict on a blocker would fall
/// through to the native `agent.is_none()` arm and hand the blocker's
/// payload to `execute_effect_once`, ledgering a phantom spend and marking
/// the key executed while the question was never actually answered by
/// anything that could act on it.
#[tokio::test]
async fn an_inert_blocker_effect_is_never_handed_to_the_native_execution_path() {
    let home_dir = tmp_home();
    let blocker_effect = Effect {
        kind: format!(
            "{}.information",
            crate::ports::blockers::BLOCKER_EFFECT_PREFIX
        ),
        group: EffectGroup::Other,
        // A real spend amount, deliberately: it is what would prove a
        // misrouted blocker WAS executed if the guard were missing.
        amount_usd: Some(7.0),
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::json!({ "question": "which environment?" }),
        agent: None,
        run_id: None,
    };
    let (rt, id) = park_one(home_dir.path().to_path_buf(), blocker_effect).await;

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("resolving a blocker's parked effect succeeds");

    assert!(
        rt.grants.peek(&id).is_none(),
        "a blocker's inert effect must never be mistaken for redeemable authority"
    );
    assert!(
        !rt.journal.is_executed(&format!("approval:{id}")),
        "a blocker's inert effect must never reach execute_effect_once"
    );
}

// ── Issue #374: journal-before-live-set ordering on a standing/single-use mint ──

/// `mint_grant` journals `ApprovalGranted` and only then arms the grant in
/// the live `GrantSet` — never the other order. A crash that lands between
/// those two steps must therefore replay as **granted** on the next boot,
/// not lose the operator's decision: the durable record already exists,
/// only the in-memory arm never ran.
///
/// Modelled by writing the journal record the way `mint_grant` does but
/// deliberately skipping the in-memory `grants.grant` call — the exact
/// state a process death between the two lines would leave on disk — then
/// rebuilding the runtime from that journal the way a real restart does.
/// This drives the claim through `RuntimeBuilder`'s own rehydrate path
/// rather than the bare `JournalStore`, so it proves what the running
/// system actually does on boot, not just what the journal file contains.
#[tokio::test]
async fn a_grant_journaled_but_not_yet_armed_in_memory_still_replays_as_live() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("supervised"))
            .build()
            .await
            .unwrap(),
    );
    let grant = GrantedCall {
        approval_id: ApprovalId::new("appr-crash-window"),
        agent: "finance".into(),
        tool: "composio_execute".into(),
        args: serde_json::json!({ "to": "a@b.test" }),
        at_millis: now_millis(),
        origin_thread: None,
        origin_parent: None,
        origin_task: None,
    };
    // The durable half of `mint_grant`, run alone: the journal record
    // lands, but — modelling the crash — `self.rt.grants.grant(grant)`
    // never runs, so the live set never learns about it.
    rt.journal.record_granted(&grant).await.unwrap();
    assert_eq!(
        rt.grants.live_count(),
        0,
        "the in-memory arm deliberately did not run, modelling the crash window"
    );
    drop(rt);

    let restarted = RuntimeBuilder::fs_defaults(home, manifest("supervised"))
        .await
        .unwrap();
    assert!(
        restarted
            .grants
            .peek(&ApprovalId::new("appr-crash-window"))
            .is_some(),
        "a crash between the journal append and the live-set insert must replay as \
         granted, re-arming the permission the operator already gave rather than losing it"
    );
}
