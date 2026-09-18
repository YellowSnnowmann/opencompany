use super::*;

fn usage() -> TokenUsage {
    TokenUsage {
        input: 120,
        output: 4,
        cached_input: 0,
        cost_usd: 0.0002,
    }
}

/// A selection that moved tokens mints a [`SampleKind::SelectorCall`] row
/// charged to the whole-company bucket — never to the teammate it picked.
#[test]
fn a_selection_samples_under_its_own_kind_and_no_teammate() {
    let sample = selector_sample(&usage(), "openrouter", None).expect("a real spend samples");
    assert_eq!(sample.kind, SampleKind::SelectorCall);
    assert_eq!(sample.agent, UNATTRIBUTED_AGENT);
    assert_eq!(sample.run_id, None);
    assert_eq!(sample.input_tokens, 120);
}

/// The offline/mock path — zero usage — mints no row: a free fake call is
/// indistinguishable from a real free one, so no row is the honest record.
#[test]
fn zero_usage_mints_no_sample() {
    assert!(selector_sample(&TokenUsage::default(), "openrouter", None).is_none());
}
