//! Tests for the episode watermark divider rendered into a transcript.

use tinyhivemind_hive::{Sequence, SessionAuthor};

use super::super::*;
use super::fixtures::*;

/// One transcript row, as the projection hands it to a prompt.
fn message(sequence: u64, author: &str, content: &str) -> tinyhivemind_hive::SessionMessage {
    tinyhivemind_hive::SessionMessage {
        sequence: Sequence(sequence),
        author: SessionAuthor::Agent {
            id: author.to_owned(),
            label: author.to_owned(),
        },
        content: content.to_owned(),
        audience: tinyhivemind_hive::aside::Audience::Desk,
        elided: None,
    }
}

/// One authorized turn, for rendering a prompt without driving an episode.
fn hive_turn(
    agent: &str,
    visibility: tinyhivemind_hive::Visibility,
    watermark: Sequence,
) -> tinyhivemind_hive::HiveTurn {
    tinyhivemind_hive::HiveTurn {
        agent_id: agent.to_owned(),
        phase: tinyhivemind_hive::Phase::Deliberate,
        visibility,
        reason: tinyhivemind_hive::BidReason::Salience,
        watermark,
        // A round of one: nothing was authored concurrently with this
        // turn, so the round boundary sits above every row and withholds
        // nothing — which is what made a width-one round bit-identical to
        // the sequential episode this fixture was written against.
        round_start: tinyhivemind_hive::Sequence(u64::MAX),
    }
}

#[test]
fn a_transcript_spanning_the_watermark_renders_the_divider_between_episodes() {
    // The live shape: a prior episode's propose/support pair sits below the
    // trigger, and this episode's own pair sits above it.
    let desk = desk_of(&three_member_manifest(), "eng").expect("a room");
    let member = desk.member("planner").expect("planner is seated").clone();
    let quorum = desk.policy().quorum;
    let visible = [
        message(1, "scout", "!propose #euler249 Old guess."),
        message(2, "scout", "!support #euler249 ^1 Because it is odd."),
        message(3, "planner", "!propose #euler301 New guess."),
        message(4, "critic", "!support #euler301 ^3 Because it holds."),
    ];

    let turn = hive_turn("planner", tinyhivemind_hive::Visibility::Full, Sequence(0));
    let prompt = EpisodePrompt::new(&member, &desk, "Decide the answer.", quorum, &[])
        .with_trigger(Sequence(2))
        .render(&turn, &visible);

    let prior_end = prompt.find("[2] scout").expect("prior row 2 is rendered");
    let divider_at = prompt
        .find("this episode's floor")
        .expect("the divider names its own meaning");
    // `planner` is the member this prompt is for, so its own row is marked
    // `(you)` — the distinction that stops a member describing itself in the
    // third person, without dropping the id colleagues cite it by.
    let current_start = prompt
        .find("[3] planner (you)")
        .expect("current row 3 is rendered, as the reader's own");
    assert!(prior_end < divider_at, "{prompt}");
    assert!(divider_at < current_start, "{prompt}");
    // Sequence numbers stay exactly as the projection assigned them, on both
    // sides of the divider.
    // `planner` reads its own row marked `(you)`; the rest keep their names.
    for needle in ["[1] scout", "[2] scout", "[3] planner (you)", "[4] critic"] {
        assert!(prompt.contains(needle), "{needle} missing:\n{prompt}");
    }
}

#[test]
fn a_transcript_entirely_after_the_trigger_renders_with_no_divider() {
    let desk = desk_of(&three_member_manifest(), "eng").expect("a room");
    let member = desk.member("planner").expect("planner is seated").clone();
    let quorum = desk.policy().quorum;
    let visible = [message(3, "planner", "!propose #stage Stage it.")];
    let turn = hive_turn("planner", tinyhivemind_hive::Visibility::Full, Sequence(0));

    let without_trigger = EpisodePrompt::new(&member, &desk, "Decide the rollout.", quorum, &[])
        .render(&turn, &visible);
    let with_trigger_below_everything =
        EpisodePrompt::new(&member, &desk, "Decide the rollout.", quorum, &[])
            .with_trigger(Sequence(1))
            .render(&turn, &visible);

    assert_eq!(
        without_trigger, with_trigger_below_everything,
        "nothing precedes the watermark, so there is nothing to divide"
    );
    assert!(
        !without_trigger.contains("this episode's floor"),
        "{without_trigger}"
    );
}

#[test]
fn a_blind_turn_still_hides_only_this_episodes_peers_not_prior_context() {
    // [1] predates the trigger and stays visible even blind. [2] is the
    // turn-holder's own line and stays visible for the same reason a blind
    // turn always sees its own work. [3] is a peer's line inside this
    // episode, which a blind opening round must not show.
    let desk = desk_of(&three_member_manifest(), "eng").expect("a room");
    let member = desk.member("planner").expect("planner is seated").clone();
    let quorum = desk.policy().quorum;
    let full = vec![
        message(1, "scout", "!propose #euler249 Old guess."),
        message(2, "planner", "!propose #euler301 My own guess."),
        message(3, "critic", "!propose #euler301-alt A rushed guess."),
    ];
    let turn = hive_turn("planner", tinyhivemind_hive::Visibility::Blind, Sequence(1));
    let visible = tinyhivemind_hive::project_for(&turn, &full);

    let prompt = EpisodePrompt::new(&member, &desk, "Pick a strategy.", quorum, &[])
        .with_trigger(Sequence(1))
        .render(&turn, &visible);

    assert!(
        prompt.contains("[1] scout"),
        "prior context stays visible even blind:\n{prompt}"
    );
    // The reader's own prior row is marked `(you)`, keeping its id.
    assert!(prompt.contains("[2] planner (you)"), "{prompt}");
    assert!(
        !prompt.contains("[3]"),
        "a peer's live position leaked into a blind turn:\n{prompt}"
    );
    // The roster always names every teammate ("In the room with you: ..."),
    // so the leak this guards against is the peer's own *line*, not its id.
    assert!(
        !prompt.contains("euler301-alt"),
        "a peer's live position leaked into a blind turn:\n{prompt}"
    );
    let prior_end = prompt.find("[1] scout").expect("prior row rendered");
    let divider_at = prompt
        .find("this episode's floor")
        .expect("the divider names its own meaning");
    // The turn-holder's own line, so it reads "You" rather than its own name.
    let current_start = prompt
        .find("[2] planner (you)")
        .expect("current row rendered");
    assert!(prior_end < divider_at, "{prompt}");
    assert!(divider_at < current_start, "{prompt}");
}

/// The reviewer's exact scenario (Codex P2 on this PR): a manifest that
/// validates cleanly becomes an unreachable-quorum desk once a seat is retired
/// through the Team API.
///
/// `manifest.rs` refuses a *declared* desk that cannot reach its quorum, but it
/// reads `[[group_chat]].members` and runs once at load. The effective roster
/// moves underneath it, so the same silent failure arrives from a direction a
/// manifest check cannot see — hence the second gate in `desk_episode`.
#[test]
fn a_retirement_that_strands_the_quorum_falls_back_to_one_responder() {
    // Three seats, quorum 2: `planner` may propose, `scout` may support, and
    // `critic` may only object. Two seats can put a distinct supporter on a
    // topic, which clears a quorum of two.
    let manifest = format!(
        "{}[group_chat.hive]\nquorum = 2\n\n[group_chat.hive.moves]\n\
         planner = [\"propose\", \"evidence\", \"defer\"]\n\
         scout = [\"support\", \"evidence\", \"defer\"]\n\
         critic = [\"object\", \"evidence\", \"defer\"]\n",
        three_member_manifest()
    );
    assert!(
        record(&manifest).manifest.validate().is_empty(),
        "the declared desk is valid, which is the whole point: {:?}",
        record(&manifest).manifest.validate()
    );
    assert!(
        desk_episode(&record(&manifest), Some("eng")).is_some(),
        "and it opens a room while everybody is seated"
    );

    // Retire `scout` — the only seat that may `!support`. What is left is a
    // two-member deliberating desk whose sole eligible supporter is `planner`,
    // against a quorum of two: nothing it ever proposes can carry.
    let mut retired = record(&manifest);
    retired.overlay_retired_agents = vec!["scout".to_owned()];
    assert!(
        desk_episode(&retired, Some("eng")).is_none(),
        "a desk that can no longer reach its own quorum must keep the \
         single-responder path rather than open a room that spends its whole \
         budget failing to carry anything"
    );

    // Retiring the seat that may only `!object` strands nothing: `planner` and
    // `scout` still clear the quorum of two, so the room still opens. The gate
    // has to be about eligibility, not about the desk merely getting smaller.
    let mut retired = record(&manifest);
    retired.overlay_retired_agents = vec!["critic".to_owned()];
    assert!(
        desk_episode(&retired, Some("eng")).is_some(),
        "losing an ineligible seat leaves the quorum reachable"
    );
}

/// **A deadlock asks the operator, rather than merely announcing itself.**
///
/// `Deadlocked` is only returned when `has_free_dissenter` is false — every
/// member has taken a side. So the room cannot break the tie, and nobody in it
/// can even choose whom to consult without one side picking its own referee.
/// Every member also had the chance to name an outsider on any of its own
/// turns: referral is considered on every committed marked line. None did.
///
/// The one party left is the operator, and the sentence has to say so — read
/// flatly it is an outcome, and an operator would have to know the mechanism to
/// realise a decision was owed.
#[test]
fn a_deadlocked_desk_asks_the_operator_to_decide() {
    let outcome = EpisodeOutcome {
        ending: EpisodeEnding::Deadlocked {
            topics: vec!["skeleton".to_string(), "spinner".to_string()],
        },
        turns: 6,
        first_seq: None,
        last_seq: None,
        report_seq: None,
        violations: Vec::new(),
        failed_turns: 0,
        referrals: Default::default(),
    };

    let summary = outcome.ending_summary();
    assert!(
        summary.contains("#skeleton") && summary.contains("#spinner"),
        "both tied topics are named, so the operator knows what it is choosing between: {summary}"
    );
    assert!(
        summary.contains("needs your call"),
        "and is asked for a decision rather than told an outcome: {summary}"
    );
    assert!(
        summary.contains("nobody was left to break the tie"),
        "with the reason the desk could not settle it alone: {summary}"
    );
}

/// **The report carries the decision, not only its label.**
///
/// A topic id is a name an LLM picked. Observed live: a desk argued
/// lazy-loading coherently, filed it under `#need-decide`, and the report read
/// "the desk settled on #need-decide" — which says nothing. The reasoning was
/// in the transcript, where `EpisodeOutcome` cannot reach it.
#[test]
fn a_settled_room_reports_what_it_decided() {
    let settled = EpisodeOutcome {
        ending: EpisodeEnding::Converged {
            topic: "need-decide".to_string(),
            supporters: vec![
                "software_engineer".to_string(),
                "junior_engineer".to_string(),
            ],
            proposal: Some(
                "lazy-load each section, so the page only pays for what is opened".to_string(),
            ),
        },
        turns: 3,
        first_seq: None,
        last_seq: None,
        report_seq: None,
        violations: Vec::new(),
        failed_turns: 0,
        referrals: Default::default(),
    };
    let summary = settled.ending_summary();
    assert!(
        summary.contains("lazy-load each section"),
        "an operator reads the decision itself: {summary}"
    );
    assert!(
        !summary.contains('\n'),
        "and it stays one line: this row is journaled onto the desk and lands in \
         the next episode's window: {summary}"
    );
    assert!(
        summary.contains("#need-decide") && summary.contains("software_engineer"),
        "with the label and backing kept as bookkeeping: {summary}"
    );

    // A room whose proposal has scrolled out of the window reads exactly as it
    // did before this field existed, rather than losing the sentence entirely.
    let unknown = EpisodeOutcome {
        ending: EpisodeEnding::Converged {
            topic: "stage".to_string(),
            supporters: vec!["engineer".to_string()],
            proposal: None,
        },
        turns: 5,
        first_seq: None,
        last_seq: None,
        report_seq: None,
        violations: Vec::new(),
        failed_turns: 0,
        referrals: Default::default(),
    };
    assert!(
        unknown.ending_summary().contains("settled on #stage"),
        "{}",
        unknown.ending_summary()
    );
}

/// **A member must be able to tell its own turns from its colleagues'.**
///
/// Every transcript row was labelled with its author's name, the reader's own
/// included, so a member had no convention for first person and copied the one
/// it was shown. Observed live: `software_engineer` closed a room with
/// "carried with support from software_engineer and junior_engineer" — naming
/// itself as though it were somebody else.
#[test]
fn a_member_reads_its_own_turns_as_its_own() {
    let visible = vec![
        message(1, "scout", "!propose #ship send it now"),
        message(
            2,
            "planner",
            "!object >1 ^1 the last rollout broke checkout",
        ),
    ];
    let rendered = crate::hivemind::prompt::render_transcript(&visible, None, Some("planner"));

    assert!(
        rendered.contains("[2] planner (you):"),
        "the reader's own row is marked as theirs: {rendered}"
    );
    assert!(
        rendered.contains("[1] scout:"),
        "and a colleague keeps its name: {rendered}"
    );
    assert!(
        !rendered.contains("[1] scout (you)"),
        "the marker is the reader's alone: {rendered}"
    );
    // The id stays in front of the marker. The transcript is the citation
    // surface — `!object >2` and `!support ^2` name this row to everyone else
    // — so a reader stripped of its own id could not tie a colleague's
    // citation back to the line it is arguing with.
    assert!(
        !rendered.contains("[2] You:"),
        "the id is not replaced by a bare pronoun: {rendered}"
    );

    // A caller with no reader renders every row by name, as before.
    let anonymous = crate::hivemind::prompt::render_transcript(&visible, None, None);
    assert!(anonymous.contains("[2] planner:"), "{anonymous}");
}

/// A seat is promised an `@handle` answer only when one could actually arrive.
///
/// `consider` returns immediately on a policy that is not `enabled`, and
/// `enabled` defaults to OFF — so deriving this from "a federation exists"
/// told every seat in a federated company that writing an `@handle` "puts your
/// question to them, they answer at once", for a question that was silently
/// dropped (CodeRabbit, #2341).
#[test]
fn the_at_handle_promise_is_made_only_when_a_referral_can_be_answered() {
    let desk = desk_of(&three_member_manifest(), "eng").expect("a room");
    let member = desk.member("planner").expect("planner is seated").clone();
    let quorum = desk.policy().quorum;
    let turn = hive_turn("planner", tinyhivemind_hive::Visibility::Full, Sequence(0));
    let visible = [message(1, "scout", "!propose #euler249 Old guess.")];

    let rendered = |can_ask: bool| {
        EpisodePrompt::new(&member, &desk, "Decide the answer.", quorum, &[])
            .able_to_ask(can_ask)
            .render(&turn, &visible)
    };

    let promised = rendered(true);
    assert!(
        promised.contains("write their @handle"),
        "a seat that can be answered is told how to ask: {promised}"
    );

    let silent = rendered(false);
    assert!(
        !silent.contains("write their @handle"),
        "a seat whose questions are dropped is promised nothing: {silent}"
    );
    // The roster itself still renders — the seat works with these colleagues,
    // it just cannot put a private question to them.
    assert!(
        silent.contains("In the room with you"),
        "the room is still described: {silent}"
    );
}
