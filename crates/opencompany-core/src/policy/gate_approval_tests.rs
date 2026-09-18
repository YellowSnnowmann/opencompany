use super::*;
use crate::ports::types::ActorKind;

/// The fence these tests run under, written out rather than taken from
/// [`DEFAULT_ALWAYS_APPROVE`](crate::company::DEFAULT_ALWAYS_APPROVE).
///
/// It used to be the default, which made every assertion below depend on a
/// constant these tests do not own — and when that constant turned out to
/// gate nothing on the *other* approval path, nothing here noticed, because
/// on this path it did match (issue #684). A test that borrows a shipped
/// default tests the default, not the mechanism; the default is empty now
/// and this list is the mechanism's own fixture.
pub(super) const FENCE: &[&str] = &["payment.send"];

pub(super) fn policy(mode: &str, cap: Option<f64>) -> Policy {
    Policy {
        mode: mode.to_string(),
        always_approve: FENCE.iter().map(|s| s.to_string()).collect(),
        auto_approve_under_usd: cap,
        approval_ttl_hours: None,
    }
}

pub(super) fn effect(kind: &str, group: EffectGroup) -> Effect {
    Effect {
        kind: kind.to_string(),
        group,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    }
}

pub(super) fn operator() -> Actor {
    Actor {
        kind: ActorKind::Operator,
        id: "owner".to_string(),
    }
}

pub(super) fn company() -> CompanyId {
    CompanyId::new("acme")
}

pub(super) async fn decide(gate: &ManifestApprovalGate, effect: &Effect) -> PolicyDecision {
    gate.evaluate(&company(), effect).await.unwrap()
}

#[test]
pub(super) fn policy_hitl_enabled_reflects_the_gate_that_reports_it() {
    let live = ManifestApprovalGate::new(policy("supervised", None));
    assert!(live.policy_hitl_enabled());

    let disabled =
        ManifestApprovalGate::new(policy("supervised", None)).with_policy_hitl_disabled();
    assert!(!disabled.policy_hitl_enabled());
}

#[tokio::test]
async fn disabled_policy_hitl_allows_legacy_parks_but_keeps_hard_denials() {
    let gate = ManifestApprovalGate::new(policy("supervised", None)).with_policy_hitl_disabled();
    assert_eq!(
        decide(&gate, &effect("payment.send", EffectGroup::Spend)).await,
        PolicyDecision::Allow
    );

    gate.set_emergency(true);
    assert_eq!(
        decide(&gate, &effect("payment.send", EffectGroup::Spend)).await,
        PolicyDecision::Deny
    );

    let readonly = ManifestApprovalGate::new(policy("readonly", None)).with_policy_hitl_disabled();
    assert_eq!(
        decide(&readonly, &effect("payment.send", EffectGroup::Spend)).await,
        PolicyDecision::Deny
    );
    assert_eq!(
        decide(&readonly, &effect("notification.post", EffectGroup::Other)).await,
        PolicyDecision::Deny
    );
    assert_eq!(
        decide(
            &readonly,
            &effect(
                crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND,
                EffectGroup::Other
            )
        )
        .await,
        PolicyDecision::Allow
    );
}

/// `mode` is validated against `POLICY_MODES` before a company loads, so an
/// unrecognized word here should be unreachable in a healthy deployment —
/// but `evaluate` itself does not re-check it. Under the HITL-*enabled*
/// dispatch (`mode_decision`), an unrecognized mode fails *closed*
/// (`RequireApproval`, see the doc comment on the `Ok(Self::mode_decision(..))`
/// line). The disabled-HITL path — the one every production company
/// actually runs, per [`RuntimeBuilder`](crate::runtime::RuntimeBuilder) —
/// has no such fence: it special-cases only `readonly` and allows
/// everything else, unrecognized words included. This pins down that real,
/// currently-shipped behavior rather than the safer one the enabled path's
/// fail-closed default might suggest it has.
#[tokio::test]
async fn disabled_policy_hitl_fails_open_on_an_unrecognized_mode() {
    let gate =
        ManifestApprovalGate::new(policy("not-a-real-tier", None)).with_policy_hitl_disabled();
    assert_eq!(
        decide(&gate, &effect("payment.send", EffectGroup::Spend)).await,
        PolicyDecision::Allow
    );
}

/// Only one of this suite's gate constructions matches how
/// [`RuntimeBuilder`](crate::runtime::RuntimeBuilder) actually builds a
/// company's gate (`.with_policy_hitl_disabled()`); this is the one test
/// that exercises *that* gate under genuine concurrent access — many
/// in-flight `evaluate` reads racing a policy write via
/// `apply_effective_policy`, exactly as concurrent operator chat turns race
/// an admin's `PUT {scope}/policy` in production. Proves the `RwLock` does
/// not deadlock or poison under the access pattern production actually
/// produces, and that once every writer has landed the same policy, every
/// reader converges on it rather than a torn mix of the old and new snapshot.
#[tokio::test]
async fn disabled_policy_hitl_evaluate_is_race_safe_under_concurrent_policy_updates() {
    let gate = std::sync::Arc::new(
        ManifestApprovalGate::new(policy("supervised", None)).with_policy_hitl_disabled(),
    );

    let mut tasks = Vec::new();
    for i in 0..50u32 {
        let gate = gate.clone();
        tasks.push(tokio::spawn(async move {
            if i % 5 == 0 {
                gate.apply_effective_policy(policy("readonly", None));
            } else {
                let _ = decide(&gate, &effect("payment.send", EffectGroup::Spend)).await;
            }
        }));
    }
    for task in tasks {
        task.await
            .expect("a concurrent read or write must not panic or poison the lock");
    }

    // Every writer converged on the same policy, so once they have all
    // landed the gate must answer from it — not a torn mix of the
    // original snapshot and the update.
    assert_eq!(
        decide(&gate, &effect("payment.send", EffectGroup::Spend)).await,
        PolicyDecision::Deny
    );
}

#[tokio::test]
async fn apply_effective_policy_updates_live_state_without_dropping_queue_or_emergency() {
    let gate = ManifestApprovalGate::new(policy("supervised", Some(5.0)));
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    gate.set_emergency(true);

    gate.apply_effective_policy(Policy {
        approval_ttl_hours: Some(2),
        ..policy("full", None)
    });

    assert_eq!(gate.parked_ids(), vec![id]);
    assert!(gate.is_emergency());
    assert_eq!(gate.ttl_millis(), 2 * 60 * 60 * 1000);
    assert_eq!(
        gate.policy(),
        Policy {
            approval_ttl_hours: Some(2),
            ..policy("full", None)
        }
    );
    assert_eq!(
        decide(&gate, &effect("misc.do", EffectGroup::Other)).await,
        PolicyDecision::Allow
    );
}
#[tokio::test]
async fn readonly_gates_everything() {
    let gate = ManifestApprovalGate::new(policy("readonly", None));
    assert_eq!(
        decide(&gate, &effect("misc.read", EffectGroup::Other)).await,
        PolicyDecision::RequireApproval
    );
}

#[tokio::test]
async fn full_allows_non_always_approve() {
    let gate = ManifestApprovalGate::new(policy("full", None));
    assert_eq!(
        decide(&gate, &effect("misc.do", EffectGroup::Other)).await,
        PolicyDecision::Allow
    );
}

#[tokio::test]
async fn always_approve_overrides_full() {
    let gate = ManifestApprovalGate::new(policy("full", None));
    // `payment.send` is in FENCE, this module's always_approve fixture.
    assert_eq!(
        decide(&gate, &effect("payment.send", EffectGroup::Spend)).await,
        PolicyDecision::RequireApproval
    );
}

#[tokio::test]
async fn supervised_spend_cap_is_strict() {
    let gate = ManifestApprovalGate::new(policy("supervised", Some(5.0)));
    let mut under = effect("x402.spend", EffectGroup::Spend);
    under.amount_usd = Some(4.99);
    assert_eq!(decide(&gate, &under).await, PolicyDecision::Allow);

    let mut at_cap = effect("x402.spend", EffectGroup::Spend);
    at_cap.amount_usd = Some(5.0);
    assert_eq!(
        decide(&gate, &at_cap).await,
        PolicyDecision::RequireApproval
    );

    // No cap configured → always parks.
    let gate_no_cap = ManifestApprovalGate::new(policy("supervised", None));
    assert_eq!(
        decide(&gate_no_cap, &under).await,
        PolicyDecision::RequireApproval
    );
}

#[tokio::test]
async fn supervised_send_distinguishes_thread() {
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    let mut established = effect("email.send", EffectGroup::Send);
    established.established_thread = true;
    assert_eq!(decide(&gate, &established).await, PolicyDecision::Allow);

    let mut new_party = effect("email.send", EffectGroup::Send);
    new_party.first_time_counterparty = true;
    assert_eq!(
        decide(&gate, &new_party).await,
        PolicyDecision::RequireApproval
    );
}

#[tokio::test]
async fn supervised_sign_publish_identity_always_park() {
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    for group in [
        EffectGroup::Sign,
        EffectGroup::Publish,
        EffectGroup::Identity,
    ] {
        assert_eq!(
            decide(&gate, &effect("some.effect", group)).await,
            PolicyDecision::RequireApproval
        );
    }
}

#[tokio::test]
async fn supervised_hire_parks_first_time_or_over_cap() {
    let gate = ManifestApprovalGate::new(policy("supervised", Some(100.0)));
    let mut first = effect("a2a.engage", EffectGroup::Hire);
    first.first_time_counterparty = true;
    assert_eq!(decide(&gate, &first).await, PolicyDecision::RequireApproval);

    let mut over = effect("a2a.engage", EffectGroup::Hire);
    over.amount_usd = Some(150.0);
    assert_eq!(decide(&gate, &over).await, PolicyDecision::RequireApproval);

    let mut cheap = effect("a2a.engage", EffectGroup::Hire);
    cheap.amount_usd = Some(10.0);
    assert_eq!(decide(&gate, &cheap).await, PolicyDecision::Allow);
}

/// Issue #2037: the two money gates read the **same** two inputs — an
/// amount that may be unknown, and a cap that may not be configured — so
/// they must not disagree about what those shapes mean.
///
/// `Spend` has always failed closed on the omitting shapes and says so in a
/// comment. `Hire` computed its `over_cap` as `(Some, Some) if amount >=
/// cap`, which is `false` whenever *either* input is absent — so an unknown
/// amount and an unconfigured cap both read as "under the cap", and a paid
/// engagement of an established counterparty was waved straight through.
///
/// Walked as one table across both groups, so neither arm can be relaxed on
/// its own again. Absence of information is not evidence of a small number.
#[tokio::test]
async fn the_money_gates_agree_on_every_cap_shape() {
    let shapes: &[(&str, Option<f64>, Option<f64>, PolicyDecision)] = &[
        (
            "under a configured cap",
            Some(10.0),
            Some(100.0),
            PolicyDecision::Allow,
        ),
        (
            "exactly at a configured cap",
            Some(100.0),
            Some(100.0),
            PolicyDecision::RequireApproval,
        ),
        (
            "over a configured cap",
            Some(250.0),
            Some(100.0),
            PolicyDecision::RequireApproval,
        ),
        (
            "an unknown amount under a configured cap",
            None,
            Some(100.0),
            PolicyDecision::RequireApproval,
        ),
        (
            "a known amount with no cap configured",
            Some(10.0),
            None,
            PolicyDecision::RequireApproval,
        ),
        (
            "an unknown amount with no cap configured",
            None,
            None,
            PolicyDecision::RequireApproval,
        ),
    ];

    let mut checked = 0;
    for (group, kind) in [
        (EffectGroup::Spend, "x402.spend"),
        (EffectGroup::Hire, "a2a.engage"),
    ] {
        for (label, amount, cap, expected) in shapes {
            let gate = ManifestApprovalGate::new(policy("supervised", *cap));
            // An established counterparty throughout: the cap is the only
            // thing under test here, and first contact is asserted
            // separately below.
            let mut eff = effect(kind, group);
            eff.amount_usd = *amount;
            assert_eq!(decide(&gate, &eff).await, *expected, "{group:?}: {label}");
            checked += 1;
        }
    }
    assert_eq!(checked, 2 * shapes.len(), "the walk skipped a shape");
}

/// First contact stays an independent reason to park, orthogonal to the cap
/// (issue #2037).
///
/// An engagement can be small, known, and comfortably under a configured
/// cap and still be the first time this company has ever paid this
/// counterparty. Folding the two reasons into one cap check would lose it.
#[tokio::test]
async fn supervised_hire_parks_first_contact_even_under_the_cap() {
    let gate = ManifestApprovalGate::new(policy("supervised", Some(100.0)));
    let mut first = effect("a2a.engage", EffectGroup::Hire);
    first.amount_usd = Some(10.0);
    first.first_time_counterparty = true;
    assert_eq!(decide(&gate, &first).await, PolicyDecision::RequireApproval);
}

// -----------------------------------------------------------------------
// The tier ladder (issue #1454)
// -----------------------------------------------------------------------

/// How permissive a decision is, so the ladder can be compared rather than
/// spot-asserted.
///
/// The order is the only thing here that is a judgement: a denied effect
/// never happens, a parked one happens if a human says so, an allowed one
/// happens. Nothing in between.
pub(super) fn permissiveness(decision: PolicyDecision) -> u8 {
    match decision {
        PolicyDecision::Deny => 0,
        PolicyDecision::RequireApproval => 1,
        PolicyDecision::Allow => 2,
    }
}

/// One effect per branch the checkpoint taxonomy actually takes, labelled in
/// the operator's terms.
///
/// Every kind is deliberately outside [`FENCE`], because `always_approve`
/// is checked **above** the tier dispatch and wins over every tier — an
/// entry in it would flatten the ladder to `RequireApproval` everywhere and
/// make the monotonicity walk pass vacuously.
///
/// Amounts are chosen against a $100 cap: `10.0` under it, `100.0` exactly
/// at it (the strict-`<` boundary), `250.0` over it, and `None` for the
/// unknown-amount branch.
pub(super) fn ladder_matrix() -> Vec<(&'static str, Effect)> {
    let mut cases: Vec<(&'static str, Effect)> = Vec::new();

    cases.push((
        "a consequence-free effect",
        effect("echo.noop", EffectGroup::Other),
    ));

    for (label, amount) in [
        ("a spend under the cap", Some(10.0)),
        ("a spend exactly at the cap", Some(100.0)),
        ("a spend over the cap", Some(250.0)),
        ("a spend of unknown amount", None),
    ] {
        let mut eff = effect("ladder.spend", EffectGroup::Spend);
        eff.amount_usd = amount;
        cases.push((label, eff));
    }

    for (label, established, first_time) in [
        ("a message on an established thread", true, false),
        ("a message to a first-time counterparty", false, true),
        ("a first message on an established thread", true, true),
        ("a message with no thread context", false, false),
    ] {
        let mut eff = effect("ladder.deliver", EffectGroup::Send);
        eff.established_thread = established;
        eff.first_time_counterparty = first_time;
        cases.push((label, eff));
    }

    for (label, group) in [
        ("a signature", EffectGroup::Sign),
        ("a publish", EffectGroup::Publish),
        ("an identity change", EffectGroup::Identity),
    ] {
        cases.push((label, effect("ladder.act", group)));
    }

    for (label, amount, first_time) in [
        (
            "an engagement of a known counterparty under the cap",
            Some(10.0),
            false,
        ),
        (
            "an engagement of a first-time counterparty",
            Some(10.0),
            true,
        ),
        ("an engagement exactly at the cap", Some(100.0), false),
        ("an engagement over the cap", Some(250.0), false),
        ("an engagement of unknown value", None, false),
    ] {
        let mut eff = effect("ladder.engage", EffectGroup::Hire);
        eff.amount_usd = amount;
        eff.first_time_counterparty = first_time;
        cases.push((label, eff));
    }

    cases
}

/// **The ladder itself, not one arm of it.**
///
/// [`POLICY_MODES`](crate::company::POLICY_MODES) is ordered by increasing
/// autonomy and the console renders it in that order under the promise that
/// moving up interrupts you less. That promise is a property of the gate,
/// and it is the property nothing checked: every existing test named a
/// single mode, so `auto` could park strictly more than `supervised` for two
/// releases with a green suite (issue #1454).
///
/// Walked over `POLICY_MODES` rather than over a hard-coded list, so a fifth
/// tier is covered the day it is added instead of the day someone remembers
/// this file. Run under both a configured cap and no cap at all, because the
/// no-cap branch of `Spend` and `Hire` is a different arm.
#[tokio::test]
async fn the_tier_ladder_is_monotonic() {
    let mut checked = 0;
    for cap in [None, Some(100.0)] {
        for (label, eff) in ladder_matrix() {
            let mut previous: Option<(&str, u8)> = None;
            for mode in crate::company::POLICY_MODES {
                let gate = ManifestApprovalGate::new(policy(mode, cap));
                let rank = permissiveness(decide(&gate, &eff).await);
                if let Some((lower, lower_rank)) = previous {
                    assert!(
                        rank >= lower_rank,
                        "{label} (cap {cap:?}): `{mode}` is stricter than `{lower}`, \
                         which sits below it on the autonomy ladder — an operator \
                         moving up a tier to be interrupted less would be \
                         interrupted more"
                    );
                }
                previous = Some((mode, rank));
                checked += 1;
            }
        }
    }
    assert_eq!(
        checked,
        2 * ladder_matrix().len() * crate::company::POLICY_MODES.len(),
        "the walk skipped a tier or a case"
    );
}

/// Every word in `POLICY_MODES` reaches a **named** arm.
///
/// The failure this exists for is invisible to any decision-level assertion:
/// a tier that fell into the fail-safe catch-all and a tier that genuinely
/// decided to park return the same `PolicyDecision`. `auto` sat in that
/// catch-all — it is in `POLICY_MODES`, it is `PROVISIONED_POLICY_MODE`, and
/// the console offers it, so it was never an unknown mode; it just had no
/// arm. Asking [`mode_decision`](ManifestApprovalGate::mode_decision) for
/// `Some` is the only way to tell those apart.
#[tokio::test]
async fn every_policy_mode_has_a_named_arm() {
    let probe = effect("misc.do", EffectGroup::Other);

    let mut checked = 0;
    for mode in crate::company::POLICY_MODES {
        // A fresh snapshot whose mode is the probed word: `mode_decision`
        // takes the `Policy` explicitly, so the fail-safe arm stays
        // reachable without building a gate for every word.
        assert!(
            ManifestApprovalGate::mode_decision(&policy(mode, None), mode, &probe).is_some(),
            "`{mode}` is a selectable tier but falls into the fail-safe \
             catch-all, so it silently behaves like `readonly`"
        );
        checked += 1;
    }
    assert_eq!(
        checked,
        crate::company::POLICY_MODES.len(),
        "the walk skipped a tier"
    );

    // The catch-all still exists, and still fails safe, for a word that is
    // not a tier at all.
    assert!(
        ManifestApprovalGate::mode_decision(&policy("moderately", None), "moderately", &probe)
            .is_none()
    );
    let unknown = ManifestApprovalGate::new(policy("moderately", None));
    assert_eq!(
        decide(&unknown, &probe).await,
        PolicyDecision::RequireApproval
    );
}

/// The reported bug: a company on `auto` parked `echo.noop`, an internal
/// no-op that neither leaves the company nor spends money, while the tier
/// below it did not.
#[tokio::test]
async fn auto_allows_a_consequence_free_effect() {
    let gate = ManifestApprovalGate::new(policy("auto", None));
    assert_eq!(
        decide(&gate, &effect("echo.noop", EffectGroup::Other)).await,
        PolicyDecision::Allow
    );
}

/// `auto` still stops at the line it advertises: anything that leaves the
/// company or spends money.
///
/// Asserted separately from the monotonicity walk on purpose — that walk
/// would stay green if `auto` were widened all the way to `full`, since
/// allowing more is monotonic. This is the other fence.
#[tokio::test]
async fn auto_still_parks_what_leaves_the_company() {
    let gate = ManifestApprovalGate::new(policy("auto", Some(100.0)));

    for group in [
        EffectGroup::Sign,
        EffectGroup::Publish,
        EffectGroup::Identity,
    ] {
        assert_eq!(
            decide(&gate, &effect("ladder.act", group)).await,
            PolicyDecision::RequireApproval,
            "{group:?} is irreversible and parks at every tier below `full`"
        );
    }

    let mut over_cap = effect("ladder.spend", EffectGroup::Spend);
    over_cap.amount_usd = Some(250.0);
    assert_eq!(
        decide(&gate, &over_cap).await,
        PolicyDecision::RequireApproval
    );

    let mut cold = effect("ladder.deliver", EffectGroup::Send);
    cold.first_time_counterparty = true;
    assert_eq!(decide(&gate, &cold).await, PolicyDecision::RequireApproval);
}

/// `auto` and `supervised` decide every native effect identically, and that
/// is the *whole* of `auto`'s native behaviour rather than an accident of
/// this fixture.
///
/// The two tiers genuinely differ — but on the tool path, where `auto` waves
/// through the `Standing::Grantable` sandbox writes `supervised` parks. The
/// native taxonomy has no such bucket: everything it parks leaves the
/// company or spends money, which is exactly `auto`'s stated stopping line.
/// Pinned so a divergence has to be written down rather than drifted into,
/// in either direction.
#[tokio::test]
async fn auto_matches_supervised_on_every_native_effect() {
    for cap in [None, Some(100.0)] {
        let supervised = ManifestApprovalGate::new(policy("supervised", cap));
        let auto = ManifestApprovalGate::new(policy("auto", cap));
        for (label, eff) in ladder_matrix() {
            assert_eq!(
                decide(&auto, &eff).await,
                decide(&supervised, &eff).await,
                "{label} (cap {cap:?}) is decided differently by `auto` and \
                 `supervised`; if that is deliberate, say so on \
                 `evaluate_auto` and update this test"
            );
        }
    }
}

/// `always_approve` is checked above the tier dispatch, so it wins over
/// `auto` exactly as it wins over `full`.
#[tokio::test]
async fn always_approve_overrides_auto() {
    let gate = ManifestApprovalGate::new(policy("auto", Some(100.0)));
    let mut small = effect("payment.send", EffectGroup::Spend);
    small.amount_usd = Some(1.0);
    assert_eq!(decide(&gate, &small).await, PolicyDecision::RequireApproval);
}

#[tokio::test]
async fn park_then_approve_returns_effect() {
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    let eff = effect("filing.submit", EffectGroup::Sign);
    let id = gate.park(&company(), eff.clone()).await.unwrap();
    assert_eq!(gate.parked_ids().len(), 1);

    let resolved = gate
        .resolve(&id, Verdict::Approve, operator())
        .await
        .unwrap();
    assert_eq!(resolved, Some(eff));
    assert!(gate.parked_ids().is_empty());
}

#[tokio::test]
async fn park_then_deny_returns_none() {
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    let resolved = gate.resolve(&id, Verdict::Deny, operator()).await.unwrap();
    assert_eq!(resolved, None);
}

#[tokio::test]
async fn expired_approval_resolves_to_deny() {
    let gate = ManifestApprovalGate::new(policy("supervised", None)).with_ttl_millis(1000);
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    // Resolve far in the future: past the TTL → deny even for Approve.
    let future = now_millis() + 10_000;
    let resolved = gate.resolve_at(&id, Verdict::Approve, operator(), future);
    assert_eq!(resolved, None);
}

#[tokio::test]
async fn resolve_amended_returns_amended_effect() {
    let gate = ManifestApprovalGate::new(policy("supervised", None));
    let original = effect("filing.submit", EffectGroup::Sign);
    let id = gate.park(&company(), original.clone()).await.unwrap();

    // The operator peeks the original, edits its payload, and approves it.
    let parked = gate.parked_effect(&id).expect("parked effect readable");
    assert_eq!(parked, original);
    let mut amended = parked;
    amended.payload = serde_json::json!({ "edited": true });

    let resolved = gate.resolve_amended(&id, amended.clone(), operator(), now_millis());
    assert_eq!(resolved, Some(amended));
    // Resolving drains the queue.
    assert!(gate.parked_ids().is_empty());
}

#[tokio::test]
async fn resolve_amended_expired_denies() {
    let gate = ManifestApprovalGate::new(policy("supervised", None)).with_ttl_millis(1000);
    let id = gate
        .park(&company(), effect("filing.submit", EffectGroup::Sign))
        .await
        .unwrap();
    let mut amended = effect("filing.submit", EffectGroup::Sign);
    amended.payload = serde_json::json!({ "edited": true });

    // Past the TTL: the amend resolves to deny even though a payload was
    // supplied — default-deny-on-silence wins.
    let future = now_millis() + 10_000;
    let resolved = gate.resolve_amended(&id, amended, operator(), future);
    assert_eq!(resolved, None);
    assert!(gate.parked_ids().is_empty());
}
