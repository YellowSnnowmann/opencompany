//! Routing-table tests: how the routing table actually reaches the
//! resolver (split out of `inference_tests.rs`).

use super::inference_tests_support::*;
use super::tests_managed::add_indexed;
use super::*;

// ---- the routing table actually reaches the resolver ---------------------
//
// The same shape as the block above, one layer along, and found the same
// way: the Routing tab wrote `inference/routes`, the status route rendered
// it, `provider_for_workload` decided over it in isolation — and the turn
// path never asked. Every row of that screen persisted, survived a reload,
// and changed nothing about where a turn went. A control that visibly fails
// is a bug; a control that reports success and is inert is a lie, and it is
// the harder one to find because nothing looks wrong.

/// The route a tier resolves to, as the wire model the plan would carry.
pub(super) fn wire_model(decl: &InferenceDecl, tier: &str) -> String {
    model_on_the_wire(decl, tier).expect("a real model id")
}

pub(super) async fn route(secrets: &MemSecrets, tier: &str, raw: &str) {
    let company = CompanyId::new("acme");
    let mut routes = store::load_routes(&company, secrets).await.unwrap();
    routes.insert(tier.to_string(), resolve::ProviderRef::parse(raw));
    store::save_routes(&company, secrets, &routes)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_route_sends_its_workload_to_the_provider_and_model_it_names() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    add_indexed(&secrets, "second", "sk-not-a-real-key-2").await;
    route(&secrets, "chat-v1", "second:deepseek/deepseek-v4-flash").await;

    let decl = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .unwrap()
    .expect("a routed workload resolves");
    assert_eq!(
        decl.base_url, "https://second.example/v1",
        "the route names the provider, so the turn goes there and not to the primary"
    );
    assert_eq!(bearer(&decl).await.as_deref(), Some("sk-not-a-real-key-2"));
    assert_eq!(
        wire_model(&decl, "chat-v1"),
        "deepseek/deepseek-v4-flash",
        "the model the route pinned is the model that goes on the wire"
    );
}

#[tokio::test]
async fn the_route_beats_the_default_providers_own_tier_map() {
    // The sharpest form of the bug, and the one an operator hit: Use Your
    // Own Models wrote `anthropic:claude-sonnet-5` into all four tiers, and
    // the next turn sent `anthropic/claude-opus-5` to OpenRouter — neither
    // their provider nor their model. The route was inert in both
    // dimensions, and the two failures hid each other: with a route naming
    // the provider that was already the default, "route honoured" and "route
    // ignored" look identical. This asserts both halves at once by pointing
    // the route at a provider that is *not* the marked default, and pinning
    // a model the default's own tier map would answer differently.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    let mut openrouter_tiers = BTreeMap::new();
    openrouter_tiers.insert("chat-v1".to_string(), "anthropic/claude-opus-5".to_string());
    store::put_provider(
        &company,
        &secrets,
        store::ProviderDraft {
            slug: "openrouter".into(),
            label: "OpenRouter".into(),
            kind: "openrouter".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            models: openrouter_tiers,
            enabled: true,
        },
    )
    .await
    .unwrap();
    secrets
        .set(
            &company,
            &store::provider_key_key("openrouter"),
            SecretValue("sk-not-a-real-key-or".into()),
        )
        .await
        .unwrap();
    store::put_provider(
        &company,
        &secrets,
        store::ProviderDraft {
            slug: "anthropic".into(),
            label: "Anthropic".into(),
            kind: "anthropic".into(),
            base_url: "https://api.anthropic.com/v1".into(),
            models: BTreeMap::new(),
            enabled: true,
        },
    )
    .await
    .unwrap();
    secrets
        .set(
            &company,
            &store::provider_key_key("anthropic"),
            SecretValue("sk-not-a-real-key-ant".into()),
        )
        .await
        .unwrap();
    store::set_default_slug(&company, &secrets, "openrouter")
        .await
        .unwrap();
    route(&secrets, "chat-v1", "anthropic:claude-sonnet-5").await;

    let decl = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .unwrap()
    .expect("the routed workload resolves");
    assert_eq!(
        decl.base_url, "https://api.anthropic.com/v1",
        "the route names anthropic, so the turn goes to anthropic and not to the default"
    );
    assert_eq!(
        bearer(&decl).await.as_deref(),
        Some("sk-not-a-real-key-ant")
    );
    assert_eq!(
        wire_model(&decl, "chat-v1"),
        "claude-sonnet-5",
        "the route's model beats the default provider's own tier map"
    );
}

#[tokio::test]
async fn a_workload_with_no_route_still_falls_through_to_the_primary() {
    // The other half of the property: pinning one row must not move the
    // others. A fix that routed everything through the chat row would pass
    // the test above and be worse than the bug.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    add_indexed(&secrets, "second", "sk-not-a-real-key-2").await;
    route(&secrets, "chat-v1", "second:deepseek/deepseek-v4-flash").await;

    let decl = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "reasoning-v1",
    )
    .await
    .unwrap()
    .expect("an unrouted workload resolves");
    assert_eq!(decl.base_url, "https://first.example/v1");
    assert_eq!(bearer(&decl).await.as_deref(), Some("sk-not-a-real-key-1"));
    // Neither row has a model of its own configured, so — keys rework,
    // issue #2306, slice 2d — an unmapped tier on either is refused
    // rather than guessed; the property under test is that this decl is
    // "first"'s, never "second"'s, which `base_url`/`bearer` above
    // already prove. Confirmed here too: if "second"'s routed model ever
    // leaked onto this decl, this would resolve to it instead of erroring.
    assert!(
        model_on_the_wire(&decl, "reasoning-v1").is_err(),
        "another row's pinned model must not leak onto this one"
    );
}

/// Round-3a review P1-4: a bare-slug default naming a provider this
/// company no longer has must fail the turn closed — never silently
/// hand it to `resolve::primary`'s first-enabled fallback, which is a
/// different account than the one the operator named.
#[tokio::test]
async fn a_default_naming_a_deleted_provider_fails_the_turn_closed() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    store::set_default_slug(&company, &secrets, "gone")
        .await
        .unwrap();

    let err = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .expect_err("a default naming a provider this company does not have must fail closed");
    let text = err.to_string();
    assert!(text.contains("gone"), "{text}");
    assert!(text.contains("removed"), "{text}");
}

/// Same decision, for a default naming a provider that still exists but
/// is switched off.
#[tokio::test]
async fn a_default_naming_a_switched_off_provider_fails_the_turn_closed() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    store::put_provider(
        &company,
        &secrets,
        store::ProviderDraft {
            slug: "second".into(),
            label: "Second".into(),
            kind: "openai_compatible".into(),
            base_url: "https://second.example/v1".into(),
            models: BTreeMap::new(),
            enabled: false,
        },
    )
    .await
    .unwrap();
    store::set_default_slug(&company, &secrets, "second")
        .await
        .unwrap();

    let err = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .expect_err("a default naming a switched-off provider must fail closed, not fall back");
    let text = err.to_string();
    assert!(text.contains("Second"), "{text}");
    assert!(text.contains("turned off"), "{text}");
}

#[tokio::test]
async fn a_route_naming_managed_resolves_through_the_managed_chain() {
    // `managed` is a word in the route grammar, not a provider slug. Read as
    // a slug it resolves to nothing and the workload fails closed — which is
    // what the Managed mode button writes into every row.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://platform.example/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    route(&secrets, "agentic-v1", "managed").await;

    let decl = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        Some(&env),
        &secrets,
        &HarnessScope::default(),
        "agentic-v1",
    )
    .await
    .unwrap()
    .expect("a managed route resolves");
    assert!(decl.is_proxied(), "the managed route rides the platform");
    assert_eq!(decl.base_url, "https://platform.example/v1");
    assert_eq!(bearer(&decl).await.as_deref(), Some("platform-key"));
}

#[tokio::test]
async fn a_named_harness_that_configured_itself_outranks_the_company_provider_list() {
    // `docs/spec/runtime/providers.md` has always said runtime, then
    // manifest, then default — **within a harness**. Putting the company's
    // provider list unconditionally above that inverted it, so connecting
    // the company's first provider in the console silently re-pointed a
    // harness with an account of its own at the company's, and charged it.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;

    // The default harness reads the company's list, which is the whole
    // point of the list existing.
    let default_scope = HarnessScope::default_harness("embedded");
    let shared = resolve_effective_scoped(
        &company,
        &inference("openrouter"),
        None,
        &secrets,
        &default_scope,
    )
    .await
    .unwrap()
    .expect("the default harness resolves through the connected provider");
    assert_eq!(shared.base_url, "https://first.example/v1");

    // A named harness that declared `[harness.inference]` of its own does
    // not. It resolves through what it declared.
    let own = HarnessScope::named("deep").declaring_own_inference(true);
    let mine = resolve_effective_scoped(&company, &inference("openrouter"), None, &secrets, &own)
        .await
        .unwrap()
        .expect("a harness with its own section resolves through it");
    assert_ne!(
        mine.base_url, "https://first.example/v1",
        "the company's connected provider must not outrank this harness's own section"
    );
    assert_eq!(
        mine.base_url, PLATFORM_BASE_URL,
        "with a section of its own and no key in it, this harness rides the subscription"
    );

    // And a named harness that declared nothing still inherits the
    // company's list — the carve-out is for a statement, not for a name.
    let inherits = HarnessScope::named("shallow");
    let theirs = resolve_effective_scoped(
        &company,
        &inference("openrouter"),
        None,
        &secrets,
        &inherits,
    )
    .await
    .unwrap()
    .expect("a harness with nothing of its own inherits");
    assert_eq!(theirs.base_url, "https://first.example/v1");
}

#[tokio::test]
async fn a_named_harness_reads_its_own_credential_before_the_company_wide_one() {
    // Step 1 of the chain is company-wide, so for a harness with inference
    // of its own it is a different owner's credential wearing the same
    // slug. A company connecting OpenRouter would otherwise have its key
    // substituted for the harness's — and a harness with its own base_url
    // would present it to a different gateway.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let named = HarnessScope::named("deep");

    secrets
        .set(
            &company,
            &provider_key_key("openrouter"),
            SecretValue("sk-company".into()),
        )
        .await
        .unwrap();
    // With nothing of its own, the harness inherits — which is what it did
    // before harness scopes existed.
    assert_eq!(
        load_inference_key_scoped(&company, &secrets, "openrouter", None, &named)
            .await
            .unwrap(),
        "sk-company"
    );

    store_key_scoped(&company, &secrets, "sk-deep", &named)
        .await
        .unwrap();
    assert_eq!(
        load_inference_key_scoped(&company, &secrets, "openrouter", None, &named)
            .await
            .unwrap(),
        "sk-deep",
        "the harness's own key wins once it has one"
    );
    // And the company's own resolution is untouched by either.
    assert_eq!(
        load_inference_key_scoped(
            &company,
            &secrets,
            "openrouter",
            None,
            &HarnessScope::default()
        )
        .await
        .unwrap(),
        "sk-company"
    );
}

#[tokio::test]
async fn managed_never_reads_a_legacy_slot_that_belongs_to_a_vendor_account() {
    // `inference/key` is one address with two possible owners. For a company
    // upgraded from a BYOK config it holds that vendor's key, and reading it
    // as managed's sent an OpenRouter credential to the platform URL — a
    // credential presented to an account that does not own it.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    // Entry zero is a vendor account, with its key still in the flat slot.
    save_runtime_config(
        &company,
        &secrets,
        &RuntimeInference {
            provider: "openrouter".into(),
            base_url: None,
            models: BTreeMap::new(),
        },
    )
    .await
    .unwrap();
    secrets
        .set(&company, KEY_KEY, SecretValue("sk-or-byok".into()))
        .await
        .unwrap();

    assert_eq!(
        load_managed_key(&company, &secrets, &HarnessScope::default())
            .await
            .unwrap(),
        "",
        "the vendor's key is not managed's to present"
    );
    // And the general reader still finds it for the row it belongs to.
    assert_eq!(
        load_inference_key_scoped(
            &company,
            &secrets,
            "openrouter",
            None,
            &HarnessScope::default()
        )
        .await
        .unwrap(),
        "sk-or-byok"
    );

    // Same rule a scope along: a named harness's own slot holds that
    // harness's credential for whatever it declared, which managed has no
    // more claim on than it does on entry zero's.
    let named = HarnessScope::named("deep");
    store_key_scoped(&company, &secrets, "sk-deep-byok", &named)
        .await
        .unwrap();
    assert_eq!(
        load_managed_key(&company, &secrets, &named).await.unwrap(),
        "",
        "a named harness's key is not managed's to present either"
    );
}

#[tokio::test]
async fn a_managed_key_on_a_fresh_company_is_what_its_turns_present() {
    // The console's Managed row writes `provider/tinyhumans/key`, and the
    // default branch read only `DEFAULT_PROVIDER`'s slot — so the key was
    // stored, reported as the step that answers, and never sent anywhere.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://platform.example/v1".into(),
        credential: Credential::from_value("instance-identity"),
    };

    // Nothing configured: the instance identity is what answers.
    let before = resolve_effective(&company, &Inference::default(), Some(&env), &secrets)
        .await
        .unwrap()
        .expect("the platform default resolves");
    assert_eq!(bearer(&before).await.as_deref(), Some("instance-identity"));

    secrets
        .set(
            &company,
            &provider_key_key(MANAGED_SLUG),
            SecretValue("sk-managed".into()),
        )
        .await
        .unwrap();
    let after = resolve_effective(&company, &Inference::default(), Some(&env), &secrets)
        .await
        .unwrap()
        .expect("the platform default still resolves");
    assert_eq!(
        bearer(&after).await.as_deref(),
        Some("sk-managed"),
        "the key the operator pasted is the one the turn presents"
    );
}

#[tokio::test]
async fn switching_managed_off_never_makes_the_status_unreadable() {
    // The refusal is a statement about a *turn*. Putting it in the shared
    // resolver put it in every status read too, so a company with nothing
    // but managed could switch it off and then get a 500 from
    // `GET …/inference` — no page, and therefore no switch to turn it back
    // on with. A read has to be able to describe the state that refuses.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://platform.example/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    store::set_managed_enabled(&company, &secrets, false)
        .await
        .unwrap();

    let described = resolve_effective(&company, &inference("managed"), Some(&env), &secrets)
        .await
        .expect("a status read must still resolve");
    assert!(
        described.is_some(),
        "the console has to render the row that switches it back on"
    );

    // And the turn path still refuses, which is the point of the switch.
    let err = resolve_effective_for_tier(
        &company,
        &inference("managed"),
        Some(&env),
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .expect_err("a turn must not ride a switched-off managed");
    assert!(err.to_string().contains("switched off"), "{err}");
}

#[tokio::test]
async fn an_unset_workload_stops_falling_back_to_managed_once_it_is_switched_off() {
    // The unset row does not take the `Managed` branch — it falls through
    // the primary to the legacy chain — so honouring the switch only there
    // left a company whose environment resolves to the platform spending
    // after it had been told to stop. The console's own sentence for this
    // state is "Managed is switched off, so it is not a fallback."
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://platform.example/v1".into(),
        credential: Credential::from_value("platform-key"),
    };

    // On, and nothing connected: the platform is the fallback.
    let decl = resolve_effective_for_tier(
        &company,
        &inference("managed"),
        Some(&env),
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .unwrap()
    .expect("managed is the fallback while it is on");
    assert!(decl.is_proxied());

    store::set_managed_enabled(&company, &secrets, false)
        .await
        .unwrap();
    let err = resolve_effective_for_tier(
        &company,
        &inference("managed"),
        Some(&env),
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .expect_err("a switched-off managed must not keep serving unset workloads");
    assert!(err.to_string().contains("switched off"), "{err}");

    // A company on its own key is untouched by the switch: it was never
    // riding the platform, so there is nothing here to refuse.
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    let own = resolve_effective_for_tier(
        &company,
        &inference("managed"),
        Some(&env),
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .unwrap()
    .expect("a connected provider is not managed");
    assert_eq!(own.base_url, "https://first.example/v1");
}

#[tokio::test]
async fn a_route_naming_managed_fails_closed_once_managed_is_switched_off() {
    // The switch is a statement about spend — "stop billing this account" —
    // and a switch that only moves a badge on the settings page keeps
    // billing it. That is the defect the routing table itself was added to
    // fix, one provider along: the page agreed and the spend continued.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let env = EnvDefault {
        base_url: "https://platform.example/v1".into(),
        credential: Credential::from_value("platform-key"),
    };
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    route(&secrets, "agentic-v1", "managed").await;
    store::set_managed_enabled(&company, &secrets, false)
        .await
        .unwrap();

    let err = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        Some(&env),
        &secrets,
        &HarnessScope::default(),
        "agentic-v1",
    )
    .await
    .expect_err("a switched-off managed row must not keep serving turns");
    let message = err.to_string();
    assert!(message.contains("switched off"), "{message}");
    assert!(message.contains("agentic"), "{message}");

    // And switching it back on restores it, so the refusal is the switch
    // rather than a route that has been broken by being touched.
    store::set_managed_enabled(&company, &secrets, true)
        .await
        .unwrap();
    let decl = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        Some(&env),
        &secrets,
        &HarnessScope::default(),
        "agentic-v1",
    )
    .await
    .unwrap()
    .expect("a managed route resolves again once it is switched back on");
    assert_eq!(decl.base_url, "https://platform.example/v1");
}

#[tokio::test]
async fn a_route_naming_a_provider_that_is_gone_fails_closed() {
    // An unset workload falls back, because nobody chose anything for it. A
    // route is a choice with a workload attached, so it fails rather than
    // quietly spending on an account the operator did not name.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    route(&secrets, "chat-v1", "ghost:gpt-5").await;

    let err = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "chat-v1",
    )
    .await
    .expect_err("a route naming nothing must not silently fall back");
    let message = err.to_string();
    assert!(message.contains("ghost"), "{message}");
    assert!(message.contains("chat"), "{message}");
}

#[tokio::test]
async fn a_route_naming_a_switched_off_provider_fails_closed_too() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    add_indexed(&secrets, "parked", "sk-not-a-real-key-2").await;
    store::set_enabled(&company, &secrets, "parked", false)
        .await
        .unwrap();
    route(&secrets, "vision-v1", "parked").await;

    let err = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "vision-v1",
    )
    .await
    .expect_err("a parked route is a choice that no longer works");
    assert!(err.to_string().contains("parked"), "{err}");
}

#[tokio::test]
async fn coding_reads_the_agentic_route_rather_than_one_of_its_own() {
    // The alias, asserted on the path that matters. A coding turn arrives
    // carrying `agentic-v1`, so this is really a statement about the tier
    // the route is keyed on — and it is the reason routes are keyed on tiers
    // rather than on workload names.
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    add_indexed(&secrets, "first", "sk-not-a-real-key-1").await;
    add_indexed(&secrets, "second", "sk-not-a-real-key-2").await;
    route(&secrets, "agentic-v1", "second").await;

    let decl = resolve_effective_for_tier(
        &company,
        &Inference::default(),
        None,
        &secrets,
        &HarnessScope::default(),
        "agentic-v1",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(decl.base_url, "https://second.example/v1");
}
