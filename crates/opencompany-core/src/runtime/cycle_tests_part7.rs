use super::tests_core::*;
use super::tests_core2::*;
use super::*;

#[tokio::test]
async fn email_send_effect_sends_and_records() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let sender = Arc::new(RecordingMailSender::new());
    let email_effect = Effect {
        kind: "email.send".into(),
        group: EffectGroup::Send,
        amount_usd: None,
        established_thread: true,
        first_time_counterparty: false,
        payload: serde_json::json!({ "to": "x@ext.com", "subject": "Hi", "body": "yo" }),
        agent: None,
        run_id: None,
    };
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(EffectBrain {
            effect: email_effect,
        }))
        .with_mail(CompanyMail {
            sender: sender.clone(),
            smtp: test_smtp("ceo@acme.test"),
        })
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "send it".into(),
        by: None,
        chat: None,
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    assert_eq!(sender.sent().len(), 1);
    // The From address is the company's own address, never spoofable via
    // the effect payload (which carries no `from` field at all).
    assert_eq!(sender.sent()[0].0, "ceo@acme.test");
    let inbox = rt.inbox().messages(rt.id(), "ceo", 10, 0).await.unwrap();
    assert!(inbox.iter().any(|r| r.outbound && r.subject == "Hi"));
}

/// **The acceptance bar for issue #227.** Parking a cold recipient's report
/// is only worth doing if approving it actually sends the mail — otherwise
/// `pending` is a nicer-looking way to drop the report.
///
/// This parks an `email.send` effect the way
/// [`crate::workflows::delivery`] does — straight onto the gate + journal,
/// with no cycle running and no brain involved — then resolves it the way
/// the HTTP handler does. The mail must go out and leave the outbound audit
/// record, through `resolve_approval` → `execute_effect_once` →
/// `perform_effect` → `send_company_email`.
///
/// Policy mode is `full` on purpose: nothing here relies on the gate
/// deciding to park. It was parked directly, exactly as delivery parks it.
#[tokio::test]
async fn a_directly_parked_email_send_is_mailed_when_approved() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let sender = Arc::new(RecordingMailSender::new());
    let rt = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("full"))
            .with_mail(CompanyMail {
                sender: sender.clone(),
                smtp: test_smtp("ceo@acme.test"),
            })
            .build()
            .await
            .unwrap(),
    );

    // What `park_cold_recipient` builds, field for field.
    let effect = Effect {
        kind: EMAIL_SEND_KIND.into(),
        group: EffectGroup::Send,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: true,
        payload: serde_json::json!({
            "to": "stranger@ext.com",
            "subject": "[Acme] Report flow — Owner summary",
            "body": "Q3 is up 12%.",
        }),
        agent: None,
        run_id: None,
    };
    let approval_id = rt.approvals.park(rt.id(), effect.clone()).await.unwrap();
    rt.journal
        .record_parked(
            &approval_id,
            &effect,
            now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    // It reaches the operator's queue — the same list a workflow's park
    // shows up in, since it is the same journal.
    assert_eq!(rt.pending_approvals().len(), 1);
    assert_eq!(rt.pending_approvals()[0].kind, EMAIL_SEND_KIND);
    assert!(sender.sent().is_empty(), "parked means not yet sent");

    rt.resolve_approval(&approval_id, Verdict::Approve, operator())
        .await
        .unwrap();

    // Approving SENDS.
    assert_eq!(sender.sent().len(), 1, "approving must mail the report");
    assert_eq!(sender.sent()[0].1.to, "stranger@ext.com");
    assert!(sender.sent()[0].1.body.contains("Q3 is up 12%."));
    // From the company's own address, never anything the payload named.
    assert_eq!(sender.sent()[0].0, "ceo@acme.test");
    // …and leaves the outbound audit record, which also makes the recipient
    // an established thread for next time.
    let inbox = rt.inbox().messages(rt.id(), "ceo", 10, 0).await.unwrap();
    assert!(
        inbox
            .iter()
            .any(|r| r.outbound && r.subject.contains("Owner summary")),
        "{inbox:?}"
    );
    assert!(rt.pending_approvals().is_empty(), "the queue drains");
    tokio::fs::remove_dir_all(&home).await.ok();
}

/// The other half of the same bar: DENYING sends nothing and drains the
/// queue. A parked report must not leak out on a refusal.
#[tokio::test]
async fn a_directly_parked_email_send_is_not_mailed_when_denied() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let sender = Arc::new(RecordingMailSender::new());
    let rt = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("full"))
            .with_mail(CompanyMail {
                sender: sender.clone(),
                smtp: test_smtp("ceo@acme.test"),
            })
            .build()
            .await
            .unwrap(),
    );

    let effect = Effect {
        kind: EMAIL_SEND_KIND.into(),
        group: EffectGroup::Send,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: true,
        payload: serde_json::json!({
            "to": "stranger@ext.com",
            "subject": "[Acme] Report flow — Owner summary",
            "body": "Q3 is up 12%.",
        }),
        agent: None,
        run_id: None,
    };
    let approval_id = rt.approvals.park(rt.id(), effect.clone()).await.unwrap();
    rt.journal
        .record_parked(
            &approval_id,
            &effect,
            now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    rt.resolve_approval(&approval_id, Verdict::Deny, operator())
        .await
        .unwrap();

    assert!(sender.sent().is_empty(), "a denied report must not go out");
    assert!(
        rt.inbox()
            .messages(rt.id(), "ceo", 10, 0)
            .await
            .unwrap()
            .iter()
            .all(|r| !r.outbound),
        "nothing was sent, so there is no outbound record"
    );
    assert!(rt.pending_approvals().is_empty());
    tokio::fs::remove_dir_all(&home).await.ok();
}

/// **Restart durability.** A parked report survives a process restart with
/// its original id and still sends on approval — the property that makes a
/// `pending` row honest even though the run itself is not persisted.
#[tokio::test]
async fn a_parked_email_send_survives_a_restart_and_still_sends() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let effect = Effect {
        kind: EMAIL_SEND_KIND.into(),
        group: EffectGroup::Send,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: true,
        payload: serde_json::json!({
            "to": "stranger@ext.com",
            "subject": "[Acme] Report flow — Owner summary",
            "body": "Q3 is up 12%.",
        }),
        agent: None,
        run_id: None,
    };
    let approval_id = {
        let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
            .build()
            .await
            .unwrap();
        let id = rt.approvals.park(rt.id(), effect.clone()).await.unwrap();
        rt.journal
            .record_parked(
                &id,
                &effect,
                now_millis(),
                TaskLink::Unlinked,
                ApprovalConversation::default(),
                None,
            )
            .await
            .unwrap();
        id
    };

    // Fresh runtime over the same home: boot replay rehydrates the card.
    let sender = Arc::new(RecordingMailSender::new());
    let rt2 = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("full"))
            .with_mail(CompanyMail {
                sender: sender.clone(),
                smtp: test_smtp("ceo@acme.test"),
            })
            .build()
            .await
            .unwrap(),
    );
    let pending = rt2.pending_approvals();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].id, approval_id, "the ORIGINAL id, not a new one");

    rt2.resolve_approval(&approval_id, Verdict::Approve, operator())
        .await
        .unwrap();
    assert_eq!(
        sender.sent().len(),
        1,
        "a card approved after a restart must still mail"
    );
    assert_eq!(sender.sent()[0].1.to, "stranger@ext.com");
    tokio::fs::remove_dir_all(&home).await.ok();
}

#[tokio::test]
async fn email_send_effect_without_mail_errors() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let email_effect = Effect {
        kind: "email.send".into(),
        group: EffectGroup::Send,
        amount_usd: None,
        established_thread: true,
        first_time_counterparty: false,
        payload: serde_json::json!({ "to": "x@ext.com", "subject": "Hi", "body": "yo" }),
        agent: None,
        run_id: None,
    };
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(EffectBrain {
            effect: email_effect,
        }))
        .build()
        .await
        .unwrap();

    let err = perform_effect(
        &rt,
        &Effect {
            kind: "email.send".into(),
            group: EffectGroup::Send,
            amount_usd: None,
            established_thread: true,
            first_time_counterparty: false,
            payload: serde_json::json!({ "to": "x@ext.com", "subject": "Hi", "body": "yo" }),
            agent: None,
            run_id: None,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("email is not configured"));
}

#[tokio::test]
async fn established_true_only_after_inbound_from_recipient() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_mail(CompanyMail {
            sender: Arc::new(RecordingMailSender::new()),
            smtp: test_smtp("ceo@acme.test"),
        })
        .build()
        .await
        .unwrap();

    assert!(!recipient_is_established(&rt, "x@ext.com").await);

    rt.inbox()
        .append(
            rt.id(),
            &crate::ports::inbox::EmailRecord {
                id: "1".into(),
                inbox: "ceo".into(),
                from_name: "".into(),
                from_email: "x@ext.com".into(),
                subject: "hi".into(),
                body: "".into(),
                at_millis: 0,
                read: false,
                outbound: false,
            },
        )
        .await
        .unwrap();

    assert!(recipient_is_established(&rt, "X@EXT.COM").await);
}

/// Issue #1113: the trigger boundary, named event by event. Outside
/// content — a webhook here, an A2A task in the sibling test — makes a
/// cycle external; the company's own machinery — operator speech,
/// payments, dispatches, schedule fires — stays Internal, per the
/// operator-facts authorship precedent.
#[test]
fn outside_content_makes_a_cycle_external_and_own_machinery_does_not() {
    use crate::ports::types::Actor;
    let webhook = CompanyEvent::WebhookReceived {
        channel: "telegram".into(),
        body: serde_json::json!({"text": "raw payload"}),
    };
    let operator = CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        text: "please do the thing".into(),
        by: Option::<Actor>::None,
        chat: None,
        parent: None,
        deliverable: None,
        attachments: Vec::new(),
    };
    // The company's own machinery stays Internal, event by event: a
    // payment landing is the company's ledger speaking, not third-party
    // prose riding an open boundary.
    let payment = CompanyEvent::PaymentReceived {
        amount_usd: 10.0,
        memo: "invoice".into(),
    };
    assert!(cycle_is_external(&[webhook]));
    assert!(!cycle_is_external(&[operator]));
    assert!(!cycle_is_external(&[payment]));
    assert!(!cycle_is_external(&[]));
}

/// Mixed batches over-taint (any(), not all()): the safe direction, asserted
/// in both orderings. A retired kind read back from an old journal carries
/// nothing, so on its own it taints nothing.
#[test]
fn mixed_batches_over_taint_and_an_unknown_row_is_internal() {
    use crate::ports::types::Actor;
    let webhook = || CompanyEvent::WebhookReceived {
        channel: "telegram".into(),
        body: serde_json::json!({"text": "do the thing"}),
    };
    let operator = || CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        text: "hi".into(),
        by: Option::<Actor>::None,
        chat: None,
        parent: None,
        deliverable: None,
        attachments: Vec::new(),
    };
    assert!(cycle_is_external(&[operator(), webhook()]));
    assert!(cycle_is_external(&[webhook(), operator()]));
    assert!(!cycle_is_external(&[CompanyEvent::Unknown]));
    assert!(cycle_is_external(&[CompanyEvent::Unknown, webhook()]));
}

/// The routing the flag drives: an externally-triggered cycle's
/// `ContextOp::Put` lands in the company's memory tagged inbound, an
/// ordinary cycle's untagged — so what the outside world said stays
/// distinguishable from the company's own conclusions (issue #1113).
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn external_cycles_tag_their_puts_inbound() {
    use crate::memory::{CONTEXT_TAG, INBOUND_TAG, MemoryQuery};
    use crate::ports::types::ContextOpResult;

    let home_dir = tmp_home();
    let rt = RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("full"))
        .with_id(CompanyId::new(format!(
            "inbound-{}",
            uuid::Uuid::new_v4().simple()
        )))
        .build()
        .await
        .unwrap();

    let mut ids = Vec::new();
    for (external, cycle) in [(false, "cyc-int"), (true, "cyc-ext")] {
        let host = CycleHostImpl::new(
            rt.id().clone(),
            cycle.into(),
            &rt,
            None,
            external,
            ApprovalConversation::default(),
        );
        let put = host
            .context_op(ContextOp::Put(crate::ports::types::ContextChunk {
                label: format!("probe/{cycle}"),
                body: format!("body {cycle}"),
            }))
            .await
            .unwrap();
        let ContextOpResult::Addr(addr) = put else {
            panic!("a put answers an address");
        };
        ids.push(addr.as_ref().to_string());
    }

    let rows = rt
        .memory()
        .list(MemoryQuery {
            tags_any: vec![CONTEXT_TAG.to_string()],
            ..MemoryQuery::default()
        })
        .await
        .unwrap()
        .items;
    let tagged = |id: &str| {
        rows.iter()
            .find(|row| row.id == id)
            .expect("the put is listed")
            .tags
            .contains(&INBOUND_TAG.to_string())
    };
    assert!(!tagged(&ids[0]), "the ordinary cycle's put is not inbound");
    assert!(
        tagged(&ids[1]),
        "the external cycle's put is tagged inbound"
    );
}

#[tokio::test]
async fn send_email_without_mail_returns_clean_error() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    // No `.with_mail(..)`: the company has no mailbox wired at all.
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .build()
        .await
        .unwrap();
    let host = CycleHostImpl::new(
        rt.id().clone(),
        "cyc-nomail".into(),
        &rt,
        None,
        false,
        ApprovalConversation::default(),
    );

    let res = host
        .send_email(serde_json::json!({ "to": "x@ext.com", "subject": "s", "body": "b" }))
        .await
        .unwrap();
    assert!(!res.ok);
    assert!(
        res.output["error"]
            .as_str()
            .unwrap_or_default()
            .contains("not configured")
    );
}

#[tokio::test]
async fn send_email_bad_args_missing_to_yields_no_effect() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("supervised"))
        .build()
        .await
        .unwrap();
    let host = CycleHostImpl::new(
        rt.id().clone(),
        "cyc-bad".into(),
        &rt,
        None,
        false,
        ApprovalConversation::default(),
    );

    let res = host
        .send_email(serde_json::json!({ "subject": "s", "body": "b" }))
        .await
        .unwrap();
    assert!(!res.ok);
    assert!(res.output["error"].is_string());
}
