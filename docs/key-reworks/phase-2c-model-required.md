# Phase 2c — a model is required everywhere a provider or default is written (backend)

Part 1 of 2 covers the backend: routes, validation, DTOs, JSON shapes, carry-over
and gotchas. Part 2,
[phase-2c-model-required-part2.md](phase-2c-model-required-part2.md), covers the
console, tests, what must not be touched, and done-when. The folder index and naming
contract are in [README.md](README.md).

- **Written:** 2026-09-14, rescoped the same day after PR #2305 closed.
- **Code read at:** `upstream/main @ fcfb3e1bc`. Every `file:line` is on that commit.
- **Order:** after [2a](phase-2a-tinyhumans-on-proxy.md) and [2b](phase-2b-default-shape.md), before [2d](phase-2d-no-tier-on-the-wire.md). 2a and 2b edit the same functions, so **re-check each function's lines on your branch before editing it.**
- **PR #2305 is closed.** Its diff (`f4c42ea48`, `/tmp/oc-s-2c944d60/pr2305.diff`) is only a reference for what went wrong.

---

## 1. Goal

**Every write that sets the company default or stores a provider row carries exactly
one validated model id, and the status reports that model.**

- **Dump items:** 1 (`inference/default` stores provider and model), 4 (a model step after adding a provider), 14 (TinyHumans: key, then a model from its catalogue).
- **Decisions:** Q2 (a default always needs a model), Q3 (TinyHumans is an ordinary `tinyhumans` row), Q1 (a bare slug still reads and is never rewritten), D-model, D-set.

## 2. What slices 2a and 2b must already provide

If any of these is missing, or is named differently on your branch, **stop and
report**. Do not invent a synonym.

| From | Name → assumed shape |
|---|---|
| 2b | `store::ModelChoice { pub provider: String, pub model: String }` |
| 2b | `store::DefaultChoice { Unset, ProviderOnly(String), Full(ModelChoice) }`, plus `fn provider(&self) -> Option<&str>` |
| 2b | `store::load_default(company, secrets) -> Result<DefaultChoice>`; `store::set_default_choice(company, secrets, &ModelChoice) -> Result<()>` (one write of `{"provider","model"}`) |
| 2b | `store::ModelOnRow { None, One(String), Ambiguous(Vec<String>) }`; `store::Provider::model() -> ModelOnRow`; `load_default_slug` kept as `load_default()?.provider()` |
| 2a | a `tinyhumans` catalogue row: `catalogue::cloud_provider("tinyhumans")`, endpoint `https://api.tinyhumans.ai/agent-integrations/openrouter`, bearer auth. Its paged catalogue is read by the probe and by `GET …/providers/{slug}/models`. A `tinyhumans` add goes through `POST …/inference/providers` |

## 3. Files and current code

| File | What changes |
|---|---|
| `src/company/inference/store.rs` | new `check_model_id` and `MAX_MODEL_ID_CHARS`, next to `check_provider_name` (`:347`) |
| `src/server/ops/inference/providers.rs` | `AddProvider` `:130-173`, `EditProvider` `:176-189`, `ProbeResultDto` `:221-261`, `add_provider` `:309-521`, `needs_an_explicit_model` `:658-670`, `tier_overrides` `:680-688`, `edit_provider` `:907-1073`, `set_default` `:1294-1317`, `clear_default_if_marked` `:1324-1345`, `probe_draft` `:1873-1943`, tests `:2110-` |
| `src/server/ops/inference.rs` | `InferenceStatusDto` `:233-356`, `ProviderDto` `:408-465`, `provider_list` `:493-537`, `effective_status_with` `:840-925`, tests `:1390-` |
| `docs/modules/inference/data-model.md:159-162`, `docs/modules/inference/routing.md:352` | "one slot holding one slug" |

### 3.1 Current code

`set_default` (`providers.rs:1294-1317`) takes no body and writes a bare slug:

```rust
let provider = require_provider(runtime, &params.slug).await?;
if !provider.enabled { return Err(/* "… is switched off, so it cannot be the default. …" */); }
store::set_default_slug(runtime.id(), runtime.secrets().as_ref(), &provider.slug)   // note: "Unrouted work now goes through {label}."
```

In `add_provider` the model is optional. A missing one is caught only after the key,
the record and the probe have all run:

```rust
let asked_model = body.model.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(str::to_string); // :317-322
models: tier_overrides(asked_model.as_deref()),                                                         // :379
if asked_model.is_none() && needs_an_explicit_model(&models) { roll_back_add(runtime, &provider).await; // :442-451
    return Err(/* "{label} does not resolve workload names like `agentic-v1` …" */); }
```

**Add order today** (module header `:25-48`): validate → slug → **key** (`:350-362`) → **record** (`:364-394`) → probe (`:400-422`) → auto-route (`:506-513`).

`edit_provider` takes `models: Option<BTreeMap<String, String>>` (`:185`), applied at `:1025`. The status (`ops/inference.rs:505-520`) sends `is_default` (from `load_default_slug` and `resolve::primary`) and `models`, but no model and no default choice.

### 3.2 What slice 2a changes in the same code (re-verify on your branch)

- **Catalogue-shape argument** on `probe::probe_models(…)` / `catalog_models(…)` in `add_provider`, `probe_draft`, `list_provider_models` and `test_provider`. The closed #2305 used `catalogue::catalog_shape_for(<kind>)`, which is `PagedEnvelope` for `tinyhumans`. Keep whatever 2a added. `catalogue_offer` still caps the browser list at `PROBE_CATALOGUE_LIMIT = 500` (`providers.rs:268`).
- **`tinyhumans` takes the cloud branch of `plan_add`** (`:717-726`, `probes: true`). If 2a added a test that posts a `tinyhumans` add with no model, update it (part 2 §7.2).
- **`check_model_id` is written fresh**; there is nothing to generalise. **Do not copy** #2305's console `author/model` rule (`isProxyCompatible`): other providers' model ids need not contain a slash.

## 4. Target code

### 4.1 `check_model_id` (`src/company/inference/store.rs`, directly after `check_provider_name`)

```rust
/// The longest model id accepted, counted in `char`s (not bytes).
pub const MAX_MODEL_ID_CHARS: usize = 256;

/// The one validation every model-id write goes through: set-default, add, edit.
/// Returns the trimmed id. Never a network check: catalogues go stale, and an Azure
/// deployment name is never in `/models`.
pub fn check_model_id(raw: &str) -> crate::error::Result<String> {
    let invalid = |m: String| OpenCompanyError::InvalidRequest(m);
    let id = raw.trim();
    if id.is_empty() {
        return Err(invalid("Choose a model. A provider needs one model id.".into()));
    }
    if id.chars().any(char::is_control) {
        return Err(invalid("A model id cannot contain control characters.".into()));
    }
    if id.chars().any(char::is_whitespace) {
        return Err(invalid("A model id cannot contain spaces.".into()));
    }
    if id.chars().count() > MAX_MODEL_ID_CHARS {
        return Err(invalid(format!("A model id can be at most {MAX_MODEL_ID_CHARS} characters.")));
    }
    if crate::company::INFERENCE_TIERS.contains(&id) {
        return Err(invalid(format!("`{id}` is a workload name, not a model. Choose a model id.")));
    }
    Ok(id.to_string())
}
```

**Checks, in exact order:** trim → non-empty → no control character → no whitespace (control comes first because `\t` is both) → at most 256 `char`s → not exactly an `INFERENCE_TIERS` name (`src/company/types.rs:80`, case-sensitive).

Accepted, for example: `acme/test-model`, `acme/test-model:free`, `test-model:8b`, `test-model`, `test.deployment-1`. **Any id an endpoint returns is valid:** nothing in this slice hardcodes, filters, prefers or rejects an id by vendor or name. Import `OpenCompanyError` if `store.rs` lacks it; callers use `.map_err(ApiError)`.

### 4.2 Wire shapes and deletions (`providers.rs`)

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddProvider {
    kind: String,
    #[serde(default)] label: Option<String>,
    #[serde(default)] base_url: Option<String>,
    #[serde(default)] key: Option<String>,
    /// The one model this row serves. Required for every kind; `Option` only so a
    /// missing field gets `check_model_id`'s sentence rather than serde's 422.
    #[serde(default)] model: Option<String>,
    /// Also make this row the company default `{provider, model}`. Written last.
    #[serde(default)] make_default: bool,          // wire: "makeDefault"
    #[serde(default)] add_anyway: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditProvider {
    #[serde(default)] label: Option<String>,
    #[serde(default)] base_url: Option<String>,
    /// The one model id. Omit to leave unchanged. Replaces `models`.
    #[serde(default)] model: Option<String>,
    #[serde(default)] key: Option<String>,
}

/// `POST …/providers/{slug}/default`.
#[derive(Debug, Deserialize)]
struct SetDefault { #[serde(default)] model: Option<String> }
```

- Keep the existing doc comments on unchanged fields. Replace the long `AddProvider.model` comment (`:148-160`) with the one above.
- **`ProbeResultDto`:** delete `needs_model` (`:256-260`) and its constructor lines (`:461`, `:499`, and in `probe_draft` near `:1917` and `:1938`). Only `ProvidersTab.tsx:237` reads it, and part 2 removes that read.
- **`tier_overrides`** (`:680-688`) stays the storage encoder: the same id under all four tier keys, so rollback binaries and the legacy arm still read a model. It now takes `model: &str` and always returns four entries.
- **Delete `needs_an_explicit_model`** (`:658-670`, doc mentions at `:244` and `:666`), then **remove `TierVocabulary` from the `use` at `:60`**, or clippy `-D warnings` fails on the unused import.

### 4.3 `add_provider`

1. Delete `asked_model` (`:317-322`).
2. Directly after the slug and collision block (`:340-348`), and before the key write
   (`:350`), add:
   `let model = store::check_model_id(body.model.as_deref().unwrap_or("")).map_err(ApiError)?;`
   It sits after `plan_add` and the slug checks on purpose, so the existing refusal
   tests (credentialed endpoint, name bound, reserved slug, empty name) still get
   their own sentence first.
3. Change `:379` to `models: tier_overrides(&model),`.
4. Delete the rollback block at `:426-451` (its comment and the `if asked_model…`).
   It is unreachable now. **Keep** the auth rollback at `:478-488`.
5. After the auto-route block (`:506-513`) and before `Ok(Json(…))`, write the
   default. This is the last store write of the request:
   ```rust
   let note = if body.make_default {
       let choice = store::ModelChoice { provider: provider.slug.clone(), model: model.clone() };
       match store::set_default_choice(runtime.id(), secrets, &choice).await {
           Ok(()) => format!("{note} New work now goes through {} · {model}.", provider.label),
           Err(err) => {
               tracing::warn!(company = %runtime.id(), provider = %provider.slug, error = %err,
                   "added a provider but could not make it the default");
               format!("{note} It could not be made the default. Use Set as default.")
           }
       }
   } else { note };
   ```
   A failed default write does **not** fail the add. The row and the key are valid and
   visible, and a retry would only answer "already connected".

### 4.4 `edit_provider`

After the label block (`:955-967`) and **before** the key write (`:1002`):

```rust
let models = match body.model.as_deref() {
    Some(raw) => tier_overrides(&store::check_model_id(raw).map_err(ApiError)?),
    None => existing.models.clone(),
};
let new_model = body.model.as_deref().map(str::trim).map(str::to_string);
```

Change `:1025` to `models,`. After the record write succeeds (`:1030-1050`) and before
the health reset, move the default with the row. The row is written first, then the
default:

```rust
if let Some(new_model) = new_model {
    match store::load_default(runtime.id(), secrets).await {
        Ok(store::DefaultChoice::Full(c)) if c.provider == provider.slug && c.model != new_model => {
            let moved = store::ModelChoice { provider: c.provider, model: new_model };
            if let Err(err) = store::set_default_choice(runtime.id(), secrets, &moved).await {
                tracing::warn!(company = %runtime.id(), provider = %provider.slug, error = %err,
                    "edited the default row's model but could not move the default with it");
            }
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(company = %runtime.id(), error = %err,
            "could not read the default while editing a provider's model"),
    }
}
```

A `ProviderOnly(slug)` default is **not** upgraded here. Under Q1, only an explicit
set-default rewrites a bare slug.

### 4.5 `set_default`

```rust
async fn set_default(
    State(state): State<AppState>,
    company: AdminScopedCompany,
    Path(params): Path<ProviderPath>,
    Json(body): Json<SetDefault>,
) -> Result<Json<ProviderMutation>, ApiError> {
    let runtime = company.runtime.as_ref();
    let secrets = runtime.secrets().as_ref();
    let model = store::check_model_id(body.model.as_deref().unwrap_or("")).map_err(ApiError)?;
    let provider = require_provider(runtime, &params.slug).await?;
    if !provider.enabled { /* unchanged 400, :1301-1306 */ }

    // 1. The row's model first, so the default never names a model its row lacks.
    //    Entry zero has no index record; `put_provider` refuses its slug (store.rs:542-549).
    let draft = |models| store::ProviderDraft {
        slug: provider.slug.clone(), label: provider.label.clone(), kind: provider.kind.clone(),
        base_url: provider.base_url.clone(), models, enabled: provider.enabled,
    };
    let rewrite = provider.origin == store::ProviderOrigin::Indexed
        && !matches!(provider.model(), store::ModelOnRow::One(ref m) if *m == model);
    if rewrite {
        store::put_provider(runtime.id(), secrets, draft(tier_overrides(&model))).await.map_err(ApiError)?;
    }
    // 2. Then the one JSON write. On failure put the row back; if that fails too, say so loudly.
    let choice = store::ModelChoice { provider: provider.slug.clone(), model: model.clone() };
    if let Err(err) = store::set_default_choice(runtime.id(), secrets, &choice).await {
        if rewrite
            && let Err(restore) = store::put_provider(runtime.id(), secrets, draft(provider.models.clone())).await
        {
            tracing::error!(company = %runtime.id(), provider = %provider.slug, error = %restore,
                "set-default rewrote the row's model, failed to write the default, and \
                 failed to restore the row's previous models");
        }
        return Err(ApiError(err));
    }
    Ok(Json(ProviderMutation {
        status: effective_status(&state, runtime).await?,
        note: format!("New work now goes through {} · {model}.", provider.label),
        probe: None,
        affected_tiers: Vec::new(),
    }))
}
```

**Extractor order:** `AdminScopedCompany` still runs before `Json`, so 401 and 403 still come first. With no body or no `content-type`, axum answers `415`; `{}` gets our `400`.

### 4.6 `clear_default_if_marked` (`:1324-1345`)

Match on `store::load_default(…)?.provider() == Some(slug)`, which covers `ProviderOnly` and `Full`, instead of `Ok(Some(marked)) if marked == slug`. The clear stays `store::clear_default_slug` (a write of `""`). If 2b made `load_default_slug` a wrapper this changes no behaviour; make the change anyway, for clarity.

### 4.7 DTOs (`src/server/ops/inference.rs`)

```rust
// ProviderDto, after `models` (:420):
/// The row's one model, or `None` when it has none or several (`model_ambiguous`).
model: Option<String>,
/// Two or more distinct ids under the tier keys. Never guessed; the console says "Needs a model".
model_ambiguous: bool,

// InferenceStatusDto, after `providers` (:344):
/// The stored company default. `None` = unset; `model: None` = a legacy bare slug.
default_choice: Option<DefaultChoiceDto>,

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DefaultChoiceDto { provider: String, model: Option<String> }

/// Pure, so the bare-slug and ambiguous mappings are unit-tested without a store.
fn default_choice_dto(choice: store::DefaultChoice) -> Option<DefaultChoiceDto> {
    match choice {
        store::DefaultChoice::Unset => None,
        store::DefaultChoice::ProviderOnly(provider) => Some(DefaultChoiceDto { provider, model: None }),
        store::DefaultChoice::Full(c) => Some(DefaultChoiceDto { provider: c.provider, model: Some(c.model) }),
    }
}
fn model_on_row_dto(row: store::ModelOnRow) -> (Option<String>, bool) {
    match row {
        store::ModelOnRow::One(m) => (Some(m), false),
        store::ModelOnRow::None => (None, false),
        store::ModelOnRow::Ambiguous(_) => (None, true),
    }
}
```

- **`provider_list`** (`:513-534`): `let (model, model_ambiguous) = model_on_row_dto(provider.model());` **before** `provider.models` is moved.
- **`effective_status_with`**: beside `provider_list(runtime)` (`:876`), add `let default_choice = default_choice_dto(store::load_default(runtime.id(), secrets).await.map_err(ApiError)?);` and set it in **both** arms (`:886`, `:905`). The store is `crate::company::inference::store`.
- **No `skip_serializing_if`:** `null` means "no default", and an older host omits the field. `is_default` is unchanged.

### 4.8 JSON examples

**Set default:** `POST /api/v1/company/inference/providers/openrouter/default`

| Body | Answer |
|---|---|
| `{"model":"acme/test-model"}` | 200 `{"status": {…}, "note": "New work now goes through OpenRouter · acme/test-model."}` |
| `{}` | 400 "Choose a model. …" |
| `{"model":"chat-v1"}` | 400 "`chat-v1` is a workload name, not a model. …" |
| valid model, row switched off | 400 "OpenRouter is switched off, so it cannot be the default. Switch it on first." |
| unknown slug | 404 |

**Add:** `POST /api/v1/company/inference/providers`

```json
{"kind":"tinyhumans","key":"th-not-a-real-key","model":"acme/test-model","makeDefault":true}
```

The same body without `model` → 400 "Choose a model. …", and nothing is written.

**Edit:** `PUT /api/v1/company/inference/providers/openrouter`, body `{"model":"acme/other-model"}`.

**Status excerpt:** `GET /api/v1/company/inference`

```json
{ "defaultChoice": { "provider": "tinyhumans", "model": "acme/test-model" },
  "providers": [ { "slug": "tinyhumans", "label": "TinyHumans", "kind": "tinyhumans",
    "baseUrl": "https://api.tinyhumans.ai/agent-integrations/openrouter",
    "models": { "agentic-v1": "acme/test-model", "chat-v1": "acme/test-model",
                "reasoning-v1": "acme/test-model", "vision-v1": "acme/test-model" },
    "model": "acme/test-model", "modelAmbiguous": false, "enabled": true,
    "keyConfigured": true, "origin": "indexed", "isDefault": true,
    "health": { "state": "ok", "at": "2026-09-14T10:00:00Z" } } ] }
```

Other states: a bare-slug default is `"defaultChoice": {"provider":"tinyhumans","model":null}`; unset is `"defaultChoice": null`; an ambiguous row is `"model": null, "modelAmbiguous": true`.

### 4.9 A TinyHumans add: probe, model step, save, health

This slice adds no TinyHumans-specific code. This ordinary flow must hold on your branch:

1. **Draft probe (console).** `probeEndpoint("tinyhumans")` returns the catalogue endpoint, because the cloud branch (`connect.ts:252`) matches before the `MANAGED_OPTION_SLUG` line (`:254`). `POST …/inference/probe {baseUrl, key, kind:"tinyhumans"}` reads the paged catalogue on the host and answers with `models`, sorted and capped at 500.
2. **Model step.** It always opens (part 2 §8.5), filled with exactly the ids the endpoint returned. The operator picks one, and any of them is valid.
3. **Save.** One `POST …/inference/providers {kind:"tinyhumans", key, model, makeDefault}`, in §4.3's order: model check, key, row, probe.
4. **Health.** `plan_add` gives `probes: true`, and `auth_style_for("tinyhumans")` is bearer with a key present, so `worth_probing` (`:409`) is true. The add's probe records `"ok"`, or records the failure class, or rolls the add back on `auth`. **The row is never left `unchecked`.** The closed #2305 left it there by sending the key through `PUT …/inference/managed/key`, which never probes. **A `tinyhumans` add goes through `add_provider` only.**
5. **Nothing is added:** no Managed-specific model dialog, no new probe route, no `proxied_model` rule, no new secret-store key. The model lives in the `tinyhumans` row's `models`, like every provider's.

## 5. Ordered edit list (backend)

1. Start from a branch that already has 2a and 2b. Open every function in §3 and note
   its current lines.
2. `store.rs`: add `MAX_MODEL_ID_CHARS` and `check_model_id` (§4.1), with tests (part 2 §7.1).
3. `providers.rs` (§4.2): wire shapes, remove `needs_model`, update `tier_overrides`, delete `needs_an_explicit_model` and the `TierVocabulary` import.
4. `providers.rs` handlers: `add_provider` (§4.3), `edit_provider` (§4.4),
   `set_default` (§4.5), `clear_default_if_marked` (§4.6).
5. `ops/inference.rs`: add the DTO fields and the two pure mappers, and wire them in (§4.7).
6. Update, add and remove tests (part 2 §7.1–7.2), then `cargo fmt --all -- --check`.
   Commit, push, and verify by head SHA (part 2 §10) before starting the console.
7. Docs: change `docs/modules/inference/data-model.md:159-162` and `routing.md:352` to describe `{provider, model}` JSON, noting that a legacy bare slug still reads. Fix any empty-body description that `grep -rn "providers/{slug}/default" docs/` finds.

## 6. Data carry-over

**None is automatic.** Nothing rewrites stored values at boot or on read (Q1;
copy-if-empty only). What existing data looks like after this slice:

| Stored state | Status reports | Console shows | Turns |
|---|---|---|---|
| bare-slug default | `{"provider":"<slug>","model":null}` | banner "Your default provider has no model. Choose one." (part 2 §8.6) | legacy path (D-legacy), unchanged |
| row with several distinct ids | `modelAmbiguous: true` | chip "Needs a model" | not blocked |
| pre-slice row with `models: {}` | `model: null, modelAmbiguous: false` | no chip, unless it is the bare-slug default | unchanged |

### 6.1 Stored values: set default

**Setup:** a `tinyhumans` row with `acme/old-model` under every tier, and `inference/default` = the bare slug `tinyhumans`. **Call:** `POST …/tinyhumans/default {"model":"acme/test-model"}`.

| Key | Before | After |
|---|---|---|
| `inference/default` | `tinyhumans` | `{"provider":"tinyhumans","model":"acme/test-model"}` |
| `inference/providers`, `tinyhumans` element | `{"id":"prv_…","slug":"tinyhumans","label":"TinyHumans","kind":"tinyhumans","base_url":"https://api.tinyhumans.ai/agent-integrations/openrouter","models":{"agentic-v1":"acme/old-model","chat-v1":"acme/old-model","reasoning-v1":"acme/old-model","vision-v1":"acme/old-model"},"enabled":true}` | the same, with all four `models` values `acme/test-model` |
| `provider/tinyhumans/key`, `inference/routes`, `inference/config`, `inference/key` | as they were | unchanged |

If the row already holds `acme/test-model` everywhere, or the row is entry zero, only `inference/default` is written.

### 6.2 Stored values: add TinyHumans with makeDefault

**Setup:** no `tinyhumans` row, and `inference/default` is `""`. **Call:** `POST …/providers {"kind":"tinyhumans","key":"th-not-a-real-key","model":"acme/test-model","makeDefault":true}`. **Writes, in order:**
1. `provider/tinyhumans/key` = `th-not-a-real-key`.
2. `inference/providers` gains the element
   `{"id":"prv_…","slug":"tinyhumans","label":"TinyHumans","kind":"tinyhumans","base_url":"https://api.tinyhumans.ai/agent-integrations/openrouter","models":{"agentic-v1":"acme/test-model","chat-v1":"acme/test-model","reasoning-v1":"acme/test-model","vision-v1":"acme/test-model"},"enabled":true}`.
3. `inference/health` gains `"tinyhumans":{"state":"ok",…}`, or the probe's class.
4. `inference/routes` gains four `tinyhumans` rows, **only if** `auto_route_sole_provider`
   applies (legacy, unchanged).
5. `inference/default` = `{"provider":"tinyhumans","model":"acme/test-model"}`.

Without `makeDefault`, step 5 does not happen. If the probe fails with `auth`, steps 1–2 are rolled back (`roll_back_add`) and nothing remains.

## 11. Gotchas (backend)

- **No transaction.** Validate everything before the first write; the default is always the last write. **This corrects the brief's "row → key → default":** the add writes the **key before the record** (`:350-394`) because the probe reads the key by slug, and a failed record write then clears the key (`clear_orphaned_key`, `:389-391`). Keep that order. This slice adds only two rules: "default last", and "row model before default" in `set_default`.
- **The rollback at `providers.rs:426-451` goes away**, because the model is now
  checked before any write. The auth rollback at `:478-488` stays. Do not use it for a
  failed default write.
- **Azure deployment names are never in `/models`** (`providers.rs:1600-1610`;
  `catalogue::is_azure_endpoint`, `catalogue.rs:929`). Never check a model id against a
  catalogue on the host.
- **Assume nothing about what a catalogue contains.** Any id returned by `GET …/models` is valid. Code never hardcodes, filters, prefers or rejects ids by vendor or name. Tests and examples use fake ids from a mock catalogue (`acme/test-model`). The only fixed facts are shapes: the TinyHumans catalogue is a paged envelope, and its chat is OpenAI-compatible.
- **The TinyHumans proxy returns 400 for tier names and for ids not in its catalogue.**
  `check_model_id` catches tier names. A non-catalogue id is accepted here and fails
  on Test or on a turn; free text must stay possible.
- **`put_provider` refuses entry zero's slug** (`store.rs:542-549`). `set_default` skips the row write for it (§4.5), and `edit_provider` already refuses it (`:917-923`). A company whose `inference/config` is `managed` has entry zero with slug `tinyhumans`, so adding the catalogue row there is refused as "already connected" (`:343-348`).
- **Keys never go to the browser.** Catalogues are read on the host by
  `POST …/inference/probe` and `GET …/providers/{slug}/models`, and neither returns a
  key. No new route is needed.
- **No new secret-store key** (D-set). If one seems needed, stop and report.
- **`tests/snapshots/auth-matrix.txt` must not move.** Auth is unchanged, and the
  matrix classifies by 401/403 only (`tests/auth_matrix.rs:140-181`). Rows
  `:1933-1939`, `:2458-2471` and `:3116-3122` stay byte-identical. A diff there means an
  extractor order changed: fix the code, not the snapshot.
- **Search has its own `load_default_slug`/`set_default_slug`**
  (`src/company/search/store.rs:622-714`). It is unrelated; do not touch it.

Continued in [phase-2c-model-required-part2.md](phase-2c-model-required-part2.md).
