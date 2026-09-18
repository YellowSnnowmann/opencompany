use super::tests_core::*;
use super::tests_core2::*;
use super::*;

#[tokio::test]
async fn effect_executes_at_most_once_across_reload() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::fs_defaults(home.clone(), manifest("full"))
        .await
        .unwrap();

    let effect = Effect {
        kind: "x402.spend".into(),
        group: EffectGroup::Spend,
        amount_usd: Some(3.0),
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    };

    execute_effect_once(&rt, "k1", &effect, None).await.unwrap();
    // Same key again: skipped, no second ledger entry.
    execute_effect_once(&rt, "k1", &effect, None).await.unwrap();

    let record = rt.store().load(rt.id()).await.unwrap().unwrap();
    assert_eq!(record.ledger.len(), 1);

    // Rebuild the runtime over the same home; journal replay must remember
    // the executed key so a replayed effect does not run twice.
    let rt2 = RuntimeBuilder::fs_defaults(home.clone(), manifest("full"))
        .await
        .unwrap();
    assert!(rt2.journal.is_executed("k1"));
    execute_effect_once(&rt2, "k1", &effect, None)
        .await
        .unwrap();
    let record = rt2.store.load(rt2.id()).await.unwrap().unwrap();
    assert_eq!(record.ledger.len(), 1);
}

/// The commit boundary holds the stop, and defers rather than destroys.
///
/// Callers check the flag before reaching the executor, and every one of
/// those checks sits behind an await — resolving an approval journals the
/// verdict first, so a stop landing in that window used to send the money
/// anyway. Refusing must also leave the key unexecuted, or the effect is
/// lost instead of postponed.
#[tokio::test]
async fn a_stop_refuses_the_effect_commit_and_leaves_it_executable_after_release() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::fs_defaults(home.clone(), manifest("full"))
        .await
        .unwrap();

    let effect = Effect {
        kind: "x402.spend".into(),
        group: EffectGroup::Spend,
        amount_usd: Some(3.0),
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    };

    rt.approval_gate.set_emergency(true);
    execute_effect_once(&rt, "k1", &effect, None)
        .await
        .expect_err("a stopped company must not commit an effect");

    assert!(
        !rt.journal.is_executed("k1"),
        "a refused commit must not carry the at-most-once mark, or the effect is lost \
         rather than deferred"
    );
    let record = rt.store().load(rt.id()).await.unwrap().unwrap();
    assert!(
        record.ledger.is_empty(),
        "and the money must not have moved"
    );

    rt.approval_gate.set_emergency(false);
    execute_effect_once(&rt, "k1", &effect, None)
        .await
        .expect("released, so the deferred effect runs");
    let record = rt.store().load(rt.id()).await.unwrap().unwrap();
    assert_eq!(record.ledger.len(), 1);
}

#[tokio::test]
async fn supervised_effect_runs_without_policy_hitl() {
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
            .with_brain(Arc::new(EffectBrain {
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
    assert!(report.parked.is_empty());
    assert!(rt.pending_approvals().is_empty());
}

/// Issue #383: a follow-up cycle that fails leaves a *recoverable* state,
/// not a stranded one.
///
/// Detaching the cycle means its failure has nowhere to be returned to, so
/// the safety net has to be the ordering rather than the caller: the verdict
/// is journaled and the grant minted before the turn is ever attempted, and
/// re-approving is a no-op that mints no second grant (issue #243). This
/// pins all three, so "the runtime logs it and the operator can retry" is a
/// property of the code rather than a claim in a PR body.
#[tokio::test]
async fn a_failed_follow_up_cycle_leaves_the_verdict_and_grant_intact() {
    let home_dir = tmp_home();
    let effect = harness_effect("finance", "composio_execute", serde_json::json!({}));
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
            .with_brain(Arc::new(FailingContinuationBrain {
                effect: effect.clone(),
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

    let failed = rt.resolve_approval(&id, Verdict::Approve, operator()).await;
    assert!(failed.is_err(), "the caller still learns the turn failed");

    // The operator's decision survived it.
    assert!(
        rt.pending_approvals().is_empty(),
        "the verdict was journaled before the turn was attempted"
    );
    assert!(rt.grants.peek(&id).is_some(), "and the grant was minted");
    assert_eq!(rt.grants.live_count(), 1);

    // Retrying is safe: a no-op report, and still exactly one grant.
    let again = rt
        .resolve_approval(&id, Verdict::Approve, operator())
        .await
        .expect("re-approving is a no-op, not a second failure");
    assert_eq!(
        again.responses[0].text,
        "This approval was already resolved."
    );
    assert_eq!(
        rt.grants.live_count(),
        1,
        "a retry after a failed continuation mints no second grant"
    );
}

/// The core of #243: approving an agent's blocked tool call mints a
/// single-use grant and does **not** execute the effect.
///
/// Executing it would be worse than useless. The payload is the tool's
/// arguments, so `perform_effect` would ledger a spend for money nothing
/// actually moved and route no message — the operator would see an approval
/// marked done, a charge on the books, and no email sent. The grant is what
/// makes approval mean "the agent may now really do this, once".
#[tokio::test]
async fn approving_a_harness_tool_call_mints_a_grant_instead_of_executing() {
    let home_dir = tmp_home();
    // Issue #470: a catalogued send, keyed the way `composio_execute`'s
    // schema keys it, with the action's parameters under `arguments`.
    let args = crate::policy::test_support::composio_args_with(
        crate::policy::test_support::COMPOSIO_SEND_SLUG,
        serde_json::json!({ "to": "a@b.test" }),
    );
    let (rt, id) = park_one(
        home_dir.path().to_path_buf(),
        harness_effect("finance", "composio_execute", args.clone()),
    )
    .await;

    rt.resolve_approval(&id, Verdict::Approve, operator())
        .await
        .unwrap();

    // A grant exists, scoped to the agent, tool and exact arguments.
    let grant = rt.grants.peek(&id).expect("a grant was minted");
    assert_eq!(grant.agent, "finance");
    assert_eq!(grant.tool, "composio_execute");
    assert_eq!(grant.args, args);

    // ...and the effect was NOT executed: no ledger row, no journal key.
    let record = rt.store.load(rt.id()).await.unwrap().unwrap();
    assert!(
        record.ledger.is_empty(),
        "a harness tool call must not be performed natively — its payload is \
         arguments, so executing it books a spend for work that never happened"
    );
    assert!(!rt.journal.is_executed(&format!("approval:{id}")));
}

// --- What the card says (issue #372) ------------------------------------

/// **Issue #1024.** The parked effect's consequence group reaches the card.
///
/// A `GMAIL_SEND_EMAIL` gate sat parked for days and mailed a five-day-old
/// digest the moment an operator cleared a backlog. The age was already on
/// the card — as a bare "5d ago" in the footer, where it reads as how long
/// the QUEUE has held the item rather than how old the PAYLOAD is. The
/// console can only tell those apart for an effect that leaves the company,
/// and it cannot work out which those are on its own: for a harness tool
/// call `kind` is the TOOL NAME (`composio_execute`), not `email.send`, so
/// a console keying on `kind` would miss exactly this send.
///
/// So the host's own classification has to ride on the summary. This pins
/// that it is the PARKED EFFECT's group and not a constant: a summary that
/// hard-coded `Other` would render every outbound send as internal and put
/// the bug straight back.
#[tokio::test]
async fn a_parked_effect_carries_its_group_to_the_card() {
    let home_dir = tmp_home();
    let mut effect = harness_effect(
        "devrel",
        "composio_execute",
        serde_json::json!({ "tool": "GMAIL_SEND_EMAIL" }),
    );
    // The group a real `composio_execute` of a send resolves to.
    effect.group = EffectGroup::Send;
    let (rt, _id) = park_one(home_dir.path().to_path_buf(), effect).await;

    let pending = rt.pending_approvals();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].group,
        EffectGroup::Send,
        "the card must carry the parked effect's own group; anything constant \
         renders an outbound send as internal"
    );
    // And the tool name is NOT the discriminator — the reason the group has
    // to be sent at all.
    assert_eq!(pending[0].kind, "composio_execute");

    // It survives the wire: the console reads this field, so a summary that
    // classified correctly and serialized nothing would be no fix.
    let wire: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&pending[0]).unwrap()).unwrap();
    assert_eq!(wire["group"], "send");
}

/// A harness-projected park reaches the operator naming its asker and what
/// it will actually do — the whole point of #372, where the card used to say
/// only "Shell".
#[tokio::test]
async fn a_harness_park_projects_its_agent_and_payload() {
    const FAKE_SECRET: &str = "NOT-A-REAL-KEY-planted-for-tests";
    let home_dir = tmp_home();
    let (rt, _id) = park_one(
        home_dir.path().to_path_buf(),
        harness_effect(
            "engineer",
            "shell",
            serde_json::json!({
                "command": "./deploy.sh --staging",
                "env": { "API_KEY": FAKE_SECRET },
            }),
        ),
    )
    .await;

    let pending = rt.pending_approvals();
    assert_eq!(pending.len(), 1);
    let summary = &pending[0];
    assert_eq!(summary.agent.as_deref(), Some("engineer"));

    let payload = summary.payload.as_ref().expect("the arguments are carried");
    // The command is verbatim: it IS the thing being consented to.
    assert_eq!(payload["command"], "./deploy.sh --staging");
    // ...and the planted credential never leaves the host.
    let wire = serde_json::to_string(summary).unwrap();
    assert!(
        !wire.contains(FAKE_SECRET),
        "secret reached the wire: {wire}"
    );
    assert!(wire.contains(crate::runtime::approval_display::REDACTED));
}

/// A **native** effect the runtime performs itself names no asker, and an
/// argument-less one carries no payload — so the card renders exactly as it
/// did before #372 rather than inventing an agent. This is also the shape a
/// journal-replayed pre-#243 park takes.
#[tokio::test]
async fn a_native_park_projects_no_agent_and_no_payload() {
    let home_dir = tmp_home();
    let (rt, _id) = park_one(
        home_dir.path().to_path_buf(),
        Effect {
            kind: "filing.submit".into(),
            group: EffectGroup::Sign,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: serde_json::Value::Null,
            agent: None,
            run_id: None,
        },
    )
    .await;

    let pending = rt.pending_approvals();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].agent.is_none());
    assert!(pending[0].payload.is_none());
}

/// The wire stays **additive**: absent fields are omitted entirely, so the
/// JSON an old console receives is byte-identical to the pre-#372 shape and
/// its unknown-key tolerance is never exercised.
#[tokio::test]
async fn absent_display_fields_are_omitted_from_the_wire() {
    let home_dir = tmp_home();
    let (rt, _id) = park_one(
        home_dir.path().to_path_buf(),
        Effect {
            kind: "filing.submit".into(),
            group: EffectGroup::Sign,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: serde_json::Value::Null,
            agent: None,
            run_id: None,
        },
    )
    .await;

    let wire: serde_json::Value =
        serde_json::to_value(&rt.pending_approvals()[0]).expect("serializes");
    let keys: Vec<&str> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    assert!(!keys.contains(&"agent"), "agent leaked as null: {keys:?}");
    assert!(
        !keys.contains(&"payload"),
        "payload leaked as null: {keys:?}"
    );
}

/// Approve-with-edit mints against the **amended** arguments.
///
/// Granting the original would let the agent re-issue the very call the
/// operator edited, silently discarding the edit — worse than not supporting
/// amend at all, because the operator has every reason to think their change
/// took effect.
#[tokio::test]
async fn amending_an_approval_grants_the_edited_arguments() {
    let home_dir = tmp_home();
    let (rt, id) = park_one(
        home_dir.path().to_path_buf(),
        harness_effect(
            "finance",
            "composio_execute",
            serde_json::json!({ "to": "wrong@b.test", "body": "hi" }),
        ),
    )
    .await;

    rt.resolve_approval_amended(&id, serde_json::json!({ "to": "right@b.test" }), operator())
        .await
        .unwrap();

    let grant = rt.grants.peek(&id).expect("a grant was minted");
    assert_eq!(
        grant.args,
        serde_json::json!({ "to": "right@b.test", "body": "hi" }),
        "the grant admits the operator's edit, overlaid onto the original"
    );
    // The un-edited call must NOT be redeemable.
    assert!(
        rt.grants
            .consume(
                "finance",
                "composio_execute",
                &serde_json::json!({ "to": "wrong@b.test", "body": "hi" })
            )
            .is_none()
    );
}

/// A denied approval grants nothing. "No" must not leave a live permission
/// behind for the agent to find.
#[tokio::test]
async fn denying_a_harness_tool_call_mints_nothing() {
    let home_dir = tmp_home();
    let (rt, id) = park_one(
        home_dir.path().to_path_buf(),
        harness_effect("finance", "composio_execute", serde_json::json!({})),
    )
    .await;

    rt.resolve_approval(&id, Verdict::Deny, operator())
        .await
        .unwrap();

    assert!(rt.grants.peek(&id).is_none());
    assert_eq!(rt.grants.live_count(), 0);
}

#[tokio::test]
async fn denying_an_explicit_request_mints_a_durable_decision_continuation() {
    let home_dir = tmp_home();
    let args = serde_json::json!({
        "title": "Submit the filing",
        "question": "May I submit it?"
    });
    let (rt, id) = park_one(
        home_dir.path().to_path_buf(),
        harness_effect(
            "finance",
            crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND,
            args.clone(),
        ),
    )
    .await;

    let summary = &rt.pending_approvals()[0];
    assert!(!summary.broadly_grantable);
    assert!(!summary.broadly_deniable);

    rt.resolve_approval(&id, Verdict::Deny, operator())
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while rt.grants.peek_continuation(&id).is_some()
            || !rt.journal.replayed_approval_continuations().is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("denial reaches its asker and retires the durable continuation");
    assert!(
        rt.journal
            .replayed_grants()
            .into_iter()
            .all(|grant| grant.approval_id != id),
        "a denied request must never replay as executable authority"
    );
    assert!(
        rt.journal.replayed_approval_continuations().is_empty(),
        "the delivered follow-up is consumed durably, so restart must not repeat its model \
         turn"
    );
}

#[tokio::test]
async fn an_explicit_question_refuses_a_standing_scope() {
    let home_dir = tmp_home();
    let (rt, id) = park_one(
        home_dir.path().to_path_buf(),
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

    let error = rt
        .resolve_approval_spawned(&id, Verdict::Deny, operator(), tool_scope())
        .await
        .expect_err("the question tool is not the proposed action");

    assert!(error.to_string().contains("can only be decided once"));
    assert_eq!(rt.pending_approvals().len(), 1);
    assert!(rt.standing_grants().is_empty());
    assert!(rt.grants.peek_continuation(&id).is_none());
}

#[tokio::test]
async fn an_explicit_question_without_an_agent_stays_pending_with_an_error() {
    let home_dir = tmp_home();
    let mut effect = harness_effect(
        "finance",
        crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND,
        serde_json::json!({
            "title": "Submit the filing",
            "question": "May I submit it?"
        }),
    );
    effect.agent = None;
    let (rt, id) = park_one(home_dir.path().to_path_buf(), effect).await;

    let error = rt
        .resolve_approval(&id, Verdict::Deny, operator())
        .await
        .expect_err("an explicit request must name the agent to resume");

    assert!(error.to_string().contains("missing its requesting agent"));
    assert_eq!(rt.pending_approvals().len(), 1);
}

#[tokio::test]
async fn an_explicit_question_refuses_approve_with_edit() {
    let home_dir = tmp_home();
    let (rt, id) = park_one(
        home_dir.path().to_path_buf(),
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

    let error = rt
        .resolve_approval_amended(
            &id,
            serde_json::json!({ "question": "May I submit it tomorrow?" }),
            operator(),
        )
        .await
        .expect_err("a question carries no executable payload to edit");

    assert!(error.to_string().contains("no executable payload to amend"));
    assert_eq!(rt.pending_approvals().len(), 1);
    assert!(rt.grants.peek_continuation(&id).is_none());
}

#[tokio::test]
async fn recovery_schedules_a_durable_explicit_decision_continuation() {
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
    registry.insert(recovered.id().clone(), recovered.clone());

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while brain.calls() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("recovery dispatches the owed follow-up");
    assert_eq!(brain.calls(), 1);
}
