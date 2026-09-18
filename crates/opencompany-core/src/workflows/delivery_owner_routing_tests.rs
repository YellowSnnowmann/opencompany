use super::tests_owner_setup::{FailingUserStore, Harness, graph, reached_output, record};
use super::*;

use crate::ports::UserRecord;
use crate::runtime::channel::OPERATOR_CHANNEL;

/// A suspended admin and a plain member are not the owner. Only active
/// admins are.
#[tokio::test]
async fn owner_ignores_suspended_admins_and_members() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);
    h.add_admin("u1", "ada@acme.test").await;
    for (id, email, role, status) in [
        (
            "u2",
            "sus@acme.test",
            UserRole::Admin,
            UserStatus::Suspended,
        ),
        ("u3", "mem@acme.test", UserRole::Member, UserStatus::Active),
    ] {
        h.users
            .upsert_user(
                &h.company,
                &UserRecord {
                    id: id.to_string(),
                    email: email.to_string(),
                    display_name: None,
                    avatar: None,
                    role,
                    status,
                    password_hash: None,
                    must_change_password: false,
                    created_at_millis: 1,
                    last_seen_at_millis: None,
                    updated_at_millis: 1,
                },
            )
            .await
            .unwrap();
    }

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(h.mail.sent().len(), 1);
    assert_eq!(h.mail.sent()[0].1.to, "ada@acme.test");
}

/// With no mailbox wired, `owner` falls back to the DURABLE operator channel
/// (issue #1757): a genuine, journal-backed delivery — not the discard-on-an-
/// in-memory-buffer failure it used to report. The report lands in the event
/// log on the dedicated Operator line, and the interactive buffer is
/// untouched.
#[tokio::test]
async fn owner_falls_back_to_the_durable_operator_channel_without_mail() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), false, true);
    h.add_admin("u1", "ada@acme.test").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Sent, "{reports:?}");
    assert_eq!(reports[0].target.as_deref(), Some(OPERATOR_CHANNEL));
    assert!(reports[0].detail.contains("no mailbox"), "{reports:?}");
    assert_eq!(reports[0].reason, DeliveryReason::OwnerFellBackNoMailbox);
    // The interactive in-memory buffer is never a delivery surface.
    assert!(h.channel.sent().is_empty());
    // The report is durable: an `AgentReply` landed on the dedicated
    // Operator line — never the General desk — carrying the workflow's
    // subject header so it reads as a workflow report, not an agent's own
    // reply.
    let landed = h.operator_reports().await;
    assert_eq!(landed.len(), 1, "the report must be journaled: {landed:?}");
    assert!(landed[0].contains("Q3 is up 12%."), "{landed:?}");
    assert!(landed[0].contains("Report flow"), "{landed:?}");
}

/// Issue #1781 review (Codex P1): the `owner`-with-no-mailbox fallback must
/// journal under a distinct author from an ordinary operator-channel report,
/// so the read path (`server::chat_history::history_for_desk`) can restrict
/// exactly this row to administrators — the same audience the sibling email
/// branch already enforces (`owner_recipients` filters to active admins).
///
/// Proven against a **contrasting pair** in the same test rather than just
/// asserting the fallback's author: an explicit `channel` destination
/// naming `operator` is a workflow author's deliberate choice, general
/// audience, and must keep the ordinary `WORKFLOW_REPLY_AUTHOR` — this is
/// what shows the fallback's marker is additive, not a wholesale change to
/// every operator-channel report.
#[tokio::test]
async fn owner_fallback_report_is_authored_distinctly_from_an_ordinary_one() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), false, true);
    h.add_admin("u1", "ada@acme.test").await;

    deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;
    deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("channel", Some(OPERATOR_CHANNEL)),
        "run-2",
        &reached_output(),
        &[],
    )
    .await;

    let authors = h.operator_report_authors().await;
    assert_eq!(authors.len(), 2, "{authors:?}");
    assert!(
        authors
            .iter()
            .any(|(agent_id, _)| agent_id == crate::runtime::OWNER_FALLBACK_REPORT_AUTHOR),
        "the owner fallback must be marked distinctly: {authors:?}"
    );
    assert!(
        authors
            .iter()
            .any(|(agent_id, _)| agent_id == crate::runtime::channel::WORKFLOW_REPLY_AUTHOR),
        "an explicit `channel: operator` destination must keep the ordinary \
         author — the marker is additive, not a wholesale change: {authors:?}"
    );
}

/// Issue #1781 review (CodeRabbit Major + Codex P2): a company whose roster
/// already grandfathers a **teammate** at the literal id `operator` (no desk
/// of the same id — see `CompanyRecord::operator_feed_channel`) must not
/// have the durable Operator system feed land on that same address. Proven
/// for **both** report shapes that can reach the operator channel — the
/// `owner` fallback and an explicit `channel: operator` destination — since
/// review found the collision on the desk-list/read side, not the write
/// guard, and either shape re-opens it if only one were fixed.
///
/// Pre-fix, both reports journaled at `chat_id == OPERATOR_CHANNEL`
/// (`"operator"`) — exactly the address `ChatView` addresses that teammate's
/// own DM by (issue #364). This test's whole point is that the two lines
/// now diverge.
#[tokio::test]
async fn a_report_diverts_off_a_grandfathered_teammates_own_operator_line() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), false, true);
    h.add_admin("u1", "ada@acme.test").await;

    let mut collided = record(&[]);
    collided.manifest = toml::from_str(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"

[[agent]]
id = "operator"
role = "Chief of Staff"
"#,
    )
    .expect("valid manifest with a grandfathered `operator` teammate");
    assert!(
        collided.is_roster_agent(OPERATOR_CHANNEL) && !collided.desk_exists(OPERATOR_CHANNEL),
        "fixture must actually be in the collision state this test exercises"
    );

    deliver_outputs(
        Some(&h.deps),
        &collided,
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;
    deliver_outputs(
        Some(&h.deps),
        &collided,
        &graph("channel", Some(OPERATOR_CHANNEL)),
        "run-2",
        &reached_output(),
        &[],
    )
    .await;

    let landed = h
        .events
        .read_from(
            &h.company,
            crate::ports::types::EventSeq::new(0),
            usize::MAX,
        )
        .await
        .expect("journal readable")
        .into_iter()
        .filter_map(|s| match s.event {
            CompanyEvent::AgentReply { chat_id, .. } => Some(chat_id),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(landed.len(), 2, "{landed:?}");
    assert!(
        landed
            .iter()
            .all(|chat_id| chat_id == crate::runtime::OPERATOR_CHANNEL_COLLISION_FALLBACK),
        "every report bound for the system feed must land off the \
         grandfathered teammate's own `operator` line, not on it: {landed:?}"
    );
    assert!(
        landed.iter().all(|chat_id| chat_id != OPERATOR_CHANNEL),
        "the literal `operator` line must stay untouched by the durable \
         feed — that is the teammate's own DM address: {landed:?}"
    );
}

/// Issue #1781 review (fresh P2 on `operator_feed_channel_fallback_shadowed`
/// itself): the residual double collision that predicate detects must not
/// merely be logged while the report ships anyway. Reuses this fixture's
/// manifest shape from `CompanyRecord`'s own
/// `operator_feed_channel_fallback_shadowed_detects_a_double_collision` test
/// (`ports::types`) — one grandfathered desk named "Operator" (shadowing the
/// primary address) and a second, different desk named "operator-feed"
/// (shadowing the collision fallback) — and proves the delivery layer
/// refuses the send rather than journaling into that second desk's own
/// transcript while still reporting `Sent`.
#[tokio::test]
async fn a_double_collision_refuses_delivery_instead_of_misrouting() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), false, true);

    let mut collided = record(&[]);
    collided.manifest = toml::from_str(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"

[[group_chat]]
id = "legacy_ops"
name = "Operator"
members = []

[[group_chat]]
id = "ops2"
name = "operator-feed"
members = []
"#,
    )
    .expect("valid manifest with a double grandfathered collision");
    assert!(
        collided.operator_feed_channel_fallback_shadowed(),
        "fixture must actually be in the double-collision state this test \
         exercises, or it proves nothing"
    );

    let reports = deliver_outputs(
        Some(&h.deps),
        &collided,
        &graph("channel", Some(OPERATOR_CHANNEL)),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(
        reports[0].status,
        DeliveryStatus::Failed,
        "a shadowed fallback must be reported as a failed delivery, never \
         `Sent` — a `Sent` row here is exactly the silent misroute this test \
         guards against: {reports:?}"
    );
    assert_eq!(
        reports[0].reason,
        DeliveryReason::ChannelCollisionShadowed,
        "{reports:?}"
    );

    let landed = h
        .events
        .read_from(
            &h.company,
            crate::ports::types::EventSeq::new(0),
            usize::MAX,
        )
        .await
        .expect("journal readable")
        .into_iter()
        .filter_map(|s| match s.event {
            CompanyEvent::AgentReply { chat_id, .. } => Some(chat_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        landed.is_empty(),
        "a refused delivery must not append anything to the event log — in \
         particular nothing must land on \"operator-feed\", the second \
         desk's own transcript: {landed:?}"
    );
}

/// A company with a mailbox but no admin address also delivers durably to the
/// operator channel rather than failing — the report still reaches the one
/// human who could act on it.
#[tokio::test]
async fn owner_falls_back_to_the_durable_operator_channel_when_no_admin_has_an_address() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Sent, "{reports:?}");
    assert_eq!(
        reports[0].reason,
        DeliveryReason::OwnerFellBackNoAdminAddress
    );
    assert!(reports[0].detail.contains("no active admin"), "{reports:?}");
    assert!(h.mail.sent().is_empty(), "nothing should have been emailed");
    assert!(h.channel.sent().is_empty());
    let landed = h.operator_reports().await;
    assert_eq!(landed.len(), 1, "the report must be journaled: {landed:?}");
}

/// Both fallbacks unavailable: no mail, no operator channel wired at all
/// (a misconfigured build). Still a row — `failed`, naming the gap — never
/// silence.
#[tokio::test]
async fn owner_with_neither_mail_nor_a_channel_reports_failure() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), false, false);

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Failed);
    assert!(reports[0].detail.contains("operator"), "{reports:?}");
}

// --- owner: standing admin invites (issue #661 / M8) ---------------------

/// **The M8 headline.** A fresh platform-provisioned tenant has nobody in
/// its manifest and nobody in the user store yet, but the platform injected
/// a bootstrap admin. An `owner` report must reach that address — not fall
/// back to the operator channel, which is the one human who could act on it
/// never hearing about it.
#[tokio::test]
async fn owner_emails_the_standing_bootstrap_admin_on_a_fresh_tenant() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true).with_bootstrap_admin("founder@acme.test");
    // No admins in the store, no `[users] admins` in the manifest.

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Sent, "{reports:?}");
    assert_eq!(reports[0].reason, DeliveryReason::OwnerEmailed);
    assert_eq!(reports[0].target.as_deref(), Some("founder@acme.test"));
    assert_eq!(h.mail.sent().len(), 1);
    assert_eq!(h.mail.sent()[0].1.to, "founder@acme.test");
    // The operator channel must be untouched — the whole bug is that the
    // report fell back to it.
    assert!(
        h.channel.sent().is_empty(),
        "the standing admin was mailed, so nothing goes to the operator channel"
    );
    // The send is mirrored into the inbox as outbound, and journaled.
    let outbound: Vec<_> = h
        .inbox_messages()
        .await
        .into_iter()
        .filter(|m| m.outbound)
        .collect();
    assert_eq!(outbound.len(), 1, "the send must leave an audit record");
    let journaled = h.journaled_deliveries().await;
    assert_eq!(journaled.len(), 1, "{journaled:?}");
}

/// A manifest `[users] admins` entry is a standing invite too, and is mailed
/// the same way — even before that person has ever signed in.
#[tokio::test]
async fn owner_emails_a_manifest_admin_standing_invite() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);
    let mut rec = record(&[]);
    rec.manifest = Harness::manifest_with_admins(&["grace@acme.test"]);

    let reports = deliver_outputs(
        Some(&h.deps),
        &rec,
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Sent, "{reports:?}");
    assert_eq!(reports[0].reason, DeliveryReason::OwnerEmailed);
    assert_eq!(h.mail.sent().len(), 1);
    assert_eq!(h.mail.sent()[0].1.to, "grace@acme.test");
}

/// **User-record-wins.** A bootstrap admin who has since signed in and been
/// *suspended* is not mailed through the leftover standing invite: their
/// record wins, and a suspended admin is not an active one. `owner` then has
/// no address to email and falls back to the durable operator channel with
/// the M8 wording — a real delivery (issue #1757), not the failure it once
/// reported.
#[tokio::test]
async fn owner_does_not_email_a_suspended_bootstrap_admin() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true).with_bootstrap_admin("founder@acme.test");
    // The bootstrap admin signed in, then was suspended: a record exists.
    h.users
        .upsert_user(
            &h.company,
            &UserRecord {
                id: "founder".to_string(),
                email: "founder@acme.test".to_string(),
                display_name: None,
                avatar: None,
                role: UserRole::Admin,
                status: UserStatus::Suspended,
                password_hash: None,
                must_change_password: false,
                created_at_millis: 1,
                last_seen_at_millis: None,
                updated_at_millis: 1,
            },
        )
        .await
        .unwrap();

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Sent, "{reports:?}");
    assert_eq!(
        reports[0].reason,
        DeliveryReason::OwnerFellBackNoAdminAddress
    );
    assert!(
        reports[0].detail.contains("standing admin invite"),
        "the fallback wording must name standing invites now: {reports:?}"
    );
    assert!(
        h.mail.sent().is_empty(),
        "a suspended admin must not be mailed, invite or not"
    );
    assert!(
        h.channel.sent().is_empty(),
        "the interactive operator buffer is not delivery"
    );
    // The report still lands, durably, on the operator channel.
    assert_eq!(h.operator_reports().await.len(), 1);
}

/// **Dedupe.** An address named both as an active admin and as the bootstrap
/// admin is one person, and is mailed exactly once.
#[tokio::test]
async fn owner_dedupes_an_active_admin_that_is_also_the_bootstrap_admin() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true).with_bootstrap_admin("ada@acme.test");
    h.add_admin("u1", "ada@acme.test").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "one recipient, one row: {reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Sent);
    assert_eq!(h.mail.sent().len(), 1, "mailed once, not twice");
    assert_eq!(h.mail.sent()[0].1.to, "ada@acme.test");
}

/// A manifest admin address is normalized the same way the login path
/// normalizes it, so `Grace@ACME.test` and `grace@acme.test` are one address
/// — the send goes to the normalized form.
#[tokio::test]
async fn owner_normalizes_a_manifest_admin_address() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);
    let mut rec = record(&[]);
    rec.manifest = Harness::manifest_with_admins(&["Grace@ACME.test"]);

    let reports = deliver_outputs(
        Some(&h.deps),
        &rec,
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Sent, "{reports:?}");
    assert_eq!(h.mail.sent()[0].1.to, "grace@acme.test");
}

/// **Store-error stance (the M8 bug's worst case).** When the user store
/// cannot be read, the standing invites are mailed anyway — dropping the only
/// humans the company is known to have back to the operator channel is
/// exactly the silent drop M8 fixes.
#[tokio::test]
async fn owner_still_emails_standing_invites_when_the_user_store_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(dir.path(), true, true).with_bootstrap_admin("founder@acme.test");
    h.deps.users = Arc::new(FailingUserStore);

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&[]),
        &graph("owner", None),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(
        reports[0].status,
        DeliveryStatus::Sent,
        "an unreadable store must still mail the standing invite: {reports:?}"
    );
    assert_eq!(reports[0].reason, DeliveryReason::OwnerEmailed);
    assert_eq!(h.mail.sent().len(), 1);
    assert_eq!(h.mail.sent()[0].1.to, "founder@acme.test");
}

// --- email ---------------------------------------------------------------

/// The happy path: granted AND established. The mail goes out and is
/// mirrored into the company inbox as outbound, for audit.
#[tokio::test]
async fn email_granted_and_established_sends_and_records_outbound() {
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::new(dir.path(), true, true);
    h.receive_from("ada@example.com").await;

    let reports = deliver_outputs(
        Some(&h.deps),
        &record(&["email.send"]),
        &graph("email", Some("ada@example.com")),
        "run-1",
        &reached_output(),
        &[],
    )
    .await;

    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].status, DeliveryStatus::Sent);
    assert_eq!(h.mail.sent().len(), 1);
    assert_eq!(h.mail.sent()[0].1.to, "ada@example.com");

    let messages = h.inbox_messages().await;
    let outbound: Vec<&EmailRecord> = messages.iter().filter(|m| m.outbound).collect();
    assert_eq!(outbound.len(), 1, "the send must leave an audit record");
    assert!(outbound[0].body.contains("Q3 is up 12%."));
}
