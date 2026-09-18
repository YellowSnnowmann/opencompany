use super::*;
use crate::metering::calendar::{MILLIS_PER_DAY, days_from_civil};
use crate::ports::{SampleKind, UsageSample};

fn sample(agent: &str, cost: f64, kind: SampleKind) -> UsageSample {
    UsageSample {
        at_millis: 0,
        agent: agent.into(),
        provider: "managed".into(),
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        cost_usd: cost,
        kind,
        run_id: None,
        model: None,
    }
}

/// The sum is per-agent: another teammate's spend never counts against this
/// one's cap. Per-agent caps that leaked into each other would make a busy
/// desk silently starve a quiet one.
#[test]
fn spend_is_attributed_to_one_agent() {
    let samples = vec![
        sample("analyst", 1.50, SampleKind::Inference),
        sample("writer", 9.00, SampleKind::Inference),
        sample("analyst", 0.25, SampleKind::Inference),
    ];
    assert!((usd_spent_by_agent(&samples, "analyst") - 1.75).abs() < f64::EPSILON);
    assert!((usd_spent_by_agent(&samples, "writer") - 9.00).abs() < f64::EPSILON);
}

/// Every metered kind counts. A cap that only saw inference would let a
/// teammate spend its whole day on searches and report `$0.00 spent`.
#[test]
fn spend_sums_across_every_sample_kind() {
    let samples = vec![
        sample("analyst", 2.00, SampleKind::Inference),
        sample("analyst", 0.50, SampleKind::SearchCall),
        // An OAuth call is zero-cost by definition — included, contributes
        // nothing, and must not be *excluded* on the assumption it always
        // will be.
        sample("analyst", 0.00, SampleKind::OauthCall),
    ];
    assert!((usd_spent_by_agent(&samples, "analyst") - 2.50).abs() < f64::EPSILON);
}

/// No samples, or none for this agent, is zero — never an error.
///
/// And specifically **positive** zero. `assert_eq!(x, 0.0)` passes for
/// `-0.0` too, so the sign is checked through `is_sign_positive`: the whole
/// bug this pins is invisible to an equality assertion and only shows up
/// once the value is serialised and formatted by the console.
#[test]
fn an_empty_window_is_positive_zero() {
    for spent in [
        usd_spent_by_agent(&[], "analyst"),
        usd_spent_by_agent(&[sample("writer", 4.0, SampleKind::Inference)], "analyst"),
    ] {
        assert_eq!(spent, 0.0);
        assert!(
            spent.is_sign_positive(),
            "an unspent budget must serialise as 0.0, not -0.0 — the console \
             renders the latter as \"$-0.00 spent today\""
        );
    }
}

/// The load-bearing pin: this cap's day boundary IS
/// [`BudgetPeriod::Daily`]'s. If these two ever diverge, a company's
/// capability budget and its per-agent spend caps reset at different
/// instants and no operator can reason about either.
#[test]
fn the_day_boundary_is_the_capability_plans_day_boundary() {
    let noon = (days_from_civil(2026, 8, 4) as u64) * MILLIS_PER_DAY + 12 * 3_600_000;
    assert_eq!(
        utc_day_start_millis(noon),
        BudgetPeriod::Daily.period_start_millis(noon),
    );
    // ...and it is midnight UTC of that same day.
    assert_eq!(
        utc_day_start_millis(noon),
        (days_from_civil(2026, 8, 4) as u64) * MILLIS_PER_DAY
    );
    // Midnight maps to itself.
    let midnight = utc_day_start_millis(noon);
    assert_eq!(utc_day_start_millis(midnight), midnight);
}

/// A negative `cost_usd` must not fold into the total: one such sample would
/// lower a teammate's measured spend below what they actually spent, and a
/// cap judged against it lets them keep spending.
#[test]
fn a_negative_cost_sample_does_not_lower_measured_spend() {
    let samples = vec![
        sample("analyst", 5.0, SampleKind::Inference),
        sample("analyst", -3.0, SampleKind::Inference),
    ];
    assert!(
        (usd_spent_by_agent(&samples, "analyst") - 5.0).abs() < f64::EPSILON,
        "a negative cost_usd sample must not silently lower measured spend, got {}",
        usd_spent_by_agent(&samples, "analyst")
    );
}

/// The non-finite case: `NaN` propagates through `+` (`x + NaN == NaN`), so a
/// single malformed sample would erase the whole daily total and leave every
/// cap comparison false.
#[test]
fn a_non_finite_cost_sample_does_not_poison_the_total() {
    let samples = vec![
        sample("analyst", 5.0, SampleKind::Inference),
        sample("analyst", f64::NAN, SampleKind::Inference),
        sample("analyst", 2.0, SampleKind::Inference),
    ];
    let spent = usd_spent_by_agent(&samples, "analyst");
    assert!(
        spent.is_finite(),
        "a NaN sample must not poison the running total, got {spent}"
    );
}

/// The console row: remaining floors at zero and `exhausted` trips on `>=`,
/// matching the boundary the harness gate and the policy arm use.
#[test]
fn status_floors_remaining_and_trips_on_reaching_the_cap() {
    let under = AgentBudgetStatus::new("analyst", 5.0, 1.25);
    assert!((under.remaining_usd - 3.75).abs() < f64::EPSILON);
    assert!(!under.exhausted);

    let exactly_at = AgentBudgetStatus::new("analyst", 5.0, 5.0);
    assert_eq!(exactly_at.remaining_usd, 0.0);
    assert!(exactly_at.exhausted, "the boundary is `>=`, not `>`");

    let over = AgentBudgetStatus::new("analyst", 5.0, 7.5);
    assert_eq!(over.remaining_usd, 0.0, "remaining never goes negative");
    assert!(over.exhausted);
}
