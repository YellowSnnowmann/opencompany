use super::tests_core::*;

#[test]
fn redeem_matching_reserves_when_the_id_still_matches() {
    let set = BudgetPauseSet::default();
    let marker = set.park("ceo", None, "hi", "paused", 1_000, RedeemContext::default());

    let outcome = set.redeem_matching("ceo", &marker.id);
    assert_eq!(outcome, RedeemMatch::Reserved(marker));
    assert!(set.peek("ceo").is_none(), "reserved out of the set");
}

#[test]
fn redeem_matching_reports_absent_when_nothing_is_parked() {
    let set = BudgetPauseSet::default();
    assert_eq!(set.redeem_matching("ceo", "some-id"), RedeemMatch::Absent);
}

#[test]
fn redeem_matching_leaves_a_background_overwrite_untouched_on_a_stale_id() {
    // Issue #1846 review (Codex #3866418876): a chat pause parks a
    // marker with a chat destination; a background turn (workflow node,
    // unstreamed task) for the SAME agent then pauses too and overwrites
    // it with a marker that has NONE. The console still shows the OLD
    // chat card because nothing about a chat-less park touches the
    // transcript-based staleness check. Redeeming by the OLD id must
    // not silently take the NEW (unrelated) marker.
    let set = BudgetPauseSet::default();
    let chat_marker = set.park(
        "ceo",
        Some("general".to_string()),
        "ship the API",
        "paused for the chat turn",
        1_000,
        RedeemContext::default(),
    );
    let background_marker = set.park(
        "ceo",
        None,
        "run the nightly workflow node",
        "paused for the background turn",
        2_000,
        RedeemContext::default(),
    );

    // The stale chat id must not reserve the background marker.
    assert_eq!(
        set.redeem_matching("ceo", &chat_marker.id),
        RedeemMatch::Stale
    );
    // Left completely untouched — still there, still the background one.
    let still_parked = set.peek("ceo").expect("the background marker survives");
    assert_eq!(still_parked.id, background_marker.id);
    assert_eq!(still_parked.message, "run the nightly workflow node");

    // The fresh id reserves correctly.
    let outcome = set.redeem_matching("ceo", &background_marker.id);
    assert_eq!(outcome, RedeemMatch::Reserved(background_marker));
    assert!(set.peek("ceo").is_none());
}

#[test]
fn redeem_matching_reserves_atomically_so_a_concurrent_stale_attempt_finds_nothing() {
    let set = BudgetPauseSet::default();
    let marker = set.park("ceo", None, "hi", "paused", 1_000, RedeemContext::default());

    let first = set.redeem_matching("ceo", &marker.id);
    assert_eq!(first, RedeemMatch::Reserved(marker.clone()));
    // A second attempt with the same id now finds nothing parked at all
    // (not "stale") — the first call already reserved it.
    assert_eq!(set.redeem_matching("ceo", &marker.id), RedeemMatch::Absent);
}

#[test]
fn budget_pauses_are_scoped_per_company() {
    let acme = CompanyId::new("acme");
    let globex = CompanyId::new("globex");
    budget_pauses_for(&acme).park(
        "ceo",
        None,
        "acme's message",
        "paused",
        1_000,
        RedeemContext::default(),
    );

    assert!(
        budget_pauses_for(&globex).peek("ceo").is_none(),
        "a marker parked for one company must not leak into another's set"
    );
    assert!(budget_pauses_for(&acme).peek("ceo").is_some());
}

#[test]
fn an_unrelated_agent_has_no_parked_marker() {
    let set = BudgetPauseSet::default();
    set.park("ceo", None, "hi", "paused", 1_000, RedeemContext::default());
    assert!(
        set.peek("engineer").is_none(),
        "parking for one agent must not be visible under another's key"
    );
}

/// Issue #1846 review (Codex #3865812419/#3865812423/#3865812432):
/// `RedeemContext::from_events` reads the ORIGINAL operator message's
/// parent/deliverable/mentions out of a cycle's event batch, skipping
/// any non-`OperatorMessage` record ahead of it.
#[test]
fn redeem_context_reads_the_first_operator_message_in_a_batch() {
    use crate::ports::types::MentionTarget;

    let mention = Mention {
        target: MentionTarget::Agent {
            id: "researcher".to_string(),
        },
        text: "@researcher".to_string(),
        offset: 0,
        quiet: false,
    };
    let events = vec![
        (
            None,
            CompanyEvent::WorkspaceChanged {
                node_id: "n-1".into(),
                change: "updated".into(),
            },
        ),
        (
            None,
            CompanyEvent::OperatorMessage {
                text: "ship it".into(),
                by: None,
                chat: Some("general".into()),
                parent: Some(EventSeq::new(9)),
                deliverable: Some(MessageIntent::Chat),
                mentions: vec![mention.clone()],
                attachments: Vec::new(),
            },
        ),
    ];

    let ctx = RedeemContext::from_events(&events);
    assert_eq!(ctx.parent, Some(EventSeq::new(9)));
    assert_eq!(ctx.deliverable, Some(MessageIntent::Chat));
    assert_eq!(ctx.mentions, vec![mention]);
}

#[test]
fn redeem_context_defaults_when_no_operator_message_is_in_the_batch() {
    let events = vec![(
        None,
        CompanyEvent::WorkspaceChanged {
            node_id: "n-1".into(),
            change: "updated".into(),
        },
    )];
    assert_eq!(
        RedeemContext::from_events(&events),
        RedeemContext::default(),
        "a batch with no OperatorMessage carries nothing to replay"
    );
}

/// Issue #1846 review: the ambient scope round-trips exactly the shape
/// `CHAT_ONLY_TURN` already proves for its own hint — set, read from
/// inside, and gone once the scope's future finishes.
#[tokio::test]
async fn current_redeem_context_reads_the_ambient_scope_and_defaults_outside_it() {
    assert_eq!(
        current_redeem_context(),
        RedeemContext::default(),
        "outside any scope, the ambient context is the default"
    );

    let ctx = RedeemContext {
        parent: Some(EventSeq::new(3)),
        deliverable: Some(MessageIntent::Workflow),
        mentions: Vec::new(),
        text: None,
        attachments: Vec::new(),
    };
    let read_back = with_redeem_context(ctx.clone(), async { current_redeem_context() }).await;
    assert_eq!(
        read_back, ctx,
        "inside the scope, the ambient context is what was set"
    );

    assert_eq!(
        current_redeem_context(),
        RedeemContext::default(),
        "the scope does not leak past its own future"
    );
}

/// The approval sweep caps how much housekeeping one tick does
/// (`MAX_RETIREMENTS_PER_TICK`); `GrantSet::sweep` has no equivalent, so a
/// company with a large expired-grant backlog processes every one of them
/// on a single minute tick. Pinned here so a future cap on the approval
/// sweep's model does not get "generalized" onto this one silently — if a
/// cap is ever added, this test is expected to need updating.
#[test]
fn sweep_processes_every_expired_grant_in_one_call_with_no_cap() {
    let set = GrantSet::default();
    for i in 0..250 {
        set.grant(call(
            &format!("g{i}"),
            "finance",
            "t",
            serde_json::json!({ "i": i }),
        ));
    }
    let expired = set.sweep(1_000 + GRANT_TTL_MILLIS, GRANT_TTL_MILLIS);
    assert_eq!(
        expired.len(),
        250,
        "every expired grant is swept in one call; nothing is held back for a later tick"
    );
    assert_eq!(set.live_count(), 0);
}

#[test]
fn rehydrate_standing_refuses_a_line_past_the_seven_day_ceiling() {
    let set = GrantSet::default();
    let far_future_expiry = 1_000 + MAX_STANDING_GRANT_MILLIS * 10;
    set.rehydrate_standing([standing("g1", "maya", "web_fetch", far_future_expiry)]);
    assert!(
        set.standing()
            .into_iter()
            .all(|g| g.expires_at_millis <= 1_000 + MAX_STANDING_GRANT_MILLIS),
        "a rehydrated standing grant must not outlive the 7-day ceiling every other \
         mint path enforces, even when the journal line itself claims a longer expiry"
    );
}

#[test]
fn rehydrate_standing_checks_each_duration_without_rebasing_its_expiry() {
    for (at_millis, expires_at_millis, accepted) in [
        (1_000, 1_001, true),
        (1_000, 1_000 + MAX_STANDING_GRANT_MILLIS, true),
        (1_000, 1_001 + MAX_STANDING_GRANT_MILLIS, false),
        (1_000, 1_000, false),
        (1_000, 999, false),
        (0, u64::MAX, false),
        (u64::MAX - MAX_STANDING_GRANT_MILLIS, u64::MAX, true),
    ] {
        for verdict in [Verdict::Approve, Verdict::Deny] {
            let set = GrantSet::default();
            let mut grant = standing("candidate", "maya", "web_fetch", expires_at_millis);
            grant.at_millis = at_millis;
            grant.verdict = verdict;
            let valid = standing("valid", "maya", "web_fetch", 2_000);
            set.rehydrate_standing([grant.clone(), valid.clone()]);

            assert_eq!(
                set.peek_standing_by_approval(&grant.approval_id),
                accepted.then_some(grant),
                "duration bounds: {at_millis}..{expires_at_millis}, {verdict:?}"
            );
            assert_eq!(
                set.peek_standing_by_approval(&valid.approval_id),
                Some(valid),
                "an invalid line must not discard a valid sibling"
            );
        }
    }
}

/// `subject()` reads an empty `agent` with no `workflow` as an agent
/// subject held by the empty-string agent, rather than refusing the line.
/// No live call names an empty agent id, so this is inert today — but
/// nothing here rejects a malformed line that reaches it. Pinned as
/// documented, accepted behaviour rather than a defect: the guard belongs
/// at whatever writes the journal line, not at replay.
#[test]
fn subject_of_an_empty_agent_with_no_workflow_is_an_agent_named_the_empty_string() {
    let malformed = standing("g1", "", "web_fetch", 9_999);
    assert_eq!(malformed.subject(), GrantSubject::agent(""));
}

/// `pending` marks are in-memory only and never rehydrated — accepted
/// because a boot sweeps every checkout regardless (see the module docs
/// on `mark_pending`). Pinned so that acceptance is asserted rather than
/// merely claimed: a restart genuinely drops the mark, and a fresh
/// process's `any_for_task` reports the task as no longer held even
/// though, in the old process, an approval was still pending on it.
#[test]
fn a_restart_drops_the_pending_mark_and_the_task_reads_as_no_longer_held() {
    let before_restart = GrantSet::default();
    before_restart.mark_pending(&ApprovalId::new("a1"), "t-1".to_string());
    assert!(before_restart.any_for_task("t-1"));

    // A restart is a fresh GrantSet; nothing seeds `pending` from the
    // journal, unlike `live` (rehydrate) and `standing` (rehydrate_standing).
    let after_restart = GrantSet::default();
    assert!(
        !after_restart.any_for_task("t-1"),
        "the pending mark does not survive a restart; the task reads as unheld \
         until its checkout sweep confirms that independently"
    );
}
