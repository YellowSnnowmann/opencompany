//! Regression tests for issue #1871 and the two CodeRabbit majors on PR #2421
//! (review comments 4084652854 and 4084652871).
//!
//! #1871: a blank first completion takes the `AttemptOutcome::Empty` arm and is
//! retried once; the failed attempt must not leak into the retry — no duplicate
//! user message in the model context, no failure rows in the durable
//! transcript.
//!
//! The two review findings were written against the pre-refactor pin, where
//! `Agent::turn` appended the user row on entry and `record_failed_turn`
//! committed the failed attempt (user row + failure note) to the durable
//! transcript, so the retry flow rolled history back by flattening typed rows
//! into `(role, content)` pairs and re-seeding:
//!
//! * 4084652854 — that rollback mapped retained `system` rows to `user` and
//!   dropped `AssistantToolCalls`/`ToolResults` rows outright.
//! * 4084652871 — the rollback touched only memory; the durable transcript
//!   kept the failed attempt, so model-context and display reads disagreed.
//!
//! At the current OpenHuman pin the session runtime owns both projections and
//! a turn is transactional: `Session::turn` builds the provider input from a
//! *clone* of the committed history and, when the driver returns
//! `EmptyProviderResponse`, the failure carries no partial history — nothing
//! is appended to `self.history` and nothing is persisted. Every turn also
//! re-resumes the durable transcript from disk (`ResumeMode::Session`), so the
//! retry's context is exactly the pre-attempt state, with system rows and
//! typed tool rows replayed losslessly from the raw rows on disk. There is no
//! rollback left to flatten or to drift: one logical turn lands as one user
//! message plus the recovered reply in BOTH projections.
//!
//! These tests pin that contract end to end, so a future vendored bump that
//! reintroduces per-attempt writes fails here instead of reaching review:
//!
//! 1. the retry arm is reachable from one ordinary blank completion;
//! 2. the retry does not duplicate the user message in the model context;
//! 3. both attempts of a retried turn send the same tool scope;
//! 4. the failed attempt leaves no durable trace, and the on-disk display
//!    companion agrees with the durable rows;
//! 5. a later turn on the same session replays the durable rows with system
//!    and assistant roles intact;
//! 6. typed tool-call / tool-result rows survive the replay across the retry.

use super::built_in_test_fixtures::test_runtime;
use super::built_in_test_fixtures::*;
use super::built_in_test_fixtures_2::*;
use super::*;

/// **Reachability** — a single `Ok(String::new())` in the scripted-provider
/// sequence causes the wrapper to make exactly two provider calls: the first
/// ends in the driver's empty-response error (blank text, no tool outcomes),
/// which `classify_turn` reads as `AttemptOutcome::Empty`, and the second
/// consumes the recovery reply.
#[tokio::test]
async fn a_single_blank_script_reaches_the_empty_retry_arm() {
    let (agent, _deps, capture) =
        scripted_agent_with_capture(vec![Ok(String::new()), Ok("recovery".into())]);
    let (outcome, _usages) = agent.run("hello").await;
    let outcome = outcome.expect("wrapper recovers");
    assert!(
        outcome.reply.contains("recovery"),
        "second attempt reply must reach the caller: {:?}",
        outcome.reply,
    );
    let calls = capture.captured.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        2,
        "exactly two provider calls must have been made — one for the blank, one for the \
         recovery: {calls:?}",
    );
}

/// **No duplication in the model context** — the retry's provider request must
/// carry exactly the message kinds the first attempt carried: the failed
/// attempt committed nothing, so the second request is built from the same
/// pre-attempt history. Before #1871 was fixed, the second request carried one
/// extra user row; the pre-refactor rollback on PR #2421 then lost `system`
/// rows to `user` flattening — both show up here as a roles-sequence mismatch.
#[tokio::test]
async fn an_empty_retry_does_not_duplicate_the_user_message_in_history() {
    let (agent, _deps, capture) =
        scripted_agent_with_capture(vec![Ok(String::new()), Ok("reply".into())]);
    let (outcome, _usages) = agent.run("hello").await;
    outcome.expect("wrapper recovers");

    let calls = capture.captured.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        2,
        "two provider calls (blank + recovery): {calls:?}"
    );
    let first_users = calls[0].count(CapturedRole::User);
    let second_users = calls[1].count(CapturedRole::User);
    assert_eq!(
        first_users, second_users,
        "the retry must NOT duplicate the user message — both provider requests must \
         carry the same number of user-role messages: {calls:?}",
    );
    assert!(
        first_users >= 1,
        "at least one user message must be present in each request: {calls:?}",
    );
    assert_eq!(
        calls[0].roles, calls[1].roles,
        "the retry must resend the pre-attempt context verbatim — same message kinds in \
         the same order, with system rows still system: {calls:?}",
    );
}

/// **Scope parity** — both attempts of a retried turn must send the same tool
/// surface: the per-turn scope is decided once, before the first attempt, and
/// the runtime holds no per-attempt mutation of it.
#[tokio::test]
async fn an_empty_retry_keeps_the_same_tool_scope_on_both_attempts() {
    let (agent, _deps, capture) =
        scripted_agent_with_capture(vec![Ok(String::new()), Ok("reply".into())]);
    let (outcome, _usages) = agent.run("do the task").await;
    outcome.expect("wrapper recovers");

    let calls = capture.captured.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        2,
        "two provider calls (blank + recovery): {calls:?}"
    );
    assert_eq!(
        calls[0].tools, calls[1].tools,
        "the same tool declarations, in the same order, must go out on both attempts: {calls:?}",
    );
    assert!(
        !calls[0].tools.is_empty(),
        "a normal turn must send at least one tool — if this fails the fixture lost its \
         toolbelt and the parity assertion above is vacuous: {calls:?}",
    );
}

/// Recursive helper: every `*.jsonl` transcript under `dir`, as `(path, body)`.
fn transcripts_under(dir: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "jsonl")
                && path
                    .components()
                    .any(|part| part.as_os_str() == "session_raw")
                && let Ok(body) = std::fs::read_to_string(&path)
            {
                out.push((path, body));
            }
        }
    }
    out
}

/// Recursive helper: session transcript display companions under `dir` whose
/// body contains `marker`. Other Markdown projections can contain the same
/// turn, but they are not the session's display companion.
fn display_companions_containing(dir: &std::path::Path, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "md")
                && let Ok(body) = std::fs::read_to_string(&path)
                && body.starts_with("# Session transcript —")
                && body.contains(marker)
            {
                out.push(body);
            }
        }
    }
    out
}

/// The durable message rows of one transcript body: parsed JSONL lines that
/// carry a `role`, as `(role, content)`; `_meta` headers and tool-snapshot
/// records have no `role` and are skipped.
fn durable_rows(body: &str) -> Vec<(String, String)> {
    body.lines()
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            let role = value.get("role")?.as_str()?.to_string();
            let content = value
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string();
            Some((role, content))
        })
        .collect()
}

/// **No durable trace, and the two projections agree** — after a blank first
/// attempt and a successful retry, the session's durable transcript holds
/// exactly one logical turn: one user row plus the recovered reply. No
/// failure note, no duplicate user row — and the rendered display companion
/// (the `.md` projection the session writer re-renders on every append)
/// carries the same content.
///
/// This is review comment 4084652871's contract pinned: the pre-refactor flow
/// persisted the failed attempt through `record_failed_turn` and the retry
/// then wrote a compaction replacement, so the display kept the failed
/// attempt but not the recovery reply. At this pin the failed attempt never
/// reaches the disk at all, so the disagreement has no source.
#[tokio::test]
async fn the_failed_attempt_leaves_no_durable_trace_and_display_matches() {
    let marker = format!("durable-1871-{}", uuid::Uuid::new_v4().simple());
    let (agent, _deps) = scripted_agent(vec![Ok(String::new()), Ok("recovered reply".into())]);
    let (outcome, _usages) = agent.run(&marker).await;
    outcome.expect("wrapper recovers");

    let root = test_runtime().root_dir().to_path_buf();
    let hits: Vec<_> = transcripts_under(&root)
        .into_iter()
        .filter(|(_, body)| body.contains(&marker))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "exactly one durable transcript may carry this turn's message: {hits:?}",
    );
    let rows = durable_rows(&hits[0].1);
    let user_rows: Vec<_> = rows
        .iter()
        .filter(|(role, content)| role == "user" && content.contains(&marker))
        .collect();
    assert_eq!(
        user_rows.len(),
        1,
        "the durable transcript must hold the user message exactly once: {rows:?}",
    );
    assert!(
        rows.iter()
            .any(|(role, content)| role == "assistant" && content.contains("recovered reply")),
        "the recovery reply must be the durable assistant row: {rows:?}",
    );
    for (role, content) in &rows {
        let lowered = content.to_lowercase();
        assert!(
            !lowered.contains("empty response") && !lowered.contains("turn failed"),
            "no failure note may persist ({role}: {content}): {rows:?}",
        );
    }

    let companions = display_companions_containing(&root, &marker);
    assert_eq!(
        companions.len(),
        1,
        "the display companion must render this turn exactly once: {companions:?}",
    );
    let display = &companions[0];
    assert!(
        display.contains("recovered reply"),
        "the display projection must render the recovery reply, not just a marker: {display}",
    );
    let lowered = display.to_lowercase();
    assert!(
        !lowered.contains("empty response") && !lowered.contains("turn failed"),
        "the display projection must not render the failed attempt: {display}",
    );
}
