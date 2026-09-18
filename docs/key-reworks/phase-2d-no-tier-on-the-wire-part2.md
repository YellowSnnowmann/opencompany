# Phase 2d — part 2

Continues [phase-2d-no-tier-on-the-wire.md](phase-2d-no-tier-on-the-wire.md).
It uses the same revision, `fcfb3e1bc` (2026-09-14). Find each line by the code
it quotes, not by its number.

## 5. Target code (continued)

### 5.4 `src/server/inference_models.rs`

Delete these three items:

- `TURN_CATALOG_BUDGET` (`:71`) with its doc;
- `discovered_vocabulary` (`:613-635`);
- `turn_vocabulary` (`:637-685`), including its `cfg_attr` lines.

Change `:30` from `use crate::company::inference::{TierVocabulary, probe};` to
`use crate::company::inference::probe;`.

Delete the tests that call the deleted functions. List them with
`git grep -n "turn_vocabulary(\|discovered_vocabulary(" -- src/server/inference_models.rs`.
On `fcfb3e1bc` those calls sit in the test around `:766-776` (the slow-catalogue
budget test) and in `a_cached_catalog_answers_the_vocabulary_question` (`:967`).

### 5.5 `src/server/ops/inference.rs`

**`test_config`.** Delete the block at `:1331-1350`: the comment
"Ask the endpoint what vocabulary it speaks…", `discovered_vocabulary(...)` and
`decl.with_vocabulary(vocabulary)`. `probe(&decl, …)` now reaches
`model_on_the_wire` through `request_plan`. A company whose Test resolves no
real id gets `NO_MODEL_CHOSEN` back as the probe error.

**`ModelCatalogDto` (`:101-131`).** Delete the fields `tier_vocabulary`
(`:110-115`) and `tier_defaults` (`:116-121`). In `list_models`, delete the
initialisers at `:179-180`, `:207-208` and `:223-224`, and the
`let vocabulary = …` at `:201-203`.

**`InferenceStatusDto`.** Delete `default_tier_models` (`:244-264`), the
`let default_tier_models` at `:879-884`, and the initialisers at `:891` and
`:909`. `BTreeMap` is still imported and still used by `models`.

### 5.6 `build.rs`, `HarnessDeps`, and the eleven call sites

**`src/harness/built_in/build.rs:183-204`.** Rename `model_for_tier` to
`legacy_workload_hint`. The body does not change. Rewrite its doc as: "The tier
**key** for a manifest agent tier. It selects an entry in a configured map
(`inference::model_on_the_wire` rule 3) and is never sent as a model."

Add this function directly after it:

```rust
/// The model string a harness request carries: the roster-wide override
/// (`OPENCOMPANY_INFERENCE_MODEL`) when set, else the agent tier's key.
pub fn turn_model_hint_from(model_override: Option<&str>, tier: Option<&str>) -> String {
    model_override
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| legacy_workload_hint(tier))
}
```

**`src/harness/built_in/mod.rs`.** `HarnessDeps` ends at `:638` and no
`impl HarnessDeps` block exists yet. Add one directly after the struct:

```rust
impl HarnessDeps {
    /// See [`build::turn_model_hint_from`]. `tier` is the agent's manifest tier;
    /// company-level passes (title, triage, planning, …) pass `None`.
    pub fn turn_model_hint(&self, tier: Option<&str>) -> String {
        crate::harness::build::turn_model_hint_from(self.model_override.as_deref(), tier)
    }
}
```

`tier` is a parameter because `build.rs:1162` passes the agent's own tier.
Dropping it would change entry-zero and manifest companies whose tiers map to
different ids (F3).

**Call sites.** At each one, replace the
`deps.model_override.clone().unwrap_or_else(|| model_for_tier(…))` expression and
remove `model_for_tier` from the `use` line.

| File | `use` line | Expression | Becomes |
|---|---|---|---|
| `src/harness/built_in/build.rs` | (same file) | `:1159-1162` | `deps.turn_model_hint(manifest_agent.tier.as_deref())` |
| `src/harness/built_in/title.rs` | `:45` | `:139-142` | `deps.turn_model_hint(None)` |
| `src/harness/built_in/triage.rs` | `:69` | `:219-222` | same |
| `src/harness/built_in/planning.rs` | `:103` → `use crate::harness::build::grants_cover;` | `:216-219` | same |
| `src/harness/built_in/selector.rs` | `:45` | `:217-220` | same |
| `src/harness/built_in/confine.rs` | `:65` → `use crate::harness::build::ensure_agent_workspace;` | `:310-313` | same |
| `src/harness/built_in/payload_extract.rs` | `:59` | `:147-150` | same |
| `src/harness/built_in/workflow_build.rs` | `:72` | `:264-267` | same |
| `src/harness/built_in/workflow_build/agent.rs` | `:34` | `:100-103` | same |
| `src/workflows/judge.rs` | `:10` | `:175-178` | same |
| `src/harness/profile_draft.rs` | `:33` | `:132-135` | same |
| `src/harness/roster_build.rs` | `:56` | `:94-97` (`from_deps`; the plan's list missed this site) | same |

Afterwards, `git grep -n "model_for_tier" -- src` must return only reworded doc
prose, or nothing.

### 5.7 `src/server/ops/inference/providers.rs`

- Rename `tier_overrides` (`:672-688`) to `uniform_models`, keeping whatever
  signature 2c left. Doc: "One model id stored under every tier key: a storage
  encoding, not a selection. The legacy arm's `configured_model_for_tier` reads
  it; a tier name is never stored (2c's `check_model_id`)." Update its call
  (`:379` on `fcfb3e1bc`).
- Rewrite the `AddProvider.model` doc (`:148-159`) and the
  `ProbeResultDto.models` doc (`:244-250`) so that neither names
  `model_for_tier`, `DEFAULT_TIER_MODELS` or `TierVocabulary`.
- Delete `needs_an_explicit_model` (`:658-670`) and the `ProbeResultDto.needs_model`
  field (`:256-260`, filled at `:461`, `:1567`, `:1830`, `:1903`). Then change
  `:60` to `use crate::company::inference::{catalogue, probe, resolve, store};`.
  Precondition: §6 step 2.

### 5.8 `src/harness/roster_build.rs` — the setup brain

In `for_setup` (`:127-213`), drop `DEFAULT_HOSTED_MODEL` from the `use` at
`:134-137`. Add this private function at the bottom of the file, outside the
impl:

```rust
/// A model id that may be sent: non-blank and not a tier name.
fn real_model_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|m| !m.is_empty() && !crate::company::inference::legacy_tiers::is_tier_name(m))
        .map(str::to_string)
}
```

- **Direct branch** (`:171-174`): `let model_name = real_model_id(model)?;`
- **Managed branch** (`:206-211`):
  `let model_name = real_model_id(model).or_else(|| real_model_id(model_override.as_deref()))?;`
- **Doc** (`:116-117`): "`None` when there is no credential **or no real model
  id** — the caller then ships the curated team." Its caller
  `propose_for_setup` (`setup.rs:1573-1577`) already maps `None` to
  `FallbackReason::NoModel`, which is the clear outcome. Update the comment at
  `:1573` to "No builder means no credential or no real model id."

### 5.9 `src/server/setup.rs` — the setup connection test

Add a field to `InferenceTestRequest` (`:1175-1187`):
`/// A model id to test with. A tier name counts as absent.` `model: Option<String>,`
(`#[serde(default)]` is already on the struct).

In `probe_inference` (`:1244-1354`):

- `let mut decl` becomes `let decl`.
- Replace `:1280-1308` (the discovered `model` and the tier-insert loop) with the
  following, **keeping** the existing discovery branch as the second step:

```rust
    let model = real_model_id(req.model.as_deref())
        .or(/* the existing `if matches!(… "ollama" | "openai_compatible")` discovery
               expression, unchanged, filtered through `real_model_id` */)
        .or_else(|| {
            crate::harness::provider::harness_inference_from_env(env)
                .and_then(|(_, m)| real_model_id(m.as_deref()))
        });
```

- After the existing `base_url.trim().is_empty()` refusal (`:1318-1325`), add:

```rust
    let Some(model) = model else {
        return InferenceTestDto {
            ok: false,
            base_url,
            model: None,
            error: Some("Choose a model for this provider, then test again.".to_string()),
        };
    };
    let decl = decl.with_chosen_model(model.clone());
```

- In both `InferenceTestDto` arms of the `match` (`:1332-1352`), set
  `model: Some(model)`.
- Put a private `real_model_id` in `setup.rs` identical to §5.8. Do not export
  the roster one.
- Run `git grep -n "setup/inference/test" -- frontend/src`. If the wizard already
  sends a `model`, nothing else changes. If it does not, the wizard's Test gives
  the clear error for providers that do not discover a model. Wiring the wizard
  to send one is a console follow-up; record it in the PR body.

### 5.10 `src/metering/model.rs`

- Delete the `use crate::company::inference::DEFAULT_TIER_MODELS;` at `:300`.
- In `a_shipped_default_model_is_named_rather_than_other` (`:314-…`), keep only
  the half that asserts each **tier** classifies. Iterate
  `crate::company::INFERENCE_TIERS`, and rename the test
  `a_workload_tier_is_named_rather_than_other`.
- Reword the doc links at `:8`, `:59`, `:73-74` and `:173` so they no longer name
  the deleted items.

## 6. Ordered edit list

1. Confirm 2a, 2b and 2c are on the branch. The two checks are:
   `git grep -n "fn resolve_for_turn" src/company/inference.rs` and
   `git grep -n "fn check_model_id" -- src`. If either misses, stop and report.
2. Run `git grep -n "needsModel" -- frontend/src`. If `ProvidersTab.tsx` still
   reads `probe.needsModel`, 2c is incomplete: **stop and report**. If only the
   type in `api/inference.ts` remains, delete it together with §5.7.
3. Add `legacy_tiers.rs` and its tests (§8.3), plus `model_on_the_wire` and its
   tests (§5.1, §5.2).
4. Apply §5.3, §5.4 and §5.5, then delete the inference.rs tier code (§5.2).
   Fix every compile error by following §8.1 and §8.2, never by restoring
   deleted code.
5. Apply §5.6 (the build hint) and §5.7 (providers).
6. Apply §5.8 and §5.9 (setup), then §5.10 (metering).
7. Update the test fixtures (§8.2), the frontend (§9) and the docs (§10).
8. Run `cargo fmt --all -- --check` and the three frontend typecheck gates. Then
   check that `git grep -n "TierVocabulary\|DEFAULT_TIER_MODELS\|model_for_tier\|turn_vocabulary" -- src frontend/src`
   returns nothing.
9. Commit in two slices, pushing each: "Never send a tier name as a model", then
   "Remove tier vocabulary DTO fields". Verify CI by head SHA with zero failures
   and zero pending, including both Console E2E lanes.

## 7. Data carry-over

**None.** No stored value is read and rewritten.

- `inference/providers` rows such as
  `"models":{"agentic-v1":"acme/test-model","chat-v1":"acme/test-model",…}`
  stay byte-identical, and rule 3 sends `acme/test-model`.
- An entry-zero `inference/config` of `{"provider":"openrouter","models":{}}`
  stays byte-identical. It now resolves to no model: its turns refuse until a
  default exists, or until `OPENCOMPANY_INFERENCE_MODEL` is set. Before 2d they
  sent a substituted id.
- A stored map value that is a tier (`"chat-v1":"chat-v1"`) stays stored and is
  never sent.

## 8. Tests

### 8.1 Delete: the behaviour is gone

- **`inference.rs`:**
  - `every_tier_resolves_to_a_concrete_model_id_on_the_direct_path` (`:2572`)
  - `every_tier_resolves_to_its_documented_default_model` (`:2596`)
  - `a_harness_override_beats_the_default_and_a_concrete_slug_passes_through` (`:2613`)
  - `a_tier_native_endpoint_keeps_the_tier_name` (`:2653`)
  - `a_catalog_publishing_tier_names_classifies_as_tier_native` (`:2665`)
  - `a_catalog_publishing_the_shipped_ids_classifies_as_concrete` (`:2690`)
  - `a_partial_mirror_substitutes_only_the_ids_its_catalog_publishes` (`:2715`)
  - `a_catalog_publishing_neither_vocabulary_offers_no_defaults` (`:2758`)
  - `tier_defaults_follow_the_vocabulary` (`:2778`)
  - `a_discovered_vocabulary_overrides_the_payer_derived_guess` (`:2805`)
- **`provider.rs`:** `request_plan_keeps_the_tier_for_a_tier_native_endpoint_on_a_tenant_key` (`:4433`)
- **`inference_models.rs`:** the two tests named in §5.4
- **`ops/inference.rs`:** `model_catalog_route_keeps_concrete_defaults_for_a_concrete_catalog` (`:1965`)
- **`providers.rs`:**
  - `a_direct_vendor_catalog_needs_a_model_named` (`:2262`)
  - `a_local_runtime_catalog_needs_a_model_named` (`:2274`)
  - `a_tier_native_catalog_needs_nothing_named` (`:2285`)
  - `a_catalog_of_shipped_ids_needs_nothing_named` (`:2297`)

### 8.2 Change without weakening

- **`inference.rs` helper `wire_model` (`:3430-3432`).** Its body becomes
  `model_on_the_wire(decl, tier).expect("a real model id")`. The route tests
  that use it (`:3468`, `:3559`, `:3590`) name real ids and keep their
  assertions.
- **2b's `a_hosted_company_with_no_default_resolves_exactly_as_before`.** Add
  two assertions: `model_on_the_wire(&decl, "agentic-v1")` is `Err` containing
  `No model is chosen`, and `model_on_the_wire(&decl, "acme/test-model")`
  is `Ok("acme/test-model")`.
- **`provider.rs` `request_plan_maps_tier_and_injects_openrouter_headers`.**
  Replace the `reasoning-v1` shipped-default block (`:4389-4404`) with
  `request_plan(&decl, "reasoning-v1", …).await.expect_err(…)`, whose text
  contains `No model is chosen`. The `chat-v1` → `deepseek/deepseek-chat`
  mapping and the explicit-slug assertions stay.
- **`provider.rs` tests that send an unmapped tier.** Change them so they
  request a real id; do **not** change what they assert.
  - For `invoke`, use `ModelRequest { model: Some("stub-model".into()), ..user_request("hi") }`.
  - For `probe(&decl, …)`, first add `decl.models.insert("chat-v1".into(), "stub-model".into())`.
  - For `request_plan`, pass `"stub-model"`.
  - The tests: `hosted_provider_resolves_the_bearer_per_request` (`:4147`),
    `hosted_provider_omits_the_bearer_without_a_credential` (`:4182`),
    `a_rejected_bearer_forces_a_re_read_on_the_next_turn` (`:4200`),
    `hosted_provider_surfaces_an_unreadable_token_file` (`:4279`),
    `request_plan_omits_bearer_for_keyless_ollama` (`:4489`),
    `request_plan_attaches_the_product_header_when_proxied` (`:4523`),
    `request_plan_never_attaches_the_product_header_for_third_party_providers` (`:4580`),
    `tenant_provider_live_switches_between_turns_without_rebuild` (`:4766`),
    `hosted_provider_invoke_carries_the_product_identity_header` (`:4913`),
    `a_rejected_tenant_turn_leaves_the_last_successful_model_in_place` (`:4990`),
    `a_rejected_hosted_turn_leaves_the_last_successful_model_in_place` (`:5054`),
    `probe_accepts_array_shaped_content` (`:5137`),
    `the_reasoning_tolerance_does_not_excuse_a_broken_tool_call` (`:5188`),
    `probe_accepts_a_reasoning_only_reply_as_proof_the_endpoint_answers` (`:5235`),
    `probe_rejects_tool_call_only_reply` (`:5284`),
    `probe_rejects_tool_call_alongside_text_preamble` (`:5326`),
    `probe_names_the_harness_that_owns_the_failing_config` (`:5644`).
  - Leave any of these alone if it already sends a real id or a mapped tier.
- **Harness turn tests built on `HostedProvider`.** Find every
  `HarnessDeps { … }` literal whose `provider` is
  `HostedProvider::new(HostedProviderConfig…)`, and set
  `model_override: Some("stub-model".to_string())` in it.
  - Find them with `git grep -n "HostedProvider::new(HostedProviderConfig" -- src`.
  - On `fcfb3e1bc` they are in: `iteration_cap_turn_test.rs:195`,
    `native_salvage_turn_test.rs:255`, `publish_turn_test.rs:308`,
    `search_turn_test.rs:266`, `workspace_provision_turn_test.rs:267`,
    `workspace_turn_test.rs:274`, `cap_publish_test.rs:305`,
    `cap_turn_test.rs:307`, `spend_halt_turn_test.rs:315`, and
    `src/workflows/gated_tool_turn_test.rs:208`.
  - Two already set it: `brain.rs:11345` and `composio_turn_test.rs:343`.
- **Integration tests.** `git grep -n "with_harness_inference\|OPENCOMPANY_INFERENCE_URL" -- tests src/server/setup/test.rs`.
  Wherever a host or `for_setup` gets an inference source and no model, add
  `Some("stub-model".to_string())` or the env variable. `tests/one_card_per_message.rs:352`
  is a stub's *response* body; leave it.
- **`ops/inference.rs` `POST …/inference/test` tests** (`:1743`, `:1775`,
  `:2709`, `:2740`, `:3018`). Where one expects `"ok": true` against a stub, add
  `"models": {"chat-v1": "stub-model"}` to its `PUT …/inference` body. Replace
  the catalogue assertions at `:1952-1959` with
  `assert!(body.get("tierVocabulary").is_none() && body.get("tierDefaults").is_none())`.
  Replace `:2779-2791` with `assert!(dto.get("defaultTierModels").is_none())`.
- **Renames.** In `build.rs`, `model_for_tier_maps_hints_and_defaults` (`:1906`)
  becomes `legacy_workload_hint_maps_hints_and_defaults`. In `providers.rs`,
  `a_named_model_covers_every_tier` (`:2308`) now calls `uniform_models`.

### 8.3 New tests, each proving no tier reaches the wire

| File | Test | Setup → call → assert |
|---|---|---|
| `legacy_tiers.rs` | `a_tier_name_is_recognised_with_whitespace` | `is_tier_name(" chat-v1 ")` true; `"acme/test-model"` false |
| `legacy_tiers.rs` | `a_configured_real_id_is_found_for_its_tier` | `{"reasoning-v1":" x/r "}` → `Some("x/r")` |
| `legacy_tiers.rs` | `a_configured_tier_name_is_never_returned` | `{"chat-v1":"agentic-v1"}` → `None`; blank and missing → `None` |
| `inference.rs` | `no_combination_puts_a_tier_on_the_wire` | `decl_for_probe("openai_compatible", Some("https://x.example/v1"), Some("sk-not-a-real-key"), None)`. For every chosen ∈ {none, `chat-v1`, `m1`}, requested ∈ {`chat-v1`, `agentic-v1`, ``, `m2`}, models ∈ {`{}`, `{"chat-v1":"chat-v1"}`, `{"chat-v1":"m3"}`}: the result is `Err`, or `Ok(v)` with `!is_tier_name(&v)` |
| `inference.rs` | `the_wire_model_order_is_chosen_then_requested_then_map` | chosen `m1` + requested `m2` → `m1`; no chosen, requested `m2` → `m2`; requested `chat-v1` + map `m3` → `m3`; a chosen tier → `Err` |
| `provider.rs` | `request_plan_never_puts_a_tier_on_the_wire` | a manifest `openai_compatible` decl with no map: every tier → `Err`; a map to a tier → `Err` |
| `provider.rs` | `a_tenant_turn_with_no_model_is_refused_before_sending` | 2b's recording stub as `EnvDefault.base_url`; empty store; `invoke(user_request("hi"))` → `Err` containing `No model is chosen`; the stub recorded nothing |
| `provider.rs` | `the_model_override_is_sent_when_nothing_is_chosen` | same, request model `acme/test-model` → recorded `body["model"]=="acme/test-model"` |
| `provider.rs` | `a_hosted_provider_refuses_a_tier_before_sending` | `HostedProvider::new` over the stub, request model `chat-v1` → `Err`; nothing recorded |
| `provider.rs` | `a_chosen_model_turn_never_consults_the_legacy_tier_map` | decl with map `chat-v1→other`, `.with_chosen_model("acme/other-model")` → `request_plan` for `chat-v1` and `reasoning-v1` sends `acme/other-model` |
| `provider.rs` | `internal_passes_on_a_full_default_send_the_default_model` | stub; indexed row `acme`; `set_default_choice(acme, acme/test-model)`; `Arc::new(TenantProvider::new(…))`; `hint = turn_model_hint_from(None, None)`; `TitleEvaluator::new(p.clone(), hint.clone()).title("Plan the launch")`, `TriageEvaluator::new(p, hint).classify("hello")` → 2 POSTs, both model `acme/test-model` |
| `build.rs` | `turn_model_hint_prefers_the_override_then_the_agent_tier` | `(Some("stub-model"), Some("reasoning"))` → `stub-model`; `(None, Some("orchestrator"))` → `agentic-v1`; `(Some("  "), None)` → `chat-v1` |
| `roster_build.rs` | `setup_without_a_real_model_ships_the_curated_team` | the env double from `provider.rs:2428` with `OPENCOMPANY_INFERENCE_KEY` only: `for_setup(env, None, None, None, None)` → `None`; model `Some("chat-v1")` → `None`; `Some("acme/test-model")` → `Some` |
| `setup/test.rs` | `the_setup_test_names_a_missing_model_instead_of_sending_a_tier` | `{provider:"openrouter", key:"sk-not-a-real-key", baseUrl:"http://127.0.0.1:9/v1"}`, no env model → `ok:false`, error `Choose a model for this provider, then test again.` |

## 9. Console / UI and E2E

- **Grep proof, run on `fcfb3e1bc` on 2026-09-14.** The command was
  `git grep -n "tierVocabulary\|tierDefaults\|defaultTierModels\|TierVocabulary" -- frontend`.
  - **Declarations only:** `api/inference.ts:85,88,265,273,301,307`.
  - **Comment only:** `inference/ModelField.tsx:74`, `inference/ProviderConnectDialog.tsx:301`.
  - **Fixtures:** `test/unit/inference-card-honesty.test.ts:38,64-67,85`,
    `test/unit/inference-models-api.test.ts:17-18,25,30-31,39-52,60`,
    `test/unit/setup-dialog-inference.test.ts:37`, `test/e2e/org-tree.spec.ts:329`.
  - **No code reads a value.** Re-run the grep before editing.
- **`frontend/src/api/inference.ts`.** Delete `defaultTierModels`, its doc
  block ending at `:88`, the `TierVocabulary` type and its doc (`:258-273`),
  `tierVocabulary` (`:288-301`) and `tierDefaults` (`:302-307`). Reword the two
  comments.
- **Fixtures.** Delete those fields from every fixture listed. In
  `inference-models-api.test.ts`, drop them from both stubs and from the
  `toEqual`, and replace `expect(catalog.tierVocabulary).toBeNull()` with
  `expect(catalog).not.toHaveProperty("tierVocabulary")`.
- **`frontend/playwright.config.ts:214-228`.** Add
  `OPENCOMPANY_INFERENCE_MODEL: "acme/test-model"` to **both**
  `inferenceEnv` objects. `passthrough` (`:289`) forwards it through `host.sh`
  automatically.
  - `mock-brain.mjs:841` accepts any model string.
  - `live-brain-proxy.mjs:126` replaces it with its own `MODEL`.
- **Browser check.** No rendered UI changes, so there is no screenshot gate.
  Both Console E2E lanes must still be green by head SHA.

## 10. Docs and the hosted deploy gate

- **`docs/spec/runtime/config.md:123`.** The `OPENCOMPANY_INFERENCE_MODEL`
  default becomes "none". Its description: "a real model id the whole roster
  sends when nothing chooses one; a tier name only selects a configured map
  entry and logs a warning."
- **`docs/modules/openhuman/README.md:82` and `docs/gitbooks/developers/configuration.md:77`:**
  the same change.
- **`docs/spec/runtime/providers.md:218-219`:** delete the `tierVocabulary` and
  `tierDefaults` rows.
- **Other tier-vocabulary prose.** Reword it in `docs/modules/inference/`
  (`architecture.md`, `current-state.md`, `routing.md`, `routing-states.md`,
  `staging.md`, `README.md`, `data-model.md`). Find it with
  `git grep -n "DEFAULT_TIER_MODELS\|TierVocabulary\|model_for_tier\|turn_vocabulary" -- docs`.
- **Deploy gate — record it in the PR body.** Before an image with 2d reaches
  hosted tenants, `opencompany-manager` must inject
  `OPENCOMPANY_INFERENCE_MODEL=<a model id from the catalog>`
  into every tenant container. Without it, a tenant with no full default refuses
  every turn, title and triage with `NO_MODEL_CHOSEN`, and setup ships the
  curated team. Adding the variable to the manager-injected list in `CLAUDE.md`
  is an operator decision; the implementer does not edit `CLAUDE.md`.

## 11. Must not touch

- Routes, `resolve.rs` and the Routing UI (slice 5b).
- The managed chain and `inference/managed/enabled`.
- Manifest `[inference].models` validation: tier **keys** stay valid.
- `INFERENCE_TIERS`, metering `EXACT`, `Agent.tier`.
- `catalog_models` and `discover_models`.
- Slice 2a's URLs and catalogue row; 2b's resolver order; stored values.
- `frontend/src/inference/proxy-compat.ts`: see §12.
- No new secret-store key, no `proxied_model`, no URL-origin module, and no
  Managed-specific model dialog or draft-probe route. Managed's model is the
  `tinyhumans` row's `models` or the default.

## 12. Done when, and gotchas

**Done when:**

- The §6 step 8 grep is empty.
- Every §8 test passes on CI by head SHA: zero failures and zero pending, both
  Console E2E lanes included.
- The PR body records the deploy gate and the setup-wizard follow-up.

**Gotchas:**

- **F3.** Nothing the legacy arm needs is deleted: `is_tier_name`,
  `configured_model_for_tier`, `legacy_workload_hint`, `uniform_models`, and the
  route model insert.
- **A tier-valued `OPENCOMPANY_INFERENCE_MODEL`** (e.g. `chat-v1`) is not a
  model. It only keys the map, and boot logs a warning.
- **Internal passes on a company with nothing chosen** now error instead of
  sending a tier. Title, triage and selector already treat a model error as
  "no answer" (inferred from their timeout handling, not re-read).
- **`proxy-compat.ts:13-40`** still calls a bare tier "an accepted value". The
  server now refuses to send one, and 2c's `check_model_id` refuses to store
  one. Record the stale console copy as a follow-up; do not change the form here.
- **`with_chosen_model` in the setup probe** is not a legacy arm, so 2b's
  "never on a legacy arm" rule is kept.
- **Unused imports after the deletions** will fail clippy. Check
  `ops/inference.rs`, `providers.rs:60`, `inference_models.rs:30` and
  `roster_build.rs:135`.
