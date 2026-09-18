use super::*;

use tinyinference::Result as TaResult;
use tinyinference::model::{ChatModel, ModelRequest, ModelResponse};

/// What the model does when the extractor calls it.
enum Behaviour {
    Reply(&'static str),
    Fail,
    Hang,
}

struct Scripted(Behaviour);

#[async_trait::async_trait]
impl ChatModel<()> for Scripted {
    async fn invoke(&self, _state: &(), _request: ModelRequest) -> TaResult<ModelResponse> {
        match self.0 {
            Behaviour::Reply(text) => Ok(ModelResponse::assistant(text)),
            Behaviour::Fail => Err(tinyinference::Error::Model("provider exploded".into())),
            Behaviour::Hang => {
                // Longer than EXTRACT_TIMEOUT, so the timeout arm is the one
                // under test rather than a race.
                tokio::time::sleep(EXTRACT_TIMEOUT * 4).await;
                Ok(ModelResponse::assistant("too late"))
            }
        }
    }
}

fn extractor(behaviour: Behaviour) -> PayloadExtractor {
    PayloadExtractor {
        model: Arc::new(Scripted(behaviour)),
        model_name: "test-model".to_string(),
        // No company handles under test here: these cases are about which
        // `SummarizeOutcome` each branch yields. The metering path is
        // exercised where the ledger and meter live.
        metering: None,
    }
}

async fn run(behaviour: Behaviour, hint: Option<&str>, raw: &str) -> SummarizeOutcome {
    let ctx = tinyagents_harness::context::RunContext::new(
        tinyagents_harness::context::RunConfig::new("payload-extract-test"),
        (),
    );
    extractor(behaviour)
        .maybe_summarize_in_parent(&ctx, "GITHUB_LIST_ISSUES", hint, raw)
        .await
        .expect("the extractor degrades, it never errors")
}

fn big() -> String {
    // Over `PASS_THROUGH_BYTES`, or the size gate short-circuits to
    // `NotNeeded` and none of the branches below is reached — which is
    // exactly what happened when that gate was added: five tests here
    // began passing through instead of exercising the paths they name.
    let payload = format!(
        "[{}]",
        vec![r#"{"number":1,"title":"a flaky test"}"#; 2_000].join(",")
    );
    assert!(
        payload.len() > PASS_THROUGH_BYTES,
        "the fixture must clear the pass-through gate"
    );
    payload
}

/// Without a hint the extraction has nothing to select against and would be
/// guessing as blindly as the byte cut it replaces, so it declines.
#[tokio::test]
async fn no_task_hint_declines_rather_than_guessing() {
    let outcome = run(Behaviour::Reply("30 issues"), None, &big()).await;
    assert!(matches!(
        outcome,
        SummarizeOutcome::Unavailable(UnavailableReason::Disabled)
    ));
}

/// A provider failure must leave the turn holding the raw payload, not fail
/// the turn: extraction is an improvement on truncation, never a dependency.
#[tokio::test]
async fn a_provider_error_leaves_the_raw_payload_standing() {
    let outcome = run(Behaviour::Fail, Some("list the issues"), &big()).await;
    assert!(matches!(
        outcome,
        SummarizeOutcome::Unavailable(UnavailableReason::Failed)
    ));
}

/// The deadline is the point: a slow extraction is worse than none, because
/// the turn is already at its cap when this runs.
#[tokio::test(start_paused = true)]
async fn a_hanging_provider_times_out_rather_than_stalling_the_turn() {
    let outcome = run(Behaviour::Hang, Some("list the issues"), &big()).await;
    assert!(matches!(
        outcome,
        SummarizeOutcome::Unavailable(UnavailableReason::Failed)
    ));
}

/// An empty answer is a failed extraction, not an empty tool result — the
/// difference decides whether the model sees the payload at all.
#[tokio::test]
async fn an_empty_answer_is_treated_as_a_failure() {
    let outcome = run(Behaviour::Reply("   \n  "), Some("list the issues"), &big()).await;
    assert!(matches!(
        outcome,
        SummarizeOutcome::Unavailable(UnavailableReason::Failed)
    ));
}

/// A "summary" at least as long as the payload has extracted nothing, and
/// substituting it would spend a model call to make the result no smaller.
#[tokio::test]
async fn a_summary_that_does_not_shrink_is_not_used() {
    let raw = r#"[{"number":1}]"#;
    let outcome = run(
        Behaviour::Reply("this reply is considerably longer than the payload it summarises"),
        Some("list the issues"),
        raw,
    )
    .await;
    assert!(matches!(outcome, SummarizeOutcome::NotNeeded));
}

/// The path that matters: a hint, a working model, and an answer shorter
/// than what it replaces.
#[tokio::test]
async fn a_shorter_answer_is_returned_as_the_summary() {
    let outcome = run(
        Behaviour::Reply("400 issues; #1 a flaky test"),
        Some("list the issues"),
        &big(),
    )
    .await;
    match outcome {
        SummarizeOutcome::Summarized(summary) => {
            assert!(
                summary.summary.contains("400 issues"),
                "the model's answer must be what is carried: {}",
                summary.summary
            );
        }
        other => panic!("expected a summary, got {other:?}"),
    }
}

/// A summary built from a prefix must say so. The summary *replaces* the
/// tool output, so without this the turn holds a confident account of a
/// payload the model only partly read — an incomplete list presented as a
/// complete one, which is the failure this extractor exists to end (codex
/// on tinyhumansai/opencompany#2153).
#[tokio::test]
async fn a_summary_from_a_truncated_payload_discloses_the_cut() {
    let huge = format!("[{}]", vec![r#"{"n":1}"#; 60_000].join(","));
    assert!(
        huge.len() > MAX_EXTRACT_INPUT_CHARS,
        "the fixture must exceed the ceiling or nothing is cut"
    );
    let outcome = run(
        Behaviour::Reply("60000 records"),
        Some("list the records"),
        &huge,
    )
    .await;

    match outcome {
        SummarizeOutcome::Summarized(summary) => {
            assert!(
                summary.summary.contains("later records were not examined"),
                "the cut must be disclosed: {}",
                summary.summary
            );
            assert!(
                summary.summary.contains("full output is on disk"),
                "the recovery route must be named: {}",
                summary.summary
            );
            assert!(
                summary.summary.contains("60000 records"),
                "the model's answer is still carried: {}",
                summary.summary
            );
        }
        other => panic!("expected a summary, got {other:?}"),
    }

    // A payload under the ceiling carries no such notice.
    let small = run(
        Behaviour::Reply("two records"),
        Some("list the records"),
        &format!("[{}]", vec![r#"{"n":1}"#; 400].join(",")),
    )
    .await;
    if let SummarizeOutcome::Summarized(summary) = small {
        assert!(
            !summary.summary.contains("not examined"),
            "nothing was cut, so nothing is disclosed: {}",
            summary.summary
        );
    }
}

/// The input ceiling exists so a multi-megabyte payload is neither billed in
/// full nor sent past a context window. Slicing must land on a character
/// boundary — a byte index inside a multi-byte character panics, and
/// provider payloads are full of them.
#[test]
fn the_input_ceiling_cuts_on_a_character_boundary() {
    let (kept, cut) = cap_input("short");
    assert_eq!(kept, "short");
    assert!(!cut, "a small payload is not cut");

    // Every character is 4 bytes, so a byte-index slice at the ceiling would
    // land mid-character unless the boundary walk works.
    let wide = "\u{1F600}".repeat(MAX_EXTRACT_INPUT_CHARS);
    let (kept, cut) = cap_input(&wide);
    assert!(cut, "an oversized payload is cut");
    assert!(kept.len() <= MAX_EXTRACT_INPUT_CHARS);
    assert!(
        wide.starts_with(kept),
        "the cut keeps the head of the payload"
    );
}

/// The archetype is referenced, not restated. A copy would drift from
/// upstream the moment either side edited the extraction contract, and the
/// first version of this file learned that the expensive way — a
/// hand-written prompt that omitted the per-record identifier line produced
/// thirty issue numbers with no titles.
///
/// This asserts the *clauses this crate depends on are present in what is
/// sent*. It deliberately no longer compares `system_prompt()` with the
/// constant it returns, which was `assert_eq!(X, X)` and could not fail
/// (tinysweeper on tinyhumansai/opencompany#2153).
#[test]
fn the_prompt_carries_the_clauses_this_crate_relies_on() {
    assert!(
        system_prompt().contains("Identifiers preserved"),
        "the per-record identifier section is what gives each kept record a line"
    );
    assert!(
        system_prompt().contains("Never drop them"),
        "identifiers are the archetype's first-order rule"
    );
}
