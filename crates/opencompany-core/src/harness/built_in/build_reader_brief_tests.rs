use super::*;

#[test]
fn the_reader_brief_sets_the_audience_not_a_length_budget() {
    assert!(READER_BRIEF.contains("A person reads what you post"));
    assert!(READER_BRIEF.contains("belong in tool calls"));
    assert!(
        !READER_BRIEF.contains('—'),
        "OpenHuman's style forbids em-dashes"
    );
    for budget in ["word limit", "at most", "no more than", "concise"] {
        assert!(
            !READER_BRIEF.to_lowercase().contains(budget),
            "`{budget}` reads as a length rule: {READER_BRIEF}"
        );
    }
}

#[test]
fn the_mention_brief_models_a_name_not_a_roster_id() {
    let examples: Vec<&str> = MENTION_BRIEF.split('"').skip(1).step_by(2).collect();
    assert!(
        !examples.is_empty(),
        "the brief shows an example: {MENTION_BRIEF}"
    );
    for example in examples {
        let id_like = example
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .any(|token| token.contains('_') || token.contains('-'));
        assert!(!id_like, "the example reads as a roster id: {example}");
    }
}
