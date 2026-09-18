# Phase 2d — no tier name on the wire; tier code quarantined

Slice **2d** of the [keys rework](README.md). Continued in
[phase-2d-no-tier-on-the-wire-part2.md](phase-2d-no-tier-on-the-wire-part2.md),
which holds the edit list, carry-over, tests, console/E2E, deploy gate,
must-not-touch, done-when and gotchas.

- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14). Every `file:line`
  is on that commit.
- **Runs after:** 2a ([tinyhumans on the proxy](phase-2a-tinyhumans-on-proxy.md)),
  2b ([default shape](phase-2b-default-shape.md)) and 2c (the model step,
  `check_model_id`), committed in that order.
- **Line numbers move.** Every one of those slices edits files named here, so
  find code by the quoted text rather than by the line number.

---

## 1. Goal

After 2d, no request any OpenCompany process sends carries a tier name as its
`model`. The model is a real id, or the request is refused before anything
leaves the process. A tier name is any entry of
`crate::company::types::INFERENCE_TIERS`
(`["chat-v1","reasoning-v1","agentic-v1","vision-v1"]`, `types.rs:80`).

- **Asked for by:** dump items 13 and 16; the operator's rule of 2026-09-14; the
  decisions D-model and F3.
- **Why now.** Slice 2a points `PLATFORM_BASE_URL` and
  `DEFAULT_TINYHUMANS_INFERENCE_URL` at `/agent-integrations/openrouter`. That
  proxy returns 400 for a tier name, and 400 for an id outside its catalogue.
  Both of today's fallbacks therefore fail: sending the tier through, and
  guessing a substitute from `DEFAULT_TIER_MODELS`.

## 2. The rule: the order in which a request's `model` is chosen

| # | Source | Where it comes from | Used when |
|---|---|---|---|
| 1 | `decl.chosen_model()` | an agent pair (3a) or a full default (2b) | it is not a tier name. If it **is** a tier name, refuse; never fall through. |
| 2 | the requested id | `ModelRequest.model`. This is `HarnessDeps.model_override`, set from `OPENCOMPANY_INFERENCE_MODEL` (`provider.rs:155` → `builder.rs:3065-3068`), or an id a caller names | it is non-blank and not a tier name |
| 3 | the legacy map value | the requested string is a tier, so look it up in `decl.models[tier]`. That map is one of: entry zero `inference/config.models`; manifest `[inference].models`; a route's pinned model (inserted at `inference.rs:1722-1728`, until slice 5b); a row's uniform models | the value is non-blank and not a tier name |
| 4 | refuse | `inference::NO_MODEL_CHOSEN`, from 2b: "No model is chosen for this company. Choose a default provider and model in Settings → Inference." | — |

**Removed:**

- the bare tier passthrough (`model_for_tier`'s `Tiers | Unknown` arm, `inference.rs:374-376`);
- guessed substitution from `DEFAULT_TIER_MODELS` (the `Concrete` arm, `:369-373`);
- the pre-discovery guess (`InferenceDecl::vocabulary`, `:565-571`).

**`HostedProvider`** has no decl, so only rule 2 applies to it; otherwise it
refuses.

## 3. What the tier vocabulary is for, and what is deleted

Every consumer on `fcfb3e1bc`:

| Item | Consumers | Purpose |
|---|---|---|
| `model_for_tier` (`inference.rs:360-378`) | `request_plan` (`provider.rs:1668`); tests | pass a tier through or substitute a shipped id |
| `InferenceDecl.vocabulary`, `vocabulary()`, `with_vocabulary()`, `vocabulary_confirmed()` (`:505-513`, `:565-587`) | `model_for_tier`'s callers; `vocabulary_confirmed` is used only by tests (`:2818`, `:2821`) | feed the choice above |
| `turn_vocabulary` + `TURN_CATALOG_BUDGET` (`inference_models.rs:660-685`, `:71`) | `TenantProvider::resolve` (`provider.rs:2125`) only | feed `model_for_tier` on a turn |
| `discovered_vocabulary` (`inference_models.rs:625-635`) | `turn_vocabulary`; ops `test_config` (`ops/inference.rs:1342`) | feed `probe`'s `model_for_tier` |
| `TierVocabulary::from_catalog_ids`, `tier_defaults`, `as_str` | the `list_models` DTO (`ops/inference.rs:201-208`); `needs_an_explicit_model` (`providers.rs:668-670`) → `ProbeResultDto.needs_model` (`:260`) → console `ProvidersTab.tsx:237` | offer tier defaults; ask for a model only when the catalogue lacks tiers. 2c always asks. |
| `DEFAULT_TIER_MODELS` (`:175-180`) | all of the above; status `default_tier_models` (`ops/inference.rs:881-884`); metering ratchet test (`metering/model.rs:300`, `:320`) | the guessed substitute |
| `TenantProvider::catalog_scope` (`provider.rs:2083`) | `:2128` only | the cache scope for `turn_vocabulary` |

**Verdict: delete all of them outright.**

- Each one either chooses between sending a tier and substituting an id, or
  offers tier defaults.
- The console reads none of the DTO fields (the grep is in part 2 §9).
- Once §2 is the rule, none of them has a purpose left.

`catalog_models` and `discover_models` **stay**, because they fill the model
picker.

**What stays, in `pub(crate) mod legacy_tiers` (`src/company/inference/legacy_tiers.rs`).**
Two functions: `is_tier_name` and `configured_model_for_tier`. They are the
tier-hint → configured-real-id lookup (§2 rule 3). F3 keeps them because the
legacy arm still holds real per-tier maps:

- entry zero;
- manifests such as `companies/hive_math_lab/company.toml:50` and
  `companies/vending_machine_co/company.toml:97`, which map the four tiers to
  three different ids;
- routes, until 5b.

Both are deleted when the fallback chains are dropped.

`pub(crate)` is lint-safe. Both functions are reached from the `pub fn
model_on_the_wire`, so the default-feature clippy lane (`ci.yml:602`) sees them
as used.

**What stays outside `legacy_tiers`, and why:**

- `build.rs` `legacy_workload_hint`, renamed from `model_for_tier` (`:188-204`).
  It still produces the tier **key** for rule 3.
- `DEFAULT_HOSTED_MODEL = "chat-v1"` (`provider.rs:58`). It is now only the key
  used when a request names no model.
- `INFERENCE_TIERS`: manifest validation, `is_tier_name`, 2c's `check_model_id`.
- The metering `EXACT` tiers (`metering/model.rs:176`), so historic usage rows
  still classify.
- `Agent.tier` (`types.rs:661`).
- `resolve.rs` `Workload`, until 5b.
- `uniform_models`, the storage encoding.
- `setup.rs:848`, which writes a real id under the tier keys.

## 4. Files

| File | Change |
|---|---|
| `src/company/inference/legacy_tiers.rs` | **new**: `is_tier_name`, `configured_model_for_tier` + tests |
| `src/company/inference.rs` | `pub(crate) mod legacy_tiers;`; new `pub fn model_on_the_wire`; delete `DEFAULT_TIER_MODELS`, `TierVocabulary`, `ConcreteTiers`, `impl TierVocabulary`, `model_for_tier`, the `vocabulary` field and its three accessors, `vocabulary: None` in six literals; tests §8 |
| `src/harness/built_in/provider.rs` | `request_plan`; `TenantProvider::resolve`; delete `catalog_scope`; `HostedProvider::invoke` guard; `harness_inference_from_env` warning; docs `:52-58`, `:140-141` |
| `src/server/inference_models.rs` | delete `discovered_vocabulary`, `turn_vocabulary`, `TURN_CATALOG_BUDGET`, the `TierVocabulary` import (`:30`), their tests |
| `src/server/ops/inference.rs` | `test_config`: delete the vocabulary block (`:1331-1350`); DTO fields `ModelCatalogDto.tier_vocabulary`/`tier_defaults`, `InferenceStatusDto.default_tier_models` |
| `src/server/ops/inference/providers.rs` | `tier_overrides` → `uniform_models`; delete `needs_an_explicit_model` + `ProbeResultDto.needs_model` (conditions in part 2 §6 step 2); `TierVocabulary` import (`:60`) |
| `src/harness/built_in/build.rs`, `src/harness/built_in/mod.rs` | `model_for_tier` → `legacy_workload_hint`; new `turn_model_hint_from`; `impl HarnessDeps { fn turn_model_hint }` |
| 11 call-site files (§5.6) | use `deps.turn_model_hint(None)` |
| `src/harness/roster_build.rs` | `for_setup`: real id or `None` |
| `src/server/setup.rs` | `probe_inference`: real id or a clear error; optional `model` on `InferenceTestRequest` |
| `src/metering/model.rs` | test and doc links no longer use `DEFAULT_TIER_MODELS` |
| `frontend/src/api/inference.ts`, test fixtures, `frontend/playwright.config.ts` | part 2 §9 |
| docs | part 2 §10 |

## 5. Target code

### 5.1 `src/company/inference/legacy_tiers.rs` (new)

```rust
//! Legacy tier lookups — decision 8 / F3. **A tier name is never a model.**
//!
//! All that survives of tiers on the request path is one lookup: an agent's
//! tier hint selects an entry in a map an operator configured (entry zero's
//! `inference/config.models`, a manifest `[inference].models`, a route's pinned
//! model until slice 5b), and only a real id found there is sent. Deleted with
//! the legacy fallback chains.

use std::collections::BTreeMap;

use crate::company::types::INFERENCE_TIERS;

/// Whether `id`, trimmed, is one of [`INFERENCE_TIERS`].
pub fn is_tier_name(id: &str) -> bool {
    INFERENCE_TIERS.contains(&id.trim())
}

/// The operator's real model id for `tier`, or `None` when the map has no
/// entry, a blank one, or one that is itself a tier name.
pub fn configured_model_for_tier(tier: &str, models: &BTreeMap<String, String>) -> Option<String> {
    models
        .get(tier.trim())
        .map(|m| m.trim())
        .filter(|m| !m.is_empty() && !is_tier_name(m))
        .map(str::to_string)
}
```

### 5.2 `src/company/inference.rs`

Add `pub(crate) mod legacy_tiers;` to the module list (`:28-32`). Put
`model_on_the_wire` directly after 2b's `NO_MODEL_CHOSEN`:

```rust
/// The model id a request carries. **Never a tier name.** Order (plan 2d §2):
/// the chosen model; else `requested` when it is a real id (this is
/// `OPENCOMPANY_INFERENCE_MODEL`); else the configured map value for the tier
/// `requested` names; else [`NO_MODEL_CHOSEN`].
pub fn model_on_the_wire(decl: &InferenceDecl, requested: &str) -> Result<String> {
    let refuse = || OpenCompanyError::Config(NO_MODEL_CHOSEN.to_string());
    if let Some(chosen) = decl.chosen_model() {
        let chosen = chosen.trim();
        if chosen.is_empty() || legacy_tiers::is_tier_name(chosen) {
            return Err(refuse());
        }
        return Ok(chosen.to_string());
    }
    let requested = requested.trim();
    if !requested.is_empty() && !legacy_tiers::is_tier_name(requested) {
        return Ok(requested.to_string());
    }
    legacy_tiers::configured_model_for_tier(requested, &decl.models).ok_or_else(refuse)
}
```

**Delete:**

- `:164-180` (`DEFAULT_TIER_MODELS` and its doc);
- `:182-333` (`TierVocabulary`, `ConcreteTiers`, `impl TierVocabulary`);
- `:335-378` (`model_for_tier`);
- the `vocabulary` field and its doc (`:505-513`);
- the methods `vocabulary`, `vocabulary_confirmed` and `with_vocabulary`
  (`:550-587`);
- `vocabulary: None,` in the six struct literals. Keep 2b's `chosen_model: None,`.

Then run `git grep -n "vocabulary" -- src/company/inference.rs`. The only hits
left must be doc prose that you reword.

### 5.3 `src/harness/built_in/provider.rs`

**`request_plan`.** Rename the parameter `abstract_model` to `requested_model`.
Replace 2b's `match`, and its comment, with:

```rust
    // Never a tier name: the chosen model, else a real requested id, else the
    // operator's configured id for the requested tier, else refuse before sending.
    let model = inference::model_on_the_wire(decl, requested_model)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
```

The first doc bullet (`:1642-1643`) becomes: "The model is
[`inference::model_on_the_wire`]: never a tier name."

**`TenantProvider::resolve`.** Delete the bearer + `turn_vocabulary` block, the
`chosen_model().is_some()` early return, and `with_vocabulary`. The function
ends:

```rust
        *self.slug.write().unwrap() = decl.telemetry_slug();
        Ok(decl)
```

Delete `fn catalog_scope` (`:2065-2085`). Afterwards,
`git grep -n catalog_scope -- src` may hit only a doc comment in
`inference_models.rs:1039`; reword that comment.

**`HostedProvider::invoke` (`:1494-1496`):**

```rust
        let messages = wire_messages(&request.messages);
        let model = request.model.as_deref().unwrap_or(DEFAULT_HOSTED_MODEL).trim();
        // No decl, so no configured map: only a real requested id is sent.
        if model.is_empty() || inference::legacy_tiers::is_tier_name(model) {
            return Err(InferenceError::Model(inference::NO_MODEL_CHOSEN.to_string()));
        }
```

**`harness_inference_from_env` (`:155`):**

```rust
    let model_override = env
        .get("OPENCOMPANY_INFERENCE_MODEL")
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    if let Some(m) = model_override.as_deref()
        && inference::legacy_tiers::is_tier_name(m)
    {
        tracing::warn!(
            model = %m,
            "OPENCOMPANY_INFERENCE_MODEL names a tier, which is never sent as a model; \
             set a model id from the provider's catalog"
        );
    }
```

**Docs.**

- `DEFAULT_HOSTED_MODEL` (`:57`): "The tier **key** a request with no model
  looks up in a configured map. Never sent as a model."
- `:141`: "model — `OPENCOMPANY_INFERENCE_MODEL`, a real model id; a tier name
  there only selects a configured map entry."

Target code for the rest — `inference_models.rs`, `ops/inference.rs`,
`providers.rs`, `build.rs`/`HarnessDeps`, `roster_build.rs`, `setup.rs` and
metering — is in [part 2 §5](phase-2d-no-tier-on-the-wire-part2.md#5-target-code-continued).
