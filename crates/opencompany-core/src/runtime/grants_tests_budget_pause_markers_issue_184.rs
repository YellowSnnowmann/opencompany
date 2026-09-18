use super::tests_core::*;
use super::*;

/// Issue #1458, the reconcile: newest standing decision wins. An approval
/// minted after a live denial of the same scope takes the denial back, or
/// `ApprovalPolicy`'s deny-above-grant ordering would leave the operator's
/// later "yes" listed but never admitting a call.
#[test]
fn an_approval_mint_supersedes_a_live_denial_of_the_same_scope() {
    let set = GrantSet::default();
    let mut deny = scoped("deny-1", "ops", "web_fetch", "https://docs.rs", 10_000);
    deny.verdict = crate::ports::types::Verdict::Deny;
    set.grant_standing(deny);

    let drained = set.opposite_polarity(
        &GrantSubject::agent("ops"),
        "web_fetch",
        Some("https://docs.rs"),
        crate::ports::types::Verdict::Approve,
        2_000,
    );
    assert_eq!(drained.len(), 1, "the shadowing denial is taken back");
    assert_eq!(drained[0].id, GrantId::new("deny-1"));
    assert!(
        set.match_standing(
            &GrantSubject::agent("ops"),
            "web_fetch",
            Some("https://docs.rs"),
            2_000
        )
        .is_none(),
        "only the new approval is left for this scope"
    );
}

/// The mirror direction: a denial minted after a live approval of the same
/// scope takes the grant back, so the operator's newer refusal is the whole
/// of the standing contract.
#[test]
fn a_denial_mint_supersedes_a_live_approval_of_the_same_scope() {
    let set = GrantSet::default();
    set.grant_standing(scoped(
        "grant-1",
        "ops",
        "web_fetch",
        "https://docs.rs",
        10_000,
    ));

    let drained = set.opposite_polarity(
        &GrantSubject::agent("ops"),
        "web_fetch",
        Some("https://docs.rs"),
        crate::ports::types::Verdict::Deny,
        2_000,
    );
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].id, GrantId::new("grant-1"));
    set.revoke_standing(&drained[0].id);
    assert_eq!(set.standing().len(), 0);
}

/// Opposite polarities for *different* scopes coexist: a denial of one host
/// neither shadows nor takes back a grant for another, exactly as they would
/// if minted in isolation.
#[test]
fn a_denial_for_one_host_does_not_take_back_a_grant_for_another() {
    let set = GrantSet::default();
    set.grant_standing(scoped(
        "grant-1",
        "ops",
        "web_fetch",
        "https://docs.rs",
        10_000,
    ));
    let mut deny = scoped(
        "deny-1",
        "ops",
        "web_fetch",
        "https://other.example",
        10_000,
    );
    deny.verdict = crate::ports::types::Verdict::Deny;
    set.grant_standing(deny);

    let drained = set.opposite_polarity(
        &GrantSubject::agent("ops"),
        "web_fetch",
        Some("https://docs.rs"),
        crate::ports::types::Verdict::Approve,
        2_000,
    );
    assert!(
        drained.is_empty(),
        "a denial for other.example does not shadow a docs.rs approval"
    );
    assert_eq!(set.standing().len(), 2);
}

/// A wildcard old policy (a scope the tool could not resolve, recorded
/// `None`) shadows *every* new policy for the same tool, so it is
/// superseded too — the newer decision is the whole of the contract.
#[test]
fn a_wildcard_denial_is_superseded_by_any_new_policy_for_the_same_tool() {
    let set = GrantSet::default();
    let mut deny = standing("deny-1", "ops", "web_fetch", 10_000); // scope None
    deny.verdict = crate::ports::types::Verdict::Deny;
    set.grant_standing(deny);

    let drained = set.opposite_polarity(
        &GrantSubject::agent("ops"),
        "web_fetch",
        Some("https://docs.rs"),
        crate::ports::types::Verdict::Approve,
        2_000,
    );
    assert_eq!(drained.len(), 1);
}

/// The mirror of the wildcard-old case: a **new** wildcard policy (an
/// unresolvable scope, recorded `None`) shadows every scoped opposite policy
/// for the same tool, so the reconcile takes the older scoped one too.
/// Otherwise it would sit listed-but-inert while the wildcard refused every
/// call, and silently resurrect when the wildcard expired — the newest
/// standing decision should be the whole of the contract.
#[test]
fn a_new_wildcard_policy_supersedes_an_older_scoped_opposite() {
    let set = GrantSet::default();
    set.grant_standing(scoped(
        "approve-1",
        "ops",
        "web_fetch",
        "https://docs.rs",
        10_000,
    ));

    let drained = set.opposite_polarity(
        &GrantSubject::agent("ops"),
        "web_fetch",
        None, // a scope the tool could not resolve
        crate::ports::types::Verdict::Deny,
        2_000,
    );
    assert_eq!(
        drained.len(),
        1,
        "the new wildcard refusal supersedes the older scoped approval"
    );
    assert_eq!(drained[0].id, GrantId::new("approve-1"));
    assert_eq!(
        set.standing().len(),
        1,
        "the reconcile is read-only; the caller persists the revocation before revoking"
    );
}

#[test]
fn same_polarity_policies_are_never_reconciled() {
    let set = GrantSet::default();
    set.grant_standing(scoped("g1", "ops", "web_fetch", "https://docs.rs", 10_000));

    let drained = set.opposite_polarity(
        &GrantSubject::agent("ops"),
        "web_fetch",
        Some("https://docs.rs"),
        crate::ports::types::Verdict::Approve,
        2_000,
    );
    assert!(drained.is_empty());
    assert_eq!(set.standing().len(), 1);
}

/// An expired opposite-polarity policy shadows nothing — the matcher
/// refuses it — so the reconcile leaves it for the sweep.
#[test]
fn an_expired_opposite_polarity_policy_is_left_for_the_sweep() {
    let set = GrantSet::default();
    let mut deny = scoped("deny-1", "ops", "web_fetch", "https://docs.rs", 5_000);
    deny.verdict = crate::ports::types::Verdict::Deny;
    set.grant_standing(deny);

    let drained = set.opposite_polarity(
        &GrantSubject::agent("ops"),
        "web_fetch",
        Some("https://docs.rs"),
        crate::ports::types::Verdict::Approve,
        6_000,
    );
    assert!(drained.is_empty());
    assert_eq!(set.standing().len(), 1);
}

#[test]
fn a_standing_grant_is_scoped_to_its_agent_and_its_tool() {
    let set = GrantSet::default();
    set.grant_standing(standing("g1", "ops", "shell", 10_000));

    assert!(
        set.match_standing(&GrantSubject::agent("marketing"), "shell", None, 2_000)
            .is_none(),
        "another teammate is not who the operator granted"
    );
    assert!(
        set.match_standing(&GrantSubject::agent("ops"), "workspace_write", None, 2_000)
            .is_none(),
        "another tool is not what the operator granted"
    );
}

/// Issue #457, the discrimination that matters. A grant minted from a
/// GitHub read admits another GitHub read — the operator consented to a
/// provider, not to one action — and refuses a Gmail read. Both are
/// catalogue reads, so nothing upstream of this predicate tells them apart:
/// same agent, same tool, same grantable verdict.
#[test]
fn a_scoped_grant_admits_its_own_provider_and_refuses_another() {
    let set = GrantSet::default();
    set.grant_standing(scoped("g1", "ops", "composio_execute", "github", 10_000));

    assert!(
        set.match_standing(
            &GrantSubject::agent("ops"),
            "composio_execute",
            Some("github"),
            2_000
        )
        .is_some(),
        "a second GitHub read is the sentence the operator agreed to"
    );
    assert!(
        set.match_standing(
            &GrantSubject::agent("ops"),
            "composio_execute",
            Some("gmail"),
            2_000
        )
        .is_none(),
        "'read from GitHub' is not consent to read a mailbox"
    );
}

/// A call the catalogue cannot place has no scope, and a scoped grant must
/// not admit it: it could belong to any provider, and guessing would guess
/// permissively. It falls through and parks, which is what this codebase
/// does with every unrecognised action.
#[test]
fn a_scoped_grant_refuses_a_call_with_no_scope_at_all() {
    let set = GrantSet::default();
    set.grant_standing(scoped("g1", "ops", "composio_execute", "github", 10_000));

    assert!(
        set.match_standing(&GrantSubject::agent("ops"), "composio_execute", None, 2_000)
            .is_none()
    );
}

/// **Replay compatibility (issue #457).** A `StandingGrantMinted` line
/// written before the scope field existed deserializes with `scope: None`,
/// and an unscoped grant behaves exactly as it did before this change —
/// admitting the tool whatever the live scope is. Without this the upgrade
/// would silently void every permission an operator granted before it.
#[test]
fn a_grant_journaled_before_scopes_existed_replays_and_behaves_as_before() {
    // The old wire shape, verbatim: no `scope` key anywhere.
    let line = serde_json::json!({
        "id": "g-old",
        "agent": "ops",
        "tool": "composio_execute",
        "granted_by": { "kind": "user", "id": "user-1" },
        "approval_id": "approval-g-old",
        "at_millis": 1_000,
        "expires_at_millis": 10_000,
    });
    let replayed: StandingGrant =
        serde_json::from_value(line).expect("an old line still deserializes");
    assert_eq!(replayed.scope, None, "absent means unscoped, not broken");

    let set = GrantSet::default();
    set.rehydrate_standing([replayed]);

    // Unscoped: it admits the tool exactly as it did before scopes existed,
    // whatever the live call resolves to.
    for live in [Some("github"), Some("gmail"), None] {
        assert!(
            set.match_standing(&GrantSubject::agent("ops"), "composio_execute", live, 2_000)
                .is_some(),
            "an unscoped grant must keep admitting: {live:?}"
        );
    }
    // …and the boundaries it always had still hold.
    assert!(
        set.match_standing(
            &GrantSubject::agent("marketing"),
            "composio_execute",
            Some("github"),
            2_000
        )
        .is_none()
    );
    assert!(
        set.match_standing(
            &GrantSubject::agent("ops"),
            "composio_execute",
            Some("github"),
            10_000
        )
        .is_none()
    );
}

/// A scoped grant round-trips, and an unscoped one still writes the old
/// shape — so a journal read by an older build is unchanged.
#[test]
fn the_scope_round_trips_and_is_omitted_when_absent() {
    let unscoped = serde_json::to_value(standing("g1", "ops", "shell", 10_000)).expect("json");
    assert!(
        unscoped.get("scope").is_none(),
        "an unscoped grant writes the pre-#457 line: {unscoped}"
    );

    let grant = scoped("g2", "ops", "composio_execute", "github", 10_000);
    let round: StandingGrant =
        serde_json::from_value(serde_json::to_value(&grant).expect("json")).expect("round trip");
    assert_eq!(round, grant);
}

#[test]
fn revoking_a_standing_grant_stops_it_matching() {
    let set = GrantSet::default();
    set.grant_standing(standing("g1", "ops", "shell", 10_000));
    assert!(
        set.match_standing(&GrantSubject::agent("ops"), "shell", None, 2_000)
            .is_some()
    );

    let revoked = set.revoke_standing(&GrantId::new("g1")).expect("was live");
    assert_eq!(revoked.tool, "shell");
    assert!(
        set.match_standing(&GrantSubject::agent("ops"), "shell", None, 2_000)
            .is_none()
    );
    assert_eq!(set.standing_count(), 0);
    assert!(
        set.revoke_standing(&GrantId::new("g1")).is_none(),
        "revoking twice reports nothing to revoke rather than pretending"
    );
}

/// `GET {scope}/grants` renders every entry `standing()`
/// returns with no pagination and no cap of its own — unlike, say, the
/// approval sweep's `MAX_RETIREMENTS_PER_TICK`. This pins that a large
/// standing-grant set comes back whole rather than a bounded page of it.
#[test]
fn standing_returns_every_live_grant_with_no_cap() {
    let set = GrantSet::default();
    for i in 0..500 {
        set.grant_standing(standing(
            &format!("g{i}"),
            "ops",
            &format!("tool-{i}"),
            10_000,
        ));
    }
    assert_eq!(
        set.standing().len(),
        500,
        "every standing grant is returned; nothing pages or truncates the list"
    );
}

/// A single-use grant must burn even when a standing grant would also have
/// admitted the call.
///
/// The ordering is enforced by the policy arm, but the primitives have to
/// make it expressible: `consume` is what removes, and a standing match
/// never touches the single-use set. If a standing match ran first, the
/// operator's one-off approval would sit live until its TTL and then be
/// announced as "the agent never acted" — a lie about work that ran.
#[test]
fn a_single_use_grant_still_burns_while_a_standing_grant_is_live() {
    let set = GrantSet::default();
    let args = serde_json::json!({ "cmd": "ls" });
    set.grant(call("a1", "ops", "shell", args.clone()));
    set.grant_standing(standing("g1", "ops", "shell", 10_000));

    assert!(set.consume("ops", "shell", &args).is_some());
    assert_eq!(set.live_count(), 0, "the single-use grant burned");
    assert_eq!(set.standing_count(), 1, "the standing grant is untouched");
    assert_eq!(set.drain_consumed().len(), 1);
}

#[test]
fn sweep_standing_removes_only_lapsed_grants() {
    let set = GrantSet::default();
    set.grant_standing(standing("old", "ops", "shell", 5_000));
    set.grant_standing(standing("new", "ops", "workspace_write", 50_000));

    let expired = set.sweep_standing(10_000);
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].id, GrantId::new("old"));
    assert_eq!(set.standing_count(), 1);
    assert!(
        set.match_standing(&GrantSubject::agent("ops"), "workspace_write", None, 10_000)
            .is_some()
    );
}

#[test]
fn the_longest_lived_match_wins_so_redemption_is_not_hash_order() {
    let set = GrantSet::default();
    set.grant_standing(standing("short", "ops", "shell", 5_000));
    set.grant_standing(standing("long", "ops", "shell", 50_000));

    let matched = set
        .match_standing(&GrantSubject::agent("ops"), "shell", None, 1_000)
        .expect("matches");
    assert_eq!(matched.id, GrantId::new("long"));
}

#[test]
fn standing_grants_rehydrate_and_are_findable_by_their_approval() {
    let set = GrantSet::default();
    set.rehydrate_standing([
        standing("g1", "ops", "shell", 10_000),
        standing("g2", "legal", "workspace_write", 10_000),
    ]);
    assert_eq!(set.standing_count(), 2);

    let found = set
        .peek_standing_by_approval(&ApprovalId::new("approval-g2"))
        .expect("provenance is queryable");
    assert_eq!(found.agent, "legal");
    assert!(
        set.peek_standing_by_approval(&ApprovalId::new("nope"))
            .is_none()
    );

    // Newest first, and `at_millis` ties break on id so the list is stable.
    let listed = set.standing();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, GrantId::new("g1"));
}

// --- budget-pause markers (issue #1846) ----------------------------------

#[test]
fn parking_then_peeking_a_budget_pause_does_not_consume_it() {
    let set = BudgetPauseSet::default();
    set.park(
        "ceo",
        Some("desk-1".to_string()),
        "hi",
        "paused",
        1_000,
        RedeemContext::default(),
    );

    let first = set.peek("ceo").expect("parked");
    let second = set.peek("ceo").expect("peek does not consume");
    assert_eq!(first.id, second.id, "the same marker both times");
    assert_eq!(first.message, "hi");
    assert_eq!(first.chat_id.as_deref(), Some("desk-1"));
}

/// Issue #1846 review (Codex #3870400579) — **the regression.** A
/// delegation re-park (`runtime/delegation.rs`) has no `LiveStream`
/// context of its own to answer `background` from, so it must carry
/// forward whatever the delegate's own `run_inner` call already set,
/// rather than resetting it to `false` via plain `park`.
#[test]
fn park_preserving_background_carries_the_flag_forward() {
    let set = BudgetPauseSet::default();
    set.park_background(
        "ceo",
        None,
        "the hand-off instruction",
        "paused",
        1_000,
        RedeemContext::default(),
    );
    assert!(
        set.peek("ceo").expect("parked").background,
        "fixture sanity: the marker `run_inner` would have parked is background"
    );

    let reparked = set.park_preserving_background(
        "ceo",
        None,
        "the corrected original request",
        "paused",
        2_000,
        RedeemContext::default(),
    );
    assert!(
        reparked.background,
        "re-parking with corrected text must not silently reset background to false"
    );
    assert_eq!(reparked.message, "the corrected original request");
}

/// The other half: an ordinary (non-background) marker's re-park must
/// stay non-background — this method is not a way to ACCIDENTALLY turn
/// an interactive marker into a background one either.
#[test]
fn park_preserving_background_leaves_a_non_background_marker_alone() {
    let set = BudgetPauseSet::default();
    set.park(
        "ceo",
        Some("general".to_string()),
        "hi",
        "paused",
        1_000,
        RedeemContext::default(),
    );
    assert!(!set.peek("ceo").expect("parked").background);

    let reparked = set.park_preserving_background(
        "ceo",
        Some("general".to_string()),
        "hi, corrected",
        "paused",
        2_000,
        RedeemContext::default(),
    );
    assert!(!reparked.background);
}

/// No prior marker to read a flag off of — defaults to `false`, the
/// same as plain `park`, rather than panicking or guessing `true`.
#[test]
fn park_preserving_background_defaults_to_false_with_nothing_parked_yet() {
    let set = BudgetPauseSet::default();
    let marker = set.park_preserving_background(
        "ceo",
        None,
        "hi",
        "paused",
        1_000,
        RedeemContext::default(),
    );
    assert!(!marker.background);
}

#[test]
fn redeeming_a_budget_pause_consumes_it_exactly_once() {
    let set = BudgetPauseSet::default();
    set.park("ceo", None, "hi", "paused", 1_000, RedeemContext::default());

    let redeemed = set.redeem("ceo").expect("a marker was parked");
    assert_eq!(redeemed.agent, "ceo");
    assert!(
        set.redeem("ceo").is_none(),
        "single-use: a second redeem finds nothing"
    );
    assert!(set.peek("ceo").is_none());
}

#[test]
fn redeem_reserves_atomically_so_a_second_concurrent_redeem_finds_nothing() {
    // Issue #1846 review (Codex #3865395849): two redeem requests that
    // both read the marker before either re-dispatches would both
    // re-dispatch the same non-idempotent message. Reserving with plain
    // `redeem` up front — rather than `peek`, then consume after — means
    // the SECOND caller's own `redeem` finds nothing, closing the race
    // before it ever gets to re-dispatch.
    let set = BudgetPauseSet::default();
    let marker = set.park("ceo", None, "hi", "paused", 1_000, RedeemContext::default());

    let first = set.redeem("ceo").expect("the first reservation wins");
    assert_eq!(first.id, marker.id);
    assert!(
        set.redeem("ceo").is_none(),
        "a second, concurrent reservation finds nothing parked"
    );
}

#[test]
fn restore_if_absent_puts_a_failed_redispatchs_marker_back() {
    let set = BudgetPauseSet::default();
    let marker = set.park("ceo", None, "hi", "paused", 1_000, RedeemContext::default());

    let reserved = set.redeem("ceo").expect("reserved for redispatch");
    assert!(set.peek("ceo").is_none(), "reserved out of the set");

    // The redispatch failed — restore it for a retry to find.
    set.restore_if_absent(reserved);
    let restored = set.peek("ceo").expect("restored after the failure");
    assert_eq!(restored.id, marker.id);
    assert_eq!(restored.message, "hi");
}

#[test]
fn restore_if_absent_leaves_a_fresher_marker_untouched() {
    // The re-dispatch a redeem triggers can itself re-pause the same
    // agent before the failed-redispatch's restore call runs. The stale
    // marker being restored must not delete the operator's not-yet-seen
    // fresh one out from under them.
    let set = BudgetPauseSet::default();
    let stale = set.park(
        "ceo",
        None,
        "first stuck message",
        "paused once",
        1_000,
        RedeemContext::default(),
    );
    let reserved = set.redeem("ceo").expect("reserved for redispatch");
    assert_eq!(reserved.id, stale.id);

    // The (failed) redispatch itself re-entered the cycle and paused
    // again before the restore call below runs.
    set.park(
        "ceo",
        None,
        "second stuck message",
        "paused again",
        2_000,
        RedeemContext::default(),
    );

    set.restore_if_absent(reserved);
    let still_parked = set.peek("ceo").expect("the fresher marker survives");
    assert_eq!(still_parked.message, "second stuck message");
}

#[test]
fn a_second_pause_on_the_same_agent_overwrites_the_first() {
    let set = BudgetPauseSet::default();
    set.park(
        "ceo",
        None,
        "first stuck message",
        "paused once",
        1_000,
        RedeemContext::default(),
    );
    set.park(
        "ceo",
        None,
        "second stuck message",
        "paused again",
        2_000,
        RedeemContext::default(),
    );

    let marker = set.redeem("ceo").expect("the latest marker");
    assert_eq!(
        marker.message, "second stuck message",
        "the operator's next redeem re-issues the LATEST stalled message, not a queue"
    );
}
