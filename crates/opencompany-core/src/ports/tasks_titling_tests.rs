use super::*;

// ── the card headline's shape invariant ──────────────────────────────────

/// Every constructor bounds its result, whatever it was handed. This is the
/// property a `String` field could not have: a producer that shoved a
/// paragraph in used to get a paragraph back.
#[test]
fn no_constructor_can_produce_an_unbounded_title() {
    let paragraph = "hey can you take a look at the pricing page, I think the tiers are \
                     confusing and we should probably reword the middle one because \
                     nobody I have shown it to can tell me what it is actually for";
    for title in [
        TaskTitle::authored(paragraph),
        TaskTitle::system(paragraph),
        TaskTitle::truncated(paragraph),
        TaskTitle::summarised(paragraph).expect("a paragraph still names something"),
    ] {
        assert!(
            title.as_str().chars().count() <= TASK_TITLE_MAX_CHARS,
            "{title}"
        );
        assert!(!title.as_str().contains('\n'));
    }
}

/// The cap counts characters, not bytes, and budgets the ellipsis inside
/// itself. A byte slice here would panic mid-codepoint; an unbudgeted
/// ellipsis would return one character more than the type advertises.
#[test]
fn the_cap_is_utf8_safe_and_includes_its_own_ellipsis() {
    let long = "價".repeat(TASK_TITLE_MAX_CHARS + 40);
    let title = TaskTitle::truncated(&long);
    assert_eq!(title.as_str().chars().count(), TASK_TITLE_MAX_CHARS);
    assert!(title.as_str().ends_with('…'));

    let exact = "a".repeat(TASK_TITLE_MAX_CHARS);
    assert_eq!(TaskTitle::truncated(&exact).as_str(), exact);
}

/// A title never breaks mid-word — the truncator prefers the last whole one.
#[test]
fn a_shortened_title_stops_at_a_word() {
    let title = TaskTitle::truncated(
        "Reword the middle pricing tier and also the top one and the bottom one              and everything else on that page",
    );
    assert!(title.as_str().ends_with('…'));
    assert!(!title.as_str().contains("  "));
}

/// Whitespace-only and empty are the same answer: nothing. A caller that
/// gets this back is expected to refuse rather than open a blank card.
#[test]
fn nothing_in_is_nothing_out() {
    for text in ["", "   ", "\n\t\n", "  \n  \n "] {
        assert!(TaskTitle::authored(text).is_empty(), "{text:?}");
        assert!(TaskTitle::truncated(text).is_empty(), "{text:?}");
        assert!(TaskTitle::summarised(text).is_none(), "{text:?}");
    }
}

/// A request that is already a good title is not degraded by passing
/// through — the commonest input on the board, and the easiest to break.
#[test]
fn an_already_good_title_survives_unchanged() {
    for good in [
        "Fix the login redirect",
        "Draft the Q3 board update",
        "Ship v2 (phase 1)",
        "Why is the pricing page slow?",
    ] {
        assert_eq!(TaskTitle::authored(good).as_str(), good, "{good}");
    }
}

/// A one-word ask is a one-word title, not padding and not an ellipsis.
#[test]
fn a_one_word_request_is_a_one_word_title() {
    assert_eq!(TaskTitle::truncated("ship").as_str(), "ship");
}

/// Casing is never touched. Upper-casing the first character reads well on
/// a sentence and corrupts every name that starts with a deliberately
/// lower-case token — which is most tool names, some brands, and every
/// title derived from a file.
#[test]
fn a_deliberately_lower_case_name_is_not_restyled() {
    for name in [
        "iPhone sync is broken",
        "notes.md",
        "npm audit is failing",
        "kubectl context keeps resetting",
        "eBay listing export",
    ] {
        assert_eq!(TaskTitle::system(name).as_str(), name, "{name}");
        assert_eq!(TaskTitle::authored(name).as_str(), name, "{name}");
    }
}

/// A multi-paragraph brief is reduced to its first line, so the detail
/// cannot ride into the headline — the note is where it belongs.
#[test]
fn a_multi_paragraph_brief_keeps_only_its_first_line() {
    let title = TaskTitle::summarised(
        "Reword the middle pricing tier\n\nBackground: three customers have \
         asked what it means.\n\nDeadline: Friday.",
    )
    .expect("a brief names something");
    assert_eq!(title.as_str(), "Reword the middle pricing tier");
}

/// The wrappers and preambles a model reaches for come off, including when
/// they are nested the other way round.
#[test]
fn model_decoration_is_stripped_rather_than_trusted() {
    for decorated in [
        "\"Reword the middle pricing tier\"",
        "**Reword the middle pricing tier**",
        "`Reword the middle pricing tier`",
        "Title: Reword the middle pricing tier",
        "Task: \"Reword the middle pricing tier\"",
        "\"Title: Reword the middle pricing tier\"",
        // Decoration nested three deep. Each layer hides the next from a
        // single-pass stripper, which is why the pass runs to a fixed point.
        "Task: \"Reword the middle pricing tier\".",
        "**Title: Reword the middle pricing tier.**",
        "\"**Reword the middle pricing tier**\"",
        "# Reword the middle pricing tier",
        "### Reword the middle pricing tier",
        "Reword the middle pricing tier.",
        "_Reword the middle pricing tier_",
        "“Reword the middle pricing tier”",
    ] {
        assert_eq!(
            TaskTitle::summarised(decorated).expect(decorated).as_str(),
            "Reword the middle pricing tier",
            "{decorated}"
        );
    }
}

/// Punctuation that is part of the name stays. Only sentence-ending
/// decoration is stripped, or `Ship v2 (phase 1)` loses its bracket.
#[test]
fn punctuation_inside_a_name_is_content_not_decoration() {
    assert_eq!(
        TaskTitle::summarised("Ship v2 (phase 1)")
            .expect("a title")
            .as_str(),
        "Ship v2 (phase 1)"
    );
    assert_eq!(
        TaskTitle::summarised("Why is checkout slow?")
            .expect("a title")
            .as_str(),
        "Why is checkout slow?"
    );
}

/// A reply that is nothing but decoration names nothing, so the caller
/// falls back rather than putting punctuation on the board.
#[test]
fn decoration_with_no_name_in_it_is_no_title() {
    for junk in [
        "\"\"",
        "**",
        "...",
        "Title:",
        "``",
        "#",
        // Odd counts and unpaired marks: these do not peel to nothing, they
        // peel to ONE punctuation character, which a length check passes.
        "\"\"\"",
        "*",
        "-",
        "—",
        "?!",
        "'",
        "\"\"\"\"\"",
        "   \"\"\"   ",
    ] {
        assert!(TaskTitle::summarised(junk).is_none(), "{junk:?}");
    }
}

/// A person's own title is kept whatever it is made of. The junk test that
/// rejects an unusable *model reply* must never reach these constructors:
/// somebody who names a card `🚀` means it, and blanking it persists a card
/// with no headline — worse than the punctuation title the test prevents.
#[test]
fn a_symbol_only_title_a_person_chose_is_kept() {
    for chosen in ["🚀", "✅", "---", "???", "42", "#1"] {
        assert_eq!(TaskTitle::authored(chosen).as_str(), chosen, "{chosen}");
        assert_eq!(TaskTitle::system(chosen).as_str(), chosen, "{chosen}");
        assert!(!TaskTitle::truncated(chosen).is_empty(), "{chosen}");
    }
    // …and the same text from a model is still refused, because that is a
    // guess at a name rather than somebody's choice of one.
    assert!(TaskTitle::summarised("---").is_none());
    assert!(TaskTitle::summarised("🚀").is_none());
}

/// Non-Latin scripts are neither mangled nor case-folded — the pass is
/// character-wise, and upper-casing is a no-op where a script has no case.
#[test]
fn a_non_english_title_is_left_intact() {
    for text in [
        "価格ページの中段プランを書き直す",
        "Переписать средний тариф",
        "إعادة صياغة الفئة الوسطى",
    ] {
        assert_eq!(
            TaskTitle::summarised(text).expect(text).as_str(),
            text,
            "{text}"
        );
    }
}

/// A stored board loads back exactly as it was written, raw-message titles
/// and all. Normalising on read would silently rewrite durable records, and
/// re-summarising would make the board unstable between refreshes.
#[test]
fn a_title_stored_by_an_older_build_round_trips_verbatim() {
    let legacy = "hey can you take a look at the pricing page, I think the tiers are…";
    let json = serde_json::to_string(&legacy).expect("serialises");
    let loaded: TaskTitle = serde_json::from_str(&json).expect("deserialises");
    assert_eq!(loaded.as_str(), legacy);
    assert_eq!(serde_json::to_string(&loaded).expect("re-serialises"), json);
}

/// The type is transparent on the wire, so no stored board needs migrating
/// and no console field changes shape.
#[test]
fn a_title_is_a_bare_string_on_the_wire() {
    let json = serde_json::to_string(&TaskTitle::authored("Ship it")).expect("serialises");
    assert_eq!(json, "\"Ship it\"");
}

/// No titler wired is the offline company and the default build: the card
/// is named exactly as it was before any of this existed.
#[tokio::test]
async fn without_a_titler_a_card_is_named_by_shortening_the_request() {
    let request = "hey can you take a look at the pricing page, I think the tiers are \
                   confusing and we should probably reword the middle one";
    assert_eq!(
        mint_task_title(request, None, None).await,
        TaskTitle::truncated(request)
    );
}

/// A titler that cannot answer — unreachable, too slow, unreadable — leaves
/// the card named, never unnamed and never failed.
#[tokio::test]
async fn a_titler_that_declines_falls_back_rather_than_failing() {
    struct Silent;

    #[async_trait]
    impl TitleSummariser for Silent {
        async fn title(&self, _request: &str) -> Option<TaskTitle> {
            None
        }
    }

    let request = "reword the middle pricing tier please";
    assert_eq!(
        mint_task_title(request, None, Some(&Silent)).await,
        TaskTitle::truncated(request)
    );
    assert!(
        !mint_task_title(request, None, Some(&Silent))
            .await
            .is_empty()
    );
}

/// The fix itself, at the seam: a rambling ask is named after the **work**,
/// and the headline is no longer the message wearing an ellipsis.
#[tokio::test]
async fn a_rambling_ask_is_named_after_the_work() {
    struct Names(&'static str);

    #[async_trait]
    impl TitleSummariser for Names {
        async fn title(&self, _request: &str) -> Option<TaskTitle> {
            TaskTitle::summarised(self.0)
        }
    }

    let request = "hey can you take a look at the pricing page, I think the tiers are \
                   confusing and we should probably reword the middle one";
    let title = mint_task_title(
        request,
        None,
        Some(&Names("Reword the middle pricing tier")),
    )
    .await;

    assert_eq!(title.as_str(), "Reword the middle pricing tier");
    // The property that actually broke: the headline is not the message.
    assert!(
        !request.starts_with(title.as_str().trim_end_matches('…')),
        "the title is still an excerpt of the request: {title}"
    );
    assert!(!title.as_str().ends_with('…'));
}
