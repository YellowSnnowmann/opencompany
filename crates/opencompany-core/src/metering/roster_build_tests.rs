use super::*;

fn usage_with(cost: f64) -> TokenUsage {
    TokenUsage {
        input: 700,
        output: 250,
        cached_input: 50,
        cost_usd: cost,
    }
}

/// The attribution rule, pinned. Setup runs before the roster exists, so
/// there is no teammate it could be charged to and no attempt row to point
/// at — and a per-agent cap must never be able to fail a company's first
/// action.
#[test]
fn a_setup_sample_is_charged_to_the_company_with_no_run() {
    let sample =
        roster_build_sample(&usage_with(0.2), "managed", None).expect("a real pass meters");
    assert_eq!(sample.agent, UNATTRIBUTED_AGENT);
    assert_eq!(sample.agent, "company");
    assert_eq!(sample.kind, SampleKind::SetupCall);
    assert!(
        sample.run_id.is_none(),
        "a setup pass mints no attempt row, so it has no run to point at"
    );
    assert_eq!(sample.input_tokens, 700);
    assert_eq!(sample.output_tokens, 250);
    assert_eq!(sample.cached_input_tokens, 50);
    assert_eq!(sample.cost_usd, 0.2);
}

/// Its own kind, distinct from planning's. Folding the two together would
/// make "what does onboarding a company cost?" unanswerable, which is the
/// question this feature is being measured on.
#[test]
fn a_setup_sample_is_not_a_planning_sample() {
    let setup = roster_build_sample(&usage_with(0.2), "managed", None).expect("sample");
    let planning =
        super::super::planning::planning_sample(&usage_with(0.2), "managed", None).expect("sample");
    assert_ne!(setup.kind, planning.kind);
    // But both belong to the company rather than to a teammate.
    assert_eq!(setup.agent, planning.agent);
}

/// The offline path, and the common one: no credential means no call, so
/// there is nothing to meter. A zero row would claim a company spent
/// something on setup when it never made the request.
#[test]
fn a_pass_that_never_called_writes_no_sample() {
    assert!(roster_build_sample(&TokenUsage::default(), "managed", None).is_none());
}

/// Cost alone and tokens alone are each enough to be worth recording — a
/// provider that reports one but not the other must not fall through the
/// zero guard.
#[test]
fn either_tokens_or_cost_is_enough_to_record() {
    assert!(
        roster_build_sample(
            &TokenUsage {
                cost_usd: 0.01,
                ..TokenUsage::default()
            },
            "managed",
            None
        )
        .is_some()
    );
    assert!(
        roster_build_sample(
            &TokenUsage {
                input: 10,
                ..TokenUsage::default()
            },
            "managed",
            None
        )
        .is_some()
    );
}

/// Normalised like every other sample's, so the Usage view's provider axis
/// does not grow a second spelling of one backend.
#[test]
fn the_provider_slug_is_normalised() {
    let sample = roster_build_sample(&usage_with(0.1), "  MANAGED ", None).expect("sample");
    assert_eq!(sample.provider, "managed");
    let blank = roster_build_sample(&usage_with(0.1), "", None).expect("sample");
    assert_eq!(blank.provider, crate::metering::UNKNOWN_PROVIDER);
}

/// Setup tokens count toward the capability-tier ceiling. Excluding them
/// would leave a tenant able to run setup on an exhausted plan.
#[test]
fn setup_tokens_count_toward_the_tier_ceiling() {
    let sample = roster_build_sample(&usage_with(0.2), "managed", None).expect("sample");
    assert_eq!(
        crate::metering::capability::tokens_in(std::slice::from_ref(&sample)),
        950
    );
}
