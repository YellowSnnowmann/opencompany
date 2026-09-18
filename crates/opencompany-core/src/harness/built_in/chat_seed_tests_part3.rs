use super::*;

#[test]
fn strip_current_message_drops_only_a_matching_trailing_user() {
    let mut seed = vec![op_entry("u1"), viewer_entry("a1"), op_entry("  current  ")];
    strip_current_message(&mut seed, "current");
    assert_eq!(
        flattened(seed),
        vec![
            ("user".to_string(), "operator: u1".to_string()),
            ("agent".to_string(), "a1".to_string()),
        ],
        "a trailing operator line matching the current message (trim-insensitive) is dropped"
    );

    // A trailing agent line is never the current operator message.
    let mut ends_in_agent = vec![viewer_entry("current")];
    strip_current_message(&mut ends_in_agent, "current");
    assert_eq!(ends_in_agent.len(), 1, "an agent tail is never stripped");

    // Nor is a teammate's — the collision `PEER_ROLE` exists for, tested
    // here on the speaker rather than on the flattened role.
    let mut ends_in_peer = vec![peer_entry("ada", "current")];
    strip_current_message(&mut ends_in_peer, "ada: current");
    assert_eq!(ends_in_peer.len(), 1, "a peer tail is never stripped");

    // A non-matching trailing operator line stays.
    let mut different = vec![op_entry("something else")];
    strip_current_message(&mut different, "current");
    assert_eq!(different.len(), 1, "a non-matching operator tail stays");
}

/// Codex review finding: on a message with an attachment, `HarnessBrain`
/// passes `with_attachment_refs(text, attachments)` — the raw text plus an
/// appended `"\n\n[Attached file: …]"` marker — as the turn's message,
/// while the journaled `OperatorMessage` (what the seed reads) carries
/// only the raw text. An exact match therefore never drops the duplicate,
/// so the operator's current request reached the model twice: once from
/// the un-stripped seed tail, once as the augmented current message
/// `run_single` appends itself. RED on the old `==` comparison, GREEN with
/// the `starts_with` fix.
#[test]
fn strip_current_message_drops_a_trailing_user_line_augmented_with_an_attachment_marker() {
    let mut seed = vec![op_entry("prior turn"), op_entry("please review this doc")];
    let augmented_with_attachment =
        "please review this doc\n\n[Attached file: report.pdf]\nEXTRACTED TEXT";
    strip_current_message(&mut seed, augmented_with_attachment);
    assert_eq!(
        flattened(seed),
        vec![("user".to_string(), "operator: prior turn".to_string())],
        "the raw journaled text is a prefix of the attachment-augmented \
         message, so the trailing duplicate must still be dropped"
    );
}

/// Issue #1890 B. Before the card recorded a root there was no honest
/// answer to which thread a settle belonged to, so [`in_thread`] rejected
/// every terminal outright. It answers now, on the same parent-pointer rule
/// a message does.
///
/// The seed mapper still drops the event for want of a conversational body
/// — seeding it as briefing context is sub-issue C — so this changes no
/// projection today. That is the point: C becomes a change to the mapper
/// alone, and this predicate is already right when it gets there.
#[test]
fn a_settle_belongs_to_the_thread_that_raised_its_card() {
    let root = EventSeq::new(41);
    assert!(in_thread(
        &threaded_desk_completed(50, Some("growth"), Some(41)),
        Some(root)
    ));
}

#[test]
fn a_settle_raised_in_a_sibling_thread_is_not_in_this_one() {
    // The leak this epic exists to close, in its terminal form: two live
    // threads in one channel, and the settle belongs to exactly one.
    assert!(!in_thread(
        &threaded_desk_completed(50, Some("growth"), Some(43)),
        Some(EventSeq::new(41))
    ));
}

#[test]
fn an_unthreaded_settle_belongs_to_the_channel_and_not_to_a_thread() {
    // `None` is the channel-level conversation on both sides — a positive
    // answer in each direction, not an absence. A card raised straight into
    // a channel settles where it always did…
    assert!(in_thread(&desk_completed(50, Some("growth")), None));
    // …and emphatically not inside somebody's open thread, which is the
    // regression a laxer rule would ship.
    assert!(!in_thread(
        &desk_completed(50, Some("growth")),
        Some(EventSeq::new(41))
    ));
}

#[test]
fn a_threaded_settle_is_not_in_the_channel_level_conversation() {
    assert!(!in_thread(
        &threaded_desk_completed(50, Some("growth"), Some(41)),
        None
    ));
}
