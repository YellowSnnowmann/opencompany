use super::*;

/// The shape of a row every backend already holds: `sqlite` stores a sample
/// as `sample_json`, `fs_ops` as a line of `usage.jsonl`, `mongodb` as a
/// BSON document. None of them is versioned, so a sample written before
/// [`UsageSample::model`] existed is read back by *this* build, and a schema
/// change that could not read it would be a worse bug than the blind spot
/// it was fixing.
const LEGACY_ROW: &str = r#"{
    "atMillis": 1750000000000,
    "agent": "ceo",
    "provider": "subscription",
    "inputTokens": 1200,
    "outputTokens": 340,
    "cachedInputTokens": 0,
    "costUsd": 0.42,
    "kind": "inference"
}"#;

#[test]
fn a_sample_written_before_the_model_field_existed_still_loads() {
    let sample: UsageSample = serde_json::from_str(LEGACY_ROW).expect("a pre-#1749 row loads");
    assert_eq!(sample.agent, "ceo");
    assert_eq!(sample.cost_usd, 0.42);
    assert_eq!(
        sample.model, None,
        "an unrecorded model is absent, not guessed at"
    );
    assert_eq!(sample.run_id, None);
}

/// The other half of the same contract: this build's rows stay readable by
/// a build that predates the field, because an absent model is omitted
/// entirely rather than written as an explicit `null` a stricter reader
/// would reject.
#[test]
fn a_sample_with_no_model_writes_no_model_key() {
    let sample = UsageSample {
        at_millis: 1,
        agent: "ceo".into(),
        provider: "subscription".into(),
        input_tokens: 1,
        output_tokens: 1,
        cached_input_tokens: 0,
        cost_usd: 0.0,
        kind: SampleKind::OauthCall,
        run_id: None,
        model: None,
    };
    let json = serde_json::to_string(&sample).expect("serialize");
    assert!(!json.contains("model"), "{json}");
}

#[test]
fn a_recorded_model_survives_a_round_trip() {
    let sample = UsageSample {
        at_millis: 1,
        agent: "ceo".into(),
        provider: "byok".into(),
        input_tokens: 10,
        output_tokens: 2,
        cached_input_tokens: 0,
        cost_usd: 0.01,
        kind: SampleKind::Inference,
        run_id: None,
        model: Some(ModelSlug::classify("anthropic/claude-haiku-4")),
    };
    let json = serde_json::to_string(&sample).expect("serialize");
    assert!(json.contains(r#""model":"anthropic-haiku""#), "{json}");
    let back: UsageSample = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, sample);
}
/// Why the field is an `Option` rather than a defaulted `ModelSlug`, shown
/// rather than asserted in prose: the same row, read by a struct that
/// requires the field, fails outright. `Option` is not a stylistic choice
/// here — it is the whole of the no-migration guarantee, and this fails if
/// someone "tidies" it into a required field with a fallback value.
#[test]
fn the_optional_shape_is_what_makes_an_old_row_readable() {
    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    struct RequiredModel {
        at_millis: u64,
        agent: String,
        provider: String,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cost_usd: f64,
        kind: SampleKind,
        model: ModelSlug,
    }
    let err = serde_json::from_str::<RequiredModel>(LEGACY_ROW).unwrap_err();
    assert!(err.to_string().contains("missing field `model`"), "{err}");
    // The shipped shape reads the same row.
    assert!(serde_json::from_str::<UsageSample>(LEGACY_ROW).is_ok());
}
