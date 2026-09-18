use super::*;

#[test]
fn the_three_verdicts_read_off_a_one_word_reply() {
    assert_eq!(TriageVerdict::parse("answer"), TriageVerdict::Answer);
    assert_eq!(TriageVerdict::parse("work"), TriageVerdict::Work);
    assert_eq!(TriageVerdict::parse("chatter"), TriageVerdict::Chatter);
}

/// Models add punctuation, capitals and the occasional preamble. None of
/// that is a failure to classify.
#[test]
fn a_verdict_survives_the_shapes_a_model_actually_replies_in() {
    for (reply, want) in [
        ("Answer.", TriageVerdict::Answer),
        ("`work`", TriageVerdict::Work),
        ("  chatter\n", TriageVerdict::Chatter),
        ("The answer is: answer", TriageVerdict::Answer),
    ] {
        assert_eq!(TriageVerdict::parse(reply), want, "{reply:?}");
    }
}

/// The closed vocabulary. Anything unrecognised is not a classification, and
/// guessing at it would spend the gate on noise.
#[test]
fn an_unreadable_reply_is_unavailable_rather_than_guessed() {
    for reply in [
        "",
        "   ",
        "I'm sorry, I can't help with that.",
        "42",
        "unknown",
    ] {
        assert_eq!(
            TriageVerdict::parse(reply),
            TriageVerdict::Unavailable,
            "{reply:?} is not a verdict"
        );
    }
}

/// Only `Answer` touches the gate. `Work` is named for the log and acts like
/// `Chatter` here — a verdict never mints a card (see the module docs).
#[test]
fn only_answer_narrows_the_claim() {
    assert!(TriageVerdict::Answer.is_answer());
    assert!(!TriageVerdict::Work.is_answer());
    assert!(!TriageVerdict::Chatter.is_answer());
    assert!(!TriageVerdict::Unavailable.is_answer());
}
