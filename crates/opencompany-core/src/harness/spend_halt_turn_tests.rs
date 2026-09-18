//! Issue #1032: end-to-end proof that a turn stopped by its in-turn spend brake
//! **says so** — and that it says something different from a turn that paused at
//! its step cap.
//!
//! The bug is a silence, and a silence built in by construction rather than
//! forgotten. #988 armed the brake: a teammate that declares a
//! `budget_usd_daily` gets a [`BudgetStopHook`] that halts a turn outrunning it.
//! But openhuman's tool loop consumes the hook's `StopDecision::Stop { reason }`
//! internally, stops iterating, and returns the run's text as an ordinary
//! `Ok(reply)` — so "I ran out of budget" and "I finished the work" arrive at the
//! operator as the same bubble. `Agent::last_turn_hit_cap()` cannot stand in
//! either: it is `false` for a spend halt, which is exactly what #988 pinned.
//!
//! Nothing shorter than a real turn can show this.
//! [`MockProvider`](crate::harness::provider::MockProvider) issues no tool
//! calls, so a turn against it is one iteration long and no between-iteration
//! hook ever fires. So this drives the **real** harness — real `build_agent`,
//! real [`HostedProvider`], real pool, real brain — and scripts exactly one
//! thing: the model's choices, over a loopback OpenAI-compatible endpoint. The
//! shape [`cap_turn_tests`](super::cap_turn_tests) and
//! `built_in::iteration_cap_turn_test` established.
//!
//! The lever that makes the brake fire is `prompt_tokens`: the stop-hook
//! middleware folds the usage the *provider* reports into openhuman's turn cost,
//! so a script that reports a million prompt tokens crosses a five-cent cap on
//! its first iteration.
//!
//! What each test is for:
//!
//! - the halt reaches [`TurnOutcome::halted_for_spend`] with the right figures,
//!   and — the negative control that stops a hardcoded `Some` passing — a turn
//!   that finishes reports `None`;
//! - a teammate that declared no budget can never report one, because no hook is
//!   installed for it;
//! - the brain emits the halt as a **second, unauthored** bubble;
//! - the spend notice and the step-cap notice do **not** cross-fire, asserted in
//!   both directions;
//! - and the notice never reaches memory, for the reason #926 established.

use std::sync::Arc;

use super::spend_halt_turn_test_fixtures::*;
use crate::harness::brain::{iteration_cap_pause_notice, spend_halt_notice};
use crate::harness::{HarnessBrain, HarnessPool};
use crate::ports::brain::Brain;
use crate::store::FsContextStore;

// ---------------------------------------------------------------------------
// The flag
// ---------------------------------------------------------------------------

/// **The headline.** A turn halted by the in-turn spend brake reports the halt,
/// with the figures the operator needs, and still comes back `Ok` — because a
/// halt is a stop, not an error.
#[tokio::test]
async fn a_turn_halted_for_spend_reports_the_halt_and_its_figures() {
    let (base_url, script) = spawn_script(write_then_answer(CAP), EXPENSIVE_TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let (deps, _ops) = deps_for(base_url, dir.path());
    let rec = record(Some(CAP_USD));
    let pool = HarnessPool::new();
    pool.ensure(&rec, &deps).await.expect("pool ensures");

    let outcome = pool
        .run(
            &rec.id,
            AGENT,
            "Write a short feature spec.",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("a spend halt is a stop, not an error — the turn must return Ok");

    let halt = outcome
        .halted_for_spend
        .as_ref()
        .expect("the brake fired, so the turn must say so — that is the whole of #1032");
    assert_eq!(
        halt.agent, AGENT,
        "the halt names the teammate whose cap was reached"
    );
    assert_eq!(halt.cap_usd, CAP_USD, "measured against the declared cap");
    assert!(
        halt.spent_usd > 0.0,
        "the turn made a paid model call, so the spend cannot be zero: {}",
        halt.spent_usd
    );

    // It really was halted, not merely finished: the script offered `CAP` tool
    // rounds and the turn spent a small handful. (Left loose because the
    // vendored turn adds a wrap-up call after a partial run, and that call is
    // not the thing under test.)
    assert!(
        script.calls() < CAP,
        "expected the brake to cut the turn well short of its {CAP} scripted rounds, got {}",
        script.calls()
    );
    // And it is NOT an iteration-cap pause. #988 pinned this; the two notices
    // must never be interchangeable, because the operator's next move differs.
    assert!(
        !outcome.hit_iteration_cap,
        "a spend halt must not also report a step pause — they are different outcomes"
    );
}

/// **The negative control.** The same declared cap, a turn cheap enough to
/// finish, and the halt stays `None`.
///
/// Without this, `halted_for_spend` wired to a hardcoded `Some` at the read site
/// would pass every other test in this file.
#[tokio::test]
async fn a_turn_that_finishes_inside_its_budget_reports_no_halt() {
    let (base_url, script) = spawn_script(write_then_answer(2), CHEAP_TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let (deps, _ops) = deps_for(base_url, dir.path());
    let rec = record(Some(CAP_USD));
    let pool = HarnessPool::new();
    pool.ensure(&rec, &deps).await.expect("pool ensures");

    let outcome = pool
        .run(
            &rec.id,
            AGENT,
            "Write a short feature spec.",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("turn runs");

    assert!(
        outcome.halted_for_spend.is_none(),
        "a turn that finished well inside its budget owes no halt notice: {:?}",
        outcome.halted_for_spend
    );
    assert!(
        outcome.reply.contains(ANSWER),
        "the control must actually finish to be a control at all: {}",
        outcome.reply
    );
    assert_eq!(
        script.calls(),
        3,
        "two tool rounds plus the answer — the fixture is armed, just not tripped"
    );
}

/// A teammate that declared **no** `budget_usd_daily` can never report a spend
/// halt, however expensive its turn — because #988 installs no hook for it.
///
/// Same million-token script as the headline test, with the manifest key omitted
/// instead of set. If this fails, either a hook is being armed for a teammate
/// who declared nothing, or a blanket default crept back in.
#[tokio::test]
async fn a_teammate_with_no_declared_budget_can_never_report_a_halt() {
    let (base_url, script) = spawn_script(write_then_answer(3), EXPENSIVE_TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let (deps, _ops) = deps_for(base_url, dir.path());
    let rec = record(None);
    let pool = HarnessPool::new();
    pool.ensure(&rec, &deps).await.expect("pool ensures");

    let outcome = pool
        .run(
            &rec.id,
            AGENT,
            "Write a short feature spec.",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("turn runs");

    assert!(
        outcome.halted_for_spend.is_none(),
        "no cap was declared, so no brake exists to have fired: {:?}",
        outcome.halted_for_spend
    );
    assert!(
        outcome.reply.contains(ANSWER),
        "an undeclared teammate must not be halted at any cost: {}",
        outcome.reply
    );
    assert_eq!(
        script.calls(),
        4,
        "every scripted round should have run — nothing should have cut it short"
    );
}

// ---------------------------------------------------------------------------
// What the operator sees
// ---------------------------------------------------------------------------

/// A spend-halted chat turn reaches the operator as **two** bubbles: whatever
/// the agent had, then the system saying it was stopped for money.
///
/// The second bubble is unauthored — no teammate said it — and carries no steps,
/// because the timeline already rode in on the first.
#[tokio::test]
async fn a_spend_halted_chat_turn_says_so_in_a_second_bubble() {
    let (base_url, _script) = spawn_script(write_then_answer(CAP), EXPENSIVE_TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let (deps, ops) = deps_for(base_url, dir.path());
    let brain =
        HarnessBrain::new(Arc::new(HarnessPool::new()), deps, record(Some(CAP_USD))).with_runs(ops);

    let result = brain
        .run_cycle(chat("Write a short feature spec."), &NoopHost)
        .await
        .expect("cycle runs");

    let bubbles = operator_bubbles(&result.channel_responses);
    assert_eq!(
        bubbles.len(),
        2,
        "a halted turn owes the operator the reply AND the halt notice: {:?}",
        bubbles.iter().map(|b| &b.text).collect::<Vec<_>>()
    );

    // The agent's text, attributed to the agent.
    assert_eq!(bubbles[0].agent.as_deref(), Some(AGENT));

    // The system's notice, attributed to nobody.
    let notice = &bubbles[1].text;
    assert!(
        bubbles[1].agent.is_none(),
        "the platform's words must not be put in the agent's mouth"
    );
    assert!(
        bubbles[1].steps.is_empty(),
        "the timeline is already on the reply bubble; repeating it doubles every row"
    );

    // It has to actually say the things the operator needs.
    assert!(
        notice.contains(SPEND_MARKER),
        "it must name the spend halt: {notice}"
    );
    assert!(
        notice.contains(AGENT),
        "it must name whose cap was reached, or the figures are unattributable: {notice}"
    );
    assert!(
        notice.contains("$0.05"),
        "it must quote the cap it was measured against: {notice}"
    );
    assert!(
        notice.contains("Nothing errored"),
        "it must say nothing failed, or a budget reads as a crash: {notice}"
    );
    // The one thing it must NOT say. A step pause is resumable with "continue";
    // a spend halt is not, and inviting the operator to reply "continue" here
    // would invite them to burn the rest of a budget that had already run out.
    assert!(
        !notice.contains("continue"),
        "the spend notice must never tell the operator to reply \"continue\": {notice}"
    );
    assert_ne!(
        *notice,
        iteration_cap_pause_notice(AGENT),
        "the two notices must not be interchangeable — the operator's next action differs"
    );
}

/// The same cycle with the turn inside its budget: exactly **one** bubble.
///
/// The pair is the real assertion — a notice that fires on every turn is as
/// useless as one that never fires.
#[tokio::test]
async fn a_turn_inside_its_budget_says_nothing_extra() {
    let (base_url, _script) = spawn_script(write_then_answer(2), CHEAP_TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let (deps, ops) = deps_for(base_url, dir.path());
    let brain =
        HarnessBrain::new(Arc::new(HarnessPool::new()), deps, record(Some(CAP_USD))).with_runs(ops);

    let result = brain
        .run_cycle(chat("Write a short feature spec."), &NoopHost)
        .await
        .expect("cycle runs");

    let bubbles = operator_bubbles(&result.channel_responses);
    assert_eq!(
        bubbles.len(),
        1,
        "a turn that finished owes no halt notice: {:?}",
        bubbles.iter().map(|b| &b.text).collect::<Vec<_>>()
    );
    assert!(
        !bubbles[0].text.contains(SPEND_MARKER),
        "and it must not be smuggled into the reply either"
    );
}

/// **The cross-fire assertion, both directions.** A step pause emits the step
/// notice and not the spend one; a spend halt emits the spend notice and not the
/// step one.
///
/// This is what stops the two becoming interchangeable. They are not two
/// spellings of one condition: a step pause means the work fits and the turn ran
/// out of room, so `"continue"` finishes it; a spend halt means the work costs
/// more than the budget allows, and asking again just spends more.
///
/// Run as one test over two cycles because the assertion IS the comparison —
/// split apart, each half could pass while the pair stayed wrong.
#[tokio::test]
async fn the_step_notice_and_the_spend_notice_do_not_cross_fire() {
    // Direction one: a turn that exhausts its iterations, by a teammate with no
    // budget to run out of. `CAP` tool rounds, then the tools-disabled wrap-up.
    let (base_url, _script) = spawn_script(write_then_answer(CAP), CHEAP_TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let (deps, ops) = deps_for(base_url, dir.path());
    let brain = HarnessBrain::new(Arc::new(HarnessPool::new()), deps, record(None)).with_runs(ops);

    let result = brain
        .run_cycle(chat("Write a short feature spec."), &NoopHost)
        .await
        .expect("cycle runs");
    let texts: Vec<&str> = operator_bubbles(&result.channel_responses)
        .iter()
        .map(|b| b.text.as_str())
        .collect();
    assert!(
        texts.contains(&iteration_cap_pause_notice(AGENT).as_str()),
        "a turn that ran out of steps must emit the STEP notice: {texts:?}"
    );
    assert!(
        texts.iter().all(|t| !t.contains(SPEND_MARKER)),
        "a step pause must not be reported as a spend halt: {texts:?}"
    );

    // Direction two: a turn halted for spend, by a teammate that declared a cap.
    let (base_url, _script) = spawn_script(write_then_answer(CAP), EXPENSIVE_TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let (deps, ops) = deps_for(base_url, dir.path());
    let brain =
        HarnessBrain::new(Arc::new(HarnessPool::new()), deps, record(Some(CAP_USD))).with_runs(ops);

    let result = brain
        .run_cycle(chat("Write a short feature spec."), &NoopHost)
        .await
        .expect("cycle runs");
    let texts: Vec<&str> = operator_bubbles(&result.channel_responses)
        .iter()
        .map(|b| b.text.as_str())
        .collect();
    assert!(
        texts.iter().any(|t| t.contains(SPEND_MARKER)),
        "a turn halted for money must emit the SPEND notice: {texts:?}"
    );
    assert!(
        !texts.contains(&iteration_cap_pause_notice(AGENT).as_str()),
        "a spend halt must not be reported as a step pause: {texts:?}"
    );
}

// ---------------------------------------------------------------------------
// What memory keeps
// ---------------------------------------------------------------------------

/// The turn's memory write carries the agent's own text and **not** the
/// platform's notice.
///
/// This is why the notice is a sibling bubble rather than text appended to the
/// reply. `HarnessPool::run` persists `outcome.reply` to the context store, so
/// appending would file "you ran out of budget" as something the agent said, and
/// the memory loop would recall it into a later turn as prior work.
#[tokio::test]
async fn the_spend_notice_never_reaches_memory() {
    let (base_url, _script) = spawn_script(write_then_answer(CAP), EXPENSIVE_TOKENS).await;
    let dir = tempfile::tempdir().unwrap();
    let (deps, ops) = deps_for(base_url, dir.path());
    let context = FsContextStore::new(dir.path());
    let brain =
        HarnessBrain::new(Arc::new(HarnessPool::new()), deps, record(Some(CAP_USD))).with_runs(ops);

    brain
        .run_cycle(chat("Write a short feature spec."), &NoopHost)
        .await
        .expect("cycle runs");

    let bodies = memory_bodies(&context).await;
    assert!(
        !bodies.is_empty(),
        "the turn must have written its outcome back, or this proves nothing"
    );
    assert!(
        bodies.iter().all(|b| !b.contains(SPEND_MARKER)),
        "the platform's halt notice must never be recalled as something the agent said: {bodies:?}"
    );
}

// ---------------------------------------------------------------------------
// The notice itself
// ---------------------------------------------------------------------------

/// The notice quotes the figures it was given, and stays distinct from the step
/// notice.
///
/// A direct test of the formatter so the wording contract does not rely on a
/// turn that takes seconds to drive.
#[test]
fn the_notice_quotes_the_spend_the_cap_and_the_teammate() {
    let notice = spend_halt_notice(&crate::harness::SpendHalt {
        agent: "researcher".to_string(),
        spent_usd: 4.02,
        cap_usd: 4.0,
    });
    assert!(notice.contains("researcher"), "{notice}");
    assert!(
        notice.contains("$4.02"),
        "the real spend, not the cap: {notice}"
    );
    assert!(notice.contains("$4.00"), "{notice}");
    assert!(
        !notice.contains("continue"),
        "a spend halt is not resumable by asking again: {notice}"
    );
}
