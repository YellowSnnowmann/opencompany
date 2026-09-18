use super::team_agent_test_support::*;
use crate::ports::types::CompanyId;

/// Drafting is a completion the tenant pays for, and `tokens_in` counts it
/// toward the plan ceiling — so a route that never *checks* that ceiling is
/// one the copilot only ever contributes to. It is operator-driven and
/// repeatable by the same click, which is the leak: every other dispatch is
/// refused past the cap and this one would keep spending.
#[tokio::test]
async fn a_company_past_its_token_ceiling_does_not_draft() {
    let at = CompanyId::new("acme-at-ceiling");
    assert!(
        super::reserve_draft_budget(&at, &FixedMeter(1_000), &plan_with(Some(1_000)), 400)
            .await
            .is_none(),
        "spend at the ceiling refuses, matching the harness's >= boundary"
    );
    let under = CompanyId::new("acme-under-ceiling");
    assert!(
        super::reserve_draft_budget(&under, &FixedMeter(999), &plan_with(Some(1_000)), 400)
            .await
            .is_some(),
        "under the ceiling still drafts"
    );
}

/// The reason the check hands back a promise instead of a boolean.
///
/// The meter can only report work that has FINISHED. The mandate copilot
/// and the persona copilot are separately openable, so two drafts a click
/// apart both read the same pre-call total, both find room, and both spend
/// — landing a tenant past a ceiling that refused everything else. The
/// first draft's promise is what the second one has to see.
#[tokio::test]
async fn two_drafts_at_once_cannot_both_spend_the_last_of_the_budget() {
    let company = CompanyId::new("acme-concurrent");
    let plan = plan_with(Some(1_000));
    // 900 spent, 100 left, and each draft may produce up to 400. The first
    // fits; the second must not, even though the meter still says 900
    // because the first has not finished.
    let first = super::reserve_draft_budget(&company, &FixedMeter(900), &plan, 400)
        .await
        .expect("the ceiling is not reached yet")
        .expect("a ceiling is configured, so a promise is held");

    assert!(
        super::reserve_draft_budget(&company, &FixedMeter(900), &plan, 400)
            .await
            .is_none(),
        "the second draft sees the first one's promise, not just the meter"
    );

    // …and the budget comes back when the first draft finishes, on every
    // path, because the promise is released by `Drop` rather than by hand.
    drop(first);
    assert!(
        super::reserve_draft_budget(&company, &FixedMeter(900), &plan, 400)
            .await
            .is_some(),
        "a finished draft releases what it promised"
    );
}

/// No ceiling configured is the common case, and it must not put a usage
/// query in front of every draft — nor refuse one.
#[tokio::test]
async fn a_company_with_no_ceiling_is_never_refused_for_budget() {
    let company = CompanyId::new("acme");
    assert!(
        super::reserve_draft_budget(&company, &FixedMeter(u64::MAX), &plan_with(None), 400)
            .await
            .is_some()
    );
    assert!(
        super::reserve_draft_budget(
            &company,
            &FixedMeter(u64::MAX),
            &crate::company::Plan::default(),
            400
        )
        .await
        .is_some(),
        "a company with no [plan] section at all has no ceiling to reach"
    );
}

/// A meter that cannot be read is not a company over its budget.
#[tokio::test]
async fn an_unreadable_meter_lets_the_draft_through() {
    let company = CompanyId::new("acme");
    assert!(
        super::reserve_draft_budget(&company, &FailingMeter, &plan_with(Some(1)), 400)
            .await
            .is_some(),
        "an unreadable meter warns and lets the draft through"
    );
}
