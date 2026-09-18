use super::readable;

#[test]
fn a_move_line_reads_as_english() {
    assert_eq!(
        readable("!propose #lazy-load defer each section until it is opened").as_deref(),
        Some("defer each section until it is opened")
    );
    assert_eq!(
        readable("!support #lazy-load ^3 agreed, and it is reversible").as_deref(),
        Some("agreed, and it is reversible")
    );
    assert_eq!(
        readable("!object >3 ^1 users bounce between sections").as_deref(),
        Some("users bounce between sections")
    );
}

/// The same characters inside a sentence are the member's own words —
/// rewriting those would edit what a teammate said.
#[test]
fn only_the_head_tokens_are_grammar() {
    assert_eq!(
        readable("!evidence #perf ^2 the p95 is > 400ms and #2 in the list is worse").as_deref(),
        Some("the p95 is > 400ms and #2 in the list is worse")
    );
}

/// Every reply on every desk that does not deliberate must survive
/// byte-for-byte.
#[test]
fn an_ordinary_reply_is_untouched() {
    assert_eq!(readable("here is the summary you asked for"), None);
    assert_eq!(readable("!notamove still ordinary prose"), None);
}

/// A bare marker still says which move it was.
#[test]
fn a_move_with_nothing_after_it_still_renders() {
    assert_eq!(
        readable("!question").as_deref(),
        Some("I have nothing further to ask."),
        "a bare move would otherwise render as an empty bubble"
    );
}
/// `line_kind` folds `!unpin` onto `pin` because both write one board — a
/// RENDERING must not, or an unpin reads as its own opposite.
#[test]
fn an_unpin_does_not_read_as_a_pin() {
    assert_eq!(
        readable("!unpin").as_deref(),
        Some("Unpinned from the room's board.")
    );
    assert_eq!(readable("!pin").as_deref(), Some("Pinned for the room."));
    // With a sentence, the member's own words stand either way.
    assert_eq!(
        readable("!unpin ^4 the window has moved past it").as_deref(),
        Some("the window has moved past it")
    );
}
