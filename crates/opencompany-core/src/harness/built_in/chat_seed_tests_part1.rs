use super::*;

/// The core projection: a journal interleaving the General desk's own
/// operator/agent turns with an unrelated desk's message, a structural
/// dispatch terminal, and an empty reply. Only the General desk's real
/// conversational turns survive, in chronological order, with the right roles.
#[tokio::test]
async fn projects_only_owning_conversational_turns_in_order() {
    let log = FixedLog(vec![
        operator(0, Some("general"), "u1"),
        reply(1, "general", "a1"),
        // Another desk entirely — must never appear in General's seed.
        operator(2, Some("engineering"), "OTHER-DESK"),
        reply(3, "engineering", "OTHER-REPLY"),
        // `owns` admits this (origin is General) but it is a structural
        // marker, not a turn — the projector must skip it.
        desk_completed(4, Some("general")),
        // A blank reply carries no body to seed.
        reply(5, "general", "   "),
        operator(6, Some("general"), "u2"),
    ]);

    let seed = seed_of(log, "general", "general", CHAT_SEED_WINDOW, "").await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: u1".to_string()),
            ("agent".to_string(), "a1".to_string()),
            ("user".to_string(), "operator: u2".to_string()),
        ],
        "only General's own operator/agent turns, chronological, correctly roled"
    );
}

/// **A private aside must not reach an agent that was not party to it.**
///
/// The aside seam lets two members of a desk compare notes in a line the
/// rest of the room cannot read — elided rather than removed, so everyone
/// still sees *that* an exchange happened and who was in it. Both
/// tinyhivemind adapters honour that: `hivemind::log` (the episode path) and
/// `runtime::hivemind` (the gated seed path) each map a non-empty
/// `audience` to `Audience::Aside { members }`, and `project_for` then
/// elides the content for a reader outside the set.
///
/// This asserts the same property on the path that is **not** either of
/// those: the ordinary chat seed a non-deliberating turn is built from,
/// which reads `CompanyEvent`s directly. That path is what every shipped
/// tenant runs — `hivemind` is absent from `TENANT_FEATURES` — so if the
/// audience is not consulted here, an aside reaches an outsider's model
/// context in full on the default build.
#[tokio::test]
async fn a_private_aside_does_not_reach_a_non_party_agents_seed() {
    const SECRET: &str = "ASIDE-ONLY-MARKER";

    let log = FixedLog(vec![
        operator(1, Some("growth"), "what should we do about the outage?"),
        // Alice and Bob compare notes privately. Carol is seated on the
        // desk but is not in the audience.
        aside_by(2, "growth", "alice", &["alice", "bob"], SECRET),
        reply_by(3, "growth", "bob", "agreed, let us raise it in the open"),
    ]);

    let carol = seed_for(log, "carol", None).await;

    let leaked = carol.iter().any(|(_, text)| text.contains(SECRET));
    assert!(
        !leaked,
        "an aside Carol was not party to reached her seed in full: {carol:?}"
    );

    // **Elided, not dropped.** Asserted separately because the absence
    // check above passes just as happily on a projection that deleted the
    // row — and a deleted row is the failure mode the seam was explicitly
    // designed against: Carol must still see that Alice and Bob conferred.
    let stub = carol
        .iter()
        .find(|(_, text)| text.contains("alice:"))
        .unwrap_or_else(|| panic!("Alice's row vanished from Carol's seed: {carol:?}"));
    assert!(
        stub.1.contains("private aside") && stub.1.contains('2'),
        "the stub must say an aside happened and how many were in it: {stub:?}"
    );
}

/// The other half of the same property: elision is **per reader**, so a
/// member of the aside must still receive it. A projection that simply
/// dropped every audienced row would pass the test above while breaking the
/// seam it is meant to protect.
#[tokio::test]
async fn a_private_aside_still_reaches_an_agent_that_was_party_to_it() {
    const SECRET: &str = "ASIDE-ONLY-MARKER";

    let log = FixedLog(vec![
        operator(1, Some("growth"), "what should we do about the outage?"),
        aside_by(2, "growth", "alice", &["alice", "bob"], SECRET),
    ]);

    let bob = seed_for(log, "bob", None).await;

    assert!(
        bob.iter().any(|(_, text)| text.contains(SECRET)),
        "Bob was addressed in the aside and must still read it: {bob:?}"
    );
}

/// Codex review finding: a prior operator message's attachment must survive
/// into the seed, not just its raw text — otherwise a follow-up like
/// "summarize that file again" loses the file context on a resumed turn,
/// even though the SAME message's attachment reached the model fine the
/// first time it was live (via `with_attachment_refs` on the current-turn
/// path). The seed must go through the identical formatter.
#[tokio::test]
async fn a_prior_message_with_an_attachment_keeps_its_attachment_marker_in_the_seed() {
    let attachment = crate::ports::types::Attachment {
        node_id: "node-1".to_string(),
        name: "report.pdf".to_string(),
        mime: "application/pdf".to_string(),
        size: 1234,
        extracted_text: Some("QUARTERLY_REPORT_MARKER".to_string()),
    };
    let log = FixedLog(vec![operator_with_attachment(
        0,
        Some("general"),
        "please review this",
        attachment,
    )]);

    let seed = seed_of(log, "general", "general", CHAT_SEED_WINDOW, "").await;

    assert_eq!(seed.len(), 1, "the one owning message is seeded");
    let (role, text) = &seed[0];
    assert_eq!(role, "user");
    assert!(
        text.starts_with("operator: please review this"),
        "the operator's own words still lead, behind their byline: {text:?}"
    );
    assert!(
        text.contains("QUARTERLY_REPORT_MARKER"),
        "the attachment's extracted text must reach a resumed turn's \
         context, exactly like it reaches a live one: {text:?}"
    );
    // The multi-line case that matters for #2075: an attachment marker is
    // appended behind blank lines, so this body is the everyday proof that
    // continuation lines are attributed too and cannot open a fresh byline.
    assert!(
        text.lines().all(|line| line.starts_with("operator: ")),
        "every line carries the speaker, marker lines included: {text:?}"
    );
}

/// The General desk answers to every spelling of itself, so a reply journaled
/// under `"General"` and a `"main"` operator line both land in the seed for a
/// desk addressed as `"main"` — the folding `owns`/`same_conversation` give.
#[tokio::test]
async fn general_desk_folds_its_spellings() {
    let log = FixedLog(vec![
        operator(0, None, "unaddressed"),
        reply(1, "General", "under-General"),
        operator(2, Some("main"), "under-main"),
    ]);

    let seed = seed_of(log, "main", "main", CHAT_SEED_WINDOW, "").await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: unaddressed".to_string()),
            ("agent".to_string(), "under-General".to_string()),
            ("user".to_string(), "operator: under-main".to_string()),
        ],
    );
}

/// DM parity: a `dm:<id>` thread is an opaque verbatim key, so its own turns
/// project and a sibling DM's do not.
#[tokio::test]
async fn dm_thread_projects_and_isolates() {
    let log = FixedLog(vec![
        operator(0, Some("dm:alice"), "hi alice"),
        reply(1, "dm:alice", "hi back"),
        operator(2, Some("dm:bob"), "hi bob"),
    ]);

    let seed = seed_of(log, "dm:alice", "dm:alice", CHAT_SEED_WINDOW, "").await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: hi alice".to_string()),
            ("agent".to_string(), "hi back".to_string()),
        ],
        "only the addressed DM's own turns, never the sibling DM's"
    );
}

/// A named desk's turns can be journaled under either its id or its name;
/// `owns` matches both, so passing the resolved `(id, name)` pair seeds every
/// line regardless of which spelling wrote it.
#[tokio::test]
async fn named_desk_matches_id_and_name() {
    let log = FixedLog(vec![
        operator(0, Some("eng-123"), "by id"),
        reply(1, "Engineering", "by name"),
        operator(2, Some("marketing"), "OTHER"),
    ]);

    let seed = seed_of(log, "eng-123", "Engineering", CHAT_SEED_WINDOW, "").await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: by id".to_string()),
            ("agent".to_string(), "by name".to_string()),
        ],
    );
}

/// The window keeps the most-recent `window` owning turns and drops older
/// ones, even when unrelated events sit between them.
#[tokio::test]
async fn window_keeps_the_most_recent_turns() {
    let mut events = Vec::new();
    for n in 0..10u64 {
        events.push(operator(n, Some("general"), &format!("m{n}")));
    }
    let log = FixedLog(events);

    let seed = seed_of(log, "general", "general", 3, "").await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: m7".to_string()),
            ("user".to_string(), "operator: m8".to_string()),
            ("user".to_string(), "operator: m9".to_string()),
        ],
        "the three newest owning turns, in chronological order"
    );
}

/// Codex review finding (P1): the chat route journals an operator message
/// the instant it is accepted, before it queues on the per-company cycle
/// lock — so two messages for the same desk accepted close together are
/// both already in the journal by the time either turn's seed projection
/// actually runs. Scanning the unbounded tail let the FIRST message's turn
/// seed the SECOND message too, as if it were prior history — and because
/// the second message's text never matches the first turn's own text,
/// `strip_current_message` cannot remove it either. The seed for "my
/// message"'s turn must stop at its own boundary: everything journaled
/// after it is excluded, not just everything after the log's current tail.
#[tokio::test]
async fn a_concurrently_journaled_later_message_is_excluded_from_the_seed() {
    let log = FixedLog(vec![
        operator(0, Some("general"), "earlier turn"),
        reply(1, "general", "earlier reply"),
        operator(2, Some("general"), "my message"),
        // Accepted by the chat route microseconds later, before either
        // turn won this desk's per-company cycle lock — same shape as two
        // browser tabs firing at once.
        operator(3, Some("general"), "a second, concurrent message"),
    ]);

    let seed = seed_of(log, "general", "general", CHAT_SEED_WINDOW, "my message").await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: earlier turn".to_string()),
            ("agent".to_string(), "earlier reply".to_string()),
            ("user".to_string(), "operator: my message".to_string()),
        ],
        "the later, concurrently-journaled message must not appear as \
         prior history for the earlier message's own turn: {seed:?}"
    );
}

/// The self-boundary in [`build_chat_seed`] degrades to the unbounded-tail
/// behaviour when it is never matched — a message with no owning entry in
/// this desk's log at all — rather than silently emptying the seed. This
/// is the fallback path every other test in this module exercises via
/// `seed_of`'s `current_message: ""`; this test names it explicitly with a
/// non-empty, non-matching message so the fallback is proven on its own
/// terms rather than only incidentally through the empty-string case.
#[tokio::test]
async fn an_unmatched_boundary_falls_back_to_the_unbounded_tail() {
    let log = FixedLog(vec![
        operator(0, Some("general"), "u1"),
        reply(1, "general", "a1"),
    ]);

    let seed = seed_of(
        log,
        "general",
        "general",
        CHAT_SEED_WINDOW,
        "no journaled message matches this text",
    )
    .await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: u1".to_string()),
            ("agent".to_string(), "a1".to_string()),
        ],
        "an unmatched boundary must not come back emptier than the \
         unbounded scan did: {seed:?}"
    );
}

/// The reported collision. Two messages land on one desk before either
/// turn takes the cycle lock, and the LATER one's text is an exact prefix
/// of this turn's — `"deploy"` against `"deploy production"`. A prefix
/// compare walking newest-first meets the later event first and accepts it
/// as this turn's own boundary, which leaves the real message inside the
/// history and `strip_current_message` (trailing entry only) unable to see
/// it: `run_single` then appends the request a second time.
///
/// Anchored on the seq the boundary is this turn's message and no other,
/// whatever anyone else's words are.
#[tokio::test]
async fn a_later_prefix_message_does_not_steal_this_turns_boundary() {
    let log = FixedLog(vec![
        operator(1, Some("general"), "hello"),
        reply(2, "general", "hi"),
        operator(3, Some("general"), "deploy production"),
        // Accepted microseconds later, already journaled, and a strict
        // prefix of the message above.
        operator(4, Some("general"), "deploy"),
    ]);

    let seed = seed_anchored(log, 3).await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: hello".to_string()),
            ("agent".to_string(), "hi".to_string()),
        ],
        "the boundary must land on seq 3 and seed neither it nor the \
         later prefix message: {seed:?}"
    );
    assert!(
        !seed.iter().any(|(_, text)| text == "deploy production"),
        "this turn's own request must not be seeded as history — \
         `run_single` appends it itself: {seed:?}"
    );
}

/// The same collision with the two messages spelled identically, which is
/// the degenerate prefix: nothing in the text can order them at all.
#[tokio::test]
async fn an_identically_worded_later_message_does_not_steal_the_boundary() {
    let log = FixedLog(vec![
        operator(1, Some("general"), "status?"),
        reply(2, "general", "all green"),
        operator(3, Some("general"), "deploy"),
        operator(4, Some("general"), "deploy"),
    ]);

    let seed = seed_anchored(log, 3).await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: status?".to_string()),
            ("agent".to_string(), "all green".to_string()),
        ],
        "identical wording orders nothing; the seq does: {seed:?}"
    );
}

/// The mirror the text compare already got right — the later message is
/// LONGER, so it never prefix-matched this turn's shorter one. Pinned so
/// the identity boundary is shown to answer it the same way rather than
/// only fixing the direction that was broken.
#[tokio::test]
async fn a_longer_later_message_is_still_excluded() {
    let log = FixedLog(vec![
        operator(1, Some("general"), "hello"),
        operator(2, Some("general"), "deploy"),
        operator(3, Some("general"), "deploy production"),
    ]);

    let seed = seed_anchored(log, 2).await;

    assert_eq!(
        seed,
        vec![("user".to_string(), "operator: hello".to_string())],
        "only what precedes this turn's own message: {seed:?}"
    );
}

/// This turn's message is the only thing the desk has ever held. The seed
/// is empty rather than a copy of the request the runner is about to
/// append anyway.
#[tokio::test]
async fn a_first_message_seeds_nothing() {
    let log = FixedLog(vec![operator(1, Some("general"), "deploy production")]);

    let seed = seed_anchored(log, 1).await;

    assert!(
        seed.is_empty(),
        "nothing precedes the first message: {seed:?}"
    );
}

/// A genuine older line whose text prefixes this turn's message is
/// history, not a duplicate — the ambiguity running in the other
/// direction. It survives, and `ChatSeedRequest::build` is what keeps it
/// there: on the seq path the boundary was never seeded, so there is
/// nothing for `strip_current_message` to remove and running it anyway
/// would take this line instead.
#[tokio::test]
async fn an_older_prefix_line_is_history_and_survives() {
    let log = FixedLog(vec![
        reply(1, "general", "morning"),
        operator(2, Some("general"), "deploy"),
        operator(3, Some("general"), "deploy production"),
    ]);

    let events: Arc<dyn EventLog> = Arc::new(log);
    let mut entries = build_seed_entries(
        &events,
        &CompanyId::new("acme"),
        "general",
        "general",
        VIEWER,
        None,
        CHAT_SEED_WINDOW,
        SelfBoundary::Seq(EventSeq::new(3)),
    )
    .await;

    assert_eq!(
        flattened(entries.clone()),
        vec![
            ("agent".to_string(), "morning".to_string()),
            ("user".to_string(), "operator: deploy".to_string()),
        ],
        "the older `deploy` is this desk's history"
    );

    // What `build` would do if it ran the text strip on this path anyway —
    // named here so the guard has a failing shape to point at. The strip
    // reads the raw entry text, so `"deploy"` is still a prefix of
    // `"deploy production"` and the older line still looks like a
    // duplicate: labelling the operator changed the rendering, not this
    // hazard, which is why the seq path still must not run the strip.
    strip_current_message(&mut entries, "deploy production");
    assert_eq!(
        flattened(entries),
        vec![("agent".to_string(), "morning".to_string())],
        "the trailing prefix line is indistinguishable from a duplicate to \
         a text compare, which is why the seq path does not run it"
    );
}

/// The compatibility path: with no seq to anchor on, the boundary is the
/// text compare, unchanged. A caller outside a cycle (a test builder, a
/// request built without one) keeps exactly the behaviour it had.
#[tokio::test]
async fn without_a_seq_the_text_boundary_still_bounds_the_scan() {
    let log = FixedLog(vec![
        operator(1, Some("general"), "hello"),
        reply(2, "general", "hi"),
        operator(3, Some("general"), "deploy production"),
    ]);
    let events: Arc<dyn EventLog> = Arc::new(log);

    let seed = build_chat_seed(
        &events,
        &CompanyId::new("acme"),
        "general",
        "general",
        VIEWER,
        None,
        CHAT_SEED_WINDOW,
        SelfBoundary::Text("deploy production"),
    )
    .await;

    assert_eq!(
        seed,
        vec![
            ("user".to_string(), "operator: hello".to_string()),
            ("agent".to_string(), "hi".to_string()),
            (
                "user".to_string(),
                "operator: deploy production".to_string()
            ),
        ],
        "the text path still collects its own boundary and leaves the \
         removal to `strip_current_message`: {seed:?}"
    );
}

/// Two live threads in ONE channel. The turn answering inside thread A must
/// see thread A's exchange and nothing of thread B's — the leak #1890 opens
/// with, where "make it shorter" arrived directly after an unrelated CAC
/// answer because the projection was scoped to the channel.
#[tokio::test]
async fn a_thread_sees_only_its_own_exchange() {
    let log = FixedLog(vec![
        operator(41, Some("growth"), "draft the launch email"), // root A
        reply_in(42, "growth", "here is a draft", 41),
        operator(43, Some("growth"), "what's our Q3 CAC?"), // root B
        reply_in(44, "growth", "$412, up 18%", 43),
        operator_in(45, Some("growth"), "make it shorter", 41),
    ]);
    let seed = seed_of_thread(log, "growth", Some(41), "make it shorter").await;
    assert_eq!(
        seed,
        vec![
            (
                "user".to_string(),
                "operator: draft the launch email".to_string()
            ),
            ("agent".to_string(), "here is a draft".to_string()),
            ("user".to_string(), "operator: make it shorter".to_string()),
        ],
        "thread A's seed must not carry thread B's turns: {seed:?}"
    );
}

/// The channel-level conversation is **roots plus each root's first
/// reply** (issue #1890 D part 3), not unparented lines only.
///
/// Part 1 threads every answer under the message that opened it, so
/// "unparented lines only" — what this asserted before — leaves the channel
/// seeding a run of questions with no answers: emptied for the model
/// exactly as folding every reply empties it on screen. One answer per
/// question is what the channel now *shows*, since part 2 renders precisely
/// that inline, so it is what the channel says too.
///
/// The follow-up typed inside the thread stays out. That is the line
/// between "the channel can see its own answers" and the pre-#1890-A leak:
/// the channel gets the exchange that opened each topic, never the topic's
/// whole body.
#[tokio::test]
async fn the_channel_sees_roots_and_their_first_replies() {
    let log = FixedLog(vec![
        operator(41, Some("growth"), "draft the launch email"),
        reply_in(42, "growth", "here is a draft", 41),
        operator_in(43, Some("growth"), "THREAD-FOLLOWUP", 41),
        reply_in(44, "growth", "SECOND-REPLY", 41),
        operator(45, Some("growth"), "unrelated channel line"),
    ]);
    let seed = seed_of_thread(log, "growth", None, "").await;
    assert_eq!(
        seed,
        vec![
            (
                "user".to_string(),
                "operator: draft the launch email".to_string()
            ),
            ("agent".to_string(), "here is a draft".to_string()),
            (
                "user".to_string(),
                "operator: unrelated channel line".to_string()
            ),
        ],
        "roots plus the FIRST reply each — never the thread's body: {seed:?}"
    );
}

/// The channel keeps the **agent's** reply, not whichever parented line
/// came first.
///
/// Deduping on the parent alone kept the operator's own follow-up when one
/// preceded the answer, so the channel seeded a question, the operator
/// asking again, and no reply at all — while the agent's actual answer was
/// dropped as a duplicate (coderabbit on #1972). A follow-up is thread
/// body; it belongs to the thread's seed, never to the channel's.
#[tokio::test]
async fn the_channel_keeps_the_agents_reply_not_a_follow_up() {
    let log = FixedLog(vec![
        operator(41, Some("growth"), "draft the launch email"),
        operator_in(42, Some("growth"), "THREAD-FOLLOWUP", 41),
        reply_in(43, "growth", "here is a draft", 41),
    ]);
    let seed = seed_of_thread(log, "growth", None, "").await;
    assert_eq!(
        seed,
        vec![
            (
                "user".to_string(),
                "operator: draft the launch email".to_string()
            ),
            ("agent".to_string(), "here is a draft".to_string()),
        ],
        "the answer, not the operator asking twice: {seed:?}"
    );
}

/// The narrowing is the channel's rule alone. A turn answering inside a
/// thread needs that thread's whole exchange; handing it the question and
/// one reply out of several would be a worse seed than the leak #1890 A
/// closed.
#[tokio::test]
async fn a_thread_still_sees_its_whole_exchange() {
    let log = FixedLog(vec![
        operator(41, Some("growth"), "draft the launch email"),
        reply_in(42, "growth", "here is a draft", 41),
        operator_in(43, Some("growth"), "make it shorter", 41),
        reply_in(44, "growth", "shortened", 41),
    ]);
    let seed = seed_of_thread(log, "growth", Some(41), "").await;
    assert_eq!(
        seed,
        vec![
            (
                "user".to_string(),
                "operator: draft the launch email".to_string()
            ),
            ("agent".to_string(), "here is a draft".to_string()),
            ("user".to_string(), "operator: make it shorter".to_string()),
            ("agent".to_string(), "shortened".to_string()),
        ],
        "every turn in the thread, not just its first reply: {seed:?}"
    );
}
