# Phase 2b — part 2

Continues [phase-2b-default-shape.md](phase-2b-default-shape.md). Same revision
(`fcfb3e1bc`, 2026-09-14). The same rule applies: slice 2a lands first, so find
code by its quoted text rather than its line number.

## 4.3 `src/company/inference.rs`, resolver

Insert everything below directly **before** `resolve_effective_for_tier`'s doc
comment (`:1594`).

```rust
/// The turn-path refusal when neither a pin, a full default, nor the legacy
/// chain gives a model. Replaces "no inference provider is configured for this
/// company" (`provider.rs:2107`).
pub const NO_MODEL_CHOSEN: &str = "No model is chosen for this company. Choose a default \
     provider and model in Settings → Inference.";

/// Which explicit choice is being resolved; only the refusal wording differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChoiceSource {
    /// An agent pair (3a).
    Pin,
    /// The company's full default.
    Default,
}

/// The decl one chosen provider resolves to, carrying the chosen model.
/// `Indexed` ⇒ [`decl_for_indexed`]. `EntryZero` ⇒ [`resolve_legacy_scoped`] at
/// the flat scope, the same rule as the entry-zero route arm, so the proxy
/// inheritance, the managed chain and `reject_unknown_provider` still apply.
async fn decl_for_choice(
    company: &CompanyId,
    manifest: &Inference,
    env_default: Option<&EnvDefault>,
    secrets: &dyn SecretStore,
    scope: &HarnessScope,
    provider: &store::Provider,
    model: &str,
) -> Result<Option<InferenceDecl>> {
    let decl = match provider.origin {
        store::ProviderOrigin::Indexed => {
            Some(decl_for_indexed(company, secrets, provider).await?)
        }
        store::ProviderOrigin::EntryZero => {
            let flat = HarnessScope::default_harness(&scope.id);
            resolve_legacy_scoped(company, manifest, env_default, secrets, &flat).await?
        }
    };
    Ok(decl.map(|d| d.with_chosen_model(model.to_string())))
}

/// A full default, for **read** paths (boot and status). A default naming a
/// missing or switched-off provider answers `None`, and the caller falls through
/// to today's steps: a read must be able to describe the state that refuses a
/// turn (see `refuse_a_managed_fallback_that_is_switched_off`). The refusal
/// lives in [`resolve_for_turn`].
async fn full_default_decl(
    company: &CompanyId,
    manifest: &Inference,
    env_default: Option<&EnvDefault>,
    secrets: &dyn SecretStore,
    scope: &HarnessScope,
) -> Result<Option<InferenceDecl>> {
    let store::DefaultChoice::Full(choice) = store::load_default(company, secrets).await? else {
        return Ok(None);
    };
    let Some(provider) = store::get_provider(company, secrets, &choice.provider).await? else {
        return Ok(None);
    };
    if !provider.enabled {
        return Ok(None);
    }
    decl_for_choice(company, manifest, env_default, secrets, scope, &provider, &choice.model).await
}

/// Resolves an explicit choice, failing closed (F6): a missing or switched-off
/// provider is an error, never a fall-through to the default or to legacy.
async fn resolve_choice(
    company: &CompanyId,
    manifest: &Inference,
    env_default: Option<&EnvDefault>,
    secrets: &dyn SecretStore,
    scope: &HarnessScope,
    choice: &store::ModelChoice,
    source: ChoiceSource,
) -> Result<InferenceDecl> {
    let slug = choice.provider.trim();
    let Some(provider) = store::get_provider(company, secrets, slug).await? else {
        return Err(OpenCompanyError::Config(match source {
            ChoiceSource::Pin => format!(
                "this agent is set to `{slug}`, which this company does not have. \
                 Choose another provider in Team → the agent → Model."
            ),
            ChoiceSource::Default => format!(
                "the default provider `{slug}` is gone. Choose a default provider and \
                 model in Settings → Inference."
            ),
        }));
    };
    if !provider.enabled {
        let label = provider.label.as_str();
        return Err(OpenCompanyError::Config(match source {
            ChoiceSource::Pin => format!(
                "this agent is set to `{label}`, which is switched off. Switch it back on \
                 in Settings → Inference, or choose another provider in Team → the agent → Model."
            ),
            ChoiceSource::Default => format!(
                "the default provider `{label}` is switched off. Switch it back on, or \
                 choose another default provider and model in Settings → Inference."
            ),
        }));
    }
    decl_for_choice(company, manifest, env_default, secrets, scope, &provider, &choice.model)
        .await?
        .ok_or_else(|| OpenCompanyError::Config(NO_MODEL_CHOSEN.to_string()))
}

/// The turn resolver. Order, and nothing else:
/// 1. `pin` (an agent pair; always `None` until 3a) ⇒ [`resolve_choice`].
/// 2. A named, non-default harness that configures itself skips step 3.
/// 3. A `DefaultChoice::Full` default ⇒ [`resolve_choice`].
/// 4. Otherwise ⇒ [`resolve_effective_for_tier`] with `legacy_hint` as the tier,
///    byte for byte; `None` becomes [`NO_MODEL_CHOSEN`].
pub async fn resolve_for_turn(
    company: &CompanyId,
    manifest: &Inference,
    env_default: Option<&EnvDefault>,
    secrets: &dyn SecretStore,
    scope: &HarnessScope,
    pin: Option<store::ModelChoice>,
    legacy_hint: &str,
) -> Result<InferenceDecl> {
    if let Some(pin) = pin {
        return resolve_choice(company, manifest, env_default, secrets, scope, &pin, ChoiceSource::Pin)
            .await;
    }
    let harness_owns_its_inference =
        !scope.is_default && harness_configures_itself(company, secrets, scope).await?;
    if !harness_owns_its_inference
        && let store::DefaultChoice::Full(choice) = store::load_default(company, secrets).await?
    {
        return resolve_choice(
            company, manifest, env_default, secrets, scope, &choice, ChoiceSource::Default,
        )
        .await;
    }
    resolve_effective_for_tier(company, manifest, env_default, secrets, scope, legacy_hint)
        .await?
        .ok_or_else(|| OpenCompanyError::Config(NO_MODEL_CHOSEN.to_string()))
}
```

**`resolve_effective_scoped` (boot and status).** Make this the **first**
statement inside the existing step-0 guard (`:1246`), before
`let providers = …`:

```rust
        if let Some(decl) = full_default_decl(company, manifest, env_default, secrets, scope).await? {
            return Ok(Some(decl));
        }
```

Change the precedence line in its doc (`:1179-1180`) to "full default >
provider list > runtime > manifest > env-default > a routing table that names
`managed`".

`src/runtime/builder.rs:3087` needs no edit, because its `configured` check calls
`resolve_effective`.

## 4.4 `src/harness/built_in/provider.rs`

**`request_plan` (`:1668`).** Replace the single `let model = …` line:

```rust
    let model = match decl.chosen_model() {
        Some(chosen) => chosen.to_string(),
        None => inference::model_for_tier(abstract_model, &decl.models, decl.vocabulary()),
    };
```

In the doc (`:1642-1643`), the first bullet becomes: "A decl with a chosen model
sends it; otherwise the abstract tier is mapped through the tenant
`[inference].models` table."

**`TenantProvider::resolve` (`:2096-2133`).**

```rust
    async fn resolve(&self, tier: &str) -> anyhow::Result<InferenceDecl> {
        let decl = inference::resolve_for_turn(
            &self.company,
            &self.manifest,
            self.env_default.as_ref(),
            self.secrets.as_ref(),
            &self.scope,
            None, // the agent pair: slice 3a passes `self.pin.clone()`
            tier,
        )
        .await
        .map_err(|e| anyhow::anyhow!("resolving inference config: {e}"))?;
        *self.slug.write().unwrap() = decl.telemetry_slug();
        // A chosen model is sent as-is: no tier vocabulary, no catalogue read.
        if decl.chosen_model().is_some() {
            return Ok(decl);
        }
        // … the existing bearer + `turn_vocabulary(…)` block, unchanged …
        Ok(decl.with_vocabulary(vocabulary))
    }
```

In its doc (`:2087-2095`), "Errors when no provider is configured at all"
becomes "Errors with `NO_MODEL_CHOSEN` when nothing resolves, or with a
fail-closed sentence for a broken full default."

## 5. Ordered edit list

1. Confirm slice 2a is on the branch: `git log --oneline -15` names its commit,
   and `git grep -n "agent-integrations/openrouter" -- src/company/inference/catalogue.rs`
   finds the row. If either check fails, stop and report.
2. `store.rs`: add `ModelOnRow`, `model_on_row` and `Provider::model` (§4.1(a)).
3. `store.rs`: add the default items and rewrite `load_default_slug`
   (§4.1(b), (c)). `Serialize` and `Deserialize` are already imported.
4. `store.rs` tests: §7.1.
5. `inference.rs`: add the field, the six `chosen_model: None` lines and the
   accessors (§4.2).
6. `inference.rs`: add the resolver items and the `resolve_effective_scoped`
   step (§4.3). Tests: §7.2.
7. `provider.rs`: §4.4. Tests: §7.3 and §7.4.
8. `docs/modules/inference/data-model.md:159-162`: the slot holds either a
   legacy bare slug or `{"provider":"…","model":"…"}`; the writer arrives in 2c.
9. `cargo fmt --all -- --check`, then commit ("Parse a provider+model default
   and send its model") and push. Verify CI by head SHA: zero failures and zero
   pending.

## 6. Data carry-over

**None.** Nothing is rewritten and nothing is written at boot; a stored value
stays byte-identical. How each value reads after 2b:

| `inference/default` raw value | `load_default` | `load_default_slug` | Turn |
|---|---|---|---|
| absent, `""`, `"   "` | `Unset` | `None` | legacy, as today |
| `acme` | `ProviderOnly("acme")` | `Some("acme")` | legacy primary marker, as today |
| `{"provider":"acme"}` or `{"provider":"acme","model":""}` | `ProviderOnly("acme")` | `Some("acme")` | as a bare slug |
| `{"provider":"acme","model":"acme/other-model"}` (written only by 2c or a test) | `Full` | `Some("acme")` | sends `acme/other-model` to the `acme` row |
| `{"provider":"tinyhumans","model":"acme/test-model"}` | `Full` | `Some("tinyhumans")` | sends `acme/test-model` to the `tinyhumans` row's URL |
| `{"provider":` | `Err(Store)` | `Err(Store)` | status read and turn error, like a corrupt index |

`inference/providers` is untouched. A row stored as
`"models":{"agentic-v1":"acme/test-model","chat-v1":"acme/test-model","reasoning-v1":"acme/test-model","vision-v1":"acme/test-model"}`
also reads as `ModelOnRow::One("acme/test-model")`.

## 7. Tests

- `MemSecrets` exists in each test module: `store.rs:1032`, `inference.rs:1927`
  and `provider.rs:4303`.
- Use fake keys only: `sk-not-a-real-key` and `th-not-a-real-key`.
- Add every test at the end of its module.

### 7.1 `src/company/inference/store.rs`

Write each raw value with
`secrets.set(&company(), DEFAULT_PROVIDER_KEY, SecretValue(raw.into()))`.

| Test | Setup | Call | Assert |
|---|---|---|---|
| `a_json_default_reads_provider_and_model` | raw `  {"provider":" acme ","model":" acme/other-model "}\n` | `load_default` | `Full(ModelChoice{provider:"acme", model:"acme/other-model"})` |
| `a_bare_slug_default_reads_as_provider_without_model` | raw ` acme\n` | `load_default`, `load_default_slug` | `ProviderOnly("acme")`; `Some("acme")` |
| `a_blank_model_in_json_reads_as_provider_only` | `{"provider":"acme"}`, `{"provider":"acme","model":null}`, `{"provider":"acme","model":"  "}` | `load_default` | each `ProviderOnly("acme")` |
| `an_unparseable_json_default_is_an_error` | `{"provider":`, `{"model":"x"}`, `{"provider":"  ","model":"x"}`, `{"provider":5}` | `load_default` | `matches!(e, OpenCompanyError::Store(_))`; message contains `inference default` |
| `a_cleared_default_reads_unset` | no key; after `clear_default_slug`; raw `"   "` | `load_default`, `load_default_slug` | `Unset` and `None`, all three |
| `setting_a_default_choice_is_one_json_write` | empty store | `set_default_choice(&ModelChoice{provider:" tinyhumans ", model:" acme/test-model "})` | map has exactly 1 entry; key `inference/default`; value `{"provider":"tinyhumans","model":"acme/test-model"}`; `load_default` is `Full` |
| `a_default_choice_without_a_model_is_refused_before_writing` | empty store | model `"  "` | `Err(InvalidRequest(_))`; map empty |
| `load_default_slug_reads_the_provider_out_of_a_json_default` | raw `{"provider":"acme","model":"acme/other-model"}` | `load_default_slug` | `Some("acme")` |
| `a_row_with_one_distinct_model_collapses_to_it` | `draft("acme")` with all four tiers `acme/other-model`, `chat-v1`=` acme/other-model `, plus `"extra":""`; `put_provider`, then `get_provider` | `.model()` | `One("acme/other-model")` |
| `a_row_with_no_model_reads_none` | `{}`; `{"chat-v1":"  "}` | `model_on_row` | `ModelOnRow::None` both |
| `a_row_with_two_distinct_models_is_ambiguous_never_picked` | `{"chat-v1":"b-model","agentic-v1":"a-model","reasoning-v1":"a-model"}` | `model_on_row` | `Ambiguous(vec!["a-model","b-model"])` |
| `entry_zero_models_collapse_the_same_way` | `provider_from_runtime(&RuntimeInference{provider:"openrouter", base_url:None, models: four tiers "acme/test-model"})`; then `chat-v1`=`x/a`, `agentic-v1`=`x/b` | `.model()` | `One("acme/test-model")`; `Ambiguous(["x/a","x/b"])` |

### 7.2 `src/company/inference.rs`

Add three helpers:

- `choose(secrets, provider, model)` calls `store::set_default_choice` for
  company `acme`.
- `staging_env_default()` returns
  `EnvDefault{ base_url: "https://staging-api.tinyhumans.ai/openai/v1".into(), credential: Credential::from_value("platform-key") }`.
  The resolver passes the URL through unchanged, whatever 2a's defaults are.
- `turn(secrets, manifest, env, scope, pin, hint)` wraps `resolve_for_turn` for
  company `acme`.

`add_indexed` (`:3280`) already exists.

| Test | Setup | Call | Assert |
|---|---|---|---|
| `a_full_default_sends_its_model_whatever_the_tier` | `add_indexed("acme","sk-not-a-real-key")`; `choose("acme","acme/other-model")` | `turn` with each of `INFERENCE_TIERS` and `"stub-model"` | `chosen_model()==Some("acme/other-model")`, `base_url=="https://acme.example/v1"` each time |
| `a_full_default_resolves_at_boot_with_its_model` | same | `resolve_effective(…, None, …)` | `Some`, `chosen_model()==Some("acme/other-model")` |
| `a_full_default_naming_entry_zero_keeps_the_legacy_chain_with_the_chosen_model` | `save_runtime_config{openai_compatible, https://legacy.example/v1}` + `store_key("sk-not-a-real-key")`; `add_indexed("second",…)`; `choose("openai_compatible","legacy-model")` | `turn(…,"chat-v1")` | `base_url` legacy, `source==Runtime`, bearer `sk-not-a-real-key`, `chosen_model()==Some("legacy-model")` |
| `a_full_default_naming_a_gone_provider_fails_closed` | `add_indexed("acme",…)`; `choose("ghost","m")` | `turn` | `Err` containing ``the default provider `ghost` is gone`` |
| `a_full_default_naming_a_switched_off_provider_fails_closed` | acme and other; `store::set_enabled(acme,false)`; `choose("acme","m")` | `turn` | `Err` containing `is switched off` |
| `a_broken_full_default_never_makes_a_status_read_fail` | as the gone test | `resolve_effective` | `Ok(Some(d))`, acme URL, `chosen_model()==None` |
| `a_bare_slug_default_resolves_exactly_as_before` | first and second; `set_default_slug("second")` | `turn(…,"chat-v1")` | second URL, `chosen_model()==None` |
| `a_hosted_company_with_no_default_resolves_exactly_as_before` | empty store; `staging_env_default()` | `turn(…,"agentic-v1")` and `resolve_effective_for_tier(…,"agentic-v1")` | both: staging URL, `is_proxied()`, `provider=="openrouter"`, `source==Default`, bearer `platform-key`, `chosen_model()==None` (no wire-model assertion: 2d owns that) |
| `a_named_harness_with_its_own_inference_ignores_the_company_default` | `add_indexed("acme")`; `choose("acme","m")`; manifest `inference("openai_compatible")` + `base_url "https://harness.example/v1"`; scope `HarnessScope::named("research").declaring_own_inference(true)` | `turn` | harness URL, `chosen_model()==None` |
| `a_pin_outranks_a_full_default` | acme, pinned; `choose("acme","m")` | pin `ModelChoice{provider:"pinned", model:"p"}` | pinned URL, `chosen_model()==Some("p")` |
| `a_pin_naming_a_gone_provider_fails_closed_without_falling_back` | acme; `choose("acme","m")` | pin `ghost` | `Err` containing ``this agent is set to `ghost` `` |
| `nothing_resolving_on_the_turn_path_asks_for_a_model` | empty store, no env | `turn` | `Err` whose text contains `NO_MODEL_CHOSEN` |
| `a_tinyhumans_row_default_sends_its_model_to_the_rows_url` | `put_provider` from slice 2a's catalogue row (the `tinyhumans` lookup 2a names; re-verify), models `{}`; key `th-not-a-real-key`; `choose("tinyhumans","acme/test-model")` | `turn(…,"agentic-v1")` | `base_url=="https://api.tinyhumans.ai/agent-integrations/openrouter"`, `!is_proxied()`, bearer `th-not-a-real-key`, `chosen_model()==Some("acme/test-model")` |

### 7.3 `src/harness/built_in/provider.rs` (`openhuman` lane)

**Helper `spawn_recording_stub()`.** Model it on `spawn_auth_recorder` (`:4109`).
It returns `(String, Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>)` and
serves two routes:

- `POST /chat/completions`: record `("POST", body)`, then reply
  `{"choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}`.
- `GET /models`: record `("GET", Null)`, then reply `{"data":[{"id":"chat-v1"}]}`.

**Helper `put_row(secrets, slug, url)`.** Call `store::put_provider` with kind
`openai_compatible`, then set `provider/<slug>/key` to `sk-not-a-real-key`.

| Test | Setup | Call | Assert |
|---|---|---|---|
| `request_plan_sends_the_chosen_model_not_the_tier` | the decl from `request_plan_maps_tier_and_injects_openrouter_headers` (`:4334`), `.with_chosen_model("acme/other-model".into())` | `request_plan` per tier | `plan.model=="acme/other-model"` |
| `a_proxied_decl_with_a_chosen_model_sends_it` | the decl from `:4523` (`assert!(is_proxied())`), `.with_chosen_model("acme/test-model".into())` | `request_plan(…,"chat-v1")` | `plan.model=="acme/test-model"` |
| `a_chosen_model_turn_reads_no_catalog` | stub; `put_row("acme", url)`; `set_default_choice(acme, acme/other-model)`; `TenantProvider::new(company, secrets, Inference::default(), None)` | `invoke(&(), user_request("hi"))`, then `tokio::time::sleep(50ms)` | no `GET`; one `POST` with `body["model"]=="acme/other-model"` |
| `a_full_default_beats_the_model_override` | same | request `.model = Some("stub-model".into())` | `body["model"]=="acme/other-model"` |
| `a_tinyhumans_row_default_reaches_the_rows_url_with_its_model` | the `tinyhumans` row from 2a; key `th-not-a-real-key`; default `acme/test-model` | `resolve_for_turn(…, None, "agentic-v1")`, then `request_plan` | `plan.url` ends `/agent-integrations/openrouter/chat/completions`; `plan.model=="acme/test-model"`; bearer `th-not-a-real-key` |

### 7.4 Existing test changed

In `provider.rs`, change the assertion in
`tenant_provider_errors_when_nothing_is_configured` (`:4895-4905`) from
`contains("no inference provider")` to
`contains("No model is chosen for this company")`. No test is moved or deleted.

## 8. Console / UI

None.

## 9. Must not touch

- Routes and the routing store: `resolve.rs`, `load_routes`/`save_routes`.
- `inference/config` writes, `inference/key`.
- The managed chain (`managed_identity`, `managed_decl`, `load_managed_key`),
  `inference/managed/enabled`, and `refuse_a_managed_fallback_that_is_switched_off`.
- `OPENCOMPANY_INFERENCE_*` handling (`provider.rs:147-195`).
- Slice 2a's catalogue row, URLs and paged catalogue.
- Every DTO; `set_default`, `add_provider` and `edit_provider` (all 2c); the
  console.
- Tier code: `DEFAULT_TIER_MODELS`, `TierVocabulary`, `model_for_tier`, and
  `build.rs::model_for_tier` (all 2d).
- `src/company/search/store.rs`, which has functions with the same names.
- No new secret-store key. No Managed-specific special case of any kind.

## 10. Done when

- `git grep -n "resolve_effective_for_tier(" -- src/harness` finds nothing.
- Every §7 test passes on CI, verified by head SHA with the `openhuman` lane
  included: zero failures and zero pending.
- `set_default_choice` has no production caller yet.
- No test was deleted.

## 11. Gotchas

- **Glob imports.** `ModelOnRow::None` shadows `Option::None` if glob-imported.
  Always spell out `ModelOnRow::None`.
- **`load_default_slug` can now return an error**, but only for a malformed
  value that starts with `{`. The error surfaces on every status read and every
  turn — the same failure class as a corrupt index.
- **Reads never fail closed.** A broken full default makes a status read show the
  primary provider while turns refuse. Slice 2c's `defaultChoice` status field
  lets the page say so.
- **A full default naming entry zero** gets no managed-switch refusal. This
  mirrors the entry-zero route arm. Slice 2a's one-row rule decides whether
  `tinyhumans` is entry zero or an indexed row; `get_provider` returns whichever
  exists.
- **The pin error names no agent.** Slice 3a prefixes it with the agent id.
  Slice 3a should not duplicate the two pin tests in §7.2.
- **`clippy::too_many_arguments`** fires above 7 arguments. Three functions
  already sit at exactly 7. If a function needs an 8th, bundle `company`,
  `manifest`, `env_default`, `secrets` and `scope` into a private struct instead.
- **Rollback (inferred).** A pre-2b binary reads the JSON value as a slug that
  matches no row, so the first enabled provider serves.
