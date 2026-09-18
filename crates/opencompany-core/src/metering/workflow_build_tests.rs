use super::*;

fn usage_with(cost: f64) -> TokenUsage {
    TokenUsage {
        input: 1_200,
        output: 400,
        cached_input: 150,
        cost_usd: cost,
    }
}

/// The attribution rule, pinned — and it is the mirror image of planning's.
/// A builder pass is charged to the card's **assignee**, carries the
/// **attempt's run id**, and reads as ordinary [`SampleKind::Inference`]
/// spend, because building the workflow is the card's In-Progress work rather
/// than the run-less, company-bucket planning pass.
#[test]
fn a_builder_sample_is_charged_to_the_assignee_with_a_run() {
    let sample = workflow_build_sample(&usage_with(0.6), "managed", "maya", "run-7", None)
        .expect("a real pass meters");
    assert_eq!(sample.agent, "maya");
    assert_eq!(sample.kind, SampleKind::Inference);
    assert_eq!(
        sample.run_id.as_deref(),
        Some("run-7"),
        "a builder pass mints an attempt row, so its spend links to that run"
    );
    assert_eq!(sample.input_tokens, 1_200);
    assert_eq!(sample.output_tokens, 400);
    assert_eq!(sample.cached_input_tokens, 150);
    assert_eq!(sample.cost_usd, 0.6);
    assert_eq!(sample.provider, "managed");
}

/// The token counter the capability-tier ceiling reads
/// ([`tokens_in`](crate::metering::capability::tokens_in)) sums
/// `Inference` samples, so a builder pass automatically counts against the
/// tier — the exact guard the module docs promise. Pinned here so a future
/// switch of the sample's kind that would silently exempt builder spend from
/// the ceiling fails a test instead.
#[test]
fn builder_spend_counts_toward_the_capability_ceiling() {
    let sample =
        workflow_build_sample(&usage_with(0.6), "managed", "maya", "run-7", None).expect("sample");
    assert_eq!(
        crate::metering::tokens_in(std::slice::from_ref(&sample)),
        1_600
    );
}

/// A provider slug is normalised the same way every other sample's is, so the
/// Usage view's provider axis does not grow a second spelling of one backend.
#[test]
fn the_provider_slug_is_normalised() {
    let sample = workflow_build_sample(&usage_with(0.1), "  MANAGED ", "maya", "run-7", None)
        .expect("sample");
    assert_eq!(sample.provider, "managed");
    let blank = workflow_build_sample(&usage_with(0.1), "", "maya", "run-7", None).expect("sample");
    assert_eq!(blank.provider, crate::metering::UNKNOWN_PROVIDER);
}

/// A pass that moved nothing writes nothing — the offline provider reports no
/// usage, and a zero row is indistinguishable from a real free call. Cost
/// alone, or tokens alone, is enough to be worth recording.
#[test]
fn a_zero_pass_writes_no_sample() {
    assert!(
        workflow_build_sample(&TokenUsage::default(), "managed", "maya", "run-7", None).is_none()
    );
    assert!(
        workflow_build_sample(
            &TokenUsage {
                cost_usd: 0.01,
                ..TokenUsage::default()
            },
            "managed",
            "maya",
            "run-7",
            None
        )
        .is_some()
    );
    assert!(
        workflow_build_sample(
            &TokenUsage {
                input: 1,
                ..TokenUsage::default()
            },
            "managed",
            "maya",
            "run-7",
            None
        )
        .is_some()
    );
}

/// Builder spend posts to the same Finances category as any other inference
/// spend, memo'd to the assignee — not to the whole-company bucket planning
/// uses. A token-bearing but zero-cost pass posts no money; the sample still
/// lands.
#[test]
fn builder_spend_posts_to_the_inference_ledger_under_the_assignee() {
    let entry = inference_ledger_entry(&usage_with(0.25), "maya").expect("a costed pass posts");
    assert_eq!(entry.kind, crate::metering::INFERENCE_SPEND_KIND);
    assert_eq!(
        entry.amount_usd, -0.25,
        "an outflow is negative (issue #1047)"
    );
    assert_eq!(entry.memo, "maya");
    assert!(inference_ledger_entry(&usage_with(0.0), "maya").is_none());
}
