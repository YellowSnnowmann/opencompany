use super::*;
use crate::company::inference::store::{ProviderId, ProviderOrigin};

fn provider(slug: &str, kind: &str, enabled: bool) -> Provider {
    Provider {
        id: ProviderId::new(),
        slug: slug.to_string(),
        label: slug.to_string(),
        kind: kind.to_string(),
        base_url: format!("https://{slug}.example/v1"),
        models: BTreeMap::new(),
        enabled,
        origin: ProviderOrigin::Indexed,
    }
}

fn routes(pairs: &[(&str, &str)]) -> Routes {
    pairs
        .iter()
        .map(|(tier, raw)| (tier.to_string(), ProviderRef::parse(raw)))
        .collect()
}

// ---- the no-inheritance rule -------------------------------------------

#[test]
fn an_unset_workload_resolves_through_the_primary_never_a_sibling() {
    // The bug this is the fix for: setting only the coding route used to
    // move chat and reasoning onto that key, silently billing ordinary
    // conversations to the operator's own account.
    let providers = vec![
        provider("openrouter", "openrouter", true),
        provider("acme", "openai_compatible", true),
    ];
    let routes = routes(&[("agentic-v1", "acme:gpt-5")]);

    match provider_for_workload(Workload::Chat, &routes, &providers) {
        Resolution::Primary => {}
        other => panic!("chat must fall through to the primary, got {other:?}"),
    }
    match provider_for_workload(Workload::Reasoning, &routes, &providers) {
        Resolution::Primary => {}
        other => panic!("reasoning must fall through to the primary, got {other:?}"),
    }
    // And the one that WAS set resolves to what it names.
    match provider_for_workload(Workload::Agentic, &routes, &providers) {
        Resolution::Resolved { provider, model } => {
            assert_eq!(provider.slug, "acme");
            assert_eq!(model.as_deref(), Some("gpt-5"));
        }
        other => panic!("agentic was set, got {other:?}"),
    }
}

#[test]
fn with_no_marker_the_primary_is_the_first_enabled_provider() {
    // Today's behaviour, unchanged for every company that existed before the
    // marker did. No migration, no backfill.
    let providers = vec![
        provider("openrouter", "openrouter", false),
        provider("acme", "openai_compatible", true),
    ];
    assert_eq!(primary(&providers, None).unwrap().slug, "acme");
    assert!(primary(&[], None).is_none());
}

#[test]
fn a_marked_default_wins_over_list_order() {
    // The whole point. First-enabled answers "which provider is my default"
    // by list order, so deleting the first silently moves a company's
    // unrouted spend to a different account with nothing on screen saying so.
    let providers = vec![
        provider("openrouter", "openrouter", true),
        provider("acme", "openai_compatible", true),
    ];
    assert_eq!(primary(&providers, Some("acme")).unwrap().slug, "acme");
    // And an unset workload follows the marker when it moves.
    assert_eq!(
        primary(&providers, Some("openrouter")).unwrap().slug,
        "openrouter"
    );
}

#[test]
fn a_stale_marker_falls_back_rather_than_stranding_the_company() {
    // Both ways it can go stale — disabled, and gone — get the same answer.
    // A route the operator DID set fails closed when it names a provider
    // that is missing or off, because that is a choice with a workload
    // attached. An unset workload has no such choice behind it, and the
    // alternative to falling back is a company that cannot think at all
    // because of a marker it forgot about.
    let providers = vec![
        provider("openrouter", "openrouter", true),
        provider("acme", "openai_compatible", false),
    ];
    assert_eq!(
        primary(&providers, Some("acme")).unwrap().slug,
        "openrouter",
        "a disabled marked provider is not a routing target"
    );
    assert_eq!(
        primary(&providers, Some("ghost")).unwrap().slug,
        "openrouter",
        "a marker naming nothing falls back"
    );
    // Nothing enabled at all is `None`, which every caller reads as the
    // managed brain — always available, and the right fallback.
    let all_off = vec![provider("acme", "openai_compatible", false)];
    assert!(primary(&all_off, Some("acme")).is_none());
}

#[test]
fn an_unset_workload_follows_the_marker_when_it_moves() {
    // The two halves together: `provider_for_workload` says "unset, use the
    // primary" and `primary` says which that is. Moving the marker moves
    // every unset workload with it, and nothing else.
    let providers = vec![
        provider("openrouter", "openrouter", true),
        provider("acme", "openai_compatible", true),
    ];
    let routes = routes(&[("reasoning-v1", "acme:gpt-5")]);

    for (marked, expected) in [(None, "openrouter"), (Some("acme"), "acme")] {
        assert!(
            matches!(
                provider_for_workload(Workload::Chat, &routes, &providers),
                Resolution::Primary
            ),
            "chat is unset whatever the marker says"
        );
        assert_eq!(primary(&providers, marked).unwrap().slug, expected);
    }

    // And the route that WAS set does not move.
    match provider_for_workload(Workload::Reasoning, &routes, &providers) {
        Resolution::Resolved { provider, .. } => assert_eq!(provider.slug, "acme"),
        other => panic!("reasoning was set, got {other:?}"),
    }
}

#[test]
fn coding_is_an_alias_of_agentic_not_a_row_of_its_own() {
    // An alias is two names for ONE configured route. That is a different
    // thing from an unset route borrowing a set one, which is the bug above.
    let providers = vec![provider("acme", "openai_compatible", true)];
    let routes = routes(&[("agentic-v1", "acme:gpt-5")]);
    assert_eq!(Workload::Coding.tier(), Workload::Agentic.tier());
    assert!(Workload::Coding.is_alias());
    assert!(!ROUTABLE_WORKLOADS.contains(&Workload::Coding));
    match provider_for_workload(Workload::Coding, &routes, &providers) {
        Resolution::Resolved { provider, .. } => assert_eq!(provider.slug, "acme"),
        other => panic!("coding reads the agentic route, got {other:?}"),
    }
}

// ---- fail closed --------------------------------------------------------

#[test]
fn a_route_naming_a_provider_that_is_gone_fails_closed() {
    // Demoting silently would attribute this workload's spend to whatever
    // the fallback happened to be.
    let providers = vec![provider("acme", "openai_compatible", true)];
    let routes = routes(&[("reasoning-v1", "ghost:gpt-5")]);
    match provider_for_workload(Workload::Reasoning, &routes, &providers) {
        Resolution::Missing { workload, slug } => {
            assert_eq!(workload, Workload::Reasoning);
            assert_eq!(slug, "ghost");
        }
        other => panic!("expected a named failure, got {other:?}"),
    }
}

#[test]
fn a_route_naming_a_disabled_provider_is_reported_not_demoted() {
    let providers = vec![
        provider("openrouter", "openrouter", true),
        provider("acme", "openai_compatible", false),
    ];
    let routes = routes(&[("reasoning-v1", "acme:gpt-5")]);
    match provider_for_workload(Workload::Reasoning, &routes, &providers) {
        Resolution::Disabled { workload, slug } => {
            assert_eq!(workload, Workload::Reasoning);
            assert_eq!(slug, "acme");
        }
        other => {
            panic!("\"off this week\" must not become \"bill another one\", got {other:?}")
        }
    }
}

#[test]
fn a_disabled_provider_is_not_a_routing_target() {
    let providers = vec![
        provider("openrouter", "openrouter", true),
        provider("acme", "openai_compatible", false),
    ];
    assert_eq!(
        routing_targets(&providers)
            .iter()
            .map(|p| p.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["openrouter"]
    );
}

#[test]
fn managed_and_unset_are_different_states() {
    let providers = vec![provider("acme", "openai_compatible", true)];
    let explicit = routes(&[("chat-v1", "managed")]);
    assert_eq!(
        provider_for_workload(Workload::Chat, &explicit, &providers),
        Resolution::Managed
    );
    assert_eq!(
        provider_for_workload(Workload::Chat, &Routes::new(), &providers),
        Resolution::Primary
    );
}

// ---- the string grammar -------------------------------------------------

#[test]
fn the_hand_editable_grammar_round_trips_through_a_person() {
    assert_eq!(ProviderRef::parse(""), ProviderRef::Default);
    assert_eq!(ProviderRef::parse("   "), ProviderRef::Default);
    assert_eq!(ProviderRef::parse("default"), ProviderRef::Default);
    assert_eq!(ProviderRef::parse("managed"), ProviderRef::Managed);
    assert_eq!(
        ProviderRef::parse("acme:gpt-5"),
        ProviderRef::Cloud {
            provider_slug: "acme".into(),
            model: Some("gpt-5".into())
        }
    );
    assert_eq!(
        ProviderRef::parse("acme"),
        ProviderRef::Cloud {
            provider_slug: "acme".into(),
            model: None
        }
    );
    assert_eq!(
        ProviderRef::parse("claude-code:opus"),
        ProviderRef::ClaudeCode {
            model: Some("opus".into())
        }
    );
    assert_eq!(
        ProviderRef::parse("local:llama3.1"),
        ProviderRef::Local {
            model: Some("llama3.1".into())
        }
    );
    // A trailing colon is a slug with no model, not a model named "".
    assert_eq!(
        ProviderRef::parse("acme:"),
        ProviderRef::Cloud {
            provider_slug: "acme".into(),
            model: None
        }
    );
}

#[test]
fn only_a_cloud_ref_carries_a_slug() {
    // This is the fact the three scrub rules exist to work around.
    assert_eq!(ProviderRef::parse("acme:gpt-5").slug(), Some("acme"));
    assert_eq!(ProviderRef::parse("local:llama3.1").slug(), None);
    assert_eq!(ProviderRef::parse("claude-code:opus").slug(), None);
    assert_eq!(ProviderRef::parse("managed").slug(), None);
}

// ---- the three scrub rules ----------------------------------------------

#[test]
fn removing_a_cloud_provider_scrubs_routes_matched_by_slug() {
    let removed = provider("acme", "openai_compatible", true);
    let remaining = vec![provider("openrouter", "openrouter", true)];
    let mut routes = routes(&[
        ("chat-v1", "acme:gpt-5"),
        ("reasoning-v1", "openrouter:big"),
        ("agentic-v1", ""),
    ]);
    let reset = scrub_removed(&mut routes, &removed, &remaining);
    assert_eq!(reset, vec!["chat-v1".to_string()]);
    assert_eq!(routes["chat-v1"], ProviderRef::Default);
    assert_eq!(
        routes["reasoning-v1"],
        ProviderRef::Cloud {
            provider_slug: "openrouter".into(),
            model: Some("big".into())
        },
        "an unrelated route must not move"
    );
}

#[test]
fn removing_a_cli_login_scrubs_its_slugless_routes() {
    // Without this, disconnecting Claude Code left workloads pinned to
    // `claude-code:<model>`, which the resolver still honours — so chats
    // kept using the CLI after the provider was removed.
    let removed = provider("claude-code", "claude-code", true);
    let remaining = vec![provider("openrouter", "openrouter", true)];
    let mut routes = routes(&[("chat-v1", "claude-code:opus")]);
    let reset = scrub_removed(&mut routes, &removed, &remaining);
    assert_eq!(reset, vec!["chat-v1".to_string()]);
    assert_eq!(routes["chat-v1"], ProviderRef::Default);
}

#[test]
fn a_local_route_survives_while_any_local_runtime_remains() {
    // Scrubbing on the first removal would unpin a route a second local
    // runtime still serves.
    let removed = provider("ollama", "ollama", true);
    let remaining = vec![provider("lmstudio", "lmstudio", true)];
    let mut routes = routes(&[("chat-v1", "local:llama3.1")]);
    let reset = scrub_removed(&mut routes, &removed, &remaining);
    assert!(reset.is_empty(), "another local runtime still serves it");
    assert_eq!(
        routes["chat-v1"],
        ProviderRef::Local {
            model: Some("llama3.1".into())
        }
    );
}

#[test]
fn a_local_route_is_scrubbed_once_no_local_runtime_remains() {
    // And before this rule existed, the local case was silently a no-op.
    let removed = provider("ollama", "ollama", true);
    let remaining = vec![provider("openrouter", "openrouter", true)];
    let mut routes = routes(&[("chat-v1", "local:llama3.1")]);
    let reset = scrub_removed(&mut routes, &removed, &remaining);
    assert_eq!(reset, vec!["chat-v1".to_string()]);
    assert_eq!(routes["chat-v1"], ProviderRef::Default);
}

#[test]
fn a_slugless_route_with_nothing_to_serve_it_fails_closed() {
    let providers = vec![provider("openrouter", "openrouter", true)];
    let routes = routes(&[("chat-v1", "local:llama3.1")]);
    match provider_for_workload(Workload::Chat, &routes, &providers) {
        Resolution::Missing { slug, .. } => assert_eq!(slug, "local"),
        other => panic!("expected a named failure, got {other:?}"),
    }
}

#[test]
fn a_slugless_route_whose_only_runtime_is_off_reports_disabled() {
    let providers = vec![provider("ollama", "ollama", false)];
    let routes = routes(&[("chat-v1", "local:llama3.1")]);
    match provider_for_workload(Workload::Chat, &routes, &providers) {
        Resolution::Disabled { slug, .. } => assert_eq!(slug, "local"),
        other => panic!("expected a disabled report, got {other:?}"),
    }
}

// ---- the second mechanism -----------------------------------------------

#[test]
fn a_route_edited_in_outside_the_ui_is_still_caught_at_load() {
    // The UI path can be bypassed by a hand-edited config or an older
    // build, so the invariant needs a second, independent check.
    let providers = vec![provider("acme", "openai_compatible", true)];
    let routes = routes(&[("chat-v1", "ghost:gpt-5"), ("reasoning-v1", "acme:gpt-5")]);
    assert_eq!(
        orphaned_routes(&routes, &providers),
        vec![("chat-v1".to_string(), "ghost".to_string())]
    );
}

// ---- one question, one matcher -----------------------------------------

/// The disable path's own bug: `parked_tiers` compared slugs, and
/// `ProviderRef::slug()` is `None` for a `local` ref — so disabling the only
/// Ollama runtime parked every `local:` route while the note said "Nothing
/// was routed through it." A false statement in the one sentence whose job
/// is to be true.
#[test]
fn a_slug_less_route_is_served_by_the_runtime_it_names() {
    let ollama = provider("ollama", "ollama", true);
    let openrouter = provider("openrouter", "openrouter", true);
    let routes = routes(&[
        ("chat-v1", "local:llama3"),
        ("reasoning-v1", "openrouter:gpt-5"),
    ]);
    assert_eq!(
        routes_served_by(&routes, &ollama, std::slice::from_ref(&openrouter)),
        vec!["chat-v1".to_string()],
        "the `local` route is served by the only local runtime there is"
    );
    assert!(
        routes_served_by(&routes, &openrouter, std::slice::from_ref(&ollama))
            .contains(&"reasoning-v1".to_string())
    );
}

/// And it is only parked once nothing of that category is left to serve it —
/// the same rule `scrub_removed` applies to a removal.
#[test]
fn a_second_runtime_of_the_category_keeps_the_route_served() {
    let ollama = provider("ollama", "ollama", true);
    let lmstudio = provider("lmstudio", "lmstudio", true);
    let routes = routes(&[("chat-v1", "local:llama3")]);
    assert!(routes_served_by(&routes, &ollama, &[lmstudio]).is_empty());
    assert_eq!(
        routes_served_by(&routes, &ollama, &[]),
        vec!["chat-v1".to_string()]
    );
}

/// `orphaned_routes` early-returned on `route.slug()?`, so a `local:` route
/// on a company holding no local runtime was reported by nothing at all —
/// while the turn refused it mid-flight. The whole point of the second
/// mechanism is that it is caught at load.
#[test]
fn a_slug_less_route_with_nothing_to_serve_it_is_reported_at_load() {
    let openrouter = provider("openrouter", "openrouter", true);
    let routes = routes(&[
        ("chat-v1", "local:llama3"),
        ("vision-v1", "claude-code:sonnet"),
    ]);
    let mut orphaned = orphaned_routes(&routes, &[openrouter]);
    orphaned.sort();
    assert_eq!(
        orphaned,
        vec![
            ("chat-v1".to_string(), "local".to_string()),
            ("vision-v1".to_string(), "claude-code".to_string()),
        ]
    );
}

/// A runtime of that category exists, so the route resolves — disabled or
/// not, which is `Resolution::Disabled`'s business rather than this one's.
#[test]
fn a_slug_less_route_is_not_orphaned_while_its_category_is_held() {
    let ollama = provider("ollama", "ollama", false);
    let routes = routes(&[("chat-v1", "local:llama3")]);
    assert!(orphaned_routes(&routes, &[ollama]).is_empty());
}

/// The tier id is an internal name and it leaked into two operator-facing
/// sentences.
#[test]
fn a_tier_is_named_the_way_every_other_sentence_names_it() {
    assert_eq!(tier_label("agentic-v1"), "Agentic");
    assert_eq!(tier_label("vision-v1"), "Vision");
    assert_eq!(
        tier_label("embedding-v1"),
        "embedding-v1",
        "a tier this runtime has no workload for passes through unchanged"
    );
}

// ---- the inferred mode --------------------------------------------------

#[test]
fn a_company_that_has_chosen_nothing_is_managed_when_managed_answers() {
    assert_eq!(
        infer_routing_mode(&Routes::new(), true),
        RoutingMode::Managed
    );
    assert_eq!(
        infer_routing_mode(&routes(&[("chat-v1", "managed"), ("vision-v1", "")]), true),
        RoutingMode::Managed
    );
}

/// The reported defect. A fresh company with no managed credential reads
/// `Managed` from an empty table while every unset row resolves to
/// [`Resolution::Primary`] — the first enabled provider. The screen named
/// one destination and the turn used another.
#[test]
fn nothing_chosen_and_managed_unresolvable_is_not_a_mode() {
    assert_eq!(
        infer_routing_mode(&Routes::new(), false),
        RoutingMode::Unset
    );
    assert_eq!(
        infer_routing_mode(&routes(&[("chat-v1", "managed"), ("vision-v1", "")]), false),
        RoutingMode::Unset
    );
}

/// Managed's availability decides **only** the managed-or-unset table. A
/// company that has named a provider on every row has a mode it can use
/// whatever the managed chain says, and reporting otherwise would hide a
/// choice the operator made.
#[test]
fn managed_availability_does_not_reach_a_table_that_names_a_provider() {
    let all = routes(&[
        ("chat-v1", "acme:gpt-5"),
        ("reasoning-v1", "acme:gpt-5"),
        ("agentic-v1", "acme:gpt-5"),
        ("vision-v1", "acme:gpt-5"),
    ]);
    assert_eq!(infer_routing_mode(&all, false), RoutingMode::Own);
    assert_eq!(infer_routing_mode(&all, true), RoutingMode::Own);
}

#[test]
fn one_provider_and_model_on_every_row_is_own() {
    let all = routes(&[
        ("chat-v1", "acme:gpt-5"),
        ("reasoning-v1", "acme:gpt-5"),
        ("agentic-v1", "acme:gpt-5"),
        ("vision-v1", "acme:gpt-5"),
    ]);
    assert_eq!(infer_routing_mode(&all, true), RoutingMode::Own);
}

#[test]
fn a_single_differing_row_makes_it_advanced() {
    let mixed = routes(&[
        ("chat-v1", "acme:gpt-5"),
        ("reasoning-v1", "acme:gpt-5"),
        ("agentic-v1", "acme:gpt-5"),
        ("vision-v1", "acme:vision"),
    ]);
    assert_eq!(infer_routing_mode(&mixed, true), RoutingMode::Advanced);

    // Partly set is also advanced: "the same on every row" is not true of a
    // row that is unset.
    let partial = routes(&[("chat-v1", "acme:gpt-5")]);
    assert_eq!(infer_routing_mode(&partial, true), RoutingMode::Advanced);
}

#[test]
fn the_mode_is_a_function_of_the_routes_and_nothing_else() {
    // There is no mode field, so there is nothing that can disagree with
    // the four routes. Re-deriving from the same map is stable.
    let map = routes(&[("chat-v1", "acme:gpt-5")]);
    assert_eq!(
        infer_routing_mode(&map, true),
        infer_routing_mode(&map.clone(), true)
    );
}

#[test]
fn every_routable_workload_owns_a_distinct_tier() {
    let mut tiers: Vec<&str> = ROUTABLE_WORKLOADS.iter().map(|w| w.tier()).collect();
    tiers.sort_unstable();
    let count = tiers.len();
    tiers.dedup();
    assert_eq!(tiers.len(), count, "two rows would write one tier's route");
    // And they are exactly the tiers the runtime has — no more, so a row
    // cannot address a tier nothing serves, and no fewer, so a tier cannot
    // be unreachable from the routing screen.
    let mut runtime_tiers = crate::company::types::INFERENCE_TIERS.to_vec();
    runtime_tiers.sort_unstable();
    assert_eq!(tiers, runtime_tiers);
}

#[test]
fn a_tier_maps_back_to_its_workload() {
    assert_eq!(Workload::from_tier("chat-v1"), Some(Workload::Chat));
    assert_eq!(Workload::from_tier(" vision-v1 "), Some(Workload::Vision));
    assert_eq!(Workload::from_tier("nope-v1"), None);
    // Coding has no row, so no tier maps back to it.
    assert_ne!(Workload::from_tier("agentic-v1"), Some(Workload::Coding));
}
/// Removing a local runtime must scrub the routes naming it by slug.
///
/// `ollama:llama3` parses as a `Cloud` ref — it carries a slug — while
/// `category_of("ollama")` is `Local`. The cloud arm refused it on category
/// and the local arm never saw it, because that arm only matches the
/// slug-less `local` ref, so the two rules never met and removal scrubbed
/// nothing. The consequence was the sharp part: `put_routes` fails closed on
/// a route naming a provider nobody holds, so the table already on disk
/// became unsaveable and the operator could not fix their own routing
/// without rewriting every row.
#[test]
fn removing_a_local_runtime_scrubs_the_routes_that_name_it() {
    let ollama = provider("ollama", "ollama", true);
    let openrouter = provider("openrouter", "openrouter", true);
    let mut routes = Routes::new();
    routes.insert("chat-v1".into(), ProviderRef::parse("ollama:llama3"));
    routes.insert(
        "reasoning-v1".into(),
        ProviderRef::parse("openrouter:gpt-5"),
    );

    let reset = scrub_removed(&mut routes, &ollama, std::slice::from_ref(&openrouter));
    assert_eq!(reset, vec!["chat-v1".to_string()]);
    assert_eq!(routes.get("chat-v1"), Some(&ProviderRef::Default));
    assert_eq!(
        routes.get("reasoning-v1"),
        Some(&ProviderRef::parse("openrouter:gpt-5")),
        "another provider's row is untouched"
    );
    // And what is left is saveable, which is the property that actually
    // broke: every remaining route names something this company holds.
    assert!(orphaned_routes(&routes, &[openrouter]).is_empty());
}

/// The slug-less `local` ref keeps its own rule: it is orphaned only once no
/// local runtime remains, because a second one still serves it.
#[test]
fn a_slug_less_local_route_survives_while_another_runtime_does() {
    let ollama = provider("ollama", "ollama", true);
    let lmstudio = provider("lmstudio", "lmstudio", true);
    let mut routes = Routes::new();
    routes.insert("chat-v1".into(), ProviderRef::parse("local:llama3"));

    let reset = scrub_removed(&mut routes, &ollama, std::slice::from_ref(&lmstudio));
    assert!(reset.is_empty(), "lmstudio still serves it");
    assert!(!scrub_removed(&mut routes, &ollama, &[]).is_empty());
}

#[test]
fn a_switched_off_runtime_does_not_keep_a_slug_less_local_route_alive() {
    // "Another runtime remains" has to mean one that can actually serve the
    // route. `provider_for_workload` looks for an **enabled** target and
    // fails the workload closed when it finds none, so counting a
    // switched-off row as a survivor left the route pinned to a hard
    // failure rather than resetting it to the primary.
    let ollama = provider("ollama", "ollama", true);
    let parked = provider("lmstudio", "lmstudio", false);
    let mut routes = Routes::new();
    routes.insert("chat-v1".into(), ProviderRef::parse("local:llama3"));

    let reset = scrub_removed(&mut routes, &ollama, std::slice::from_ref(&parked));
    assert_eq!(reset, vec!["chat-v1".to_string()]);
    assert_eq!(routes.get("chat-v1"), Some(&ProviderRef::Default));
}
