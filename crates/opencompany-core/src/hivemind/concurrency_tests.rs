//! The headline regression for `EpisodeScope`: two hive episodes
//! deliberating in the same thread must never fold each other's turns as
//! their own votes.
//!
//! # Why `tokio::join!`, and what it actually proves
//!
//! The bug this guards needs two things to be true at once: both episodes'
//! triggering messages are durable in the journal *before either episode's
//! first turn lands*, and episode B's very first `project_session` read
//! happens only after episode A has already deposited turns above B's own
//! trigger. `YieldFirstTurn` below is the seam that forces the second half —
//! episode B's runner will not answer its first turn until episode A's whole
//! run has notified it — while both episodes are driven from the same
//! `tokio::join!`, i.e. as two live futures the executor is actually holding
//! concurrently, not two sequential function calls the test happens to write
//! next to each other. That is the exact shape the finding describes: "two
//! follow-ups in the same existing thread are accepted before the first turn
//! finishes."

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use super::moves_fixtures_tests::{Runner, manifest_with};
use super::test::{MemoryLog, desk_of};
use super::*;
use crate::Result;
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, EventSeq};

/// Journal one operator message and return its own sequence.
async fn opened(log: &MemoryLog, text: &str, parent: Option<EventSeq>) -> EventSeq {
    log.append(
        &MemoryLog::company(),
        CompanyEvent::OperatorMessage {
            text: text.into(),
            by: None,
            chat: Some("eng".into()),
            parent,
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        },
    )
    .await
    .expect("the journal accepts the operator's message")
}

/// Wraps a scripted runner so its first turn does not resolve until `gate`
/// is notified — the seam that turns two episodes opened in one thread into
/// two episodes genuinely interleaved on the executor, rather than one
/// finishing before the other's first read is even polled.
struct YieldFirstTurn<'a> {
    inner: &'a Runner,
    waited: AtomicBool,
    gate: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl HiveTurnRunner for YieldFirstTurn<'_> {
    async fn speak(&self, agent_id: &str, prompt: &str) -> Result<String> {
        if !self.waited.swap(true, Ordering::SeqCst) {
            self.gate.notified().await;
        }
        self.inner.speak(agent_id, prompt).await
    }
}

/// A script that converges on `topic`, after first floating `rival` as a
/// competing proposal nobody seconds — the same shape
/// `a_scripted_room_converges_and_journals_the_right_authors` uses, so this
/// is a known-good convergence, not a script this test invented to pass.
fn converging_script(topic: &'static str, rival: &'static str) -> Runner {
    Runner::new(&[
        (
            "planner",
            leak(format!("!propose #{topic} Stage it behind a flag.")),
        ),
        (
            "scout",
            leak(format!("!propose #{rival} Ship it all at once.")),
        ),
        (
            "critic",
            leak(format!(
                "!evidence #{topic} ^3 The last full rollout took the checkout down."
            )),
        ),
        (
            "planner",
            leak(format!(
                "!support #{topic} ^3 Staging bounds the blast radius."
            )),
        ),
        (
            "scout",
            leak(format!(
                "!support #{topic} ^3 Agreed, and it is reversible."
            )),
        ),
        (
            "critic",
            leak(format!("!commit #{topic} ^3 The room settled.")),
        ),
        ("planner", leak(format!("!commit #{topic} ^3 Recorded."))),
        ("scout", leak(format!("!commit #{topic} ^3 Recorded."))),
    ])
}

/// Leaks one formatted script line for the `'static` `Runner::new` slice.
///
/// A test-only shortcut: the process exits with the test, so there is nobody
/// left to reclaim the string, and it keeps `converging_script` readable
/// instead of threading a `Vec<String>` arena through it.
fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// **Two hive episodes deliberating in the same existing thread must not
/// fold each other's turns as their own votes.**
///
/// Both `ALPHA_QUESTION` and `BETA_QUESTION` are replies inside the same
/// pre-existing thread (`root`), so both episodes open with
/// `.in_thread(Some(root))` — identical conversations, exactly the shape
/// `parent: Some(_)` produces in `HarnessBrain`. Before `EpisodeScope`
/// existed, episode B's watermark (its own trigger) was the *only* bound on
/// what it folded, so episode A's turns — appended above B's trigger while
/// both episodes share one conversation — read as B's own live traces.
/// Episode B would converge on `#alpha` immediately, having taken no turns
/// of its own, on a question nobody in it ever asked.
#[tokio::test]
async fn two_episodes_in_the_same_thread_do_not_fold_each_others_traces() {
    let log = Arc::new(MemoryLog::default());
    let root = opened(&log, "Kickoff", None).await;
    // Both accepted into the thread before either episode's first turn ever
    // runs — the ordering the finding depends on.
    let trigger_a = opened(&log, "ALPHA_QUESTION", Some(root)).await;
    let trigger_b = opened(&log, "BETA_QUESTION", Some(root)).await;

    let manifest = manifest_with("");
    let desk_a = desk_of(&manifest, "eng").expect("a room");
    let desk_b = desk_of(&manifest, "eng").expect("a room");

    let runner_a = converging_script("alpha", "beta");
    let runner_b_inner = converging_script("beta", "alpha");
    let gate = Arc::new(tokio::sync::Notify::new());
    let runner_b = YieldFirstTurn {
        inner: &runner_b_inner,
        waited: AtomicBool::new(false),
        gate: Arc::clone(&gate),
    };

    let events = Arc::clone(&log) as Arc<dyn EventLog>;
    let driver_a = EpisodeDriver::new(
        MemoryLog::company(),
        desk_a,
        Arc::clone(&events),
        &runner_a,
        "ALPHA_QUESTION",
    )
    .in_thread(Some(root));
    let driver_b = EpisodeDriver::new(
        MemoryLog::company(),
        desk_b,
        Arc::clone(&events),
        &runner_b,
        "BETA_QUESTION",
    )
    .in_thread(Some(root));

    // Episode A runs to completion and only then releases episode B's first
    // turn — both futures are driven concurrently by the same `join!`, so
    // episode B's driver exists and is polled throughout episode A's run; it
    // simply has nothing to do until its own first turn is unblocked.
    let run_a = async {
        let outcome = driver_a.run(trigger_a).await;
        gate.notify_one();
        outcome
    };
    let (outcome_a, outcome_b) = tokio::join!(run_a, driver_b.run(trigger_b));
    let outcome_a = outcome_a.expect("episode A runs");
    let outcome_b = outcome_b.expect("episode B runs");

    assert!(
        matches!(&outcome_a.ending, EpisodeEnding::Converged { topic, .. } if topic == "alpha"),
        "episode A must settle on its own question: {outcome_a:?}"
    );
    assert!(
        matches!(&outcome_b.ending, EpisodeEnding::Converged { topic, .. } if topic == "beta"),
        "episode B must settle on its OWN question rather than inheriting \
         episode A's already-carried #alpha vote: {outcome_b:?}"
    );
    assert!(
        outcome_b.turns > 0,
        "episode B must have taken turns of its own rather than converging \
         on a standing it read from episode A: {outcome_b:?}"
    );

    // The journal itself: both episodes' turns are parented to the one
    // shared thread root, which is exactly what makes this scenario the
    // concurrency hazard rather than the already-isolated top-level case.
    let logged = log.replies("eng");
    assert!(
        logged.iter().any(|(_, text)| text.contains("#alpha")),
        "{logged:?}"
    );
    assert!(
        logged.iter().any(|(_, text)| text.contains("#beta")),
        "{logged:?}"
    );
}

/// Regression cover for the case the fix must leave untouched: two episodes
/// on the same desk that were never in the same thread at all (each opens
/// its own, exactly as an unaddressed top-level send does today) already do
/// not fold each other's traces, and must keep not doing so however
/// `EpisodeScope` changes in the future.
#[tokio::test]
async fn two_episodes_with_different_thread_roots_do_not_fold_each_others_traces() {
    let log = Arc::new(MemoryLog::default());
    let trigger_a = opened(&log, "ALPHA_QUESTION", None).await;
    let trigger_b = opened(&log, "BETA_QUESTION", None).await;

    let manifest = manifest_with("");
    let desk_a = desk_of(&manifest, "eng").expect("a room");
    let desk_b = desk_of(&manifest, "eng").expect("a room");

    let runner_a = converging_script("alpha", "beta");
    let runner_b = converging_script("beta", "alpha");
    let events = Arc::clone(&log) as Arc<dyn EventLog>;

    // Each top-level message becomes the root of its own thread — the
    // per-trigger isolation `HarnessBrain` already applies before this fix,
    // via `Some(parent.unwrap_or(trigger))`.
    let outcome_a = EpisodeDriver::new(
        MemoryLog::company(),
        desk_a,
        Arc::clone(&events),
        &runner_a,
        "ALPHA_QUESTION",
    )
    .in_thread(Some(trigger_a))
    .run(trigger_a)
    .await
    .expect("episode A runs");
    let outcome_b = EpisodeDriver::new(
        MemoryLog::company(),
        desk_b,
        Arc::clone(&events),
        &runner_b,
        "BETA_QUESTION",
    )
    .in_thread(Some(trigger_b))
    .run(trigger_b)
    .await
    .expect("episode B runs");

    assert!(
        matches!(&outcome_a.ending, EpisodeEnding::Converged { topic, .. } if topic == "alpha"),
        "{outcome_a:?}"
    );
    assert!(
        matches!(&outcome_b.ending, EpisodeEnding::Converged { topic, .. } if topic == "beta"),
        "{outcome_b:?}"
    );
}
