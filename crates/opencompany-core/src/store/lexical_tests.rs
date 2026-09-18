use super::*;

fn scores(query: &str, candidates: &[(&str, &str)]) -> Vec<(String, f64)> {
    rank(candidates.iter().copied(), query, usize::MAX)
        .into_iter()
        .map(|h| (h.addr.as_ref().to_string(), h.score))
        .collect()
}

#[test]
fn an_empty_query_finds_nothing() {
    assert!(scores("", &[("a", "anything at all")]).is_empty());
    assert!(scores("   ", &[("a", "anything at all")]).is_empty());
}

#[test]
fn one_word_of_difference_is_still_a_hit() {
    // This is the core of it: the substring test that stood here returned
    // NOTHING, because the query does not occur verbatim in the body.
    let out = scores(
        "draw up a quarterly overview of revenue",
        &[
            ("a", "Task: draw up an overview of revenue\nOutcome: done"),
            ("b", "Task: file the supplier invoices\nOutcome: done"),
        ],
    );
    assert_eq!(out.len(), 1, "partial overlap must hit");
    assert_eq!(out[0].0, "a");
    assert!(out[0].1 > 0.0);
}

#[test]
fn a_term_that_appears_nowhere_still_counts() {
    // Measured and reverted: at first terms with df = 0 were given weight
    // zero, "because they say nothing about the choice between chunks".
    // That is exactly backwards. For a question about a subject NOT in
    // memory, only stopword weight then remained in the denominator, and
    // every arbitrary chunk containing "the" and "of" scored 1.00.
    //
    // An unfindable term therefore counts as if it were rare (df = 1): it
    // costs the query part of its weight, which is the right message —
    // memory does not hold this subject.
    let with = scores(
        "quarterly figures shipyard",
        &[("a", "the quarterly figures for February")],
    );
    let without = scores(
        "quarterly figures",
        &[("a", "the quarterly figures for February")],
    );
    assert!(with[0].1 < without[0].1, "the unfindable term must cost");
}

#[test]
fn a_question_about_an_unknown_subject_scores_much_lower() {
    // The regression above, as a test on the outcome. Deliberately a ratio
    // and not an absolute threshold: IDF is a corpus statistic and with
    // three chunks it says little. What must hold at *every* corpus size is
    // that a question about an unknown subject lands below one about a known
    // subject.
    //
    // Measured on a real 402-chunk store: an off-topic question went from
    // 1.00 (five hits about an unrelated loan) to 0.19, below the floor.
    let memory: &[(&str, &str)] = &[
        (
            "a",
            "Task: produce the quarterly revenue figures for the north region\nOutcome: ready",
        ),
        (
            "b",
            "Task: send the quotation to the customer in Ashford\nOutcome: sent",
        ),
        (
            "c",
            "Task: file the supplier invoices for March\nOutcome: done",
        ),
    ];
    let known = scores("send the quotation to the customer in Ashford", memory);
    let unknown = scores(
        "put the christmas tree in the canteen and hang some lights on it",
        memory,
    );
    assert!(
        unknown.first().map(|h| h.1).unwrap_or(0.0) < known[0].1 / 2.0,
        "unknown {unknown:?} must land far below known {known:?}"
    );
}

#[test]
fn a_long_chunk_that_mentions_everything_does_not_win() {
    let short = "Task: draw up the quotation for Ashford\nOutcome: sent";
    let long = format!(
        "Task: take a screenshot\nOutcome: cannot. [Open work: {} quotation Ashford              quarterly revenue north supplier invoices]",
        "noise ".repeat(400)
    );
    let out = scores(
        "draw up the quotation for Ashford",
        &[("long", &long), ("short", short)],
    );
    assert_eq!(
        out[0].0, "short",
        "a chunk that mentions everything mentions nothing in particular"
    );
}

#[test]
fn rare_words_outweigh_stopwords() {
    // "the" and "of" are in all three, "quarterly" in one. The candidate
    // sharing only stopwords must not win.
    let out = scores(
        "the quarterly figures of March",
        &[
            ("stop", "the minutes of the meeting of Tuesday"),
            ("hit", "the quarterly figures of February are ready"),
            ("noise", "the agenda of the coming week"),
        ],
    );
    assert_eq!(out[0].0, "hit", "the rare term must decide the ranking");
    // The stopword candidates fall through the floor; if they survive, they
    // are at least below "hit".
    for (addr, score) in out.iter().skip(1) {
        assert!(score < &out[0].1, "{addr} must not outrank the real hit");
    }
}

#[test]
fn the_best_beats_the_oldest() {
    // The second half of the bug: the previous code truncated to `limit` in
    // insertion order, so "old1" would have won here.
    let candidates: Vec<(&str, &str)> = vec![
        ("old1", "revenue"),
        ("old2", "revenue"),
        ("old3", "revenue"),
        ("new", "revenue margin quarter report"),
    ];
    let out = rank(candidates, "revenue margin quarter report", 1);
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0].addr.as_ref(),
        "new",
        "sorting belongs before cutting"
    );
}

#[test]
fn equal_scores_keep_the_order_of_the_store() {
    let out = scores("revenue", &[("first", "revenue"), ("second", "revenue")]);
    assert_eq!(out[0].0, "first");
    assert_eq!(out[1].0, "second");
}

#[test]
fn the_score_stays_inside_the_port_contract() {
    for (_, score) in scores(
        "revenue margin",
        &[("a", "revenue margin"), ("b", "revenue")],
    ) {
        assert!((0.0..=1.0).contains(&score), "score outside [0,1]: {score}");
    }
}

#[test]
fn the_snippet_wraps_the_hit_and_not_the_start_of_the_body() {
    let body = format!("{}NEEDLE{}", "x".repeat(400), "y".repeat(400));
    let out = rank([("a", body.as_str())], "needle", 1);
    assert_eq!(out.len(), 1);
    let s = &out[0].snippet;
    assert!(s.contains("NEEDLE"), "the snippet must contain the hit");
    assert!(
        s.starts_with('x') && s.ends_with('y'),
        "with context on both sides"
    );
    assert!(s.len() <= 2 * SNIPPET_WINDOW_BYTES + "needle".len());
}

#[test]
fn multibyte_text_never_splits_a_character() {
    // 'é' is two bytes; a window computed in bytes must still land on a
    // character boundary, or this is a panic instead of a search result.
    let body = format!("{}needle{}", "é".repeat(200), "é".repeat(200));
    let out = rank([("a", body.as_str())], "needle", 1);
    assert_eq!(out.len(), 1);
    assert!(out[0].snippet.contains("needle"));
}

#[test]
fn case_does_not_matter() {
    let out = scores("REVENUE", &[("a", "the Revenue of March")]);
    assert_eq!(out.len(), 1);
}

#[test]
fn no_overlap_is_no_hit() {
    assert!(scores("quarterly", &[("a", "the agenda for tomorrow")]).is_empty());
}
