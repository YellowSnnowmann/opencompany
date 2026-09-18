use super::tests_owner_setup::{
    COMPANY_ADDRESS, Harness, RefusingMailSender, graph, reached_output, record, smtp_creds,
};
use super::*;

use async_trait::async_trait;

use crate::error::OpenCompanyError;
use crate::runtime::channel::DeskChannel;
use crate::server::ops::mailer::MailSender;

/// **The security boundary.** With no `email` grant the send is REFUSED
/// outright — before the mailbox, before the thread check — and nothing
/// leaves the process.
#[tokio::test]
async fn email_without_the_grant_is_denied_and_nothing_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);
    // Established thread AND a wired mailbox: the ONLY thing missing is the
    // grant, so a pass here could only come from the grant check.
    h.receive_from("ada@example.com").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["docs.*", "web"]),
        &graph("email", Some("ada@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Denied);
    assert!(reports[0].detail.contains("[tools].allow"), "{reports:?}");
    assert!(h.mail.sent().is_empty(), "a denied send must not go out");
    assert!(
        h.inbox_messages().await.iter().all(|m| !m.outbound),
        "a denied send must leave no outbound record"
    );
}

/// **The security boundary, second gate.** Granted but COLD: the company's
/// inbox holds nothing from this address, so the workflow may not open the
/// conversation. Skipped and reported — never sent.
#[tokio::test]
async fn email_to_a_cold_recipient_is_skipped_and_nothing_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);
    // A different address wrote in; the target never did.
    h.receive_from("someone-else@example.com").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("stranger@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Skipped);
    assert!(reports[0].detail.contains("never written"), "{reports:?}");
    assert!(
        h.mail.sent().is_empty(),
        "a cold recipient must not be mailed"
    );
}

// --- email: cold recipients park (issue #227) ----------------------------

/// **Issue #227, the headline.** Cold and granted, with an approvals queue
/// wired: the report is PARKED rather than dropped. One `pending` row,
/// nothing mailed, and a real card in the journal the operator's
/// `/approvals` list reads.
///
/// Note the policy mode: `full`. That is deliberately the mode under which
/// `ApprovalGate::evaluate` returns `Allow` for a `Send` effect, so if this
/// path ever grew an evaluate-then-dispatch step the mail would go out here
/// and this test would fail on `sent()`.
#[tokio::test]
async fn a_cold_recipient_is_parked_for_approval_and_nothing_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true).with_parking(dir.path(), "full");
    h.receive_from("someone-else@example.com").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("stranger@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Pending, "{reports:?}");
    assert_eq!(
        reports[0].target.as_deref(),
        Some("stranger@example.com"),
        "{reports:?}"
    );
    // The row has to point somewhere, or `pending` is just a nicer word for
    // dropped.
    assert!(reports[0].detail.contains("Approvals"), "{reports:?}");
    // THE INVARIANT: a cold recipient never auto-sends.
    assert!(
        h.mail.sent().is_empty(),
        "a cold recipient must not be mailed, parked or not"
    );
    assert!(
        h.inbox_messages().await.iter().all(|m| !m.outbound),
        "nothing was sent, so there is no outbound record to leave"
    );

    // And the card really is in the durable queue — this is what
    // `/approvals` lists and what boot replay rehydrates.
    let pending = h.journal.as_ref().unwrap().pending();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].effect.kind, EMAIL_SEND_KIND);
}

/// The parked effect must have the **same shape as the agent path's**
/// (`CycleHostImpl::send_email`), field for field. Not cosmetic: the
/// operator sees one kind of card either way, and `perform_effect` keys on
/// `kind` plus the `to`/`subject`/`body` payload to actually mail it on
/// approval. A drift here parks cards that do nothing when approved.
#[tokio::test]
async fn the_parked_effect_matches_the_agent_paths_shape() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true).with_parking(dir.path(), "full");

    deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("stranger@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    let pending = h.journal.as_ref().unwrap().pending();
    let effect = &pending[0].effect;
    assert_eq!(effect.kind, EMAIL_SEND_KIND, "same kind constant");
    assert_eq!(effect.group, EffectGroup::Send);
    assert_eq!(effect.amount_usd, None, "a send costs nothing to gate on");
    // The two flags that say *why* this parked: cold counterparty.
    assert!(!effect.established_thread);
    assert!(effect.first_time_counterparty);
    // The payload `perform_effect` reads.
    assert_eq!(effect.payload["to"], "stranger@example.com");
    assert!(
        effect.payload["subject"].as_str().unwrap().contains("Acme"),
        "{effect:?}"
    );
    assert!(
        effect.payload["body"]
            .as_str()
            .unwrap()
            .contains("Q3 is up 12%."),
        "the report itself is the body, or approving sends an empty mail: {effect:?}"
    );
    // The gate holds the identical effect under the same id, so resolving
    // the approval returns something executable.
    let parked = h
        .gate
        .as_ref()
        .unwrap()
        .parked_effect(&pending[0].id)
        .expect("the gate holds the same id the journal recorded");
    assert_eq!(parked.payload, effect.payload);
    assert_eq!(parked.kind, effect.kind);
}

/// **Data integrity (PR #256 review).** A journal write that fails AFTER the
/// gate accepted the park must leave **no gate entry behind**.
///
/// The half-wired state the bundled [`DeliveryParking`] makes unrepresentable
/// is a *construction* mistake; this is the *runtime* version of it, and
/// bundling does nothing for it. An orphaned gate entry is the worst of the
/// three outcomes: an executable effect sitting in the queue with no durable
/// record, visible now and gone on the next restart, backing a row that
/// promises a card which will not survive.
///
/// Asserts all four halves of the rollback: `skipped` (not `pending`), no
/// gate entry, no in-memory queue entry, and nothing sent.
#[tokio::test]
async fn a_failed_journal_write_leaves_no_orphaned_gate_entry() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true).with_failing_journal(dir.path(), "full");
    h.receive_from("someone-else@example.com").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("stranger@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    // Degrades to the pre-#227 row, never `pending`: there is no durable
    // card to point the operator at.
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(
        reports[0].status,
        DeliveryStatus::Skipped,
        "a park that could not be journaled is not pending: {reports:?}"
    );
    assert!(
        reports[0].detail.contains("could not be queued"),
        "{reports:?}"
    );

    // THE FINDING: the gate must not still hold the effect.
    assert!(
        h.gate.as_ref().unwrap().parked_ids().is_empty(),
        "a journal write failure must retract the gate entry, not orphan it"
    );
    // …and the operator's queue must not list a card the gate can no longer
    // execute. `record_parked` inserts before it appends, so this only holds
    // because the rollback clears it too.
    assert!(
        h.journal.as_ref().unwrap().pending().is_empty(),
        "no phantom card may be left in the approvals queue"
    );
    // The refusal still held throughout.
    assert!(h.mail.sent().is_empty(), "nothing may leave the process");
}

/// **Fail-closed (issue #227).** With no approvals queue wired, delivery
/// degrades to the pre-#227 `skipped` row rather than promising a `pending`
/// card that nothing is backing. A `pending` row on a runtime with no queue
/// would send the operator to an empty Approvals list.
#[tokio::test]
async fn a_cold_recipient_without_a_queue_falls_back_to_skipped() {
    let dir = tempfile::tempdir().unwrap();
    // No `with_parking`: exactly the shape every non-production
    // construction site builds.
    let h = Harness::new(dir.path(), true, true);
    assert!(h.deps.parking.is_none());

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("stranger@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(
        reports[0].status,
        DeliveryStatus::Skipped,
        "never `pending` with no queue to back it: {reports:?}"
    );
    assert!(reports[0].detail.contains("never written"), "{reports:?}");
    assert!(h.mail.sent().is_empty());
}

/// The established-thread gate is unchanged by #227: a recipient who DID
/// write in still sends immediately, and parks nothing. Parking is what
/// happens to the refusal, not a new hurdle in front of a legitimate send.
#[tokio::test]
async fn an_established_recipient_still_sends_without_parking() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true).with_parking(dir.path(), "full");
    h.receive_from("ada@example.com").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("ada@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports[0].status, DeliveryStatus::Sent, "{reports:?}");
    assert_eq!(h.mail.sent().len(), 1);
    assert!(
        h.journal.as_ref().unwrap().pending().is_empty(),
        "an established send must not clutter the approvals queue"
    );
}

/// The grant gate is unchanged by #227 too, and still runs FIRST: an
/// ungranted company's cold send is `denied` outright, never parked. Parking
/// an effect the company has no grant for would put a card in front of the
/// operator that policy already refused — approving it would be an end-run
/// around `[tools].allow`.
#[tokio::test]
async fn an_ungranted_cold_recipient_is_denied_not_parked() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true).with_parking(dir.path(), "full");

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["docs.*"]),
        &graph("email", Some("stranger@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports[0].status, DeliveryStatus::Denied, "{reports:?}");
    assert!(
        h.journal.as_ref().unwrap().pending().is_empty(),
        "a denied destination must not reach the approvals queue"
    );
    assert!(h.mail.sent().is_empty());
}

/// A company with no mailbox is still `skipped`, not parked: there is
/// nothing to send from, so approving a card would fail at the transport.
/// That arm is checked before the thread gate and #227 does not move it.
#[tokio::test]
async fn a_company_without_a_mailbox_is_skipped_not_parked() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), false, true).with_parking(dir.path(), "full");

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("stranger@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports[0].status, DeliveryStatus::Skipped, "{reports:?}");
    assert!(reports[0].detail.contains("no mailbox"), "{reports:?}");
    assert!(h.journal.as_ref().unwrap().pending().is_empty());
}

/// **Regression (PR #226 review).** A busy company's inbox must not lose an
/// established recipient. `InboxStore::messages` returns oldest-first, so a
/// capped read takes the OLDEST page — and an inbox that outgrows the cap
/// silently stops finding anyone whose mail arrived after it. The failure
/// is fail-closed (never a wrong send) but it is still wrong, and it bites
/// exactly the longest-lived tenants.
///
/// Note the direction: the sender's message must be buried *past* the cap,
/// i.e. among the NEWEST mail. A sender whose message is the oldest sits at
/// index 0 and was always found, cap or no cap.
#[tokio::test]
async fn an_established_sender_is_found_past_the_old_scan_cap() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);
    // 600 older messages from other people fill the first page…
    for i in 0..600 {
        h.receive_from(&format!("filler{i}@example.com")).await;
    }
    // …so the real correspondent's mail lands well past a 500-message cap.
    h.receive_from("ada@example.com").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("ada@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(
        reports[0].status,
        DeliveryStatus::Sent,
        "a correspondent buried past the scan cap is still an established \
         thread: {reports:?}"
    );
    assert_eq!(h.mail.sent().len(), 1);
}

/// **The default-configuration case (after #230).** A company with no
/// `[tools]` section at all now defaults to the globals `default_allow`,
/// and `*` satisfies the `email` grant — so on the majority of tenants the
/// grant gate is open and the established-thread gate is the one actually
/// holding the line. Pin that it does: a default-configured company still
/// cannot cold-email a stranger.
#[tokio::test]
async fn a_default_configured_company_still_cannot_cold_email() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);
    let manifest = toml::from_str(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"
"#,
    )
    .expect("valid manifest");
    let mut rec = record(&[]);
    rec.manifest = manifest;
    // Sanity: the default really does grant `email` — if this ever stops
    // being true the test below would pass for the wrong reason.
    assert!(
        crate::harness::build::grants_cover(&rec.manifest.tools.allow, "email"),
        "expected the post-#230 default belt to cover `email`, got {:?}",
        rec.manifest.tools.allow
    );

    let reports = deliver_outputs(
        Some(&h.deps),
        &rec,
        &graph("email", Some("stranger@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(
        reports[0].status,
        DeliveryStatus::Skipped,
        "the established-thread gate must still refuse a stranger: {reports:?}"
    );
    assert!(h.mail.sent().is_empty(), "nothing may leave the process");
}

/// The company's OWN prior outbound mail to an address does not make that
/// address established — otherwise one send would bootstrap the next.
#[tokio::test]
async fn a_prior_outbound_does_not_establish_a_thread() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);
    h.inbox
        .append(
            &h.company,
            &EmailRecord {
                id: generate_id(),
                inbox: local_part(COMPANY_ADDRESS),
                from_name: String::new(),
                from_email: "stranger@example.com".to_string(),
                subject: "earlier".to_string(),
                body: "earlier".to_string(),
                at_millis: 1,
                read: true,
                outbound: true,
            },
        )
        .await
        .unwrap();

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("stranger@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports[0].status, DeliveryStatus::Skipped);
    assert!(h.mail.sent().is_empty());
}

/// Granted and established, but the company has no mailbox: skipped, with a
/// reason distinct from the cold-recipient one.
#[tokio::test]
async fn email_without_a_mailbox_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), false, true);
    h.receive_from("ada@example.com").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("ada@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports[0].status, DeliveryStatus::Skipped);
    assert!(reports[0].detail.contains("no mailbox"), "{reports:?}");
}

/// A transport refusal is reported as `failed` — and, critically,
/// `deliver_outputs` still returns normally, because the run's work is done
/// and must not be thrown away over a mail hiccup.
#[tokio::test]
async fn a_send_failure_is_reported_and_does_not_abort_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(dir.path(), true, true);
    h.deps.mail = Some(CompanyMail {
        sender: Arc::new(RefusingMailSender),
        smtp: smtp_creds(),
    });
    h.receive_from("ada@example.com").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some("ada@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Failed);
    assert!(reports[0].detail.contains("smtp said no"), "{reports:?}");
    assert_eq!(reports[0].reason, DeliveryReason::MailTransportRefused);
    // A refused send leaves no outbound audit record — the mail never went.
    assert!(h.inbox_messages().await.iter().all(|m| !m.outbound));
}

/// **Issue #248 at the source.** A real SMTP refusal quotes the mailbox it
/// refused, so the transport's own words are an address-bearing string. This
/// asserts the split holds where the row is built: `detail` keeps the reply
/// (the operator needs it), `reason` cannot carry it.
///
/// `.invalid` is reserved by RFC 2606 and can never resolve, so the fixture
/// names nobody even if it escapes.
#[tokio::test]
async fn a_refusal_that_quotes_the_address_keeps_it_out_of_the_loggable_half() {
    const ADDRESS: &str = "recipient@example.invalid";

    /// Refuses the way a real MTA does: `550` with the rejected mailbox
    /// echoed back inside the reply.
    struct AddressQuotingMailSender;

    #[async_trait]
    impl MailSender for AddressQuotingMailSender {
        async fn send(
            &self,
            _creds: &MailCredentials,
            email: &OutboundEmail,
        ) -> Result<(), OpenCompanyError> {
            Err(OpenCompanyError::Config(format!(
                "550 5.1.1 <{}>: Recipient address rejected: User unknown in local recipient \
                 table",
                email.to
            )))
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(dir.path(), true, true);
    h.deps.mail = Some(CompanyMail {
        sender: Arc::new(AddressQuotingMailSender),
        smtp: smtp_creds(),
    });
    h.receive_from(ADDRESS).await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["*"]),
        &graph("email", Some(ADDRESS)),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    let row = &reports[0];
    assert_eq!(row.status, DeliveryStatus::Failed);

    // The operator's half is untouched: the reply is what makes this
    // fixable, and the run response goes to the tenant, not the platform.
    assert!(row.detail.contains(ADDRESS), "{row:?}");
    assert!(row.detail.contains("550 5.1.1"), "{row:?}");

    // The loggable half classifies the same failure and cannot carry the
    // address — not by scrubbing it, but by having nowhere to put it.
    assert_eq!(row.reason, DeliveryReason::MailTransportRefused);
    let reason = row.reason.to_string();
    assert!(!reason.contains(ADDRESS), "{reason}");
    assert!(!reason.contains('@'), "{reason}");
    assert!(
        reason.contains("the mail transport refused the message"),
        "{reason}"
    );
}

// --- channel -------------------------------------------------------------

#[tokio::test]
async fn channel_posts_to_the_wired_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(dir.path(), true, true);
    h.deps.channels = vec![Arc::new(DeskChannel::new(
        h.company.clone(),
        "engineering".to_string(),
        h.events.clone(),
    ))];

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("channel", Some("engineering")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Sent);
    let events = h
        .events
        .read_from(&h.company, crate::ports::types::EventSeq::new(0), 20)
        .await
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.event,
        CompanyEvent::AgentReply { chat_id, text, .. }
            if chat_id == "engineering" && text.contains("Q3 is up 12%.")
    )));
}
