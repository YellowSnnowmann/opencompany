use super::*;

fn usage_with(cost: f64) -> TokenUsage {
    TokenUsage {
        input: 900,
        output: 300,
        cached_input: 100,
        cost_usd: cost,
    }
}

/// The attribution rule, pinned. A planning sample is charged to the
/// company bucket with no run — never to a teammate — because planning is
/// frequently what chooses the teammate, and because a teammate at its
/// daily cap must not be unable to have work planned for it.
#[test]
fn a_planning_sample_is_charged_to_the_company_with_no_run() {
    let sample = planning_sample(&usage_with(0.4), "managed", None).expect("a real pass meters");
    assert_eq!(sample.agent, UNATTRIBUTED_AGENT);
    assert_eq!(sample.agent, "company");
    assert_eq!(sample.kind, SampleKind::PlanningCall);
    assert!(
        sample.run_id.is_none(),
        "a planning pass mints no attempt row, so it has no run to point at"
    );
    assert_eq!(sample.input_tokens, 900);
    assert_eq!(sample.output_tokens, 300);
    assert_eq!(sample.cached_input_tokens, 100);
    assert_eq!(sample.cost_usd, 0.4);
    assert_eq!(sample.provider, "managed");
}

/// A provider slug is normalised the same way every other sample's is, so
/// the Usage view's provider axis does not grow a second spelling of one
/// backend.
#[test]
fn the_provider_slug_is_normalised() {
    let sample = planning_sample(&usage_with(0.1), "  MANAGED ", None).expect("sample");
    assert_eq!(sample.provider, "managed");
    let blank = planning_sample(&usage_with(0.1), "", None).expect("sample");
    assert_eq!(blank.provider, crate::metering::UNKNOWN_PROVIDER);
}

/// A pass that moved nothing writes nothing. The offline provider reports
/// no usage, and a zero row in the Usage view is indistinguishable from a
/// real free call.
#[test]
fn a_zero_pass_writes_no_sample() {
    assert!(planning_sample(&TokenUsage::default(), "managed", None).is_none());
    // Cost alone is enough to be worth recording, and so are tokens alone.
    assert!(
        planning_sample(
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
        planning_sample(
            &TokenUsage {
                input: 1,
                ..TokenUsage::default()
            },
            "managed",
            None
        )
        .is_some()
    );
}

/// Planning spend posts to the same Finances category as any other
/// inference spend, memo'd to the company. A separate category would split
/// one line item for a distinction that only the usage breakdown cares
/// about.
#[test]
fn planning_spend_posts_to_the_inference_ledger_under_the_company() {
    let entry =
        inference_ledger_entry(&usage_with(0.25), UNATTRIBUTED_AGENT).expect("a costed pass posts");
    assert_eq!(entry.kind, crate::metering::INFERENCE_SPEND_KIND);
    assert_eq!(
        entry.amount_usd, -0.25,
        "an outflow is negative (issue #1047)"
    );
    assert_eq!(entry.memo, "company");
    // A token-bearing but zero-cost pass (the managed passthrough) posts no
    // money — the sample still lands, the ledger stays honest.
    assert!(inference_ledger_entry(&usage_with(0.0), UNATTRIBUTED_AGENT).is_none());
}
