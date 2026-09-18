use super::*;

#[test]
fn iso8601_formats_a_known_instant() {
    // 2024-01-02T03:04:05Z, independently computed.
    assert_eq!(iso8601(1_704_164_645_000), "2024-01-02T03:04:05Z");
}

#[test]
fn iso8601_formats_the_epoch() {
    assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
}

#[test]
fn generated_ids_are_distinct_and_monotonic() {
    let a = generate_id();
    let b = generate_id();
    assert_ne!(a, b);
    // Zero-padded fixed-width hex makes lexicographic order match mint order.
    assert!(b > a, "expected {b} > {a}");
}

#[test]
fn agent_slug_lowercases_and_joins_words_with_underscores() {
    assert_eq!(agent_slug("Dana Designer"), "dana_designer");
    assert_eq!(agent_slug("Backend Engineer"), "backend_engineer");
    assert_eq!(agent_slug("QA2"), "qa2");
}

#[test]
fn agent_slug_collapses_and_trims_separator_runs() {
    assert_eq!(agent_slug("Designer!!"), "designer");
    assert_eq!(agent_slug("  Head   of   Ops  "), "head_of_ops");
    assert_eq!(agent_slug("__Dana--Designer__"), "dana_designer");
    assert_eq!(agent_slug("Ana Maria (Growth)"), "ana_maria_growth");
}

/// A name with no ASCII letter to start on has no readable slug in it, and
/// none is invented — it takes the shared stem instead.
#[test]
fn agent_slug_falls_back_for_names_with_no_legal_stem() {
    for name in ["***", "", "   ", "24/7 Support", "設計者", "🙂", "7", "_"] {
        assert_eq!(
            agent_slug(name),
            AGENT_SLUG_FALLBACK,
            "expected the fallback stem for {name:?}"
        );
    }
}

#[test]
fn agent_slug_caps_length_without_a_trailing_separator() {
    // The cap is a hard byte cap, not a word boundary: a word straddling it
    // is truncated rather than dropped, which keeps the rule one sentence.
    let long = "Chief ".repeat(40);
    let slug = agent_slug(&long);
    assert_eq!(slug.len(), AGENT_SLUG_MAX);
    assert!(slug.starts_with("chief_chief_"), "{slug}");
    assert!(
        !slug.ends_with('_'),
        "cap left a dangling separator: {slug}"
    );

    // When the cap lands exactly on a separator, that separator goes with
    // it — a roster id never ends in `_`.
    let aligned = "abcdefg ".repeat(20);
    let slug = agent_slug(&aligned);
    assert_eq!(slug.len(), AGENT_SLUG_MAX - 1, "{slug}");
    assert!(slug.ends_with("abcdefg"), "{slug}");

    let dense = "a".repeat(100);
    assert_eq!(agent_slug(&dense).len(), AGENT_SLUG_MAX);
}

/// The one property that matters: whatever comes out is something the
/// manifest validator would accept as a roster id. Coupled to the real
/// `is_snake_case` rather than a copy of its rules, so a change to the
/// grammar fails here instead of shipping ids the validator rejects.
#[test]
fn every_slug_satisfies_the_manifest_id_grammar() {
    let mut names: Vec<String> = [
        "Dana Designer",
        "Designer!!",
        "***",
        "",
        "24/7 Support",
        "設計者",
        "🙂 Growth 🙂",
        "O'Brien-Smith, Jr.",
        "___",
        "9Lives",
        "a",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    names.push("Chief ".repeat(40));
    names.push("a".repeat(100));
    for name in &names {
        let slug = agent_slug(name);
        assert!(
            crate::company::is_snake_case(&slug),
            "slug {slug:?} from name {name:?} is not a legal roster id"
        );
    }
}
