use super::*;

/// An advertised window may lower the budget and must never raise it — the
/// asymmetry the reported `chat-v1` failure is the argument for. A model
/// claiming a million tokens still gets the flat ceiling.
#[test]
fn an_advertised_window_only_lowers_the_budget() {
    assert_eq!(budget_chars(None), DEFAULT_UPSTREAM_BUDGET_CHARS);
    assert_eq!(budget_chars(Some(0)), DEFAULT_UPSTREAM_BUDGET_CHARS);
    assert_eq!(budget_chars(Some(1_000_000)), DEFAULT_UPSTREAM_BUDGET_CHARS);
    // 32k tokens → an eighth of it, at 3 chars/token.
    assert_eq!(budget_chars(Some(32_000)), 12_000);
    // A tiny window yields a tiny budget rather than the ceiling.
    assert_eq!(budget_chars(Some(8_000)), 3_000);
    // No overflow on an absurd advertisement.
    assert_eq!(budget_chars(Some(u64::MAX)), DEFAULT_UPSTREAM_BUDGET_CHARS);
}

/// Everything fits → nothing is touched, and the budget is not spent for its
/// own sake.
#[test]
fn sources_that_fit_are_untouched() {
    assert_eq!(allocate_fairly(&[10, 10, 10], 32_000), vec![10, 10, 10]);
    assert_eq!(allocate_fairly(&[], 32_000), Vec::<usize>::new());
}

/// The headline allocation property: one enormous source cannot starve a
/// small one, and the total never exceeds the budget.
#[test]
fn a_small_source_survives_beside_enormous_ones() {
    let kept = allocate_fairly(&[300, 200_000, 200_000], 32_000);
    assert_eq!(kept[0], 300, "the small source is served in full");
    assert_eq!(
        kept[1], kept[2],
        "the two large sources split what is left evenly"
    );
    assert_eq!(
        kept.iter().sum::<usize>(),
        32_000,
        "the budget is not exceeded"
    );
}

/// Equal-sized oversized sources split the budget equally, and the split is
/// independent of the order they arrive in.
#[test]
fn equal_oversized_sources_split_the_budget_and_order_does_not_matter() {
    assert_eq!(
        allocate_fairly(&[90_000, 90_000, 90_000], 30_000),
        vec![10_000, 10_000, 10_000]
    );
    let ascending = allocate_fairly(&[10, 50_000, 90_000], 20_000);
    let descending = allocate_fairly(&[90_000, 50_000, 10], 20_000);
    assert_eq!(ascending[0], descending[2]);
    assert_eq!(ascending[1], descending[1]);
    assert_eq!(ascending[2], descending[0]);
}

/// A budget too small to give every source a character still terminates and
/// still respects the budget — the degenerate case a division-by-share must
/// not divide by zero on.
#[test]
fn a_budget_smaller_than_the_source_count_still_allocates() {
    let kept = allocate_fairly(&[100, 100, 100], 2);
    assert_eq!(kept.iter().sum::<usize>(), 2);
    assert_eq!(allocate_fairly(&[100], 0), vec![0]);
}

/// Truncation is on a character boundary — a fetched page is exactly where a
/// multi-byte character lands on an arbitrary offset, and a byte slice there
/// panics.
#[test]
fn truncation_respects_character_boundaries() {
    assert_eq!(truncate_chars("héllo wörld", 5), "héllo");
    assert_eq!(truncate_chars("abc", 99), "abc");
    assert_eq!(truncate_chars("abc", 0), "");
}

/// The marker names the source, both sizes, and that the rest was not read —
/// an agent that cannot tell it is holding a fragment will present the
/// fragment as the whole.
#[test]
fn the_marker_says_what_was_cut_and_that_it_was_not_read() {
    let marker = truncation_marker(2, 3, 14_700, 10_000);
    assert!(marker.contains("source 2 of 3"), "{marker}");
    assert!(marker.contains("14700"), "{marker}");
    assert!(marker.contains("10000"), "{marker}");
    assert!(
        marker.contains("4700"),
        "the dropped count is stated: {marker}"
    );
    assert!(marker.contains("NOT read"), "{marker}");

    // Nothing survived at all: the wording must not read as "0 characters
    // above are all that was kept".
    let none = truncation_marker(1, 2, 500, 0);
    assert!(none.contains("none of it fitted"), "{none}");
    assert!(none.contains("NOT read"), "{none}");
}

/// A fold where everything fitted says nothing to the operator.
#[test]
fn an_untruncated_fold_raises_no_notice() {
    let report = UpstreamReport {
        sources: vec![
            SourceBudget {
                produced: 10,
                kept: 10,
            },
            SourceBudget {
                produced: 20,
                kept: 20,
            },
        ],
        budget: 32_000,
        omitted: 0,
    };
    assert!(!report.truncated_any());
    assert_eq!(report.notice(), None);
}

/// A fold that cut something reports how much arrived, how much fitted, how
/// many sources were cut, and the remedy.
#[test]
fn a_truncated_fold_reports_the_sizes_and_the_remedy() {
    let report = UpstreamReport {
        sources: vec![
            SourceBudget {
                produced: 100,
                kept: 100,
            },
            SourceBudget {
                produced: 40_000,
                kept: 31_900,
            },
        ],
        budget: 32_000,
        omitted: 0,
    };
    assert!(report.truncated_any());
    let notice = report.notice().expect("a cut fold speaks");
    assert!(notice.contains("40100 characters"), "{notice}");
    assert!(notice.contains("2 sources"), "{notice}");
    assert!(notice.contains("1 of them was truncated"), "{notice}");
    assert!(notice.contains("32000 characters"), "{notice}");
    assert!(notice.contains("summarise each one"), "{notice}");
}

/// An empty source in the omitted tail must not erase the truncation half of
/// the notice.
///
/// CodeRabbit on PR #851: `cut` was derived by subtracting `omitted` from the
/// count of every `truncated()` row, which assumes every omitted row is also
/// truncated. One that produced nothing has `kept == produced`, so it is
/// *not* truncated, and subtracting it anyway under-counted `cut` — here to
/// zero, which took the omission-only branch and left the operator never told
/// that a rendered source had been cut.
#[test]
fn an_empty_omitted_source_does_not_hide_a_truncated_one() {
    let report = UpstreamReport {
        sources: vec![
            // Rendered, and cut.
            SourceBudget {
                produced: 40_000,
                kept: 31_900,
            },
            // Omitted, and produced nothing — so `truncated()` is false.
            SourceBudget {
                produced: 0,
                kept: 0,
            },
        ],
        budget: 32_000,
        omitted: 1,
    };
    let notice = report.notice().expect("a cut fold speaks");
    assert!(
        notice.contains("1 of them were truncated"),
        "the truncated rendered source is still reported: {notice}"
    );
    assert!(
        notice.contains("a further 1 could not be shown"),
        "the omission is still reported alongside it: {notice}"
    );
}

// ── The bound must hold for the reporting of the bound, too ──
//
// CodeRabbit on PR #851: the first cut of this module bounded source text and
// then appended an *unbudgeted* marker per truncated source and an unbudgeted
// separator between each pair. A thousand oversized sources therefore kept
// 32,000 characters of text and added ~216,000 characters of our own
// accounting — 7.7× the budget, which is issue #849's own failure reappearing
// through its fix. These are the assertions whose absence let that through.

/// `n` synthetic oversized sources.
fn oversized_sources(n: usize, chars: usize) -> Vec<String> {
    (0..n)
        .map(|index| format!("SOURCE_{index} {}", "x".repeat(chars)))
        .collect()
}

/// The headline invariant, at the scale that broke it: the **whole rendered
/// section** — text, markers, separators, omission line — is inside the
/// budget.
#[test]
fn a_thousand_oversized_sources_stay_inside_the_budget() {
    let budget = DEFAULT_UPSTREAM_BUDGET_CHARS;
    let (section, report) = bound_sections(&oversized_sources(1_000, 200), budget);
    assert!(
        section.chars().count() <= budget,
        "the section must be bounded including its own accounting: {} characters for a \
         {budget}-character budget",
        section.chars().count()
    );
    // Every input is still accounted for, even the ones with no room.
    assert_eq!(report.sources.len(), 1_000);
    assert!(report.omitted > 0, "the tail is omitted, not silently kept");
    let notice = report.notice().expect("the operator is told");
    assert!(notice.contains("1000 sources"), "{notice}");
    assert!(notice.contains("could not be shown"), "{notice}");
}

/// The same at absurd scale, and with a tiny budget — the two directions that
/// break a cap derived by division.
#[test]
fn the_bound_holds_at_every_scale_and_budget() {
    for count in [1usize, 2, 31, 32, 999, 5_000] {
        for budget in [
            0usize,
            1,
            120,
            1_023,
            1_024,
            5_000,
            DEFAULT_UPSTREAM_BUDGET_CHARS,
        ] {
            let (section, _) = bound_sections(&oversized_sources(count, 400), budget);
            assert!(
                section.chars().count() <= budget,
                "{count} sources under a {budget}-character budget produced {} characters",
                section.chars().count()
            );
        }
    }
}

/// A source too small to be worth reading is not rendered as a shard — it is
/// aggregated into one line that names how many were left out, so the count
/// cannot scale with the number of sources.
#[test]
fn sources_past_the_useful_limit_are_aggregated_into_one_line() {
    let budget = DEFAULT_UPSTREAM_BUDGET_CHARS;
    assert_eq!(max_rendered_sources(budget), 31);
    let (section, report) = bound_sections(&oversized_sources(500, 5_000), budget);
    assert_eq!(report.omitted, 500 - 31);
    assert_eq!(
        section.matches("OMITTED BY OPENCOMPANY").count(),
        1,
        "one line for all of them, never one per source"
    );
    assert!(
        section.contains("469 of this step's 500 inputs"),
        "{}",
        &section[section.len().saturating_sub(400)..]
    );
    assert!(
        section.matches("TRUNCATED BY OPENCOMPANY").count() <= 31,
        "per-source markers are bounded by the render limit"
    );
}

/// The marker is reserved out of the source it describes, so saying "this was
/// cut" costs the budget nothing extra — and a source still keeps a readable
/// amount of text after paying for its own marker.
#[test]
fn a_truncation_marker_is_paid_for_by_its_own_source() {
    let budget = 8_000;
    let (section, report) = bound_sections(&oversized_sources(4, 100_000), budget);
    assert!(section.chars().count() <= budget);
    for source in &report.sources {
        assert!(
            source.kept >= MIN_SOURCE_SHARE_CHARS.saturating_sub(MAX_MARKER_CHARS) / 2,
            "a rendered source keeps a readable amount after its marker: {source:?}"
        );
    }
}

/// Neither marker may outgrow the space reserved for it, at any numbers. A
/// wordier marker in a future edit fails here rather than quietly spending
/// budget it was not given.
#[test]
fn no_marker_can_exceed_its_reserved_size() {
    let extremes = [
        truncation_marker(usize::MAX, usize::MAX, usize::MAX, usize::MAX - 1),
        truncation_marker(usize::MAX, usize::MAX, usize::MAX, 0),
        truncation_marker(1, 1, 1, 0),
        omitted_sources_marker(usize::MAX, usize::MAX),
        omitted_sources_marker(1, 2),
    ];
    for marker in extremes {
        assert!(
            marker.chars().count() <= MAX_MARKER_CHARS,
            "marker is {} characters, over the {MAX_MARKER_CHARS} reserved: {marker}",
            marker.chars().count()
        );
    }
}

/// The backstop is a backstop. If it is what enforces the budget on an
/// ordinary path, the arithmetic above it is wrong and this says so — a clamp
/// that fires routinely would be hiding a mis-accounting rather than guarding
/// against one.
#[test]
fn the_backstop_is_never_what_enforces_the_budget() {
    for count in [1usize, 3, 31, 500] {
        for chars in [10usize, 5_000, 200_000] {
            let (section, _) = bound_sections(
                &oversized_sources(count, chars),
                DEFAULT_UPSTREAM_BUDGET_CHARS,
            );
            assert!(
                !section.contains("whole input was clipped"),
                "the clamp fired for {count} sources of {chars} characters, so the budgeting \
                 above it did not add up"
            );
        }
    }
}

/// And the common case is still untouched: sources that fit are rendered
/// verbatim, joined by the same separator, with no marker and no notice.
#[test]
fn an_ordinary_fold_is_unchanged_by_any_of_this() {
    let rendered = vec![
        "Predecessor A: market is up.".to_string(),
        "Predecessor B: sentiment is positive.".to_string(),
    ];
    let (section, report) = bound_sections(&rendered, DEFAULT_UPSTREAM_BUDGET_CHARS);
    assert_eq!(section, rendered.join(SECTION_SEPARATOR));
    assert!(!report.truncated_any());
    assert_eq!(report.omitted, 0);
    assert_eq!(report.notice(), None);
}

/// The reported failure's exact provider string is recognised, and the
/// rewrite drops the advice that cannot be followed while keeping the
/// provider's own words.
#[test]
fn the_reported_provider_refusal_is_translated() {
    let raw = r#"tinyagents harness run failed: model error: inference returned 400 Bad \
         Request: {"success":false,"error":"The conversation is too long for model \
         'chat-v1'. Please start a new chat.","errorCode":"CONTEXT_LENGTH_EXCEEDED"}"#;
    let advice = context_overflow_advice(raw).expect("recognised as a context overflow");
    assert!(
        advice.contains("no chat to restart"),
        "the unactionable vendor advice is contradicted: {advice}"
    );
    assert!(
        advice.contains("summarise each source"),
        "a remedy is named: {advice}"
    );
    assert!(
        advice.contains("session history"),
        "the remaining suspects are named: {advice}"
    );
    assert!(
        advice.contains("CONTEXT_LENGTH_EXCEEDED"),
        "the provider's own words survive for support: {advice}"
    );
}

/// The other providers' wordings are recognised too — the phrasing is the
/// provider's, and a BYOK tenant reaches several.
#[test]
fn other_provider_wordings_are_recognised() {
    for raw in [
        "This model's maximum context length is 8192 tokens",
        "prompt is too long: 210000 tokens > 200000 maximum",
        "error code: context_length_exceeded",
        "input exceeds the context window",
    ] {
        assert!(
            context_overflow_advice(raw).is_some(),
            "not recognised: {raw}"
        );
    }
}

/// Anything that is not a context overflow is left exactly as it was — this
/// must never become a catch-all that relabels unrelated failures.
#[test]
fn unrelated_errors_are_not_relabelled() {
    for raw in [
        "inference returned 401 Unauthorized",
        "tool_call 'web_fetch': URL is not allowed",
        "the conversation was cancelled",
        "",
    ] {
        assert_eq!(context_overflow_advice(raw), None, "wrongly claimed: {raw}");
    }
}
