use super::tests_core::*;
use super::tests_core2::*;
use super::*;

#[tokio::test]
async fn shutdown_registration_defers_replayed_approval_work_to_next_boot() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (rt, id) = park_one(
        home.clone(),
        harness_effect(
            "finance",
            crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND,
            serde_json::json!({
                "title": "Submit the filing",
                "question": "May I submit it?"
            }),
        ),
    )
    .await;
    CycleRunner::new(&rt)
        .settle_approval(&id, Verdict::Approve, operator(), GrantScope::Once)
        .await
        .unwrap();
    drop(rt);

    let brain = Arc::new(CountingBrain::default());
    let recovered = Arc::new(
        RuntimeBuilder::new(home, manifest("supervised"))
            .with_brain(brain.clone())
            .build()
            .await
            .unwrap(),
    );
    let registry = crate::runtime::CompanyRegistry::new();
    registry.begin_shutdown();
    registry.insert(recovered.id().clone(), recovered.clone());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(recovered.is_quiesced());
    assert_eq!(brain.calls(), 0);
    assert_eq!(recovered.journal.replayed_approval_continuations().len(), 1);
}

#[tokio::test]
async fn explicit_dispatch_claim_waits_for_every_sibling_decision() {
    let home_dir = tmp_home();
    let brain = Arc::new(CountingBrain::default());
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
            .with_brain(brain.clone())
            .build()
            .await
            .unwrap(),
    );
    let host = CycleHostImpl::new(
        rt.id().clone(),
        "explicit-batch".into(),
        &rt,
        None,
        false,
        ApprovalConversation::default(),
    );
    let first = host
        .park_effect(harness_effect(
            "finance",
            crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND,
            serde_json::json!({ "title": "First", "question": "First?" }),
        ))
        .await
        .unwrap();
    let second = host
        .park_effect(harness_effect(
            "finance",
            crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND,
            serde_json::json!({ "title": "Second", "question": "Second?" }),
        ))
        .await
        .unwrap();

    rt.resolve_approval(&first, Verdict::Deny, operator())
        .await
        .unwrap();
    assert_eq!(brain.calls(), 0, "one sibling is still pending");
    assert_eq!(rt.journal.replayed_approval_continuations().len(), 1);

    rt.resolve_approval(&second, Verdict::Deny, operator())
        .await
        .unwrap();
    assert_eq!(brain.calls(), 1, "the released batch runs one follow-up");
    assert!(rt.journal.replayed_approval_continuations().is_empty());
}

#[tokio::test]
async fn a_denied_explicit_request_from_a_workflow_node_returns_to_its_agent() {
    let home_dir = tmp_home();
    let brain = Arc::new(CountingBrain::default());
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
            .with_brain(brain.clone())
            .build()
            .await
            .unwrap(),
    );
    let turn = crate::runtime::workflow_resume::workflow_node_turn_key("run-1", "work");
    let host = CycleHostImpl::new(
        rt.id().clone(),
        turn,
        &rt,
        None,
        false,
        ApprovalConversation::default(),
    );
    let id = host
        .park_effect(harness_effect(
            "finance",
            crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND,
            serde_json::json!({
                "title": "Submit the filing",
                "question": "May I submit it?"
            }),
        ))
        .await
        .unwrap();

    rt.resolve_approval(&id, Verdict::Deny, operator())
        .await
        .unwrap();

    assert_eq!(
        brain.calls(),
        1,
        "the workflow-node fork must deliver the denial as an agent continuation"
    );
}

/// An approval that expired past its TTL grants nothing either, even though
/// the operator clicked approve — default-deny-on-silence wins, and it must
/// win here too or expiry would become a way to smuggle a live grant out of
/// a stale approval.
#[tokio::test]
async fn an_expired_approval_mints_nothing_even_on_approve() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let gate = Arc::new(
        ManifestApprovalGate::new(manifest("supervised").policy.clone()).with_ttl_millis(0),
    );
    let rt = Arc::new(
        RuntimeBuilder::new(home, manifest("supervised"))
            .with_approvals(gate)
            .with_brain(Arc::new(EffectBrain {
                effect: harness_effect("finance", "composio_execute", serde_json::json!({})),
            }))
            .build()
            .await
            .unwrap(),
    );
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
    assert_eq!(
        rt.grants.live_count(),
        0,
        "an expired approval is a deny, so it hands out no permission"
    );
}

/// **The assertion this whole fix exists for** (issue #1449).
///
/// The safety half was always right — an expired approval mints nothing, and
/// [`an_expired_approval_mints_nothing_even_on_approve`] pins that. What was
/// missing was the *reporting* half: the arm fell through to
/// `record_resolved`, so the immutable journal said **a named operator
/// approved this** about a call the host had already refused. That is a
/// false statement about a person, written permanently, on the surface whose
/// entire job is answering "who authorised this?".
///
/// So: after a late approve, the journal must carry the expiry — the same
/// line the sweeper writes when the identical outcome is reached by silence
/// — and must carry **no** `ApprovalResolved` for that id at all.
#[tokio::test]
async fn a_late_approve_is_journaled_as_an_expiry_never_as_the_operators_verdict() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (rt, id) = park_one_past_its_deadline(home.clone()).await;

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .unwrap();

    let raw = tokio::fs::read_to_string(Bundle::new(&home, rt.id()).journal_jsonl())
        .await
        .unwrap();
    let lines: Vec<serde_json::Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let about_this_approval = |record: &str| {
        lines.iter().any(|line| {
            line["record"] == record && line["id"] == serde_json::json!(id.as_ref() as &str)
        })
    };

    assert!(
        about_this_approval("ApprovalExpired"),
        "a late click leaves the SAME record as a deadline nobody noticed, got {raw}"
    );
    assert!(
        !about_this_approval("ApprovalResolved"),
        "the journal must never say this operator resolved an approval the \
         host had already default-denied, got {raw}"
    );
    assert!(
        !about_this_approval("ApprovalAmended"),
        "and it must not record an amendment either, got {raw}"
    );
    // The safety half, re-checked here rather than assumed: reporting the
    // truth is only half a fix if the grant came back.
    assert_eq!(rt.grants.live_count(), 0);
    assert!(rt.pending_approvals().is_empty());
}

/// The event log — what the brain and the operator's SSE feed read — must
/// agree with the journal (issue #1449).
///
/// Before this it received `ApprovalResolved { verdict: Approve, by: <the
/// operator> }`, so the agent was re-dispatched to make a call it had never
/// been granted, and the timeline named a person who approved nothing. An
/// expiry is a default-**deny** by the **system**, exactly as the sweeper
/// appends it.
#[tokio::test]
async fn a_late_approve_appends_a_system_deny_not_the_operators_approve() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_past_its_deadline(home_dir.path().to_path_buf()).await;

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .unwrap();

    let events = rt
        .events()
        .read_from(rt.id(), EventSeq::new(0), usize::MAX)
        .await
        .unwrap();
    let resolutions: Vec<_> = events
        .iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::ApprovalResolved {
                approval_id,
                verdict,
                by,
            } if approval_id == &id => Some((*verdict, by.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        resolutions.len(),
        1,
        "exactly one resolution event, got {resolutions:?}"
    );
    assert_eq!(resolutions[0].0, Verdict::Deny);
    assert_eq!(
        resolutions[0].1.kind,
        ActorKind::System,
        "the deadline decided this, not the person who clicked"
    );
}

/// The receipt says which end state was reached, so the HTTP layer can too.
#[tokio::test]
async fn a_late_approve_answers_with_an_expired_receipt() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_past_its_deadline(home_dir.path().to_path_buf()).await;

    let (receipt, follow_up) = rt
        .resolve_approval_spawned(&id, Verdict::Approve, operator(), GrantScope::Once)
        .await
        .unwrap();
    assert_eq!(receipt.outcome(), "expired");
    assert!(receipt.expired());
    assert!(
        !receipt.already_resolved(),
        "the approval WAS parked — it ran out, which is a different answer \
         from somebody else having decided it"
    );
    // And it owes no continuation of its own: `retire_approval` already
    // released the turn.
    let report = crate::company::runtime::join_follow_up(follow_up)
        .await
        .unwrap();
    assert!(report.responses[0].text.contains("deadline"));

    // A second click on the same card is now the ordinary already-gone case.
    let (again, _) = rt
        .resolve_approval_spawned(&id, Verdict::Approve, operator(), GrantScope::Once)
        .await
        .unwrap();
    assert_eq!(again.outcome(), "already_resolved");
}

/// The amend half of the same defect: an edit applied after the deadline is
/// still not a decision, and recorded an `ApprovalAmended` on top of the
/// false approval before this (issue #1449).
#[tokio::test]
async fn a_late_amend_records_an_expiry_and_no_amendment() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (rt, id) = park_one_past_its_deadline(home.clone()).await;

    let (receipt, _) = rt
        .resolve_approval_amended_spawned(
            &id,
            serde_json::json!({ "to": "elsewhere@b.test" }),
            operator(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.outcome(), "expired");

    let raw = tokio::fs::read_to_string(Bundle::new(&home, rt.id()).journal_jsonl())
        .await
        .unwrap();
    assert!(raw.contains("ApprovalExpired"));
    assert!(
        !raw.contains("ApprovalAmended"),
        "an edit the host refused is not an amendment the operator made, got {raw}"
    );
    assert!(
        !raw.contains("elsewhere@b.test"),
        "and the edited arguments must not be recorded as approved, got {raw}"
    );
    assert_eq!(rt.grants.live_count(), 0);
}

/// A live grant survives a restart; a consumed one does not come back.
///
/// The window between approve and re-issue spans a model turn, so a deploy
/// inside it is ordinary — and a resurrected single-use grant is no longer
/// single-use.
#[tokio::test]
async fn grants_replay_on_boot_but_a_spent_one_does_not() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let args = serde_json::json!({ "to": "a@b.test" });
    let (rt, id) = park_one(
        home.clone(),
        harness_effect("finance", "composio_execute", args.clone()),
    )
    .await;
    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .unwrap();
    drop(rt);

    // Restart: the grant comes back.
    let rt2 = RuntimeBuilder::fs_defaults(home.clone(), manifest("supervised"))
        .await
        .unwrap();
    assert_eq!(rt2.grants.live_count(), 1);
    // Redeem it and journal the consumption the way a cycle would.
    assert!(
        rt2.grants
            .consume("finance", "composio_execute", &args)
            .is_some()
    );
    for spent in rt2.grants.drain_consumed() {
        rt2.journal
            .record_grant_consumed(&spent, None)
            .await
            .unwrap();
    }
    drop(rt2);

    // Restart again: the spent grant stays spent.
    let rt3 = RuntimeBuilder::fs_defaults(home, manifest("supervised"))
        .await
        .unwrap();
    assert_eq!(
        rt3.grants.live_count(),
        0,
        "a redeemed grant must not be re-armed by a restart"
    );
}

/// A restart inside the window between grant redemption and the cycle drain
/// must not re-arm the call.
#[tokio::test]
async fn an_undrained_consumption_stays_spent_during_a_restart() {
    struct ConsumingBrain {
        effect: Effect,
        grants: Arc<std::sync::Mutex<Option<crate::runtime::grants::GrantSet>>>,
        consumed: Arc<tokio::sync::Barrier>,
        release: Arc<tokio::sync::Barrier>,
    }

    #[async_trait]
    impl Brain for ConsumingBrain {
        async fn run_cycle(&self, req: CycleRequest, host: &dyn CycleHost) -> Result<CycleResult> {
            for event in &req.events {
                match event {
                    CompanyEvent::OperatorMessage { .. } => {
                        host.park_effect(self.effect.clone()).await?;
                    }
                    CompanyEvent::ApprovalResolved { .. } => {
                        let grants = self
                            .grants
                            .lock()
                            .expect("grant slot")
                            .clone()
                            .expect("runtime grant set installed");
                        assert!(
                            grants
                                .consume(
                                    "finance",
                                    "composio_execute",
                                    &serde_json::json!({ "to": "a@b.test" }),
                                )
                                .is_some(),
                            "the approved call is redeemed inside the follow-up turn"
                        );
                        self.consumed.wait().await;
                        self.release.wait().await;
                    }
                    _ => {}
                }
            }
            Ok(CycleResult {
                channel_responses: Vec::new(),
                new_traces: Vec::new(),
                ledger_deltas: Vec::new(),
                token_usage: TokenUsage::default(),
            })
        }
    }

    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let grants = Arc::new(std::sync::Mutex::new(None));
    let consumed = Arc::new(tokio::sync::Barrier::new(2));
    let release = Arc::new(tokio::sync::Barrier::new(2));
    let rt = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("supervised"))
            .with_brain(Arc::new(ConsumingBrain {
                effect: harness_effect(
                    "finance",
                    "composio_execute",
                    serde_json::json!({ "to": "a@b.test" }),
                ),
                grants: Arc::clone(&grants),
                consumed: Arc::clone(&consumed),
                release: Arc::clone(&release),
            }))
            .build()
            .await
            .unwrap(),
    );
    *grants.lock().expect("grant slot") = Some(rt.grants.clone());
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
    let resolving = {
        let rt = Arc::clone(&rt);
        tokio::spawn(async move { rt.resolve_approval(&id, Verdict::Approve, operator()).await })
    };
    consumed.wait().await;
    assert_eq!(rt.grants.live_count(), 0, "the grant was redeemed");

    let restarted = RuntimeBuilder::fs_defaults(home, manifest("supervised"))
        .await
        .unwrap();
    assert_eq!(
        restarted.grants.live_count(),
        0,
        "a dispatch claim must keep an undrained consumption spent"
    );
    release.wait().await;
    resolving.await.expect("follow-up task").unwrap();
}

/// Issue #243: a grant the agent never redeemed expires, is journaled, and
/// the operator is TOLD.
///
/// The silent version of this is the failure worth designing against: the
/// operator approves, watches nothing happen, and has no way to tell whether
/// the work is in flight, already done, or quietly dead. Announcing the lapse
/// is what makes re-approving an informed choice rather than a guess.
#[tokio::test]
async fn an_unredeemed_grant_expires_journals_and_tells_the_operator() {
    let home_dir = tmp_home();
    let operator_channel = Arc::new(crate::runtime::channel::OperatorChannel::new());
    let rt = RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
        .with_channels(vec![operator_channel.clone()])
        .build()
        .await
        .unwrap();

    // `at_millis: 0` is unambiguously past the 15-minute TTL.
    rt.grants.grant(GrantedCall {
        approval_id: ApprovalId::new("appr-stale"),
        agent: "finance".into(),
        tool: "composio_execute".into(),
        args: serde_json::json!({ "to": "a@b.test" }),
        at_millis: 0,
        origin_thread: None,
        origin_parent: None,
        origin_task: None,
    });
    // A fresh one, to prove the sweep is selective rather than a flush.
    rt.grants.grant(GrantedCall {
        approval_id: ApprovalId::new("appr-fresh"),
        agent: "finance".into(),
        tool: "workspace_write".into(),
        args: serde_json::json!({}),
        at_millis: now_millis(),
        origin_thread: None,
        origin_parent: None,
        origin_task: None,
    });

    let expired = rt.sweep_expired_grants().await.unwrap();
    assert_eq!(expired, vec![ApprovalId::new("appr-stale")]);
    assert_eq!(rt.grants.live_count(), 1, "the fresh grant is untouched");
    assert!(rt.grants.peek(&ApprovalId::new("appr-fresh")).is_some());

    // The operator was told, and told which tool and which agent — enough to
    // decide whether to re-approve without going digging.
    let sent = operator_channel.sent();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].text.contains("composio_execute"),
        "{}",
        sent[0].text
    );
    assert!(sent[0].text.contains("finance"), "{}", sent[0].text);
    assert!(sent[0].text.contains("re-approve"), "{}", sent[0].text);

    // The expiry is durable: a restart must not hand the permission back.
    assert!(
        rt.journal
            .replayed_grants()
            .iter()
            .all(|g| g.approval_id != ApprovalId::new("appr-stale"))
    );
}
