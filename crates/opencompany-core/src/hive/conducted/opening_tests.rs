use super::*;

fn trigger(text: &str, carried_on: bool) -> Trigger {
    Trigger {
        seq: EventSeq::new(3),
        text: text.to_owned(),
        parent: None,
        mentions: Vec::new(),
        carried_on,
    }
}

#[test]
fn a_question_reads_as_one_and_a_request_does_not() {
    assert!(trigger("What is on the board this week?", false).is_question());
    assert!(!trigger("Write the launch post and book the venue.", false).is_question());
}

#[test]
fn a_carry_on_is_never_a_question() {
    assert!(!trigger("What is on the board this week?", true).is_question());
}
