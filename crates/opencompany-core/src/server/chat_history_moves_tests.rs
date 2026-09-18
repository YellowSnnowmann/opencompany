use super::*;

/// A desk-visible row by `author`, or an aside when `to` names somebody.
fn row(id: &str, author: &str, text: &str, to: &[&str]) -> MessageView {
    MessageView {
        id: id.to_owned(),
        channel: author.to_owned(),
        admin_only: false,
        cue_author: author.to_owned(),
        author: author.to_owned(),
        cue_text: text.to_owned(),
        text: text.to_owned(),
        at_millis: 0.0,
        mine: false,
        by_person: false,
        referred_from: None,
        referral_conversation: None,
        aside_audience: to.iter().map(|id| (*id).to_owned()).collect(),
        aside_conversation: None,
        steps: Vec::new(),
        task_id: None,
        parent_id: None,
        reactions: Vec::new(),
        mentions: Vec::new(),
        attachments: Vec::new(),
        outputs: Vec::new(),
        resolution_user_facing: false,
        resolution_code: None,
        resolution_pair_agent_id: None,
        resolution_provider_slug: None,
    }
}

#[test]
fn an_aside_folds_onto_the_move_it_rode_under() {
    let mut messages = vec![
        row("1", "exchanges", "!propose #swap the clicky variant", &[]),
        row(
            "2",
            "exchanges",
            "!aside @refunds the difference is -$16.63",
            &["refunds"],
        ),
        row("3", "refunds", "!support #swap ^1", &[]),
    ];
    fold_asides(&mut messages);

    // The aside is lifted out of the transcript...
    assert_eq!(
        messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["1", "3"],
        "the aside row no longer stands in the desk's own conversation"
    );
    // ...and hangs on its author's move, with the marker head stripped.
    let Some(AsideConversation { members, lines }) = &messages[0].aside_conversation else {
        panic!("the move carries the aside");
    };
    assert_eq!(members, &["exchanges".to_owned(), "refunds".to_owned()]);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "the difference is -$16.63");
    assert!(
        !lines[0].text.contains("!aside"),
        "the grammar never reaches the operator's view"
    );
}

#[test]
fn an_orphan_aside_is_kept_rather_than_dropped() {
    // No move above it — a page that begins mid-exchange. A line in the wrong
    // shape beats a line nobody can read.
    let mut messages = vec![row(
        "1",
        "exchanges",
        "!aside @refunds mid-page",
        &["refunds"],
    )];
    fold_asides(&mut messages);
    assert_eq!(messages.len(), 1, "the row survives");
    assert!(messages[0].aside_conversation.is_none());
}

#[test]
fn an_aside_never_hangs_on_another_seats_move() {
    let mut messages = vec![
        row("1", "refunds", "!propose #refund take the return", &[]),
        row(
            "2",
            "exchanges",
            "!aside @refunds are you sure?",
            &["refunds"],
        ),
    ];
    fold_asides(&mut messages);
    // `exchanges` has no move above it, so its aside stays put rather than
    // being attributed to the seat that happened to speak last.
    assert_eq!(messages.len(), 2);
    assert!(messages[0].aside_conversation.is_none());
}

#[test]
fn aside_body_strips_every_addressee_and_leaves_other_text_alone() {
    assert_eq!(aside_body("!aside @a @b the point"), "the point");
    assert_eq!(aside_body("  !aside   @a   spaced  "), "spaced");
    // Not an aside: untouched, including a marker this host does not police.
    assert_eq!(aside_body("!propose #x y"), "!propose #x y");
    assert_eq!(aside_body("plain prose"), "plain prose");
}

pub(super) fn agent_reply(chat_id: &str) -> CompanyEvent {
    CompanyEvent::AgentReply {
        audience: Vec::new(),
        mentions: Vec::new(),
        mention_depth: 0,
        parent: None,
        task_id: None,
        outputs: Vec::new(),
        chat_id: chat_id.to_string(),
        agent_id: "ceo".to_string(),
        text: "hi".to_string(),
        steps: Vec::new(),
    }
}

/// `None` is the shape the chat route stores for an unaddressed post.
fn operator_message(chat: Option<&str>) -> CompanyEvent {
    CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "hi".to_string(),
        by: None,
        chat: chat.map(str::to_string),
        deliverable: None,
        attachments: Vec::new(),
    }
}

/// The whole difference between the two predicates, in one place.
///
/// `same_conversation` folds a missing id into General because an
/// unaddressed *message* went to the company-wide line.
/// `stamped_conversation_is` refuses to, because a missing id *stamped on a
/// record* means no conversation produced it — and handing those to General
/// is what let a thread-less parked blocker eat a founder's first line in
/// `#general` (B-059).
#[test]
fn a_stamped_origin_of_none_names_no_conversation_including_general() {
    for desk in [GENERAL_DESK, MAIN_THREAD_ID, "general", ""] {
        assert!(
            same_conversation(None, Some(desk)),
            "an unaddressed message still folds into General ({desk:?})"
        );
        assert!(
            !stamped_conversation_is(None, desk),
            "but a record stamped with no conversation belongs to none, {desk:?} included"
        );
    }
    assert!(!stamped_conversation_is(None, "engineering"));
}

/// Everything that *is* stamped compares exactly as `same_conversation`
/// does, so the carve-out cannot quietly become "refuse everything".
#[test]
fn a_stamped_origin_folds_general_and_compares_every_other_desk_verbatim() {
    for origin in [GENERAL_DESK, MAIN_THREAD_ID, "general", ""] {
        for desk in [GENERAL_DESK, MAIN_THREAD_ID, "general", ""] {
            assert!(
                stamped_conversation_is(Some(origin), desk),
                "every spelling of General is one conversation: {origin:?} vs {desk:?}"
            );
        }
    }
    assert!(stamped_conversation_is(Some("dm:eng"), "dm:eng"));
    assert!(!stamped_conversation_is(Some("dm:eng"), "dm:ops"));
    assert!(!stamped_conversation_is(Some("dm:eng"), GENERAL_DESK));
    assert!(
        !stamped_conversation_is(Some("Engineering"), "engineering"),
        "a desk id is opaque — the General fold is not a licence to loosen the rest"
    );
}

#[test]
fn general_desk_owns_agent_replies_under_general_and_main() {
    assert!(owns(GENERAL_DESK, GENERAL_DESK, &agent_reply(GENERAL_DESK)));
    assert!(owns(
        GENERAL_DESK,
        GENERAL_DESK,
        &agent_reply(MAIN_THREAD_ID)
    ));
    assert!(owns(GENERAL_DESK, GENERAL_DESK, &agent_reply("")));
    assert!(!owns(GENERAL_DESK, GENERAL_DESK, &agent_reply("strategy")));
}

/// The console asks for its default line as `?desk=main`, which resolves to
/// `("main", "main")` — no group chat is named `main` — so the desk side has
/// to fold too (issue #435).
///
/// The pair that made this reachable: an unaddressed chat post journals the
/// operator message with `chat: None` and its answer with
/// `chat_id: "General"`, so before this both halves of that conversation were
/// missing from the one transcript that should hold them.
#[test]
fn the_main_line_owns_what_was_journaled_under_general() {
    for stored in [GENERAL_DESK, MAIN_THREAD_ID, ""] {
        assert!(
            owns(MAIN_THREAD_ID, MAIN_THREAD_ID, &agent_reply(stored)),
            "a reply stored as `{stored}` belongs to the main line",
        );
        assert!(
            owns(
                MAIN_THREAD_ID,
                MAIN_THREAD_ID,
                &operator_message(Some(stored))
            ),
            "an operator message stored as `{stored}` belongs to the main line",
        );
    }
    // The unaddressed post itself — the case that produces the pair above.
    assert!(owns(
        MAIN_THREAD_ID,
        MAIN_THREAD_ID,
        &operator_message(None)
    ));

    // …and the fold stops at the General family: a named desk's traffic
    // does not join the main line, in either direction.
    assert!(!owns(
        MAIN_THREAD_ID,
        MAIN_THREAD_ID,
        &agent_reply("strategy")
    ));
    assert!(!owns(
        "strategy",
        "Strategy desk",
        &operator_message(Some(GENERAL_DESK))
    ));
}

#[test]
fn non_general_desk_only_owns_its_own_id_or_name() {
    assert!(owns("strategy", "Strategy desk", &agent_reply("strategy")));
    assert!(owns(
        "strategy",
        "Strategy desk",
        &agent_reply("Strategy desk")
    ));
    assert!(!owns(
        "strategy",
        "Strategy desk",
        &agent_reply(MAIN_THREAD_ID)
    ));
    assert!(!owns("strategy", "Strategy desk", &agent_reply("")));
}

#[test]
fn general_desk_owns_every_operator_message() {
    let event = CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "hi".to_string(),
        by: Some(Actor {
            kind: ActorKind::User,
            id: "u1".to_string(),
        }),
        chat: Some(MAIN_THREAD_ID.to_string()),
        deliverable: None,
        attachments: Vec::new(),
    };
    assert!(owns(GENERAL_DESK, GENERAL_DESK, &event));
    assert!(!owns("strategy", "Strategy desk", &event));
}

#[test]
fn main_thread_owns_operator_messages_it_stored() {
    let event = CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "hi".to_string(),
        by: None,
        chat: Some(MAIN_THREAD_ID.to_string()),
        deliverable: None,
        attachments: Vec::new(),
    };
    // The console queries the main thread with desk = ("main", "main").
    assert!(owns(MAIN_THREAD_ID, MAIN_THREAD_ID, &event));
    // And it is still owned when read under the General desk's own id/name.
    assert!(owns(GENERAL_DESK, GENERAL_DESK, &event));
    // But it must not leak into an unrelated desk.
    assert!(!owns("strategy", "Strategy desk", &event));
}

#[test]
fn desk_addressed_operator_message_belongs_to_that_desk() {
    let event = CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "hi".to_string(),
        by: None,
        chat: Some("strategy".to_string()),
        deliverable: None,
        attachments: Vec::new(),
    };
    assert!(owns("strategy", "Strategy desk", &event));
    assert!(!owns(MAIN_THREAD_ID, MAIN_THREAD_ID, &event));
}
