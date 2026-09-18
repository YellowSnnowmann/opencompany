use super::*;
use crate::metering::calendar::{MILLIS_PER_DAY, days_from_civil};

fn inference_sample(input: u64, output: u64) -> UsageSample {
    UsageSample {
        at_millis: 0,
        agent: "ceo".into(),
        provider: "managed".into(),
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: 0,
        cost_usd: 0.0,
        kind: SampleKind::Inference,
        run_id: None,
        model: None,
    }
}

fn oauth_sample() -> UsageSample {
    UsageSample {
        at_millis: 0,
        agent: "ceo".into(),
        provider: "github".into(),
        input_tokens: 999,
        output_tokens: 999,
        cached_input_tokens: 0,
        cost_usd: 0.0,
        kind: SampleKind::OauthCall,
        run_id: None,
        model: None,
    }
}

// --- period boundaries --------------------------------------------------

#[test]
fn daily_period_start_snaps_to_midnight_utc() {
    let day = days_from_civil(2026, 7, 16) as u64;
    let noon = day * MILLIS_PER_DAY + 12 * 3_600_000;
    assert_eq!(
        BudgetPeriod::Daily.period_start_millis(noon),
        day * MILLIS_PER_DAY
    );
    assert_eq!(
        BudgetPeriod::Daily.period_start_millis(day * MILLIS_PER_DAY),
        day * MILLIS_PER_DAY
    );
}

#[test]
fn monthly_period_start_snaps_to_the_first() {
    let mid = (days_from_civil(2026, 7, 16) as u64) * MILLIS_PER_DAY + 12 * 3_600_000;
    let first = (days_from_civil(2026, 7, 1) as u64) * MILLIS_PER_DAY;
    assert_eq!(BudgetPeriod::Monthly.period_start_millis(mid), first);
}

#[test]
fn period_parse_round_trips_and_rejects_unknown() {
    assert_eq!(BudgetPeriod::parse("daily"), Some(BudgetPeriod::Daily));
    assert_eq!(BudgetPeriod::parse("Monthly"), Some(BudgetPeriod::Monthly));
    assert_eq!(BudgetPeriod::parse("hourly"), None);
}

// --- tokens_in ----------------------------------------------------------

#[test]
fn tokens_in_sums_inference_and_ignores_oauth() {
    let samples = vec![
        inference_sample(100, 50),
        oauth_sample(),
        inference_sample(20, 5),
    ];
    assert_eq!(tokens_in(&samples), 175);
}

#[test]
fn tokens_in_empty_is_zero() {
    assert_eq!(tokens_in(&[]), 0);
}

/// Issue #337: a planning pass spends the tenant's inference budget just as
/// a teammate's turn does, so it counts toward the tier ceiling. Without
/// this a company could keep planning after the budget that was supposed to
/// stop it had been exhausted — the tokens are spent either way, and a
/// ceiling that only some completions respect is not a ceiling.
#[test]
fn tokens_in_counts_planning_passes() {
    let planning = UsageSample {
        kind: SampleKind::PlanningCall,
        agent: crate::metering::UNATTRIBUTED_AGENT.into(),
        ..inference_sample(300, 100)
    };
    assert_eq!(tokens_in(std::slice::from_ref(&planning)), 400);
    // And it adds to a teammate's, rather than replacing or shadowing it.
    assert_eq!(
        tokens_in(&[inference_sample(100, 50), planning, oauth_sample()]),
        550
    );
}

// --- exhaustion / denial ------------------------------------------------

#[test]
fn ge_boundary_exhausts_the_tier() {
    let plan = plan_named("starter").unwrap();
    // Under budget, the *mapped* tiers (shell/code) are granted. web/subagent
    // are absent from starter's map, so they are always denied — assert on the
    // mapped tiers specifically, not on emptiness.
    let under = plan.denied_namespaces(199_999);
    assert!(!under.contains("shell") && !under.contains("code"));
    // Exactly at budget: both mapped tiers exhausted (>= boundary).
    let at = plan.denied_namespaces(200_000);
    assert!(at.contains("shell") && at.contains("code"));
    // Over budget: still denied.
    assert!(plan.denied_namespaces(200_001).contains("shell"));
}

#[test]
fn absent_namespace_is_always_denied() {
    let plan = plan_named("starter").unwrap();
    let denied = plan.denied_namespaces(0);
    assert!(denied.contains("web"), "web absent from map → denied");
    assert!(denied.contains("subagent"), "subagent absent → denied");
    assert!(!denied.contains("shell"), "shell granted under budget");
}

#[test]
fn zero_budget_namespace_is_denied_from_the_first_token() {
    let mut plan = plan_named("free").unwrap();
    plan.budgets.insert("shell".into(), 0);
    assert!(plan.denied_namespaces(0).contains("shell"));
}

#[test]
fn free_plan_denies_every_gateable_namespace() {
    let plan = plan_named("free").unwrap();
    let denied = plan.denied_namespaces(0);
    for ns in GATEABLE_NAMESPACES {
        assert!(denied.contains(ns), "free must deny {ns}");
    }
}

#[test]
fn unlimited_plan_grants_everything() {
    let plan = plan_named("unlimited").unwrap();
    assert!(plan.denied_namespaces(u64::MAX - 1).is_empty());
}

/// The real-money `media` tier (issue #109) is uncapped under `unlimited`
/// but denied by every other built-in tier — a company must opt into it via
/// an explicit `token_budgets = { media = N }`, never a wildcard.
#[test]
fn media_tier_is_unlimited_only_and_denied_elsewhere() {
    assert!(
        !plan_named("unlimited")
            .unwrap()
            .denied_namespaces(0)
            .contains("media"),
        "unlimited grants media"
    );
    for tier in ["free", "starter", "pro"] {
        assert!(
            plan_named(tier)
                .unwrap()
                .denied_namespaces(0)
                .contains("media"),
            "{tier} must deny the real-money media tier by default"
        );
    }
}

/// The per-tenant `composio` tier (issue #110) is uncapped under `unlimited`
/// but denied by every other built-in tier — a company opts into it via an
/// explicit `token_budgets = { composio = N }`, never a wildcard.
#[test]
fn composio_tier_is_unlimited_only_and_denied_elsewhere() {
    assert!(
        !plan_named("unlimited")
            .unwrap()
            .denied_namespaces(0)
            .contains("composio"),
        "unlimited grants composio"
    );
    for tier in ["free", "starter", "pro"] {
        assert!(
            plan_named(tier)
                .unwrap()
                .denied_namespaces(0)
                .contains("composio"),
            "{tier} must deny the composio tier by default"
        );
    }
}

/// The metered `search` tier (issue #238) follows media/composio: uncapped
/// under `unlimited`, denied by every other built-in tier unless the
/// manifest opts in with `token_budgets = { search = N }`. Every search is
/// a priced request, so a company should not inherit one from a tier it
/// chose for its token allowance.
#[test]
fn search_tier_is_unlimited_only_and_denied_elsewhere() {
    assert!(
        !plan_named("unlimited")
            .unwrap()
            .denied_namespaces(0)
            .contains("search"),
        "unlimited grants search"
    );
    for tier in ["free", "starter", "pro"] {
        assert!(
            plan_named(tier)
                .unwrap()
                .denied_namespaces(0)
                .contains("search"),
            "{tier} must deny the metered search tier by default"
        );
    }
}

/// A manifest can opt a non-`unlimited` plan into `composio` with an explicit
/// token budget; exhausting that budget drops exactly `composio`.
#[test]
fn explicit_composio_budget_grants_then_exhausts_only_composio() {
    let mut token_budgets = BTreeMap::new();
    token_budgets.insert("composio".to_string(), 100_000);
    let plan = CapabilityPlan::from_manifest(&Plan {
        name: Some("starter".into()),
        period: "daily".into(),
        token_budgets,
        total_tokens: None,
    })
    .unwrap();
    // Under budget: composio granted, shell/code still granted.
    let under = plan.denied_namespaces(50_000);
    assert!(!under.contains("composio"));
    assert!(!under.contains("shell"));
    // At the composio budget but under starter's 200k shell/code: only
    // composio drops.
    let at = plan.denied_namespaces(100_000);
    assert!(at.contains("composio"), "composio exhausted at its budget");
    assert!(!at.contains("shell"), "shell still under its 200k budget");
}

#[test]
fn unknown_plan_name_is_none() {
    assert!(plan_named("enterprise").is_none());
}

// --- status -------------------------------------------------------------

#[test]
fn status_reports_per_tier_rows_against_shared_spend() {
    let plan = plan_named("starter").unwrap();
    let rows = plan.status(200_000);
    assert_eq!(rows.len(), 2, "one row per configured budget");
    for row in &rows {
        assert_eq!(row.spent, 200_000);
        assert_eq!(row.budget, 200_000);
        assert_eq!(row.remaining, 0);
        assert!(row.exhausted);
    }
    // Sorted namespace order (BTreeMap): code before shell.
    assert_eq!(rows[0].namespace, "code");
    assert_eq!(rows[1].namespace, "shell");
}

#[test]
fn status_remaining_saturates_and_tracks_exhaustion() {
    let plan = plan_named("pro").unwrap();
    let rows = plan.status(600_000);
    for row in rows {
        assert_eq!(row.remaining, 400_000);
        assert!(!row.exhausted);
    }
}

// --- from_manifest ------------------------------------------------------

#[test]
fn from_manifest_none_when_unset() {
    let plan = Plan::default();
    assert!(CapabilityPlan::from_manifest(&plan).is_none());
}

#[test]
fn from_manifest_named_tier_resolves_budgets() {
    let plan = Plan {
        name: Some("starter".into()),
        period: "daily".into(),
        token_budgets: BTreeMap::new(),
        total_tokens: None,
    };
    let resolved = CapabilityPlan::from_manifest(&plan).unwrap();
    assert_eq!(resolved.period, BudgetPeriod::Daily);
    assert_eq!(resolved.budgets.get("shell"), Some(&200_000));
    assert_eq!(resolved.budgets.get("code"), Some(&200_000));
    assert!(!resolved.budgets.contains_key("web"));
}

#[test]
fn from_manifest_token_budgets_override_and_extend_named() {
    let mut token_budgets = BTreeMap::new();
    token_budgets.insert("shell".to_string(), 42);
    token_budgets.insert("web".to_string(), 7);
    let plan = Plan {
        name: Some("starter".into()),
        period: "monthly".into(),
        token_budgets,
        total_tokens: None,
    };
    let resolved = CapabilityPlan::from_manifest(&plan).unwrap();
    assert_eq!(resolved.period, BudgetPeriod::Monthly, "period field wins");
    assert_eq!(resolved.budgets.get("shell"), Some(&42), "override");
    assert_eq!(resolved.budgets.get("code"), Some(&200_000), "kept");
    assert_eq!(resolved.budgets.get("web"), Some(&7), "extended");
}

#[test]
fn from_manifest_bare_token_budgets_without_name() {
    let mut token_budgets = BTreeMap::new();
    token_budgets.insert("shell".to_string(), 500);
    let plan = Plan {
        name: None,
        period: "daily".into(),
        token_budgets,
        total_tokens: None,
    };
    let resolved = CapabilityPlan::from_manifest(&plan).unwrap();
    assert_eq!(resolved.budgets.len(), 1);
    assert_eq!(resolved.budgets.get("shell"), Some(&500));
}

// --- total budget (issue #188) ------------------------------------------

#[test]
fn total_exhausted_none_budget_is_never_exhausted() {
    let plan = plan_named("starter").unwrap();
    assert!(plan.total_budget.is_none(), "named tiers carry no total");
    assert!(!plan.total_exhausted(0));
    assert!(!plan.total_exhausted(u64::MAX));
}

#[test]
fn total_exhausted_trips_at_the_ge_boundary() {
    let mut plan = plan_named("free").unwrap();
    plan.total_budget = Some(1_000);
    assert!(!plan.total_exhausted(999), "under budget runs");
    assert!(plan.total_exhausted(1_000), ">= boundary refuses");
    assert!(plan.total_exhausted(1_001), "over budget refuses");
}

#[test]
fn total_exhausted_zero_budget_refuses_from_the_first_token() {
    let mut plan = plan_named("free").unwrap();
    plan.total_budget = Some(0);
    assert!(
        plan.total_exhausted(0),
        "a zero ceiling refuses immediately"
    );
}

#[test]
fn total_status_none_when_no_ceiling() {
    let plan = plan_named("pro").unwrap();
    assert!(plan.total_status(0).is_none());
}

#[test]
fn total_status_reports_budget_spend_remaining_and_exhaustion() {
    let mut plan = plan_named("free").unwrap();
    plan.total_budget = Some(1_000);
    let under = plan.total_status(400).unwrap();
    assert_eq!(under.budget, 1_000);
    assert_eq!(under.spent, 400);
    assert_eq!(under.remaining, 600);
    assert!(!under.exhausted);
    // At/over the ceiling: remaining saturates at zero and exhausted flips.
    let over = plan.total_status(1_500).unwrap();
    assert_eq!(over.remaining, 0);
    assert!(over.exhausted);
}

#[test]
fn from_manifest_carries_total_tokens_and_named_tier_leaves_it_none() {
    // A bare total-only plan: no name, no per-namespace budgets.
    let plan = Plan {
        name: None,
        period: "daily".into(),
        token_budgets: BTreeMap::new(),
        total_tokens: Some(5_000),
    };
    let resolved = CapabilityPlan::from_manifest(&plan).unwrap();
    assert_eq!(resolved.total_budget, Some(5_000));
    assert!(resolved.budgets.is_empty(), "no per-namespace gate");

    // A named tier with an explicit total overlay carries the ceiling too.
    let overlaid = CapabilityPlan::from_manifest(&Plan {
        name: Some("starter".into()),
        period: "daily".into(),
        token_budgets: BTreeMap::new(),
        total_tokens: Some(9_000),
    })
    .unwrap();
    assert_eq!(overlaid.total_budget, Some(9_000));
    assert_eq!(overlaid.budgets.get("shell"), Some(&200_000));

    // A named tier with no overlay leaves the total gate off.
    let plain = plan_named("starter").unwrap();
    assert!(plain.total_budget.is_none());
}
