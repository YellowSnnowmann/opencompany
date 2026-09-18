use super::tests_core2::*;

// ── path three: the relay turn's discard ────────────────────────────────

/// A card the relay turn opens is no longer swallowed. The relay runs after
/// the desk answered, which is exactly when the orchestrator is best placed
/// to decide something should be followed up — and that decision used to be
/// dropped by design.
///
/// Driven through [`Turn::tooling`] on purpose (issue #267, review round 2):
/// the relay's `spawn_task` goes through the real
/// [`DelegationQueue::push_within_cap`] boundary, so this pins that the
/// board write is *accepted* there rather than only that a delegation pushed
/// onto the queue behind the boundary's back gets executed. The fixture
/// message is deliberately not a question — the sibling below is the other
/// half.
#[tokio::test]
async fn the_relay_turn_can_no_longer_lose_a_card() {
    let instruction = "the pricing repo needs a map";
    assert!(
        !crate::company::task_intent::triage_message(instruction).is_answer(),
        "the fixture must NOT be a question, or #267 holds the relay's card back \
         and this proves nothing about #442"
    );
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling("asking", vec![handoff("what's the status of the build?")]),
            Turn::reply("it's red — someone should fix the flaky test"),
            Turn::tooling(
                "it's red; I've opened a card",
                vec![Delegation::SpawnTask {
                    title: "Fix the flaky test".to_string(),
                    note: None,
                    assignee: None,
                }],
            ),
        ],
    );
    fx.runner(&turns)
        .handle_operator_message("chief", instruction, None)
        .await
        .expect("operator message handled");

    assert_eq!(
        turns.staged(),
        vec![orchestrator::Staged::Queued, orchestrator::Staged::Queued],
        "the relay turn's board write is accepted at the tool boundary, not \
         merely executed once past it"
    );
    let cards = fx.cards().await;
    assert!(
        cards.iter().any(|c| c.title == "Fix the flaky test"),
        "the relay's card survives: {cards:?}"
    );
}

/// …and the other half: on a **question** the relay's board write is held
/// back too, refused at the boundary with the triage named as the cause.
///
/// This is the #442 × #267 interaction, and it is deliberate rather than an
/// oversight. #442 restored the relay's board writes because a card the
/// relay opens is not a re-delegation — it is the orchestrator deciding,
/// having now seen what came back, that something should be tracked. #267
/// says a message the operator posed as a question mints no card by *any*
/// door, and the relay turn is a door: it runs under the same live
/// `claim_answering` as the turn that asked, so the narrowing reaches it.
/// Answering "is the build ok?" with a card nobody asked for is exactly the
/// behaviour #267 exists to stop, and the relay seeing the answer first does
/// not change who asked.
///
/// The refusal is [`orchestrator::NoDrainReason::Triage`], not `Unwired` —
/// the claim is live throughout, so the relay is told *this message* is a
/// question rather than that its context can never do board work.
#[tokio::test]
async fn the_relay_turns_card_is_held_back_on_a_question_turn() {
    let question = "is the build ok?";
    assert!(
        crate::company::task_intent::triage_message(question).is_answer(),
        "fixture must triage as a question, or this proves nothing"
    );
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling("asking", vec![handoff("what's the status of the build?")]),
            Turn::reply("it's red — someone should fix the flaky test"),
            Turn::tooling(
                "it's red; I've opened a card",
                vec![Delegation::SpawnTask {
                    title: "Fix the flaky test".to_string(),
                    note: None,
                    assignee: None,
                }],
            ),
        ],
    );
    let turn = fx
        .runner(&turns)
        .handle_operator_message("chief", question, None)
        .await
        .expect("operator message handled");

    assert_eq!(
        turns.claim_at_turn(2),
        orchestrator::DrainClaim::Answering,
        "the answering claim is still live for the relay turn"
    );
    assert_eq!(
        turns.staged(),
        vec![
            orchestrator::Staged::Queued,
            orchestrator::Staged::NoDrain(orchestrator::NoDrainReason::Triage),
        ],
        "the hand-off answers and stages; the relay's card is refused, and the \
         refusal names the triage rather than blaming an unwired context"
    );
    assert!(
        fx.cards().await.is_empty(),
        "a question minted a card through the relay"
    );
    assert!(turn.spawned_task.is_none());
}

/// …while the rule the discard existed for still holds: a relay may relay,
/// never re-delegate. A hand-off queued by the relay turn is dropped, so
/// there is no second desk turn and no loop.
///
/// Through the real boundary too: the relay's hand-off *stages* — it
/// answers, so even the narrowed claim admits it — and is dropped at the
/// drain by [`HandOffs::Drop`]. Which is the point: the loop is stopped by
/// the drain's own rule and not incidentally by #267's gate, so the bound
/// survives on a non-question message as well.
#[tokio::test]
async fn the_relay_turn_still_cannot_re_delegate() {
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling("asking", vec![handoff("what's the status of the build?")]),
            Turn::reply("green"),
            Turn::tooling("relaying", vec![handoff("now write the release notes")]),
        ],
    );
    fx.runner(&turns)
        .handle_operator_message("chief", "is the build ok?", None)
        .await
        .expect("operator message handled");

    assert_eq!(
        turns.staged(),
        vec![orchestrator::Staged::Queued, orchestrator::Staged::Queued],
        "the relay's hand-off is not refused at the boundary — it is dropped at \
         the drain, which is what bounds the turn count"
    );
    // Three turns total — orchestrator, desk, relay. A fourth would be the
    // re-delegation the drain exists to prevent.
    let calls = turns.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[2].0, "chief", "the last turn is the relay: {calls:?}");
    assert!(
        fx.cards().await.is_empty(),
        "a dropped hand-off opens no card either"
    );
}

// ── Issue #176: recursive desk-member delegation ────────────────────────

/// The whole point of the slice: a desk lead handed work by the
/// orchestrator hands a slice on to a second desk, that second lead's turn
/// really runs, and its answer arrives folded into the first lead's reply
/// rather than lost.
///
/// Without the nested drain this is #453's failure one level down — the
/// member's tool told it the hand-off "will be answered this turn", the
/// delegation sat in the queue, and the next `clear()` destroyed it while
/// the member reported it as done.
///
/// `Turn::tooling`, not `Turn::queueing`: the escape hatch bypasses
/// `push_within_cap` entirely and would assert nothing about the gate the
/// depth bound lives on.
#[tokio::test]
async fn a_desk_lead_can_hand_a_slice_on_and_the_answer_comes_back() {
    let fx = Fixture::nested();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            // 0 — the orchestrator hands the work to engineering.
            Turn::tooling("handing it to engineering", vec![handoff("ship the API")]),
            // 1 — the engineering lead does its part and hands a slice on.
            Turn::tooling(
                "I built it; asking research about the rate limits",
                vec![nested_handoff("what rate limits do competitors use?")],
            ),
            // 2 — the research lead answers.
            Turn::reply("everyone lands around 100 rps"),
            // 3 — the CEO relay.
            Turn::reply("Built, and research says ~100 rps is the norm."),
        ],
    );

    let out = fx
        .runner(&turns)
        .handle_operator_message("chief", "ship the API", Some("general"))
        .await
        .expect("operator message handled");

    let calls = turns.calls();
    assert_eq!(
        calls.len(),
        4,
        "four turns: chief, engineer, researcher, relay: {calls:?}"
    );
    assert_eq!(calls[1].0, "engineer", "{calls:?}");
    assert_eq!(
        calls[2].0, "researcher",
        "the nested hand-off must actually run the second lead's turn: {calls:?}"
    );
    assert_eq!(calls[3].0, "chief", "{calls:?}");
    assert_eq!(
        turns.staged(),
        vec![orchestrator::Staged::Queued, orchestrator::Staged::Queued],
        "both hand-offs passed the tool boundary"
    );

    // The nested answer reaches the relay folded into the engineer's reply,
    // attributed to who said it and who asked them — one bubble, not two.
    let relay_prompt = &calls[3].1;
    assert!(
        relay_prompt.contains("everyone lands around 100 rps"),
        "the nested answer must reach the relay: {relay_prompt}"
    );
    assert!(
        relay_prompt.contains("researcher (delegated by engineer) replied"),
        "the fold must name both ends of the chain: {relay_prompt}"
    );
    assert_eq!(
        out.reply, "Built, and research says ~100 rps is the norm.",
        "the operator still gets ONE coherent answer from the relay"
    );

    // The card the hand-off opened is settled from the folded reply, so the
    // board carries the nested answer too.
    let cards = fx.cards().await;
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0].assignee, "engineer", "{cards:?}");
    assert!(
        cards[0]
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("everyone lands around 100 rps"),
        "the nested answer must be on the card note: {:?}",
        cards[0].note
    );
}

// ── issue #1032: the spend halt folds like the answer does ──────────────

/// **The plumbing this issue is really about.** A delegate two levels down
/// runs out of money, and the operator is told — because its halt folds into
/// the member's answer exactly as its reply and steps already do.
///
/// The researcher's answer does not surface as its own bubble: it is folded
/// into the engineer's reply, which the CEO relay then *replaces* with one
/// coherent sentence. So there are two places the halt can be dropped
/// silently — the nested fold in `run_hand_off`, and the relay overwrite in
/// `handle_operator_message` — and either one leaves the operator reading a
/// confident answer whose missing half was cut for spend.
#[tokio::test]
async fn a_nested_delegates_spend_halt_reaches_the_operator_turn() {
    let fx = Fixture::nested();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling("handing it to engineering", vec![handoff("ship the API")]),
            Turn::tooling(
                "I built it; asking research about the rate limits",
                vec![nested_handoff("what rate limits do competitors use?")],
            ),
            // Two levels down, and out of money partway through.
            Turn::spend_halted("I got as far as two competitors", "researcher", 4.02, 4.0),
            Turn::reply("Built. Research is partial."),
        ],
    );

    let out = fx
        .runner(&turns)
        .handle_operator_message("chief", "ship the API", Some("general"))
        .await
        .expect("operator message handled");

    let halt = out
        .halted_for_spend
        .expect("a halt two levels down must reach the operator bubble");
    assert_eq!(
        halt.agent, "researcher",
        "the notice must name the teammate that actually ran out, not the one relaying it"
    );
    assert_eq!(halt.cap_usd, 4.0);
    assert_eq!(halt.spent_usd, 4.02);
    // The relay really did replace the reply — so the halt survived an
    // overwrite rather than riding along on text that happened to persist.
    assert_eq!(out.reply, "Built. Research is partial.");
}

/// The negative control the test above needs: the same four-turn chain with
/// nobody halted reports no halt.
///
/// Without this, `halted_for_spend` wired to a hardcoded `Some` would pass
/// every other assertion in this file.
#[tokio::test]
async fn a_chain_where_nobody_ran_out_reports_no_spend_halt() {
    let fx = Fixture::nested();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling("handing it to engineering", vec![handoff("ship the API")]),
            Turn::tooling(
                "I built it; asking research about the rate limits",
                vec![nested_handoff("what rate limits do competitors use?")],
            ),
            Turn::reply("everyone lands around 100 rps"),
            Turn::reply("Built, and research says ~100 rps is the norm."),
        ],
    );

    let out = fx
        .runner(&turns)
        .handle_operator_message("chief", "ship the API", Some("general"))
        .await
        .expect("operator message handled");

    assert!(
        out.halted_for_spend.is_none(),
        "a notice that fires on every turn is as useless as one that never fires: {:?}",
        out.halted_for_spend
    );
}

// ── issue #1846: the budget pause folds like the spend halt does ────────

/// The delegation-fold analogue of
/// [`a_nested_delegates_spend_halt_reaches_the_operator_turn`]: a delegate
/// two levels down pauses for lack of inference budget/credits, and the
/// operator is told — because the pause folds into the member's answer
/// exactly as its reply and steps already do.
///
/// Issue #1846 review (Codex #3870516681): the pause no longer survives a
/// relay OVERWRITE, because there is no relay turn to overwrite it. A desk
/// that paused has not answered, so the relay is skipped (see
/// [`a_delegates_budget_pause_does_not_launch_the_ceo_relay`]). This test
/// scripts only the three turns that actually run: a fourth would be the
/// relay, and `ScriptedTurns` running dry is how a regression surfaces.
///
/// Issue #1906: "reaches the operator turn" is about `budget_paused`, the
/// field the caller builds its notice from — not about the reply text. The
/// delegates' own words do NOT ride the bubble on this path and never did;
/// see the assertions below.
#[tokio::test]
async fn a_nested_delegates_budget_pause_reaches_the_operator_turn() {
    let fx = Fixture::nested();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling("handing it to engineering", vec![handoff("ship the API")]),
            Turn::tooling(
                "I built it; asking research about the rate limits",
                vec![nested_handoff("what rate limits do competitors use?")],
            ),
            // Two levels down, and out of inference credits partway through.
            Turn::budget_paused(
                "Paused — researcher's turn ran out of inference budget/credits.",
                "researcher",
                "Paused — researcher's turn ran out of inference budget/credits, so it \
                 stopped instead of failing silently. Add credits to your account, then \
                 resend your message to continue.",
            ),
        ],
    );

    let out = fx
        .runner(&turns)
        .handle_operator_message("chief", "ship the API", Some("general"))
        .await
        .expect("operator message handled");

    let pause = out
        .budget_paused
        .expect("a pause two levels down must reach the operator bubble");
    assert_eq!(
        pause.agent, "researcher",
        "the notice must name the teammate that actually paused, not the one relaying it"
    );
    assert!(pause.summary.contains("add credits") || pause.summary.contains("Add credits"));
    // Issue #1906: the bubble is the RESPONDER's own text, untouched. The
    // three assertions that used to stand here required the delegates'
    // replies to be folded onto it — a property production never had, since
    // `HarnessBrain::handle_operator_message` replaces the whole reply with
    // `BUDGET_PAUSED_PLACEHOLDER_REPLY` on any pause. Pinning the absence
    // instead is what keeps the fold from being reintroduced on the strength
    // of a rationale that reads plausible and is not true.
    assert_eq!(
        out.reply, "handing it to engineering",
        "the skipped relay leaves the responder's own reply alone: {}",
        out.reply
    );
    assert!(
        !out.reply.contains("replied:"),
        "no fold: `build_relay_prompt`'s shape on the operator bubble is text the caller \
         discards, and appending it only makes the code read as though the operator sees \
         it: {}",
        out.reply
    );
}

/// Issue #1846 review (Codex #3864988176): the marker a delegated pause
/// parks must carry the OPERATOR's own words, not the model-generated
/// hand-off instruction — `run_inner` (the harness pool, exercised only by
/// a real model turn) parks whatever it was CALLED with, which for a
/// nested hand-off is `researcher`'s instruction ("what rate limits do
/// competitors use?"), not "ship the API". Redeeming that wrong marker
/// would re-dispatch the instruction as a brand-new operator message,
/// silently running a different task than the one the operator asked for.
///
/// This exercises the DELEGATION-LAYER half of the fix — the re-park in
/// `run_hand_off` keyed on `reissue_message` — which is exactly what
/// `ScriptedTurns` (a fake `RunTurn`) CAN prove, since the real park lives
/// one layer down in `run_inner` where only a live model turn reaches it.
#[tokio::test]
async fn a_delegated_budget_pause_parks_the_operators_words_not_the_handoff_instruction() {
    let fx = Fixture::nested();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling("handing it to engineering", vec![handoff("ship the API")]),
            Turn::tooling(
                "I built it; asking research about the rate limits",
                vec![nested_handoff("what rate limits do competitors use?")],
            ),
            Turn::budget_paused(
                "Paused — researcher's turn ran out of inference budget/credits.",
                "researcher",
                "Paused — researcher's turn ran out of inference budget/credits, so it \
                 stopped instead of failing silently.",
            ),
            Turn::reply("Built. Research is paused for credits."),
        ],
    );

    let out = fx
        .runner(&turns)
        // The production wiring in `brain.rs` sets this from the operator's
        // own composed message before calling `handle_operator_message`.
        .reissue_message("ship the API")
        .handle_operator_message("chief", "ship the API", Some("general"))
        .await
        .expect("operator message handled");
    assert!(
        out.budget_paused.is_some(),
        "sanity: the pause still folds through"
    );

    let marker = crate::runtime::grants::budget_pauses_for(&fx.record.id)
        .peek("researcher")
        .expect("a marker was parked for the paused delegate");
    assert_eq!(
        marker.message, "ship the API",
        "the marker must carry the OPERATOR's original words — a redeem re-dispatches \
         `marker.message` verbatim as a fresh operator message, so parking the nested \
         hand-off's own instruction here would silently run a different task"
    );
}

/// The DELEGATION-LAYER re-park (`run_hand_off`, same call site as the
/// test above) also stamps the marker with the ambient `RedeemContext` a
/// cycle sets around it — issue #1846 review, Codex
/// #3865812419/#3865812423/#3865812432. Same fixture, wrapped in
/// `with_redeem_context` the way `CycleRunner::run_bracketed` does in
/// production, with a non-default parent/deliverable/mentions to prove
/// they land on the marker instead of being silently dropped the way
/// the pre-fix `redeem_budget_pause` dropped them on the OTHER side of a
/// redeem.
#[tokio::test]
async fn a_delegated_budget_pause_parks_the_ambient_redeem_context() {
    use crate::ports::types::{Attachment, EventSeq, Mention, MentionTarget, MessageIntent};

    // A fixture over `nested_record()`'s manifest, but NOT `Fixture::nested()`
    // itself: that helper hardcodes `CompanyId::new("acme")`, which is
    // exactly the fixture the sibling test above also runs under, parking
    // under the same "researcher" agent. `BudgetPauseSet` is a single
    // registry keyed globally by company id (`budget_pauses_for`), and
    // Rust runs tests in parallel by default — sharing that key with a
    // concurrently-running test would let either test's `park()` overwrite
    // the other's marker (last-write-wins, by design — see
    // `a_second_pause_on_the_same_agent_overwrites_the_first` in
    // `grants.rs`), making this test's assertions racy against a test it
    // has no other relationship to. A private company id sidesteps that
    // without touching the shared `nested_record()`/`Fixture::nested()`
    // helpers every other test in this module also relies on.
    let mut record = nested_record();
    record.id = CompanyId::new("acme-delegated-redeem-context");
    let fx = Fixture::over(record);
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling("handing it to engineering", vec![handoff("ship the API")]),
            Turn::tooling(
                "I built it; asking research about the rate limits",
                vec![nested_handoff("what rate limits do competitors use?")],
            ),
            Turn::budget_paused(
                "Paused — researcher's turn ran out of inference budget/credits.",
                "researcher",
                "Paused — researcher's turn ran out of inference budget/credits, so it \
                 stopped instead of failing silently.",
            ),
            Turn::reply("Built. Research is paused for credits."),
        ],
    );

    // Issue #1846 review (Codex #3866418891): `text`/`attachments` are the
    // same "raw operator message" pair `park_message` prefers over the
    // delegated turn's own COMPOSED `message`/`original` — assert they
    // reach the marker through this call site too, not just the
    // top-level one `redeem_replays_the_markers_attachments` covers.
    let redeem = crate::runtime::grants::RedeemContext {
        parent: Some(EventSeq::new(7)),
        deliverable: Some(MessageIntent::Once),
        mentions: vec![Mention {
            target: MentionTarget::Agent {
                id: "engineering".to_string(),
            },
            text: "@engineering".to_string(),
            offset: 0,
            quiet: false,
        }],
        text: Some("ship the API, and see the attached spec".to_string()),
        attachments: vec![Attachment {
            node_id: "node-delegated-1".to_string(),
            name: "spec.pdf".to_string(),
            mime: "application/pdf".to_string(),
            size: 2048,
            extracted_text: Some("API spec v2".to_string()),
        }],
    };

    let out = crate::runtime::grants::with_redeem_context(redeem.clone(), async {
        fx.runner(&turns)
            .reissue_message("ship the API")
            .handle_operator_message("chief", "ship the API", Some("general"))
            .await
            .expect("operator message handled")
    })
    .await;
    assert!(
        out.budget_paused.is_some(),
        "sanity: the pause still folds through"
    );

    let marker = crate::runtime::grants::budget_pauses_for(&fx.record.id)
        .peek("researcher")
        .expect("a marker was parked for the paused delegate");
    assert_eq!(
        marker.parent, redeem.parent,
        "the marker must carry the ambient cycle's thread parent"
    );
    assert_eq!(
        marker.deliverable, redeem.deliverable,
        "the marker must carry the ambient cycle's deliverable choice"
    );
    assert_eq!(
        marker.mentions, redeem.mentions,
        "the marker must carry the ambient cycle's resolved mentions"
    );
    assert_eq!(
        marker.message,
        redeem.text.clone().unwrap(),
        "the marker must carry the ambient context's RAW text, not the delegated turn's \
         own composed message"
    );
    assert_eq!(
        marker.attachments, redeem.attachments,
        "the marker must carry the ambient context's structured attachments"
    );
}

/// The negative control the test above needs: the same four-turn chain
/// with nobody paused reports no budget pause.
///
/// Without this, `budget_paused` wired to a hardcoded `Some` would pass
/// every other assertion in this file.
#[tokio::test]
async fn a_chain_where_nobody_paused_reports_no_budget_pause() {
    let fx = Fixture::nested();
    let turns = ScriptedTurns::new(
        &fx,
        vec![
            Turn::tooling("handing it to engineering", vec![handoff("ship the API")]),
            Turn::tooling(
                "I built it; asking research about the rate limits",
                vec![nested_handoff("what rate limits do competitors use?")],
            ),
            Turn::reply("everyone lands around 100 rps"),
            Turn::reply("Built, and research says ~100 rps is the norm."),
        ],
    );

    let out = fx
        .runner(&turns)
        .handle_operator_message("chief", "ship the API", Some("general"))
        .await
        .expect("operator message handled");

    assert!(
        out.budget_paused.is_none(),
        "a notice that fires on every turn is as useless as one that never fires: {:?}",
        out.budget_paused
    );
}
