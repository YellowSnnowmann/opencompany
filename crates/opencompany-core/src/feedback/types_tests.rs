use super::*;

#[test]
fn detects_complaint_intents_only() {
    assert_eq!(
        detect_chat_intent("that invoice was wrong — flag it"),
        Some(FeedbackCategory::WrongOutput)
    );
    assert_eq!(
        detect_chat_intent("it can't do multi-currency"),
        Some(FeedbackCategory::MissingCapability)
    );
    assert_eq!(
        detect_chat_intent("the server crashed"),
        Some(FeedbackCategory::Bug)
    );
    // Neutral chat carries no intent.
    assert_eq!(detect_chat_intent("hi"), None);
    assert_eq!(detect_chat_intent("file it under Q3"), None);
}

#[test]
fn category_serializes_kebab_case() {
    assert_eq!(
        serde_json::to_string(&FeedbackCategory::WrongOutput).unwrap(),
        "\"wrong-output\""
    );
    assert_eq!(
        FeedbackCategory::MissingCapability.as_str(),
        "missing-capability"
    );
}

#[test]
fn consent_defaults_to_manual() {
    assert_eq!(ConsentMode::default(), ConsentMode::Manual);
    assert_eq!(
        serde_json::to_string(&ConsentMode::Auto).unwrap(),
        "\"auto\""
    );
}

#[test]
fn capture_stamps_version_and_caps_excerpt() {
    let input = FeedbackInput {
        category: FeedbackCategory::Bug,
        note: "x".repeat(CONTEXT_EXCERPT_CAP + 500),
        work_ref: Some("email.send".into()),
        template_name: None,
        template_version: None,
    };
    let item = FeedbackItem::capture(input, "9.9.9", ConsentMode::Manual);
    assert_eq!(item.runtime_version, "9.9.9");
    assert_eq!(item.context_excerpt.len(), CONTEXT_EXCERPT_CAP);
    // The full operator words stay local, uncapped.
    assert_eq!(item.operator_words.len(), CONTEXT_EXCERPT_CAP + 500);
    assert_eq!(item.work_item.as_deref(), Some("email.send"));
}

#[test]
fn capture_caps_multibyte_excerpt_on_a_char_boundary() {
    // "😀" is 4 bytes; a run of them makes byte CONTEXT_EXCERPT_CAP land
    // mid-character, which would panic a naive String::truncate.
    let input = FeedbackInput {
        category: FeedbackCategory::Bug,
        note: "😀".repeat(CONTEXT_EXCERPT_CAP),
        work_ref: None,
        template_name: None,
        template_version: None,
    };
    let item = FeedbackItem::capture(input, "9.9.9", ConsentMode::Manual);
    assert!(item.context_excerpt.len() <= CONTEXT_EXCERPT_CAP);
    // Truncated on a boundary: the excerpt is still valid UTF-8 emoji.
    assert!(item.context_excerpt.chars().all(|c| c == '😀'));
}

#[test]
fn item_round_trips_through_json() {
    let item = FeedbackItem::capture(
        FeedbackInput {
            category: FeedbackCategory::TemplateGap,
            note: "roster too thin".into(),
            work_ref: None,
            template_name: Some("marketing_agency".into()),
            template_version: Some("1.2".into()),
        },
        "0.1.0",
        ConsentMode::Auto,
    );
    let json = serde_json::to_string(&item).unwrap();
    let back: FeedbackItem = serde_json::from_str(&json).unwrap();
    assert_eq!(back, item);
}
