use super::*;

#[test]
fn truncate_leaves_short_text_alone() {
    assert_eq!(truncate("  hello  ", 40), "hello");
}

#[test]
fn truncate_marks_the_cut() {
    let cut = truncate(&"a".repeat(100), 10);
    assert_eq!(cut.chars().count(), 10);
    assert!(cut.ends_with('…'), "a cut field must not read as complete");
}

/// Cutting on a codepoint boundary rather than a byte one: the byte-indexed
/// version of this panics.
#[test]
fn truncate_cuts_multibyte_text_safely() {
    let cut = truncate(&"é".repeat(50), 5);
    assert_eq!(cut.chars().count(), 5);
}

#[test]
fn listed_bounds_rows_and_reports_the_remainder() {
    let (rows, dropped) = listed(0..100, 10, |out, n| {
        let _ = writeln!(out, "- {n}");
    });
    assert_eq!(rows.lines().count(), 10);
    assert_eq!(dropped, 90);
}

#[test]
fn elided_is_empty_when_nothing_was_dropped() {
    assert_eq!(elided(0, "tasks"), "");
    assert!(elided(3, "tasks").contains("read_ledger"));
}

/// The property a ceiling alone cannot catch: past the bound, more entries
/// must not mean more file.
#[test]
fn past_the_bound_more_entries_do_not_mean_more_file() {
    let render = |count: usize| {
        let (rows, dropped) = listed(0..count, MAX_LISTED, |out, n| {
            let _ = writeln!(out, "- {}", truncate(&"x".repeat(5_000), REASON_CHARS));
            let _ = write!(out, "{n}");
        });
        rows.len() + elided(dropped, "tasks").len()
    };
    let small = render(60);
    let huge = render(600);
    assert!(
        huge.saturating_sub(small) < 200,
        "the file grew with entry count past the bound: {small} → {huge}"
    );
}
