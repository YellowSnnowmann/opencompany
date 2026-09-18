use super::*;

fn parse(text: &str) -> CompanyManifest {
    toml::from_str(text).expect("valid toml")
}

/// Every problem mentioning `harness`, so a test asserting on this block is
/// not perturbed by unrelated validation output.
fn harness_problems(m: &CompanyManifest) -> Vec<String> {
    m.validate()
        .into_iter()
        .filter(|p| p.contains("harness"))
        .collect()
}

const BASE: &str = "[company]\nname = \"X\"\n\n[[agent]]\nid = \"ceo\"\nrole = \"CEO\"\n";

/// The compatibility case, and the one every shipped company under
/// `companies/` hits: no `[[harness]]` block at all still yields exactly one
/// harness — `built_in`, default, on the company-level `[inference]`.
///
/// This is the test that makes "named harnesses" a purely additive feature.
#[test]
fn a_manifest_with_no_harness_block_gets_one_implicit_built_in_default() {
    let manifest = parse(BASE);

    assert!(manifest.harnesses.is_empty(), "nothing was declared");

    let effective = manifest.effective_harnesses();
    assert_eq!(effective.len(), 1);
    assert_eq!(effective[0].id, IMPLICIT_HARNESS_ID);
    assert_eq!(effective[0].kind, "built_in");
    assert!(effective[0].default);
    assert!(effective[0].inference.is_none(), "inherits `[inference]`");

    assert_eq!(manifest.default_harness_id(), IMPLICIT_HARNESS_ID);
    assert_eq!(
        manifest.harness_for("ceo").map(|h| h.id),
        Some(IMPLICIT_HARNESS_ID.to_string()),
        "an agent naming no harness lands on the implicit one"
    );
    assert!(harness_problems(&manifest).is_empty());
}

/// `default_harness` resolves the same entry `default_harness_id` names,
/// full struct and all — for both the implicit `built_in` case and a
/// declared `acp` default. Pinned separately from `default_harness_id`
/// because `lanes::build` (issue #1244) reads `.kind` off this to decide
/// whether the default lane is even runnable; a lookup that silently
/// resolved to the wrong harness would reintroduce the bug that fixed.
#[test]
fn default_harness_resolves_the_full_declared_entry() {
    let implicit = parse(BASE);
    assert_eq!(implicit.default_harness().id, IMPLICIT_HARNESS_ID);
    assert_eq!(implicit.default_harness().kind, "built_in");

    let acp_default = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"laptop\"\nkind = \"acp\"\ndefault = true\n\n\
         [harness.acp]\ntransport = \"local\"\nagent = \"claude\"\n"
    ));
    assert_eq!(acp_default.default_harness().id, "laptop");
    assert_eq!(acp_default.default_harness().kind, "acp");
}

/// Naming a harness when none is declared is an error rather than a silent
/// fallback to the implicit one: the operator wrote down an intent, and
/// quietly ignoring it is how "my agent is on the wrong model" happens.
#[test]
fn naming_a_harness_with_no_harness_block_is_rejected() {
    let manifest = parse(&format!("{BASE}harness = \"deep\"\n"));
    let problems = harness_problems(&manifest);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("ceo") && problems[0].contains("deep"));
}

#[test]
fn agents_route_to_their_named_harness_and_others_to_the_default() {
    let manifest = parse(
        r#"
[company]
name = "X"

[[agent]]
id = "ceo"
role = "CEO"

[[agent]]
id = "researcher"
role = "Researcher"
harness = "deep"

[[harness]]
id = "embedded"
kind = "built_in"
default = true

[[harness]]
id = "deep"
kind = "built_in"

[harness.inference]
provider = "openrouter"
"#,
    );
    assert!(harness_problems(&manifest).is_empty());
    assert_eq!(manifest.default_harness_id(), "embedded");
    assert_eq!(
        manifest.harness_for("ceo").map(|h| h.id),
        Some("embedded".to_string())
    );
    assert_eq!(
        manifest.harness_for("researcher").map(|h| h.id),
        Some("deep".to_string())
    );
    // The sub-table attached to the *second* entry, not the first — the
    // array-of-tables shape that is easy to misread.
    let deep = manifest.harness_for("researcher").expect("declared");
    assert_eq!(
        deep.inference.as_ref().and_then(|i| i.provider.clone()),
        Some("openrouter".to_string())
    );
    assert!(
        manifest
            .effective_harnesses()
            .iter()
            .find(|h| h.id == "embedded")
            .expect("declared")
            .inference
            .is_none()
    );
}

#[test]
fn an_agent_naming_an_undeclared_harness_is_rejected() {
    let manifest = parse(&format!(
        "{BASE}harness = \"ghost\"\n\n[[harness]]\nid = \"embedded\"\nkind = \"built_in\"\ndefault = true\n"
    ));
    let problems = harness_problems(&manifest);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("ghost") && problems[0].contains("ceo"));
    assert!(
        problems[0].contains("embedded"),
        "names what IS declared: {}",
        problems[0]
    );
}

/// Issue #1245's detected-harness follow-up: whether `claude-agent-acp` is
/// installed is a fact about the **machine**, so binding to it must not
/// require a `company.toml` edit — the same manifest is opened from
/// machines where the answer differs. Accepted with a harness block
/// declared and without one, and it resolves to a `local` acp harness.
#[test]
fn a_coding_cli_is_bindable_without_being_declared() {
    for tail in [
        "",
        "\n[[harness]]\nid = \"embedded\"\nkind = \"built_in\"\ndefault = true\n",
    ] {
        let manifest = parse(&format!("{BASE}harness = \"claude\"\n{tail}"));
        assert!(
            harness_problems(&manifest).is_empty(),
            "`claude` needs no declaration ({tail:?}): {:?}",
            harness_problems(&manifest)
        );

        let resolved = manifest.harness_for("ceo").expect("resolves");
        assert_eq!(resolved.id, "claude");
        assert_eq!(resolved.kind, "acp");
        let acp = resolved.acp.expect("acp section");
        assert_eq!(acp.transport, "local");
        assert_eq!(acp.agent.as_deref(), Some("claude"));
    }
}

/// The synthesized harness must never be the default: which harness an
/// *unbound* teammate runs on stays a blueprint decision, or something a
/// machine happens to have installed could silently redirect the roster.
#[test]
fn an_implicit_local_harness_is_never_the_default() {
    let manifest = parse(&format!("{BASE}harness = \"claude\"\n"));
    assert_ne!(manifest.default_harness_id(), "claude");
    assert!(manifest.default_harness().is_built_in());
    assert!(!Harness::implicit_local("claude").default);
}

/// A declared `[[harness]]` of the same id wins — otherwise a company that
/// deliberately pinned a model on its `claude` harness would silently get
/// the bare synthesized one instead.
#[test]
fn a_declared_harness_wins_over_the_synthesized_one() {
    let manifest = parse(&format!(
        "{BASE}harness = \"claude\"\n\n[[harness]]\nid = \"embedded\"\nkind = \"built_in\"\ndefault = true\n\n\
         [[harness]]\nid = \"claude\"\nkind = \"acp\"\n\n[harness.acp]\ntransport = \"local\"\nagent = \"claude\"\nmodel = \"opus-4-5\"\n"
    ));
    assert!(harness_problems(&manifest).is_empty());
    let resolved = manifest.harness_for("ceo").expect("resolves");
    assert_eq!(
        resolved.acp.expect("acp").model.as_deref(),
        Some("opus-4-5"),
        "the declared harness, not the synthesized one"
    );
}

#[test]
fn duplicate_harness_ids_are_rejected() {
    let manifest = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"built_in\"\ndefault = true\n\n[[harness]]\nid = \"a\"\nkind = \"built_in\"\n"
    ));
    let problems = harness_problems(&manifest);
    assert!(
        problems.iter().any(|p| p.contains("more than once")),
        "{problems:?}"
    );
}

/// Zero and two defaults are both errors. Zero would leave an agent naming
/// no harness with nowhere to run; two makes the answer depend on list
/// order, which is exactly what marking a default exists to avoid.
#[test]
fn there_must_be_exactly_one_default_harness() {
    let none = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"built_in\"\n\n[[harness]]\nid = \"b\"\nkind = \"built_in\"\n"
    ));
    let problems = harness_problems(&none);
    assert!(
        problems.iter().any(|p| p.contains("no `[[harness]]` sets")),
        "{problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("`a`") && p.contains("`b`")),
        "names the candidates: {problems:?}"
    );

    let two = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"built_in\"\ndefault = true\n\n[[harness]]\nid = \"b\"\nkind = \"built_in\"\ndefault = true\n"
    ));
    let problems = harness_problems(&two);
    assert!(
        problems.iter().any(|p| p.contains("2 `[[harness]]`")),
        "{problems:?}"
    );
}

/// A section on the wrong kind is an error, not an ignored key — both
/// directions.
#[test]
fn a_section_on_the_wrong_kind_is_an_error_not_an_ignored_key() {
    let inference_on_acp = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"acp\"\ndefault = true\n\n[harness.acp]\ntransport = \"local\"\nagent = \"claude\"\n\n[harness.inference]\nprovider = \"openrouter\"\n"
    ));
    let problems = harness_problems(&inference_on_acp);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("[harness.inference]") && p.contains("own credential")),
        "{problems:?}"
    );

    let acp_on_built_in = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"built_in\"\ndefault = true\n\n[harness.acp]\ntransport = \"local\"\nagent = \"claude\"\n"
    ));
    let problems = harness_problems(&acp_on_built_in);
    assert!(
        problems.iter().any(|p| p.contains("[harness.acp]")),
        "{problems:?}"
    );
}

#[test]
fn an_unknown_harness_kind_is_rejected_without_confusing_follow_on_problems() {
    let manifest = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"telepathy\"\ndefault = true\n"
    ));
    let problems = harness_problems(&manifest);
    assert_eq!(
        problems.len(),
        1,
        "one problem, not a cascade: {problems:?}"
    );
    assert!(problems[0].contains("telepathy") && problems[0].contains("built_in"));
}

/// Each ACP transport requires its own addressing field and forbids the
/// other's, so a manifest cannot claim to spawn a local agent *and* name a
/// remote runner.
#[test]
fn acp_transports_require_their_own_addressing_field() {
    let cases: &[(&str, &str)] = &[
        ("transport = \"local\"\n", "names no `agent`"),
        (
            "transport = \"local\"\nagent = \"claude\"\nrunner = \"laptop\"\n",
            "but names a `runner`",
        ),
        ("transport = \"runner\"\n", "names no `runner`"),
        (
            "transport = \"runner\"\nrunner = \"laptop\"\nagent = \"claude\"\n",
            "but names an `agent`",
        ),
        ("transport = \"carrier_pigeon\"\n", "must be one of"),
        (
            "transport = \"local\"\nagent = \"emacs\"\n",
            "must be one of",
        ),
    ];
    for (acp, expected) in cases {
        let manifest = parse(&format!(
            "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"acp\"\ndefault = true\n\n[harness.acp]\n{acp}"
        ));
        let problems = harness_problems(&manifest);
        assert!(
            problems.iter().any(|p| p.contains(expected)),
            "`{acp}` should report {expected:?}, got {problems:?}"
        );
    }
}

#[test]
fn a_valid_acp_harness_of_each_transport_passes() {
    for acp in [
        "transport = \"local\"\nagent = \"claude\"\n",
        "transport = \"runner\"\nrunner = \"stevens_laptop\"\n",
    ] {
        let manifest = parse(&format!(
            "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"acp\"\ndefault = true\n\n[harness.acp]\n{acp}"
        ));
        assert!(
            harness_problems(&manifest).is_empty(),
            "`{acp}` should be valid: {:?}",
            harness_problems(&manifest)
        );
    }
}

/// Issue #1245: `model` is a hint forwarded to the agent's own startup
/// lever, not a credential — so unlike `[harness.inference]` it is
/// perfectly valid on a `local` acp harness. It is rejected on `runner`
/// (no wire protocol yet) and when set to an empty string (nothing to
/// forward, and silently accepting it invites "my model setting does
/// nothing").
#[test]
fn model_is_valid_on_local_rejected_on_runner_and_must_not_be_empty() {
    let cases: &[(&str, Option<&str>)] = &[
        (
            "transport = \"local\"\nagent = \"claude\"\nmodel = \"claude-opus-4-5\"\n",
            None,
        ),
        (
            "transport = \"runner\"\nrunner = \"laptop\"\nmodel = \"claude-opus-4-5\"\n",
            Some("but names a `model`"),
        ),
        (
            "transport = \"local\"\nagent = \"claude\"\nmodel = \"   \"\n",
            Some("is set but empty"),
        ),
    ];
    for (acp, expected) in cases {
        let manifest = parse(&format!(
            "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"acp\"\ndefault = true\n\n[harness.acp]\n{acp}"
        ));
        let problems = harness_problems(&manifest);
        match expected {
            None => assert!(problems.is_empty(), "`{acp}` should be valid: {problems:?}"),
            Some(msg) => assert!(
                problems.iter().any(|p| p.contains(msg)),
                "`{acp}` should report {msg:?}, got {problems:?}"
            ),
        }
    }
}

/// Issue #1245's per-agent follow-up: `agent.model` follows the exact
/// same doctrine as `[harness.acp].model` — valid on `local`, rejected on
/// `runner`, rejected when empty — plus one rule the harness-level field
/// has no need for: on a `built_in` harness `model` is now half of the
/// keys rework's `{provider, model}` pair (slice 3a), so a bare `model`
/// with no `provider` is refused for naming an incomplete pair rather
/// than for having nowhere to forward to.
#[test]
fn agent_model_follows_the_harness_level_models_own_doctrine() {
    let cases: &[(&str, &str, Option<&str>)] = &[
        (
            "kind = \"built_in\"\ndefault = true",
            "model = \"opus-4-5\"",
            Some("names a `model` but no `provider`"),
        ),
        (
            "kind = \"acp\"\ndefault = true\n\n[harness.acp]\ntransport = \"local\"\nagent = \"claude\"",
            "model = \"opus-4-5\"",
            None,
        ),
        (
            "kind = \"acp\"\ndefault = true\n\n[harness.acp]\ntransport = \"runner\"\nrunner = \"laptop\"",
            "model = \"opus-4-5\"",
            Some("uses `transport = \"runner\"`"),
        ),
        (
            "kind = \"acp\"\ndefault = true\n\n[harness.acp]\ntransport = \"local\"\nagent = \"claude\"",
            "model = \"   \"",
            Some("is set but empty"),
        ),
    ];
    for (harness, agent_model, expected) in cases {
        let manifest = parse(&format!(
            "[company]\nname = \"X\"\n\n[[agent]]\nid = \"ceo\"\nrole = \"CEO\"\n{agent_model}\n\n\
             [[harness]]\nid = \"a\"\n{harness}\n"
        ));
        let problems = manifest.validate();
        match expected {
            None => assert!(
                !problems.iter().any(|p| p.contains("model")),
                "{harness} / {agent_model} should be valid: {problems:?}"
            ),
            Some(msg) => assert!(
                problems.iter().any(|p| p.contains(*msg)),
                "{harness} / {agent_model} should report {msg:?}, got {problems:?}"
            ),
        }
    }
}

/// The pair's happy path (keys rework slice 3a): both halves set together
/// on a `built_in` harness names no problem about either field.
#[test]
fn a_built_in_agent_may_pin_provider_and_model_together() {
    let mut manifest = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"built_in\"\ndefault = true\n"
    ));
    manifest.agents[0].provider = Some("anthropic".to_string());
    manifest.agents[0].model = Some("test-model-large".to_string());
    let problems = manifest.validate();
    assert!(
        !problems
            .iter()
            .any(|p| p.contains("provider") || p.contains("model")),
        "a full pair should be valid: {problems:?}"
    );
}

/// One half of the pair with no `provider` is refused on a `built_in`
/// harness — a model with nothing to serve it is not a config the resolver
/// can act on.
#[test]
fn a_built_in_agent_with_model_but_no_provider_is_refused() {
    let mut manifest = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"built_in\"\ndefault = true\n"
    ));
    manifest.agents[0].model = Some("test-model-large".to_string());
    let problems = manifest.validate();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("names a `model` but no `provider`")),
        "{problems:?}"
    );
}

/// The other half missing: a `provider` with no `model` is just as
/// incomplete a pair.
#[test]
fn a_built_in_agent_with_provider_but_no_model_is_refused() {
    let mut manifest = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"built_in\"\ndefault = true\n"
    ));
    manifest.agents[0].provider = Some("anthropic".to_string());
    let problems = manifest.validate();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("names a `provider` but no `model`")),
        "{problems:?}"
    );
}

/// An ACP agent brings its own credential — a `provider` naming a
/// console-managed one is refused outright, independent of `model`.
#[test]
fn an_acp_agent_may_not_name_a_provider() {
    let mut manifest = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"acp\"\ndefault = true\n\n\
         [harness.acp]\ntransport = \"local\"\nagent = \"claude\"\n"
    ));
    manifest.agents[0].provider = Some("anthropic".to_string());
    let problems = manifest.validate();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("which brings its own provider")),
        "{problems:?}"
    );
}

/// `provider` is checked for slug shape the same way a console-added
/// provider's slug is (`store::slugify`) — a manifest cannot see the
/// company's actual provider list, so this is the only check available at
/// load time.
#[test]
fn a_provider_slug_that_is_not_a_slug_is_refused() {
    let mut manifest = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"built_in\"\ndefault = true\n"
    ));
    manifest.agents[0].provider = Some("Anthropic API".to_string());
    manifest.agents[0].model = Some("test-model-large".to_string());
    let problems = manifest.validate();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("is not a provider slug")),
        "{problems:?}"
    );
}

/// G2: the pair rule must not go silent on the manifest shape every
/// shipped company without an explicit `[[harness]]` block has — the
/// implicit `built_in` default. `harness_for` resolves that case to the
/// synthesized implicit harness, so the incomplete-pair refusal still
/// fires with no `[[harness]]` section anywhere in the manifest.
#[test]
fn a_pair_on_a_manifest_with_no_harness_section_is_validated() {
    let mut manifest = parse(BASE);
    manifest.agents[0].provider = Some("anthropic".to_string());
    let problems = manifest.validate();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("names a `provider` but no `model`")),
        "the pair rule must run even with no `[[harness]]` declared: {problems:?}"
    );
}

#[test]
fn an_acp_harness_with_no_acp_section_is_rejected() {
    let manifest = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"a\"\nkind = \"acp\"\ndefault = true\n"
    ));
    let problems = harness_problems(&manifest);
    assert!(
        problems.iter().any(|p| p.contains("needs a `transport`")),
        "{problems:?}"
    );
}

#[test]
fn harness_ids_must_be_snake_case() {
    let manifest = parse(&format!(
        "{BASE}\n[[harness]]\nid = \"My Harness\"\nkind = \"built_in\"\ndefault = true\n"
    ));
    let problems = harness_problems(&manifest);
    assert!(
        problems.iter().any(|p| p.contains("snake_case")),
        "{problems:?}"
    );
}

/// The per-file roster form carries `harness` through, so the two authoring
/// forms agree.
#[test]
fn a_per_file_agent_carries_its_harness_binding() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join(MANIFEST_FILE),
        "[company]\nname = \"X\"\n\n[[harness]]\nid = \"embedded\"\nkind = \"built_in\"\ndefault = true\n\n[[harness]]\nid = \"deep\"\nkind = \"built_in\"\n",
    )
    .expect("write manifest");
    let agents = dir.path().join(super::super::agent_file::AGENTS_DIR);
    std::fs::create_dir_all(&agents).expect("agents dir");
    std::fs::write(
        agents.join("researcher.toml"),
        "role = \"Researcher\"\nharness = \"deep\"\n",
    )
    .expect("write agent");

    let manifest = CompanyManifest::from_path(dir.path()).expect("parses");
    assert_eq!(
        manifest.harness_for("researcher").map(|h| h.id),
        Some("deep".to_string())
    );
}
