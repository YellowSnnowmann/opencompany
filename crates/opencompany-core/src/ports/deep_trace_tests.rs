use super::*;

fn detail(reasoning: Option<&str>, output: Option<&str>) -> TurnStepDetail {
    TurnStepDetail {
        reasoning: reasoning.map(str::to_string),
        output: output.map(str::to_string),
        ..TurnStepDetail::default()
    }
}

#[test]
fn a_record_round_trips_as_camel_case_json() {
    let record = RunStepDetailRecord {
        run_id: "run-1".to_string(),
        step_seq: 3,
        at_millis: 42,
        detail: TurnStepDetail {
            reasoning: Some("memoise the chain".to_string()),
            arguments: Some(r#"{"command":"python3 solve.py"}"#.to_string()),
            output: Some("837799\n".to_string()),
            display_detail: Some("solve.py".to_string()),
            iteration: Some(2),
            clipped: false,
        },
    };
    let json = serde_json::to_string(&record).unwrap();
    assert!(json.contains(r#""runId":"run-1""#), "{json}");
    assert!(json.contains(r#""stepSeq":3"#), "{json}");
    assert!(json.contains(r#""displayDetail":"solve.py""#), "{json}");
    // `clipped: false` is skipped, so a record that was not clipped
    // serializes exactly as it did before the field existed.
    assert!(!json.contains("clipped"), "{json}");
    let back: RunStepDetailRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, record);
}

#[test]
fn an_absent_half_stays_absent() {
    // A tool step has no reasoning and a thinking step has no output; both
    // must serialize without the other's keys rather than as explicit nulls.
    let json = serde_json::to_string(&detail(Some("why"), None)).unwrap();
    assert!(json.contains("reasoning"));
    assert!(!json.contains("output"), "{json}");
}

#[test]
fn a_step_with_nothing_to_say_is_empty() {
    assert!(TurnStepDetail::default().is_empty());
    assert!(!detail(Some("x"), None).is_empty());
    assert!(!detail(None, Some("x")).is_empty());
}

#[test]
fn iteration_alone_does_not_make_a_record_worth_writing() {
    // Otherwise every step would write a row saying which loop pass it was
    // and nothing else.
    let only_iteration = TurnStepDetail {
        iteration: Some(4),
        ..TurnStepDetail::default()
    };
    assert!(only_iteration.is_empty());
}

#[test]
fn a_value_under_the_cap_is_untouched() {
    let bounded = bound_detail(detail(Some("short"), Some("also short")));
    assert_eq!(bounded.reasoning.as_deref(), Some("short"));
    assert_eq!(bounded.output.as_deref(), Some("also short"));
    assert!(!bounded.clipped);
}

#[test]
fn an_oversized_value_is_clipped_and_flagged() {
    let huge = "x".repeat(DEEP_OUTPUT_CHAR_CAP + 500);
    let bounded = bound_detail(detail(None, Some(&huge)));
    assert!(bounded.clipped);
    let kept = bounded.output.unwrap();
    assert!(kept.ends_with(ELLIPSIS));
    assert_eq!(
        kept.chars().count(),
        DEEP_OUTPUT_CHAR_CAP + ELLIPSIS.chars().count()
    );
}

#[test]
fn clipping_never_splits_a_character() {
    // A multi-byte body clipped mid-sequence would be invalid UTF-8, which
    // `String::truncate` panics on rather than silently corrupting.
    let multibyte = "é".repeat(DEEP_OUTPUT_CHAR_CAP + 100);
    let bounded = bound_detail(detail(None, Some(&multibyte)));
    let kept = bounded.output.unwrap();
    assert!(bounded.clipped);
    assert!(kept.starts_with('é'));
    // Round-tripping proves it is still valid UTF-8.
    assert_eq!(String::from_utf8(kept.clone().into_bytes()).unwrap(), kept);
}

#[test]
fn exactly_at_the_cap_is_not_clipped() {
    let exact = "x".repeat(DEEP_REASONING_CHAR_CAP);
    let bounded = bound_detail(detail(Some(&exact), None));
    assert!(!bounded.clipped);
    assert_eq!(
        bounded.reasoning.unwrap().chars().count(),
        DEEP_REASONING_CHAR_CAP
    );
}

#[test]
fn each_field_has_its_own_cap() {
    // Arguments are capped tighter than output; a body that fits the output
    // cap must still be clipped when it arrives as arguments.
    let between = "x".repeat(DEEP_ARGUMENTS_CHAR_CAP + 10);
    let bounded = bound_detail(TurnStepDetail {
        arguments: Some(between.clone()),
        output: Some(between),
        ..TurnStepDetail::default()
    });
    assert!(bounded.arguments.unwrap().ends_with(ELLIPSIS));
    assert!(!bounded.output.unwrap().ends_with(ELLIPSIS));
}

#[test]
fn an_already_clipped_record_stays_clipped() {
    // The flag is sticky: a caller that clipped upstream must not have it
    // cleared by a second pass that happened to find everything in bounds.
    let bounded = bound_detail(TurnStepDetail {
        reasoning: Some("short".to_string()),
        clipped: true,
        ..TurnStepDetail::default()
    });
    assert!(bounded.clipped);
}
