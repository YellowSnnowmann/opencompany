use super::*;
use crate::ports::types::ChunkAddr;

fn hit(snippet: &str) -> ChunkHit {
    ChunkHit {
        addr: ChunkAddr::new("addr"),
        snippet: snippet.to_string(),
        score: 1.0,
    }
}

#[test]
fn inject_with_no_hits_is_unchanged() {
    assert_eq!(inject("do the thing", &[]), "do the thing");
}

#[test]
fn inject_truncates_an_oversized_snippet() {
    let big = "x".repeat(10_000);
    let out = inject("do it", &[hit(&big)]);
    // The 10k snippet is capped to MAX_SNIPPET_CHARS, not injected whole.
    assert!(
        out.chars().count() < 10_000,
        "oversized snippet must truncate"
    );
    assert!(out.contains('…'), "truncation is marked with an ellipsis");
    assert!(out.trim_end().ends_with("do it"));
}

#[test]
fn a_truncated_snippet_reserves_room_for_its_own_ellipsis() {
    let big = "x".repeat(10_000);
    let cut = truncate_chars(&big, MAX_SNIPPET_CHARS);
    assert_eq!(
        cut.chars().count(),
        MAX_SNIPPET_CHARS,
        "the marker is budgeted inside the cap, not added on top of it"
    );
    assert!(cut.ends_with('…'));
    // Multibyte input cuts on a character boundary, never mid-codepoint.
    let multibyte = "é".repeat(10_000);
    let cut = truncate_chars(&multibyte, MAX_SNIPPET_CHARS);
    assert_eq!(cut.chars().count(), MAX_SNIPPET_CHARS);
}

/// The count the "omitted for space" marker names, if the preamble has one.
fn omitted_count(out: &str) -> Option<usize> {
    let line = out.lines().find(|l| l.contains("omitted for space"))?;
    line.trim_start_matches("- […")
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Lines carrying an actual retrieved snippet (not the marker).
fn shown_count(out: &str) -> usize {
    out.lines()
        .filter(|l| l.starts_with("- ") && !l.contains("omitted for space"))
        .count()
}

#[test]
fn inject_stops_at_the_total_history_budget() {
    // Many mid-size snippets: the injected preamble stays within budget.
    let hits: Vec<ChunkHit> = (0..50).map(|_| hit(&"y".repeat(400))).collect();
    let out = inject("go", &hits);
    let injected = out.chars().count() - "go".chars().count();
    assert!(
        injected <= MAX_HISTORY_CHARS + MAX_SNIPPET_CHARS,
        "history is budget-capped, got {injected} injected chars"
    );
}

#[test]
fn inject_names_how_many_hits_the_budget_dropped() {
    let hits: Vec<ChunkHit> = (0..50).map(|_| hit(&"y".repeat(400))).collect();
    let out = inject("go", &hits);
    let shown = shown_count(&out);
    assert!(shown > 0, "some prior work fits in the budget");
    assert!(shown < 50, "not all of it does — this is the drop path");
    assert_eq!(
        omitted_count(&out),
        Some(50 - shown),
        "every hit is either shown or counted: {out}"
    );
}

#[test]
fn inject_accounts_for_every_nearly_fitting_hit() {
    // Sized so the budget runs out partway through rather than cleanly.
    let hits: Vec<ChunkHit> = (0..9).map(|i| hit(&"z".repeat(230 + i))).collect();
    let out = inject("go", &hits);
    let shown = shown_count(&out);
    let omitted = omitted_count(&out).unwrap_or(0);
    assert_eq!(shown + omitted, 9, "no hit vanishes unaccounted: {out}");
    let injected = out.chars().count() - "go".chars().count();
    assert!(
        injected <= MAX_HISTORY_CHARS + MAX_SNIPPET_CHARS,
        "still budget-capped, got {injected} injected chars"
    );
}

#[test]
fn inject_with_nothing_fitting_still_says_prior_work_existed() {
    // A budget too small for any hit: the preamble must still appear
    // carrying only the marker, so suppression is distinguishable from a
    // genuinely cold memory.
    let hits: Vec<ChunkHit> = (0..3).map(|_| hit("a stored outcome")).collect();
    let out = inject_within("go", &hits, 2);
    assert!(out.starts_with("## Relevant prior work\n"), "{out}");
    assert_eq!(omitted_count(&out), Some(3), "{out}");
    assert_eq!(shown_count(&out), 0, "nothing fit: {out}");
    assert!(out.trim_end().ends_with("go"));
    assert_ne!(out, "go", "a suppressed preamble is not a bare message");
}

#[test]
fn inject_prepends_a_preamble_and_keeps_the_task() {
    let out = inject("ship it", &[hit("Task: plan\nOutcome: drafted plan")]);
    assert!(out.starts_with("## Relevant prior work\n"));
    assert!(out.contains("drafted plan"));
    assert!(out.trim_end().ends_with("ship it"));
}

#[test]
fn outcome_chunk_labels_and_carries_both_sides() {
    let chunk = outcome_chunk("ceo", "plan the launch", "here is the plan");
    assert_eq!(chunk.label, "task-outcome/ceo");
    assert!(chunk.body.contains("plan the launch"));
    assert!(chunk.body.contains("here is the plan"));
}

#[test]
fn outcome_chunk_redacts_secrets_on_both_sides() {
    // This write path bypasses `Memory::store`, so the operator message
    // and the reply must be redacted here — an operator message carrying
    // a one-time-secret URL or bearer token must not persist verbatim.
    let chunk = outcome_chunk(
        "ceo",
        "open https://ots.example/secret/AbCdEf123456",
        "opened it with Bearer sk-verylongsecrettoken",
    );
    assert!(chunk.body.contains("/secret/[REDACTED]"));
    assert!(chunk.body.contains("Bearer [REDACTED]"));
    assert!(!chunk.body.contains("AbCdEf123456"));
    assert!(!chunk.body.contains("sk-verylongsecrettoken"));
}

#[test]
fn multiline_snippets_are_flattened_before_injection() {
    // Embedded newlines in a stored snippet must be collapsed before
    // truncation; otherwise they cross the `## Task` boundary and break
    // the preamble structure (CWE-74 — injection of untrusted whitespace
    // into the prompt format).
    let out = inject("now", &[hit("Task: plan\nOutcome: drafted\n\nNotes: good")]);
    let preamble = out.split("## Task").next().unwrap();
    let injected_line = preamble
        .lines()
        .find(|l| l.starts_with("- "))
        .expect("the multiline snippet produces an injected line");
    assert!(
        !injected_line.contains('\n'),
        "newlines must be collapsed, got: {injected_line:?}"
    );
    assert!(
        injected_line.contains("Task: plan Outcome: drafted Notes: good"),
        "all whitespace runs are collapsed to single spaces: {injected_line:?}"
    );
}
