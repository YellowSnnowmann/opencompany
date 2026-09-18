use super::*;

fn usage_with(cost: f64) -> TokenUsage {
    TokenUsage {
        input: 400,
        output: 120,
        cached_input: 20,
        cost_usd: cost,
    }
}

/// The company pays and no teammate does — the rule this module exists to
/// hold, asserted rather than described.
#[test]
fn a_draft_is_charged_to_the_company_and_to_no_teammate() {
    let sample =
        profile_draft_sample(&usage_with(0.02), "managed", None).expect("a real draft meters");
    assert_eq!(sample.agent, UNATTRIBUTED_AGENT);
    assert_eq!(sample.kind, SampleKind::AuthoringCall);
    assert_eq!(sample.run_id, None, "a draft mints no run");
    assert_eq!(sample.input_tokens, 400);
    assert_eq!(sample.output_tokens, 120);
}

/// Its own kind, so "what does onboarding cost?" stays answerable — the
/// question [`SampleKind::SetupCall`] was split out to keep answerable.
#[test]
fn a_draft_is_not_filed_as_a_setup_pass() {
    let sample = profile_draft_sample(&usage_with(0.02), "managed", None).expect("sample");
    assert_ne!(sample.kind, SampleKind::SetupCall);
    assert_ne!(sample.kind, SampleKind::Inference);
}

/// A pass that never reached a provider cost nothing, and a zero row would
/// report drafting spend for a company that has none.
#[test]
fn a_pass_that_spent_nothing_writes_no_row() {
    assert!(profile_draft_sample(&TokenUsage::default(), "managed", None).is_none());
}

/// Drafting counts toward the tier ceiling: an excluded kind would let an
/// operator keep pressing Draft past the budget that stopped everything
/// else.
#[test]
fn a_draft_counts_toward_the_tier_ceiling() {
    let sample = profile_draft_sample(&usage_with(0.02), "managed", None).expect("sample");
    assert_eq!(super::super::capability::tokens_in(&[sample]), 520);
}
