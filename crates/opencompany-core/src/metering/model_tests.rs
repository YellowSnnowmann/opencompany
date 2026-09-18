use super::*;

/// Every literal the classifier is capable of emitting.
fn every_slug() -> Vec<&'static str> {
    let mut slugs: Vec<&'static str> = EXACT.to_vec();
    for vendor in VENDORS {
        slugs.push(vendor.slug);
        slugs.extend(vendor.lines.iter().map(|(_, slug)| *slug));
    }
    slugs.sort_unstable();
    slugs.dedup();
    slugs
}

#[test]
fn a_workload_tier_is_named_rather_than_other() {
    // The ratchet that stops the tier vocabulary rotting: the moment a
    // workload tier stops classifying via `EXACT`, historic usage rows
    // that were metered as a tier name (the legacy arm, keys rework issue
    // #2306 slice 2d) start reporting `other`, and this test says so.
    for tier in crate::company::INFERENCE_TIERS {
        assert_ne!(
            ModelSlug::classify(tier),
            ModelSlug::OTHER,
            "the workload tier `{tier}` classifies to `other`; add it to EXACT"
        );
    }
}

#[test]
fn classifying_a_slug_again_returns_the_same_slug() {
    // What makes `Deserialize` (which re-classifies) safe to apply to a
    // value this crate wrote.
    for slug in every_slug() {
        assert_eq!(
            ModelSlug::classify(slug).as_str(),
            slug,
            "`{slug}` is not a fixed point of the classifier"
        );
    }
}

#[test]
fn an_operator_named_model_never_reaches_the_slug() {
    // The BYOK leak this type exists to stop: a self-hosted endpoint can
    // name a model after the customer it was built for.
    for raw in [
        "acme-corp-internal-v3",
        "northwind-legal-review",
        "ollama/my-finetune-2026-01",
        "hr-screening-model",
        "",
        "   ",
    ] {
        let slug = ModelSlug::classify(raw);
        assert_eq!(slug, ModelSlug::OTHER, "`{raw}` should classify to other");
        assert!(
            !raw.to_ascii_lowercase().contains(slug.as_str()),
            "`{raw}` leaked into the slug `{slug}`"
        );
    }
}

#[test]
fn a_known_vendor_line_is_named_at_the_granularity_it_is_priced_at() {
    for (raw, expected) in [
        ("anthropic/claude-sonnet-4-6", "anthropic-sonnet"),
        ("Claude-3-Haiku", "anthropic-haiku"),
        ("anthropic/claude-opus-4-1", "anthropic-opus"),
        ("anthropic/claude-next", "anthropic"),
        ("openai/gpt-5.2", "openai-gpt"),
        ("o3-mini", "openai"),
        ("google/gemini-2.5-pro", "google-gemini-pro"),
        ("google/gemini-2.5-flash", "google-gemini-flash"),
        ("deepseek/deepseek-v4-flash", "deepseek-v4-flash"),
        ("deepseek/deepseek-v4-pro", "deepseek-v4-pro"),
        ("deepseek/deepseek-r1", "deepseek"),
        ("qwen/qwen3.8-max", "qwen3-max"),
        ("qwen/qwen3.7-plus", "qwen3-plus"),
        ("meta-llama/llama-4-70b", "meta-llama"),
        ("mistralai/mistral-large", "mistral"),
        ("chat-v1", "chat-v1"),
        ("AGENTIC-V1", "agentic-v1"),
    ] {
        assert_eq!(
            ModelSlug::classify(raw).as_str(),
            expected,
            "classifying `{raw}`"
        );
    }
}

#[test]
fn a_slug_serializes_as_its_literal_and_re_folds_on_the_way_back() {
    let slug = ModelSlug::classify("anthropic/claude-sonnet-4-6");
    let json = serde_json::to_string(&slug).unwrap();
    assert_eq!(json, "\"anthropic-sonnet\"");
    assert_eq!(serde_json::from_str::<ModelSlug>(&json).unwrap(), slug);

    // A store row that somehow holds raw operator text — a hand-edited
    // document, a forked build, a foreign writer — is folded on read, so
    // the raw name cannot reach a reader through the store.
    let smuggled: ModelSlug = serde_json::from_str("\"acme-corp-internal-v3\"").unwrap();
    assert_eq!(smuggled, ModelSlug::OTHER);
}
