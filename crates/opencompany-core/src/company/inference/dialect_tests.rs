use super::*;

/// One endpoint for the table tests: the rules under test are the static
/// ones, and only a learned omission is endpoint-scoped.
const AT: &str = "https://api.example/v1";

#[test]
fn an_intent_with_no_opinion_puts_nothing_on_the_wire() {
    // The original defect, stated as intent: `Default` is "no opinion", and
    // `unwrap_or(0.0)` turned it into the most opinionated value there is.
    assert!(Sampling::Default.knobs().is_empty());
    assert!(translate(AT, "claude-sonnet-5", Sampling::Default.knobs()).is_empty());
}

#[test]
fn determinism_survives_a_model_that_forbids_a_temperature() {
    // The whole point of the intent layer. Anthropic post-4.6 rejects every
    // temperature but 1.0, so a caller asking for determinism used to get a
    // hard 400. Now the request goes out, and `seed` carries what
    // repeatability is available.
    let fields = translate(AT, "claude-sonnet-5", Sampling::Deterministic.knobs());
    let by_name: std::collections::HashMap<_, _> = fields.into_iter().collect();
    assert_eq!(by_name["temperature"], serde_json::json!(1.0));
    assert!(
        by_name.contains_key("seed"),
        "seed is the lever that still works here"
    );
}

#[test]
fn the_same_intent_is_spelled_differently_per_model() {
    // One intent, three dialects, no caller aware of any of them.
    let anthropic = translate(
        AT,
        "anthropic/claude-opus-5",
        Sampling::Deterministic.knobs(),
    );
    assert!(
        anthropic
            .iter()
            .any(|(k, v)| k == "temperature" && *v == serde_json::json!(1.0))
    );

    let openai = translate(AT, "gpt-6-astra", Sampling::Deterministic.knobs());
    assert!(
        !openai.iter().any(|(k, _)| k == "temperature"),
        "a reasoning model takes no temperature at all"
    );

    let ordinary = translate(AT, "llama3:latest", Sampling::Deterministic.knobs());
    assert!(
        ordinary
            .iter()
            .any(|(k, v)| k == "temperature" && *v == serde_json::json!(0.0))
    );
}

#[test]
fn a_namespaced_id_and_a_bare_id_get_the_same_answer() {
    // The same model arrives bare from a direct vendor and namespaced from a
    // gateway. A rule that only matched one of the two would be right half
    // the time and silent about the other half.
    assert_eq!(
        rule_for(AT, "claude-opus-5", "temperature"),
        rule_for(AT, "anthropic/claude-opus-5", "temperature")
    );
}

#[test]
fn a_rename_moves_the_value_and_drops_the_old_name() {
    let fields = translate(AT, "gpt-5.6-sol", vec![Knob::new("max_tokens", 16384)]);
    assert_eq!(
        fields,
        vec![(
            "max_completion_tokens".to_string(),
            serde_json::json!(16384)
        )]
    );
}

#[test]
fn a_clamp_brings_a_value_inside_the_range_rather_than_failing() {
    let fields = translate(
        AT,
        "meta-llama/Llama-3.3-70B",
        vec![Knob::new("temperature", 1.8)],
    );
    assert_eq!(
        fields,
        vec![("temperature".to_string(), serde_json::json!(1.0))]
    );
}

/// The nine in-repo workloads asking for determinism reach the providers
/// through a vendored `ModelRequest` that carries a bare float, so the intent
/// has to survive that round trip or the call sites are decorative.
#[test]
fn determinism_survives_the_vendored_float_boundary() {
    assert_eq!(
        Sampling::from_request(Some(DETERMINISTIC)),
        Sampling::Deterministic
    );
    assert_eq!(Sampling::from_request(None), Sampling::Default);
    // A caller that named a real value is not reinterpreted: triage keeps
    // 0.2 deliberately, "so a genuinely borderline message is not forced".
    assert_eq!(Sampling::from_request(Some(0.2)), Sampling::Exact(0.2));

    // And the whole point: that intent, through the boundary, onto a model
    // that rejects every temperature — without a 400.
    let fields = translate(
        AT,
        "claude-opus-5",
        Sampling::from_request(Some(DETERMINISTIC)).knobs(),
    );
    assert!(
        fields
            .iter()
            .any(|(k, v)| k == "temperature" && *v == serde_json::json!(1.0))
    );
}

#[test]
fn an_unknown_model_is_free_rather_than_guessed_at() {
    // The table is an optimisation. A model nobody has written a row for
    // gets exactly what the caller asked for, and the 400-learning layer
    // corrects us if that turns out to be wrong.
    assert_eq!(
        rule_for(AT, "some-model-nobody-has-seen", "temperature"),
        Rule::Free
    );
}

#[test]
fn a_rejection_blames_only_a_parameter_we_actually_sent() {
    let sent = vec!["temperature".to_string(), "max_tokens".to_string()];
    assert_eq!(
        parameter_blamed_by(
            "Unsupported value: 'temperature' does not support 0.2 with this model.",
            &sent
        )
        .as_deref(),
        Some("temperature")
    );
    // A field we did not send is not ours to drop.
    assert_eq!(
        parameter_blamed_by("Unsupported parameter: 'logit_bias'", &sent),
        None
    );
    // And a 400 that is not about the request's shape teaches us nothing.
    assert_eq!(
        parameter_blamed_by("the temperature in Paris is 19 degrees", &sent),
        None
    );
}

/// The retry drops one parameter, not the request's meaning. Whatever the
/// model rejected, the messages and the model id are still what the caller
/// asked for — otherwise "retry once" would silently answer a different
/// question.
#[test]
fn a_rejection_names_one_parameter_and_only_from_what_we_sent() {
    let sent = vec![
        "temperature".to_string(),
        "max_completion_tokens".to_string(),
    ];
    // A rename means the wire name is not the caller's name, so the blame
    // has to be matched against what actually went out.
    assert_eq!(
        parameter_blamed_by(
            "400: Unsupported parameter: 'max_completion_tokens' is not supported",
            &sent
        )
        .as_deref(),
        Some("max_completion_tokens")
    );
    // Nothing we sent is named, so there is nothing to drop and no retry.
    assert_eq!(
        parameter_blamed_by(
            "400: Extra inputs are not permitted: 'reasoning_effort'",
            &sent
        ),
        None
    );
}

#[test]
fn what_is_learned_from_a_rejection_outranks_the_table() {
    // The property that makes the table an optimisation rather than a
    // dependency: a vendor that changes silently corrects us without a
    // release.
    let model = "learning-test-model-v1";
    assert_eq!(rule_for(AT, model, "top_p"), Rule::Free);
    remember_omit(AT, model, "top_p");
    assert_eq!(rule_for(AT, model, "top_p"), Rule::Omit);
    assert!(translate(AT, model, vec![Knob::new("top_p", 0.5)]).is_empty());
}

#[test]
fn a_rejection_is_learned_about_the_endpoint_that_made_it() {
    // A model id is not unique across endpoints. Two OpenAI-compatible
    // gateways both publishing one id are two services, and one of them
    // refusing a parameter says nothing about the other — keyed on the
    // model alone, one gateway's 400 dropped that parameter from every
    // later request for that id in every company on this host, and for
    // `max_tokens` that is a bill rather than a difference.
    let model = "endpoint-scope-test-model-v1";
    let one = "https://gateway-one.example/v1";
    let two = "https://gateway-two.example/v1";

    remember_omit(one, model, "max_tokens");
    assert_eq!(rule_for(one, model, "max_tokens"), Rule::Omit);
    assert_eq!(
        rule_for(two, model, "max_tokens"),
        Rule::Free,
        "the other gateway never rejected anything"
    );

    // Same service, different operation: `/chat/completions` and
    // `/responses` are one endpoint answering two ways, so what one learns
    // the other knows.
    for operation in [
        "https://gateway-one.example/v1/chat/completions",
        "https://gateway-one.example/v1/responses",
        "https://gateway-one.example/v1/",
    ] {
        assert_eq!(
            rule_for(operation, model, "max_tokens"),
            Rule::Omit,
            "{operation} is the same service"
        );
    }

    // But a path-routed gateway is several services behind one origin, and
    // that is the arrangement most likely to disagree about a parameter at
    // all — one upstream's 400 says nothing about the next.
    assert_eq!(
        rule_for(
            "https://gateway-one.example/vendor-b/v1",
            model,
            "max_tokens"
        ),
        Rule::Free,
        "another upstream behind the same host has not rejected anything"
    );
}
