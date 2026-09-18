use super::tests_core::*;
use super::*;

/// The defect this adapter exists to remove: a shared desk's transcript
/// keeps the author of every line, so one agent reads its colleague as its
/// colleague and not as itself.
#[tokio::test]
async fn a_shared_desk_reads_back_attributed() {
    let journal = Journal(vec![
        stored(
            1,
            message(None, Some("engineering"), "who owns the migration?"),
        ),
        stored(2, reply("engineer", "engineering", "I do.")),
        stored(
            3,
            reply("designer", "engineering", "I will take the console half."),
        ),
    ]);
    let record = record();
    let people = HashMap::new();
    let company = company();
    let log = JournalSessionLog::new(&journal, &company, &record, &people);

    let projected = project_session(
        &log,
        &SessionQuery {
            // These fold the whole desk — attribution and desk scoping —
            // and carry no asides, so they read as the room does.
            viewer: tinyhivemind_hive::aside::Viewer::Operator,
            conversation: Conversation {
                desk_id: "engineering".to_string(),
                desk_name: "Engineering".to_string(),
                thread_root: None,
            },
            before: None,
            window: SESSION_WINDOW,
        },
    )
    .await
    .expect("the projection folds");

    let authors: Vec<&SessionAuthor> = projected.iter().map(|m| &m.author).collect();
    assert_eq!(
        authors,
        vec![
            &SessionAuthor::Operator,
            &SessionAuthor::Agent {
                id: "engineer".to_string(),
                label: "Engineer".to_string(),
            },
            &SessionAuthor::Agent {
                id: "designer".to_string(),
                label: "Designer".to_string(),
            },
        ],
        "three lines, three authors — none of them collapsed into the reader"
    );
    assert!(
        projected.iter().all(|m| !m.content.is_empty()),
        "content travels with the author"
    );
}

/// The four spellings of General are the library's business, not the
/// adapter's: an unaddressed post stores `None` and still belongs to the
/// General desk.
#[tokio::test]
async fn an_unaddressed_post_projects_under_general() {
    let journal = Journal(vec![
        stored(1, message(None, None, "morning")),
        stored(2, reply("engineer", "main", "morning")),
        stored(3, reply("designer", "strategy", "not this desk")),
    ]);
    let record = record();
    let people = HashMap::new();
    let company = company();
    let log = JournalSessionLog::new(&journal, &company, &record, &people);

    let projected = project_session(
        &log,
        &SessionQuery {
            // These fold the whole desk — attribution and desk scoping —
            // and carry no asides, so they read as the room does.
            viewer: tinyhivemind_hive::aside::Viewer::Operator,
            conversation: Conversation {
                desk_id: "General".to_string(),
                desk_name: "General".to_string(),
                thread_root: None,
            },
            before: None,
            window: SESSION_WINDOW,
        },
    )
    .await
    .expect("the projection folds");

    assert_eq!(projected.len(), 2, "`None` and `main` are one conversation");
    assert!(
        projected.iter().all(|m| m.content != "not this desk"),
        "a named desk's traffic stays out of General"
    );
}

/// A user's message is that person, and an unattributed one is the
/// operator — the reading `chat_history` already gives a stored `by`.
#[tokio::test]
async fn an_operator_message_keeps_who_sent_it() {
    let journal = Journal(vec![
        stored(1, message(None, Some("engineering"), "legacy send")),
        stored(
            2,
            message(
                Some(Actor {
                    kind: ActorKind::User,
                    id: "u1".to_string(),
                }),
                Some("engineering"),
                "mine",
            ),
        ),
    ]);
    let record = record();
    let people = HashMap::from([("u1".to_string(), "Ada".to_string())]);
    let company = company();
    let log = JournalSessionLog::new(&journal, &company, &record, &people);
    let page = log.read_before(None, 8).await.expect("the journal reads");

    assert_eq!(
        page.messages[1].author,
        SessionAuthor::Operator,
        "an event journaled before per-user auth is the operator"
    );
    assert_eq!(
        page.messages[0].author,
        SessionAuthor::Person {
            id: "u1".to_string(),
            label: "Ada".to_string(),
        },
        "and a user's send is that person, labelled the way the console labels them"
    );
}

/// The page contract, asserted rather than assumed: newest-first, never
/// larger than asked for, and a cursor no newer than the oldest row.
#[tokio::test]
async fn a_page_is_newest_first_and_bounded() {
    let journal = Journal(
        (1..=10)
            .map(|seq| stored(seq, reply("engineer", "engineering", "hi")))
            .collect(),
    );
    let page = read(&journal, None, 4).await;

    let sequences: Vec<u64> = page.messages.iter().map(|m| m.sequence.0).collect();
    assert_eq!(
        sequences,
        vec![10, 9, 8, 7],
        "newest first, at most `limit`"
    );
    assert_eq!(
        page.next_before,
        Some(Sequence(7)),
        "the cursor is exclusive, so the next page starts at 6"
    );

    let older = read(&journal, Some(7), 4).await;
    let sequences: Vec<u64> = older.messages.iter().map(|m| m.sequence.0).collect();
    assert_eq!(
        sequences,
        vec![6, 5, 4, 3],
        "and it neither repeats nor skips"
    );
}

/// The reason [`JournalSessionLog::read_before`] loops. A run of non-message
/// events longer than one page must not read as the end of the log: an empty
/// page may only carry `None`, and `None` means "no older rows".
#[tokio::test]
async fn a_run_of_non_messages_does_not_end_the_transcript() {
    let mut events = vec![stored(
        1,
        reply("engineer", "engineering", "the oldest line"),
    )];
    events.extend((2..=20).map(noise));
    events.push(stored(
        21,
        reply("designer", "engineering", "the newest line"),
    ));
    let journal = Journal(events);

    // Page size 4, so the walk crosses five straight pages of pure noise.
    let newest = read(&journal, None, 4).await;
    assert_eq!(newest.messages.len(), 1, "the newest page holds one line");

    let older = read(&journal, newest.next_before.map(|cursor| cursor.0), 4).await;
    assert_eq!(
        older.messages.len(),
        1,
        "and the next page reaches past the noise to the oldest line"
    );
    assert_eq!(older.messages[0].sequence, Sequence(1));
    assert_eq!(
        older.next_before, None,
        "a short read is the end of the log, and only then is the cursor absent"
    );
}

/// A settle marker is a line on the desk, said by the runtime.
#[tokio::test]
async fn a_settled_card_is_a_system_line() {
    let journal = Journal(vec![stored(
        1,
        CompanyEvent::DeskTaskCompleted {
            task_id: "t1".into(),
            desk: "engineering".into(),
            output: "done".into(),
            column: "done".into(),
            artifact_ids: Vec::new(),
            origin_chat_id: Some("engineering".into()),
            origin_parent: None,
        },
    )]);
    let page = read(&journal, None, 8).await;

    assert_eq!(
        page.messages[0].author,
        SessionAuthor::System {
            kind: "task_settled".to_string(),
            label: crate::ports::SYSTEM_AUTHOR.to_string(),
        },
    );
    assert_eq!(
        page.messages[0].content,
        crate::server::chat_history::dispatch_marker_text("done"),
        "through the one function that owns the sentence"
    );
}
// -----------------------------------------------------------------------
// SPIKE: the two cross-desk referral journeys, decided against a real
// `software_company`-shaped topology.
//
// These exercise the PURE decision only — `referral()` performs no I/O and
// enqueues nothing. What a host still owes before either journey can run is
// the `ReferralQueue` transaction (idempotency, dual-conversation
// authorization, the back edge); this proves the decision half fits this
// host's desks, which is the question that comes first.
// -----------------------------------------------------------------------

/// The shipped software company's shape: three desks, no overlap.
/// **A console-created desk can opt into referral.**
///
/// The rig that exercised all of this has `group_chat = []` — every desk is
/// an overlay desk. Reading the block from the manifest alone therefore
/// left referral permanently off for it, with nothing an operator could
/// write to turn it on: there was no `[[group_chat]]` entry to hang
/// `hive.referral` on, and `OverlayDesk` had no field for it.
#[test]
fn an_overlay_desk_carries_its_own_referral_block() {
    let mut record = software_company();
    record.manifest.group_chats.clear();
    record.overlay_desks.push(crate::ports::types::OverlayDesk {
        id: "engineering".to_string(),
        name: "Engineering".to_string(),
        description: None,
        members: vec!["software_engineer".to_string()],
        responder: crate::ports::types::ResponderMode::default(),
        hive: crate::hivemind::HiveConfig {
            referral: crate::hivemind::ReferralConfig {
                enabled: Some(true),
                max_hops: Some(4),
                peer_cap: Some(3),
                ..Default::default()
            },
            ..Default::default()
        },
    });

    let config = referral_config(&record, "engineering");
    assert!(
        config.policy().enabled,
        "an overlay desk that opted in refers: {config:?}"
    );
    assert_eq!(config.policy().max_hops, 4);
    assert_eq!(config.peer_cap(), 3);

    // And a desk that declared nothing still refers nothing — the
    // conservative default is what the manifest-only read got right.
    assert!(
        !referral_config(&record, "design").policy().enabled,
        "a desk with no block anywhere stays off"
    );
}

/// **The asker is told it may ask again — that is what makes it able to.**
///
/// The returning answer never reaches the channel, so this frame is the
/// only thing the asker ever reads about it. Raw, it offered one ending;
/// framed, it offers both and gives the spelling that reaches back.
#[test]
fn a_returning_answer_offers_both_endings_and_a_forward_is_untouched() {
    let record = software_company();
    let members = roster_members(&record);
    let people: Vec<tinyhivemind_core::roster::Person> = Vec::new();
    let retired: Vec<String> = Vec::new();
    let roster = tinyhivemind_core::roster::Roster::new(&members, &people, &retired);
    let desks = desk_snapshots(&record);

    let body = "@product_designer can you take the login screen?";
    let mentions = tinyhivemind_core::mention::resolve(
        body,
        None,
        &tinyhivemind_core::mention::MentionAuthor::Agent {
            id: "software_engineer".to_string(),
        },
        &roster,
        &desks.set(),
    );
    let tinyhivemind_core::referral::ReferralDecision::One { mut referral } =
        tinyhivemind_core::referral::referral(
            tinyhivemind_core::referral::ReferralPolicy {
                enabled: true,
                max_hops: 4,
                reach: tinyhivemind_core::referral::ReferralReach::Channels,
                returns: true,
            },
            &referral_input(body, mentions),
            &roster,
            &desks.set(),
        )
        .expect("decides")
    else {
        panic!("a crossing referral was available");
    };

    assert_eq!(
        returned_answer(&record, &referral, REFERRAL_MAX_HOPS),
        body,
        "a forward carries the asker's own words into a room people read"
    );

    // A real return has the mirrored geometry: it was committed on the desk
    // that answered, and travels to the one that asked. Flipping `kind`
    // alone would leave a forward's desks in place and let this pass while
    // naming the wrong room to go back to.
    referral.kind = tinyhivemind_core::referral::ReferralKind::Return;
    referral.from = referral.to.clone();
    referral.source_id = "product_designer".to_string();
    referral.content = "use a skeleton, not a spinner".to_string();
    let framed = returned_answer(&record, &referral, REFERRAL_MAX_HOPS);
    assert!(
        framed.contains("use a skeleton, not a spinner"),
        "the answer survives the framing: {framed}"
    );
    assert!(
        framed.contains("@#product_design"),
        "and names the exact spelling that reaches back: {framed}"
    );
    assert!(
        framed.contains("report back") && framed.contains("ask them again"),
        "both endings are stated, so choosing is the asker's: {framed}"
    );
    assert!(
        framed.contains("exchange 1 of 2"),
        "and the asker is told how much room is left: {framed}"
    );

    // **The last exchange offers one ending, not two.** Inviting a
    // follow-up the policy will refuse is worse than never inviting one:
    // the question gets journaled in the channel and never delivered, so it
    // reads as though the other desk had ignored it.
    referral.child_hop = REFERRAL_MAX_HOPS;
    let last = returned_answer(&record, &referral, REFERRAL_MAX_HOPS);
    assert!(
        last.contains("exchange 2 of 2") && last.contains("last exchange"),
        "the asker is told this is the end: {last}"
    );
    assert!(
        !last.contains("@#product_design"),
        "and is not invited to write a question that cannot be delivered: {last}"
    );
    assert!(
        last.contains("could not get it"),
        "an unmet need is escalated to a person instead of vanishing: {last}"
    );
}

/// **The chain deepens, and therefore ends.**
///
/// Each generation must sit one hop below the one that caused it, or the
/// bound is decorative. The host used to hand every referred turn a
/// hardcoded depth of `1`, which is true of the first one and of no other:
/// a follow-up claimed the same depth as the answer it followed, so
/// `max_hops` was never approached however long two desks went on. Nothing
/// drove such a loop then — the asker could not ask again — so the defect
/// was invisible until the moment it mattered.
///
/// Walked here as the policy sees it: the depth a turn reports is the depth
/// its own replies are offered at, so the walk is `child_hop` feeding the
/// next `hop`. Four hops is ask, answer, ask again, answer again — and the
/// fifth is refused.
#[test]
fn a_follow_up_is_one_hop_deeper_and_the_budget_ends_it() {
    let record = software_company();
    let members = roster_members(&record);
    let people: Vec<tinyhivemind_core::roster::Person> = Vec::new();
    let retired: Vec<String> = Vec::new();
    let roster = tinyhivemind_core::roster::Roster::new(&members, &people, &retired);
    let desks = desk_snapshots(&record);
    let policy = tinyhivemind_core::referral::ReferralPolicy {
        enabled: true,
        max_hops: 4,
        reach: tinyhivemind_core::referral::ReferralReach::Channels,
        returns: true,
    };

    let body = "@product_designer can you take the login screen?";
    let mut depths: Vec<u32> = Vec::new();
    let mut hop = 0;
    loop {
        let mentions = tinyhivemind_core::mention::resolve(
            body,
            None,
            &tinyhivemind_core::mention::MentionAuthor::Agent {
                id: "software_engineer".to_string(),
            },
            &roster,
            &desks.set(),
        );
        let mut input = referral_input(body, mentions);
        input.hop = hop;
        match tinyhivemind_core::referral::referral(policy, &input, &roster, &desks.set())
            .expect("decides")
        {
            tinyhivemind_core::referral::ReferralDecision::One { referral } => {
                assert_eq!(
                    referral.child_hop,
                    hop + 1,
                    "a child sits exactly one below its cause"
                );
                depths.push(referral.child_hop);
                hop = referral.child_hop;
            }
            tinyhivemind_core::referral::ReferralDecision::None { reason } => {
                assert_eq!(
                    reason,
                    tinyhivemind_core::referral::NoReferralReason::HopLimitReached,
                    "the chain ends because the budget ran out, not for some other reason"
                );
                break;
            }
        }
        assert!(hop <= 8, "the walk must terminate; it did not");
    }

    assert_eq!(
        depths,
        vec![1, 2, 3, 4],
        "four hops: ask, answer, ask again, answer again"
    );
}

/// **Journey 1** — a named teammate who is not on this desk runs on THEIR
/// desk, rather than being pulled into this conversation.
#[test]
fn a_named_outsider_is_referred_to_their_own_desk() {
    let record = software_company();
    let members = roster_members(&record);
    let people: Vec<tinyhivemind_core::roster::Person> = Vec::new();
    let retired: Vec<String> = Vec::new();
    let roster = tinyhivemind_core::roster::Roster::new(&members, &people, &retired);
    let desks = desk_snapshots(&record);

    let body = "@product_designer can you take the login screen?";
    let mentions = tinyhivemind_core::mention::resolve(
        body,
        None,
        &tinyhivemind_core::mention::MentionAuthor::Agent {
            id: "software_engineer".to_string(),
        },
        &roster,
        &desks.set(),
    );
    let decision = tinyhivemind_core::referral::referral(
        tinyhivemind_core::referral::ReferralPolicy {
            enabled: true,
            max_hops: 2,
            reach: tinyhivemind_core::referral::ReferralReach::Channels,
            returns: true,
        },
        &referral_input(body, mentions),
        &roster,
        &desks.set(),
    )
    .expect("decides");

    match decision {
        tinyhivemind_core::referral::ReferralDecision::One { referral } => {
            assert_eq!(referral.target_id, "product_designer");
            assert_eq!(referral.from.desk_id, "engineering");
            assert_eq!(
                referral.to.desk_id, "product_design",
                "the outsider runs on their OWN desk, not as a guest here"
            );
            assert!(
                referral.origin.is_some(),
                "a crossing forward carries the way home"
            );
        }
        other => panic!("expected a referral, got {other:?}"),
    }
}

/// **Journey 2** — the asker names a DESK, not a person, and the library
/// picks that desk's responder.
///
/// This is the one that answers "I can't help with this and I don't know
/// who can": addressing is at desk granularity, so the engineering agent
/// needs to know that design exists, not who is on it.
#[test]
fn a_desk_mention_selects_that_desks_responder() {
    let record = software_company();
    let members = roster_members(&record);
    let people: Vec<tinyhivemind_core::roster::Person> = Vec::new();
    let retired: Vec<String> = Vec::new();
    let roster = tinyhivemind_core::roster::Roster::new(&members, &people, &retired);
    let desks = desk_snapshots(&record);

    let body = "@#product_design who owns the login screen?";
    let mentions = tinyhivemind_core::mention::resolve(
        body,
        None,
        &tinyhivemind_core::mention::MentionAuthor::Agent {
            id: "software_engineer".to_string(),
        },
        &roster,
        &desks.set(),
    );
    let decision = tinyhivemind_core::referral::referral(
        tinyhivemind_core::referral::ReferralPolicy {
            enabled: true,
            max_hops: 2,
            // `Desks` is what makes an `@#desk` mention selectable at all;
            // under `Channels` it decides nothing.
            reach: tinyhivemind_core::referral::ReferralReach::Desks,
            returns: true,
        },
        &referral_input(body, mentions),
        &roster,
        &desks.set(),
    )
    .expect("decides");

    match decision {
        tinyhivemind_core::referral::ReferralDecision::One { referral } => {
            assert_eq!(
                referral.to.desk_id, "product_design",
                "the desk mention chose the desk"
            );
            assert_eq!(
                referral.target_id, "product_designer",
                "and the library picked its responder — the asker never named a person"
            );
        }
        other => panic!("expected a desk referral, got {other:?}"),
    }
}
