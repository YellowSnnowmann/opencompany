use super::*;

#[test]
fn a_retry_word_asks_for_the_same_step_again() {
    for reply in [
        "retry",
        "try it again",
        "go ahead",
        "yes, proceed",
        "approved",
    ] {
        assert_eq!(
            classify_blocker_reply(reply),
            BlockerReplyIntent::Retry,
            "reply: {reply}"
        );
    }
}

#[test]
fn a_skip_word_waives_the_blocker() {
    for reply in [
        "skip it",
        "waive this",
        "just ignore it",
        "bypass the check",
    ] {
        assert_eq!(
            classify_blocker_reply(reply),
            BlockerReplyIntent::Skip,
            "reply: {reply}"
        );
    }
}

#[test]
fn a_cancel_word_abandons_the_work() {
    for reply in [
        "cancel",
        "abort this",
        "drop it",
        "forget about it",
        "scrap the task",
    ] {
        assert_eq!(
            classify_blocker_reply(reply),
            BlockerReplyIntent::Cancel,
            "reply: {reply}"
        );
    }
}

/// Abandon outranks retry: "cancel this and retry the other" is a cancel of
/// the thing in hand.
#[test]
fn abandon_outranks_retry_when_both_appear() {
    assert_eq!(
        classify_blocker_reply("cancel this one, retry the other"),
        BlockerReplyIntent::Cancel
    );
}

#[test]
fn a_substantive_answer_is_an_amendment() {
    for reply in [
        "use gpt-4o-mini instead",
        "deploy to staging, not prod",
        "the brief in the January doc is the current one",
    ] {
        assert_eq!(
            classify_blocker_reply(reply),
            BlockerReplyIntent::Amend,
            "reply: {reply}"
        );
    }
}

#[test]
fn a_greeting_or_a_question_back_is_unrelated() {
    for reply in [
        "hey",
        "hello there",
        "what do you mean?",
        "which one is blocked?",
        "   ",
    ] {
        assert_eq!(
            classify_blocker_reply(reply),
            BlockerReplyIntent::Unrelated,
            "reply: {reply}"
        );
    }
}

/// The word match is on whole words: `okra` is not `ok`.
#[test]
fn a_verdict_word_matches_only_as_a_whole_word() {
    assert_eq!(
        classify_blocker_reply("order some okra for the office"),
        BlockerReplyIntent::Amend,
        "a substring of a verdict word is not that verdict"
    );
}

#[test]
fn a_negated_verdict_word_is_not_that_verdict() {
    for reply in [
        "don't retry this",
        "do not retry",
        "not approved",
        "no, cancel",
    ] {
        assert_ne!(
            classify_blocker_reply(reply),
            BlockerReplyIntent::Retry,
            "a negated verdict word must not read as the positive verdict: {reply:?}"
        );
    }
    assert_ne!(
        classify_blocker_reply("no, cancel"),
        BlockerReplyIntent::Cancel,
        "a negated cancel is not a cancel"
    );
}
