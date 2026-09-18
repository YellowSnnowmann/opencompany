use super::*;

fn usage() -> TokenUsage {
    TokenUsage {
        input: 90,
        output: 7,
        cached_input: 0,
        cost_usd: 0.0001,
    }
}

/// A titling pass that moved tokens mints a [`SampleKind::TitleCall`] row
/// charged to the whole-company bucket — never to the card's assignee.
#[test]
fn a_titling_pass_samples_under_its_own_kind_and_no_teammate() {
    let sample = title_sample(&usage(), "openrouter", None).expect("a real spend samples");
    assert_eq!(sample.kind, SampleKind::TitleCall);
    assert_eq!(sample.agent, UNATTRIBUTED_AGENT);
    assert_eq!(sample.run_id, None);
    assert_eq!(sample.input_tokens, 90);
}

/// The offline/mock path — zero usage — mints no row: a free fake call is
/// indistinguishable from a real free one, so no row is the honest record.
#[test]
fn zero_usage_mints_no_sample() {
    assert!(title_sample(&TokenUsage::default(), "openrouter", None).is_none());
}
