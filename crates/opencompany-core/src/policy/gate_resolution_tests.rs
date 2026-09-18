use super::gate_approval_tests::*;
use super::*;

/// Issue #243: the four outcomes the port's `Option<Effect>` cannot tell
/// apart. The important pair is `Denied` vs `NotParked` — the caller must
/// journal and re-cycle the first and do nothing at all for the second.
#[tokio::test]
async fn resolve_outcome_distinguishes_all_four_results() {
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    let eff = effect("filing.submit", EffectGroup::Sign);

    // Unknown id.
    assert_eq!(
        gate.resolve_outcome(
            &ApprovalId::new("never-parked"),
            Verdict::Approve,
            operator(),
            now_millis()
        ),
        ResolveOutcome::NotParked
    );

    // Approved in time.
    let id = gate.park(&company(), eff.clone()).await.unwrap();
    assert_eq!(
        gate.resolve_outcome(&id, Verdict::Approve, operator(), now_millis()),
        ResolveOutcome::Approved(eff.clone())
    );
    // ...and resolving it a second time is NOT a deny, it is a no-op.
    assert_eq!(
        gate.resolve_outcome(&id, Verdict::Approve, operator(), now_millis()),
        ResolveOutcome::NotParked,
        "an already-resolved approval must not look like a fresh deny"
    );

    // Denied.
    let id = gate.park(&company(), eff.clone()).await.unwrap();
    assert_eq!(
        gate.resolve_outcome(&id, Verdict::Deny, operator(), now_millis()),
        ResolveOutcome::Denied(eff.clone()),
        "a deny must carry the parked effect so a standing denial can be \
         scoped to what the operator actually refused (issue #1458)"
    );

    // Expired: past the TTL, an approve still resolves to a default-deny,
    // and reports it as expiry rather than as the operator's choice.
    let short = ManifestApprovalGate::new(policy("supervised", None)).with_ttl_millis(1000);
    let id = short.park(&company(), eff).await.unwrap();
    assert_eq!(
        short.resolve_outcome(&id, Verdict::Approve, operator(), now_millis() + 10_000),
        ResolveOutcome::Expired
    );
    assert!(
        short.parked_ids().is_empty(),
        "expiry still drains the queue"
    );
}

/// Two operators (or one double-click, or a retried request) resolving the
/// same approval concurrently: exactly one wins.
///
/// The `remove` and the outcome decision share a critical section precisely
/// so this cannot double-fire. A check-then-act caller — "is it parked? then
/// resolve it" — would let both threads through and execute the approved
/// effect twice; the at-most-once journal key would catch the *effect*, but
/// the duplicate journal record and the duplicate follow-up cycle would
/// still land.
#[tokio::test]
async fn concurrent_resolves_of_one_approval_yield_exactly_one_winner() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let gate = Arc::new(ManifestApprovalGate::new(policy("supervised", None)));
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();

    let approvals = Arc::new(AtomicUsize::new(0));
    let not_parked = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(std::sync::Barrier::new(8));

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let gate = Arc::clone(&gate);
        let id = id.clone();
        let approvals = Arc::clone(&approvals);
        let not_parked = Arc::clone(&not_parked);
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::task::spawn_blocking(move || {
            barrier.wait();
            match gate.resolve_outcome(&id, Verdict::Approve, operator(), now_millis()) {
                ResolveOutcome::Approved(_) => approvals.fetch_add(1, Ordering::SeqCst),
                ResolveOutcome::NotParked => not_parked.fetch_add(1, Ordering::SeqCst),
                other => panic!("unexpected outcome {other:?}"),
            };
        }));
    }
    for t in tasks {
        t.await.expect("task");
    }

    assert_eq!(approvals.load(Ordering::SeqCst), 1, "exactly one winner");
    assert_eq!(not_parked.load(Ordering::SeqCst), 7, "the rest are no-ops");
    assert!(gate.parked_ids().is_empty());
}

/// Issue #351: the irreversibility question, answered by the same taxonomy
/// that decides what parks — and answered the same way whatever mode the
/// company runs in.
///
/// The mode-independence is the part worth pinning. A `full`-mode company
/// executes a filing without ever parking it, so the retry dialog is the
/// only place anybody will ever be told it happened; a predicate that read
/// the company's mode would say "nothing to warn about" in exactly the
/// configuration that needs the warning most.
#[test]
pub(super) fn irreversibility_follows_the_supervised_taxonomy_in_every_mode() {
    for mode in ["supervised", "full", "readonly"] {
        let gate = ManifestApprovalGate::new(policy(mode, Some(100.0)));

        for group in [
            EffectGroup::Sign,
            EffectGroup::Publish,
            EffectGroup::Identity,
        ] {
            assert!(
                gate.is_irreversible(&effect("filing.submit", group)),
                "{mode}: {group:?} is irreversible by construction",
            );
        }

        // Spend: the cap decides, strictly.
        let mut under = effect("x402.spend", EffectGroup::Spend);
        under.amount_usd = Some(99.0);
        assert!(!gate.is_irreversible(&under), "{mode}: under the cap");
        let mut at_cap = effect("x402.spend", EffectGroup::Spend);
        at_cap.amount_usd = Some(100.0);
        assert!(gate.is_irreversible(&at_cap), "{mode}: at the cap");

        // Send: first contact is the irreversible half.
        let mut established = effect("email.send", EffectGroup::Send);
        established.established_thread = true;
        assert!(!gate.is_irreversible(&established), "{mode}: a live thread");
        let mut cold = effect("email.send", EffectGroup::Send);
        cold.first_time_counterparty = true;
        assert!(gate.is_irreversible(&cold), "{mode}: first contact");

        // Hire: the same cap, read the same way — plus first contact.
        let mut cheap_hire = effect("a2a.engage", EffectGroup::Hire);
        cheap_hire.amount_usd = Some(99.0);
        assert!(!gate.is_irreversible(&cheap_hire), "{mode}: under the cap");
        let mut at_cap_hire = effect("a2a.engage", EffectGroup::Hire);
        at_cap_hire.amount_usd = Some(100.0);
        assert!(gate.is_irreversible(&at_cap_hire), "{mode}: at the cap");
        let mut first_hire = effect("a2a.engage", EffectGroup::Hire);
        first_hire.amount_usd = Some(10.0);
        first_hire.first_time_counterparty = true;
        assert!(gate.is_irreversible(&first_hire), "{mode}: first contact");

        // Issue #2037: the shapes that omit one of the two inputs, for both
        // money groups. An engagement whose price nobody stated, and one in
        // a company that configured no cap, are irreversible for the same
        // reason a spend is — and they are the shapes the retry dialog lost,
        // because `record_executed` files nothing for a reversible effect.
        let uncapped = ManifestApprovalGate::new(policy(mode, None));
        for (label, group, kind) in [
            ("a spend", EffectGroup::Spend, "x402.spend"),
            ("an engagement", EffectGroup::Hire, "a2a.engage"),
        ] {
            assert!(
                gate.is_irreversible(&effect(kind, group)),
                "{mode}: {label} of unknown amount",
            );
            let mut known = effect(kind, group);
            known.amount_usd = Some(10.0);
            assert!(
                uncapped.is_irreversible(&known),
                "{mode}: {label} with no cap configured",
            );
        }

        // A read changes nothing and warns about nothing.
        assert!(
            !gate.is_irreversible(&effect("web.search", EffectGroup::Other)),
            "{mode}: an ordinary read must not raise a retry warning",
        );
    }
}

/// `always_approve` is a *parking* rule, not an irreversibility one, so it
/// deliberately does not leak into the warning.
///
/// Both live on the same policy block and it would be easy to fold them
/// together. They answer different questions: `always_approve` is "ask me
/// first", which an operator sets for anything they want a say in, while
/// this dialog claims something cannot be taken back. Widening it would
/// warn about routine work and teach people to click through.
#[tokio::test]
async fn always_approve_does_not_widen_what_counts_as_irreversible() {
    let gate = ManifestApprovalGate::new(policy("supervised", Some(100.0)));
    let mut cheap = effect("payment.send", EffectGroup::Spend);
    cheap.amount_usd = Some(1.0);
    // `payment.send` is in FENCE, this module's always_approve fixture, so it parks...
    assert_eq!(decide(&gate, &cheap).await, PolicyDecision::RequireApproval);
    // ...but a dollar under a hundred-dollar cap is not irreversible.
    assert!(!gate.is_irreversible(&cheap));
}

/// **T5 (issue #971).** The deadline binds even when nothing swept.
///
/// The sweep is housekeeping — it retires an entry and tells the operator —
/// but it is emphatically not the enforcement. `resolve_at` re-checks the
/// TTL under the same lock that removes the entry, so a 25-hour-old
/// approval default-denies on the operator's click whether or not any
/// maintenance tick ever ran for that company. That property is what made
/// the missing ticker a *visibility* bug rather than a safety one, and it
/// has to survive the ticker landing: a future refactor that made the sweep
/// the only expiry path would turn a paused host into one that honours
/// week-old consent.
#[tokio::test]
async fn the_default_deadline_binds_at_resolve_with_no_sweep() {
    // No `with_ttl_millis`: the default the constant resolves to, so this
    // fails if the 24h default is quietly widened again.
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    assert_eq!(gate.ttl_millis(), 24 * 60 * 60 * 1000);
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    let twenty_five_hours = now_millis() + 25 * 60 * 60 * 1000;
    // Nothing swept: the entry is still parked right up to the click.
    assert_eq!(gate.parked_ids(), vec![id.clone()]);
    assert_eq!(
        gate.resolve_at(&id, Verdict::Approve, operator(), twenty_five_hours),
        None,
        "a 25h-old approval must default-deny even though no sweep ran"
    );
    // And an hour earlier it would still have been approvable, so the
    // assertion above is about the deadline and not about `resolve_at`
    // refusing everything.
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    let twenty_three_hours = now_millis() + 23 * 60 * 60 * 1000;
    assert!(
        gate.resolve_at(&id, Verdict::Approve, operator(), twenty_three_hours)
            .is_some()
    );
}

/// `[policy].approval_ttl_hours` is what the gate enforces, and an absent
/// knob resolves to the default **here** rather than at parse (issue #971).
#[tokio::test]
async fn the_policy_knob_sets_the_deadline() {
    let configured = Policy {
        approval_ttl_hours: Some(2),
        ..policy("supervised", None)
    };
    let gate = ManifestApprovalGate::new(configured);
    assert_eq!(gate.ttl_millis(), 2 * 60 * 60 * 1000);

    let silent = policy("supervised", None);
    assert_eq!(
        silent.approval_ttl_hours, None,
        "a silent manifest must stay `None` through parse — see the field's note"
    );
    assert_eq!(
        ManifestApprovalGate::new(silent).ttl_millis(),
        DEFAULT_TTL_MILLIS
    );
}

/// The per-tick cap takes the **oldest** expired entries, not an arbitrary
/// subset of them (issue #971).
///
/// `parked` is a `HashMap`, so without the sort this passes or fails by
/// process-random iteration order — and in production an entry could sit
/// unretired across many ticks while newer ones drained ahead of it.
#[tokio::test]
async fn the_sweep_cap_drains_oldest_first() {
    let gate = ManifestApprovalGate::new(policy("supervised", None)).with_ttl_millis(0);
    let mut ids = Vec::new();
    for i in 0..5u64 {
        let id = gate
            .park(&company(), effect("filing.submit", EffectGroup::Sign))
            .await
            .unwrap();
        // Re-park under a known instant: `park` stamps `now`, and five
        // parks inside one millisecond would make "oldest" undecidable.
        gate.rehydrate(
            id.clone(),
            effect("filing.submit", EffectGroup::Sign),
            1_000 + i,
        );
        ids.push(id);
    }
    let first = gate.sweep_expired_capped(10_000, 2);
    assert_eq!(first, ids[..2].to_vec());
    let second = gate.sweep_expired_capped(10_000, 2);
    assert_eq!(second, ids[2..4].to_vec());
    // The uncapped form is the same sweep with no limit.
    assert_eq!(gate.sweep_expired(10_000), ids[4..].to_vec());
    assert!(gate.parked_ids().is_empty());
}

#[tokio::test]
async fn sweep_expired_removes_stale_entries() {
    let gate = ManifestApprovalGate::new(policy("supervised", None)).with_ttl_millis(0);
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    // TTL 0 → everything is immediately expired at a strictly-later time.
    let expired = gate.sweep_expired(now_millis() + 1);
    assert_eq!(expired, vec![id]);
    assert!(gate.parked_ids().is_empty());
}

/// Issue #1805: extending re-anchors the TTL window, so an entry that was
/// one tick from expiry survives the sweep that would have retired it and
/// only expires on the fresh window.
#[test]
pub(super) fn extend_keeps_a_near_expiry_entry_out_of_the_sweep() {
    let gate = ManifestApprovalGate::new(policy("supervised", None)).with_ttl_millis(1_000);
    let id = ApprovalId::from("appr-extend".to_string());
    gate.rehydrate(id.clone(), effect("filing.submit", EffectGroup::Sign), 0);
    // Just before the original deadline (0 + 1000) the operator extends it.
    assert!(gate.extend(&id, 900));
    // Past the ORIGINAL deadline the sweep now leaves it: its window runs
    // from 900, so 1500 - 900 = 600 < 1000.
    assert!(gate.sweep_expired(1_500).is_empty());
    assert_eq!(gate.parked_ids(), vec![id.clone()]);
    // It still expires, on the NEW window: 1901 - 900 = 1001 >= 1000.
    assert_eq!(gate.sweep_expired(1_901), vec![id]);
    // Extending an id that is not parked reports it rather than pretending.
    assert!(!gate.extend(&ApprovalId::from("ghost".to_string()), 0));
}

// -- Emergency stop (issue #86) -----------------------------------------

/// Every side-effecting group is denied. Enumerated rather than sampled:
/// this is a kill switch, and a group added to the taxonomy without being
/// added here is a category of work that quietly keeps running.
#[tokio::test]
async fn emergency_denies_every_side_effecting_group() {
    // `full` mode — the most permissive policy there is. If the stop only
    // worked under `supervised` it would be useless exactly when it matters.
    let gate = ManifestApprovalGate::new(policy("full", None));
    gate.set_emergency(true);

    for group in [
        EffectGroup::Spend,
        EffectGroup::Send,
        EffectGroup::Sign,
        EffectGroup::Publish,
        EffectGroup::Hire,
        EffectGroup::Identity,
    ] {
        assert_eq!(
            decide(&gate, &effect("some.effect", group)).await,
            PolicyDecision::Deny,
            "{group:?} was not denied under emergency stop"
        );
    }
}

/// Chat survives, or the operator cannot ask what happened.
#[tokio::test]
async fn emergency_permits_other_so_chat_keeps_working() {
    let gate = ManifestApprovalGate::new(policy("full", None));
    gate.set_emergency(true);
    assert_eq!(
        decide(&gate, &effect("chat.reply", EffectGroup::Other)).await,
        PolicyDecision::Allow
    );
}

/// The stop outranks `always_approve`, which would otherwise park — and
/// parking is a way *out* of the stop, since the operator could then approve
/// the effect they just stopped without ever releasing the switch.
#[tokio::test]
async fn emergency_denies_rather_than_parking_an_always_approve_effect() {
    let gate = ManifestApprovalGate::new(policy("supervised", Some(100.0)));
    // Baseline: this kind parks under normal policy.
    assert_eq!(
        decide(&gate, &effect("payment.send", EffectGroup::Spend)).await,
        PolicyDecision::RequireApproval
    );
    gate.set_emergency(true);
    assert_eq!(
        decide(&gate, &effect("payment.send", EffectGroup::Spend)).await,
        PolicyDecision::Deny
    );
    // And nothing was parked as a side effect of being denied.
    assert!(gate.parked_ids().is_empty());
}

/// Parks *new* work without corrupting *in-flight* work: an approval already
/// waiting on a person stays resolvable, and approving it still yields the
/// effect to execute. Denying already-parked work instead would strand
/// decisions the operator had already been asked for.
#[tokio::test]
async fn emergency_leaves_already_parked_approvals_resolvable() {
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    let parked = effect("filing.submit", EffectGroup::Sign);
    let id = gate.park(&company(), parked.clone()).await.unwrap();

    gate.set_emergency(true);

    // Still visible to the operator...
    assert_eq!(gate.parked_ids(), vec![id.clone()]);
    assert_eq!(gate.parked_effect(&id), Some(parked.clone()));
    // ...and still resolvable, yielding the effect to execute.
    let resolved = gate
        .resolve(&id, Verdict::Approve, operator())
        .await
        .unwrap();
    assert_eq!(resolved, Some(parked));
}

/// A denial for a parked approval is still a denial while stopped — the
/// switch must not turn "deny" into "approve" by accident.
#[tokio::test]
async fn emergency_refuses_to_park_new_side_effecting_effects() {
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    gate.set_emergency(true);

    // A side-effecting effect cannot be queued for approval while stopped —
    // the harness approval route parks without consulting `evaluate`, so
    // without this veto a gated effect could be released after the stop.
    let err = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap_err();
    assert!(matches!(err, crate::OpenCompanyError::EmergencyStop(_)));
    assert!(gate.parked_ids().is_empty());

    // Chat still parks while stopped — the veto is the *same* one `evaluate`
    // applies, so an `EffectGroup::Other` effect is not caught by it.
    let id = gate
        .park(&company(), effect("chat.reply", EffectGroup::Other))
        .await
        .expect("an Other-group effect parks while stopped");
    assert_eq!(gate.parked_ids(), vec![id]);
}

/// A denial for a parked approval is still a denial while stopped — the
/// switch must not turn "deny" into "approve" by accident.
#[tokio::test]
async fn emergency_does_not_change_a_denied_resolution() {
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    gate.set_emergency(true);
    let resolved = gate.resolve(&id, Verdict::Deny, operator()).await.unwrap();
    assert_eq!(resolved, None);
}

/// Releasing restores the *previous* policy exactly — the stop is a veto
/// layered over evaluation, not a rewrite of it.
#[tokio::test]
async fn releasing_restores_normal_evaluation() {
    let gate = ManifestApprovalGate::new(policy("full", None));
    let spend = effect("payment.send", EffectGroup::Spend);
    // `payment.send` is in FENCE, this module's always_approve fixture, so `full` parks it.
    let before = decide(&gate, &spend).await;
    assert_eq!(before, PolicyDecision::RequireApproval);

    gate.set_emergency(true);
    assert_eq!(decide(&gate, &spend).await, PolicyDecision::Deny);

    gate.set_emergency(false);
    assert_eq!(decide(&gate, &spend).await, before);
}

/// **No implicit un-pause.** The TTL sweep expires parked *approvals*; it
/// must never touch the switch. A kill switch with a timeout is a delay, and
/// the failure mode is silent: work resumes at 3am with nobody watching.
#[tokio::test]
async fn emergency_does_not_decay_with_the_approval_ttl() {
    let gate = ManifestApprovalGate::new(policy("supervised", None)).with_ttl_millis(0);
    gate.park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    gate.set_emergency(true);

    // Sweep far past any TTL: every parked approval expires...
    let expired = gate.sweep_expired(now_millis() + DEFAULT_TTL_MILLIS * 1000);
    assert_eq!(expired.len(), 1);

    // ...and the stop is still engaged.
    assert!(gate.is_emergency());
    assert_eq!(
        decide(&gate, &effect("payment.send", EffectGroup::Spend)).await,
        PolicyDecision::Deny
    );
}

/// Boot replay: the *last* `EmergencyPauseChanged` decides, so a stop
/// survives a restart and a release survives one too.
///
/// This is the property the whole persistence design exists for — a kill
/// switch that evaporates when the process restarts is not a kill switch,
/// and a release that does not stick would strand a company nobody can
/// restart.
#[tokio::test]
async fn replay_takes_the_last_emergency_event() {
    use crate::ports::EventLog;
    use crate::ports::types::CompanyEvent;
    use std::sync::Arc;

    let home = tempfile::Builder::new()
        .prefix("oc-emergency-replay-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(crate::store::FsEventLog::new(home.path()));
    let id = company();

    // A log with no such event was never stopped.
    assert!(!replayed_emergency(&events, &id).await.unwrap());

    let change = |engaged: bool| CompanyEvent::EmergencyPauseChanged {
        engaged,
        by: operator(),
        reason: None,
    };

    events.append(&id, change(true)).await.unwrap();
    assert!(replayed_emergency(&events, &id).await.unwrap());

    events.append(&id, change(false)).await.unwrap();
    assert!(!replayed_emergency(&events, &id).await.unwrap());

    // A second stop after a release wins again — the switch is not
    // one-shot, and "last write wins" must hold in both directions.
    events.append(&id, change(true)).await.unwrap();
    assert!(replayed_emergency(&events, &id).await.unwrap());
}

/// The realistic shape of a company's log: the last
/// `EmergencyPauseChanged` is not the last event in the log at all — chat,
/// webhooks and everything else keep being appended after an operator
/// releases (or engages) the switch, right up to the moment this reads it.
/// `replayed_emergency` finds the *last matching* event scanning backward
/// ([`Iterator::rev`] plus [`Iterator::find_map`]), not the last event of
/// any kind, so trailing unrelated events must not shadow it.
#[tokio::test]
async fn replay_finds_the_last_emergency_event_under_trailing_unrelated_events() {
    use crate::ports::EventLog;
    use crate::ports::types::CompanyEvent;
    use std::sync::Arc;

    let home = tempfile::Builder::new()
        .prefix("oc-emergency-replay-trailing-")
        .tempdir()
        .expect("tempdir");
    let events: Arc<dyn EventLog> = Arc::new(crate::store::FsEventLog::new(home.path()));
    let id = company();

    let filler = || CompanyEvent::WebhookReceived {
        channel: "test".to_string(),
        body: serde_json::Value::Null,
    };
    let change = |engaged: bool| CompanyEvent::EmergencyPauseChanged {
        engaged,
        by: operator(),
        reason: None,
    };

    for _ in 0..5 {
        events.append(&id, filler()).await.unwrap();
    }
    events.append(&id, change(true)).await.unwrap();
    for _ in 0..25 {
        events.append(&id, filler()).await.unwrap();
    }

    assert!(
        replayed_emergency(&events, &id).await.unwrap(),
        "25 trailing unrelated events must not shadow the last real \
         EmergencyPauseChanged"
    );

    events.append(&id, change(false)).await.unwrap();
    for _ in 0..25 {
        events.append(&id, filler()).await.unwrap();
    }

    assert!(
        !replayed_emergency(&events, &id).await.unwrap(),
        "the release must still be found under the same trailing noise"
    );
}

/// A fresh gate is not stopped. The boot path is the only caller that can
/// tell "not stopped" from "could not find out", and it says so explicitly.
#[tokio::test]
async fn a_new_gate_is_not_stopped() {
    let gate = ManifestApprovalGate::new(policy("full", None));
    assert!(!gate.is_emergency());
    // A side-effecting group, so this would be `Deny` if the switch
    // defaulted engaged — but a kind outside `always_approve`, so under
    // `full` the undisturbed answer is `Allow` rather than a park.
    assert_eq!(
        decide(&gate, &effect("blog.post", EffectGroup::Publish)).await,
        PolicyDecision::Allow
    );
}

/// The live-policy update keeps the parked queue and the emergency switch
/// while the evaluation snapshot and the derived deadline move (issue
/// #1455). This is the property the boot and per-cycle refresh rely on: a
/// console override lands on a gate that may already hold an approval the
/// operator was asked about and a stop an operator pulled, and neither may
/// be disturbed.
#[tokio::test]
async fn apply_effective_policy_moves_snapshot_and_ttl_but_not_parked_or_emergency() {
    let gate = ManifestApprovalGate::new(policy("full", None)).with_ttl_millis(1000);
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    gate.set_emergency(true);

    let stricter = Policy {
        mode: "supervised".to_string(),
        always_approve: FENCE.iter().map(|s| s.to_string()).collect(),
        auto_approve_under_usd: Some(5.0),
        approval_ttl_hours: Some(48),
    };
    gate.apply_effective_policy(stricter);

    // Parked queue and stop survive...
    assert_eq!(gate.parked_ids(), vec![id.clone()]);
    assert!(gate.is_emergency());
    // ...and the deadline moved.
    assert_eq!(gate.ttl_millis(), 48 * 60 * 60 * 1000);

    // With the stop released, evaluation reflects the new snapshot: `full`
    // waved every spend through, while the new `supervised` snapshot parks
    // one over the cap.
    gate.set_emergency(false);
    let mut over = effect("x402.spend", EffectGroup::Spend);
    over.amount_usd = Some(6.0);
    assert_eq!(decide(&gate, &over).await, PolicyDecision::RequireApproval);
}

/// The TTL-only update — what the ops handler applies immediately after a
/// policy PUT/DELETE — moves the deadline without touching the snapshot, so
/// an in-flight turn evaluating under the old tier is not disturbed.
#[tokio::test]
async fn apply_effective_ttl_moves_only_the_deadline() {
    let gate = ManifestApprovalGate::new(policy("full", None)).with_ttl_millis(1000);
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    gate.set_emergency(true);

    let new_deadline = Policy {
        mode: "readonly".to_string(),
        always_approve: Vec::new(),
        auto_approve_under_usd: None,
        approval_ttl_hours: Some(1),
    };
    gate.apply_effective_ttl(&new_deadline);

    assert_eq!(gate.ttl_millis(), 60 * 60 * 1000);
    assert_eq!(gate.parked_ids(), vec![id]);
    assert!(gate.is_emergency());

    // Snapshot untouched: with the stop released, `blog.post` (Publish, not
    // in FENCE) still `Allow`s under `full`, which a `readonly` snapshot
    // would park.
    gate.set_emergency(false);
    assert_eq!(
        decide(&gate, &effect("blog.post", EffectGroup::Publish)).await,
        PolicyDecision::Allow
    );
}
