# Target state: every key after phase 5b (part 1 of 2)

This describes every key **after** all slices have landed, in this order: 1a,
1b, 1c, 2a, 2b, 2c, 2d, 3a, 3b, 4a, 4b, 4c, 5a, 5b, 6a. Unless a row says
otherwise, "after the rework" means after 6a, the last slice.

- **Slice numbering** follows the 2026-09-14 renumbering:
  - **2a** puts TinyHumans on the proxy (`phase-2a-tinyhumans-on-proxy.md`);
  - **2b** changes the default's shape;
  - **2c** makes a model required;
  - **2d** stops any tier name reaching the wire.
- **PR #2305 was closed without merging** on 2026-09-14. Nothing from it is
  assumed; slice 2a delivers the TinyHumans provider row.
- **Today's values** are in [current-state.md](current-state.md), which carries
  every current `file:line` on `fcfb3e1bc`.
- **Part 2** ([target-state-part2.md](target-state-part2.md)) covers:
  - the keys that stop being read or change shape, with before/after JSON;
  - the keys that stay exactly as today;
  - the item 15 "is it set?" rule.

## Standing rules

- **No new secret-store key.** Nothing is renamed or deleted in the store.
- **Carry-over takes only these shapes:**
  - **copy-if-empty:** write a new address only when it is empty;
  - **converge-on-write:** a real save writes the new address;
  - **dual-write for one release** (1a, 1b): a save also mirrors the same
    value to the old address, and a clear clears both.
- **No boot step clears or overwrites a value.**
- **A company with no full default and no agent pair** still resolves through
  today's *sources*: entry zero `inference/config`, the manifest
  `[inference]`, the env default, the managed chain, and routes until 5b. Two
  things change for it:
  - after 2a the platform URL constants name the proxy;
  - after 2d no tier name is sent as a model.

All example values are illustrative. Credentials are fake:
`th-not-a-real-key`, `ak-not-a-real-key` and `sk-not-a-real-key`.

---

## 1. Resolution after the rework (the readers everything below feeds)

```text
resolve_for_turn(company, manifest, env, secrets, scope, pin: Option<ModelChoice>, legacy_hint)
  1. agent pair (3a)       pin = Some -> store::get_provider(pin.provider)
                                         missing/disabled -> Err (F6, never falls through)
                                         else decl_for_choice(p).with_chosen_model(pin.model)
  2. named harness         !scope.is_default && harness_configures_itself -> step 4
  3. company default (2b)  store::load_default() == Full(c) -> same as step 1 with c
                           ProviderOnly(_) | Unset -> step 4
  4. legacy sources        entry zero, manifest [inference], env default, managed chain
                           (routes read here until 5b)
                           model (2d): OPENCOMPANY_INFERENCE_MODEL or a configured real id
                           (never a tier name), else
                           Err("No model is chosen for this company. Choose a default
                                provider and model in Settings → Inference.")
```

- **The wire model** (`request_plan`) is `decl.chosen_model()` when it is
  `Some`. Otherwise it follows the step 4 rule from 2d. **A tier name is never
  sent** after 2d. The TinyHumans proxy answers 400 to tier names and to ids
  outside its catalogue.
- **When `OPENCOMPANY_INFERENCE_MODEL` and a configured id both exist,**
  phase-2d fixes which one wins.
- **Internal passes** (title, triage, planning and the rest) use the default's
  chosen model (Q13), else the step 4 rule.

---

## 2. Account

### `tinyhumans/key` (credential; shape unchanged)

- **Value:** a raw string, trimmed. Example: `th-not-a-real-key`.

**Writers:**

| Writer | Slice | What changes |
|---|---|---|
| `PUT …/credential` → `set_key` (`ops/company_key.rs:243`) | 4a | After `store_key`, it calls `company_key::fan_out` under a per-company lock (rule below). |
| `POST …/credential/link/finish` → `finish_link` (`ops/company_key.rs:446`) | 4a (Q10) | It runs the same `fan_out` and **stops** calling `save_runtime_config` (`:486`). |

**Readers:**

| Reader | Slice | Note |
|---|---|---|
| `company_key::load` / `resolve` | unchanged | Still managed LLM chain step 3 and Composio chain step 2. Item 10 is not handled: see [not-handled.md](not-handled.md). |
| `get_billing` (`ops/company_key.rs:562`) | unchanged | |
| `company_key::fan_out` | 4a | Reads the **old** value before the write, for rule Q7. |
| The reuse-banner copy routes | 4c | The source of a host-side copy. The key never leaves the host. |

**The `company_key::fan_out` rule (4a, Q7).** Every derived slot follows this
table:

| Derived slot's current value (trimmed) | Save of a new account key | Clear of the account key |
|---|---|---|
| empty | write the new key | leave it |
| equal to the **old** account key | write the new key | write `""` |
| anything else | leave it | leave it |

The fan-out runs these writes in this order:

1. `tinyhumans/key`.
2. `composio/tinyhumans/key`, by the rule above, and mirrored to
   `composio/token` for one release (1a).
3. `provider/tinyhumans/key`, by the rule above.
4. A `tinyhumans` row in `inference/providers`, only when no row has that
   slug. The row is built from the 2a catalogue row: `kind` = `tinyhumans`,
   `base_url` = `https://api.tinyhumans.ai/agent-integrations/openrouter`,
   `enabled` = true. `put_provider` refuses it when entry zero's slug is
   already `tinyhumans` (`inference/store.rs:542-549`); the fan-out then skips
   the row and names that in its note.
5. `inference/default`, only when `load_default()` is `Unset` **and** a model
   was sent (for example `acme/test-model`).
6. The health probe records `inference/health["tinyhumans"]`.

On health class `auth` (Q6), keep steps 1 and 2 and roll back whatever **this
request** wrote in steps 3 to 5, restoring the previous value or `""`.

---

## 3. LLM

### 3.0 TinyHumans endpoints and URL constants (2a)

| Constant or endpoint | Today (`fcfb3e1bc`) | After 2a |
|---|---|---|
| `PLATFORM_BASE_URL` (`src/company/inference.rs:152`) | `https://api.tinyhumans.ai/openai/v1` | `https://api.tinyhumans.ai/agent-integrations/openrouter` |
| `DEFAULT_TINYHUMANS_INFERENCE_URL` (`src/harness/built_in/provider.rs:55`) | `https://api.tinyhumans.ai/openai/v1` | `https://api.tinyhumans.ai/agent-integrations/openrouter` |
| `tinyhumans` row in `CLOUD_PROVIDERS` (`src/company/inference/catalogue.rs`, plus the console mirror `frontend/src/inference/catalogue.ts`) | none | endpoint `https://api.tinyhumans.ai/agent-integrations/openrouter`, bearer auth, key placeholder `th-...` |
| Model list | `GET {base}/models`, OpenAI shape | `GET https://api.tinyhumans.ai/agent-integrations/openrouter/models`, a paged envelope (below) |
| Chat | `{base}/chat/completions` | `https://api.tinyhumans.ai/agent-integrations/openrouter/chat/completions` |
| An injected `OPENCOMPANY_INFERENCE_URL` | wins over the constant (`resolve_endpoint`, `inference.rs:659-661`) | **still wins.** A manager that injects `…/openai/v1` keeps hosted tenants on the old surface. |

The paged envelope:

```json
{"success":true,"data":{"data":[{"id":"acme/test-model"}],"total":1,"limit":500,"offset":0}}
```

`limit` goes up to 500; page until `total` is reached. Pickers list exactly what
it returns, and any id it returns is valid: code never hardcodes, filters,
prefers or rejects ids by vendor or name.

### 3.1 `inference/providers` (setting; storage shape unchanged)

- **The JSON keys stay the same.** A row still stores its one model under all
  four tier keys: that is the storage encoding, not a selection.
- **2d renames `tier_overrides`** (`providers.rs:680-688`) to `uniform_models`.
  Its output is unchanged.

```json
[{"id":"prv_…","slug":"anthropic","label":"Anthropic","kind":"anthropic",
  "base_url":"https://api.anthropic.com/v1",
  "models":{"agentic-v1":"test-model-small","chat-v1":"test-model-small",
            "reasoning-v1":"test-model-small","vision-v1":"test-model-small"},
  "enabled":true},
 {"id":"prv_…","slug":"tinyhumans","label":"TinyHumans","kind":"tinyhumans",
  "base_url":"https://api.tinyhumans.ai/agent-integrations/openrouter",
  "models":{"agentic-v1":"acme/test-model","chat-v1":"acme/test-model",
            "reasoning-v1":"acme/test-model","vision-v1":"acme/test-model"},
  "enabled":true}]
```

**Writers:**

| Writer | Slice | Change |
|---|---|---|
| `POST …/inference/providers` → `add_provider` | 2a, 2c | 2a makes `{"kind":"tinyhumans","key":"th-not-a-real-key","model":"acme/test-model"}` addable; its catalogue read is the paged envelope. 2c makes `model` **required** for every kind, refused before any write and validated by `check_model_id`, and adds optional `makeDefault: bool`, which writes the default last. |
| `PUT …/inference/providers/{slug}` → `edit_provider` | 2c | The body `models` (a map) becomes `model: Option<String>`. When this row is the `Full` default, the default's model is rewritten in the same request, row first. |
| `POST …/inference/providers/{slug}/default` → `set_default` | 2c | May rewrite the row's model through `put_provider` with uniform models, before writing the default. |
| `DELETE …/inference/providers/{slug}`, `POST …/{slug}/enabled` | unchanged | Deleting the `tinyhumans` row clears `provider/tinyhumans/key` (`inference/store.rs:634-644`). |
| `company_key::fan_out` | 4a | Writes the `tinyhumans` row when absent (§2). |

**Readers:**

| Reader | Slice | Change |
|---|---|---|
| `store::list_providers` | unchanged | Entry zero is still first. |
| `Provider::model() -> ModelOnRow` | 2b | `One(id)` when the trimmed non-empty values hold exactly one distinct id. `None` when there are no values. `Ambiguous(Vec<String>)` for two or more; it never guesses. Entry zero collapses the same way. |
| `decl_for_choice(p)` → `decl_for_indexed` / entry zero via `resolve_legacy_scoped` | 2b | |
| `ProviderDto.model`, `ProviderDto.modelAmbiguous` | 2c | Status wire fields. |
| Agent-pair validation in `team_agent.rs` | 3a | The row must exist and be enabled. |
| `routes_carry::carry_routes_into_default` | 5a | The routed slug must exist and be enabled. |

A `tinyhumans` **index row** is resolved by `decl_for_indexed`, which sets
`proxied = false` (`inference.rs:1413-1443`). It therefore presents only
`provider/tinyhumans/key`, never `tinyhumans/key` or the instance identity
(`managed_identity` returns early at `inference.rs:1123-1125`).

### 3.2 `provider/<slug>/key` (credential; unchanged)

Its readers and writers are the same as today (current-state §3.2), plus two:

- **`company_key::fan_out` (4a)** writes `provider/tinyhumans/key` by the Q7
  rule.
- **The reuse banner's host-side copy (4c)** writes it from `tinyhumans/key`,
  on an explicit Yes only.

After 2a, `PUT …/inference/managed/key` and the `tinyhumans` row read and
write the **same** slot, `provider/tinyhumans/key`.

**Removing the TinyHumans key never touches `tinyhumans/key`** (Q8):

- the default is kept;
- the dialog warns;
- the banner appears.

### 3.3 `inference/default` (setting; **new value shape**, 2b)

```json
{"provider":"tinyhumans","model":"acme/test-model"}
```

**Parsing** (`store::load_default`, 2b):

1. Trim the value. Empty or missing is `DefaultChoice::Unset`.
2. A value starting with `{` is deserialised with serde as
   `{provider: String, model: Option<String>}`.
   - JSON that will not parse, or a blank `provider`, is a `Store` error.
   - A blank or absent `model` is `ProviderOnly(provider)`.
   - Otherwise it is `Full(ModelChoice { provider, model })`.
3. Any other value is a bare slug: `ProviderOnly(slug)`. It is **never
   rewritten**.

**Writers:**

| Writer | Slice | Writes |
|---|---|---|
| `store::set_default_choice(company, secrets, &ModelChoice)` | 2b | One JSON write. |
| `POST …/inference/providers/{slug}/default` `{"model":"…"}` → `set_default` | 2c | 400 without a model. The provider must be enabled (as today). Note: "New work now goes through <label> · <model>." |
| `POST …/inference/providers` `{…, "model":"…", "makeDefault":true}` | 2c | Written last. |
| `edit_provider` on the default row | 2c | Keeps the provider and replaces the model. |
| `clear_default_if_marked` | 2c | Writes `""` when `load_default()?.provider()` equals the slug being deleted or disabled. |
| `company_key::fan_out` | 4a | Only when `Unset` and a model was sent. Rolled back on `auth` (Q6). |
| `routes_carry::carry_routes_into_default` | 5a | Copy-if-empty: only when `Unset` (part 2 §1.6). |

**Readers:**

| Reader | Slice |
|---|---|
| `resolve_for_turn` step 3 | 2b |
| Boot `resolve_effective_scoped`: a `Full` default that resolves returns `Some(decl.with_chosen_model)` ahead of step 0 | 2b |
| `store::load_default_slug`, kept as `load_default()?.provider()` so its callers stay put: `decl_for_primary`, `provider_list` `isDefault`, `is_primary`, `managed_parked_tiers` (until 5b) | 2b |
| `InferenceStatusDto.defaultChoice: {provider, model: string \| null} \| null` | 2c |
| Agent editor label "Company default · <provider> · <model>" | 3b |
| `fan_out` empty check; `routes_carry` eligibility | 4a; 5a |

### 3.4 `inference/routes` (setting; **unread by turns after 5b**)

| Phase | Writers | Readers |
|---|---|---|
| through 5a | as today | as today, plus `routes_carry` (5a) and the status "routes not carried" banner field (5a) |
| after 5b | **none** (`put_routes`, auto-route and the scrub are deleted) | `routes_carry::carry_routes_into_default` only, with its own raw `ROUTES_KEY` read and its own minimal `slug:model` parsing |

### 3.5 `inference/health` (status; unchanged)

- It gains one writer: the `fan_out` probe (4a).
- 5b removes nothing that reads it.

### 3.6 `inference/managed/enabled`, `inference/config`, `inference/key`, `harness/<id>/…`

These keep their shape and readers; see part 2 §2. What changes:

- **The legacy managed chain** keeps its steps. Its URL becomes the proxy path
  (2a), and it sends a real model or fails closed (2d).
- **`finish_link`** stops writing `inference/config` (4a).
- **An agent pair** outranks a named harness's `[harness.inference]` (3a, Q5).

---

## 4. Composio (1a: renamed, dual-written for one release)

```rust
// src/company/composio.rs (1a)
pub const TINYHUMANS_KEY_KEY: &str = "composio/tinyhumans/key";
pub const BYOK_KEY_KEY: &str = "composio/byok/key";
/// Read fallback for `TINYHUMANS_KEY_KEY` only; also the one-release mirror target.
pub const LEGACY_TOKEN_KEY: &str = "composio/token";
/// Read fallback for `BYOK_KEY_KEY` only; also the one-release mirror target.
pub const LEGACY_API_KEY_KEY: &str = "composio/api_key";
```

| Key | Value | Writers (slice) | Readers (slice) |
|---|---|---|---|
| `composio/tinyhumans/key` | raw string, e.g. `th-not-a-real-key` | `store_token`: new address first, then the same value to `composio/token`; a clear writes `""` to both and a failed legacy write returns `Err` (1a). Also `fan_out` (4a) and the reuse-banner Composio copy (4c), both with the same mirror. | `resolve_credential`: new, then `LEGACY_TOKEN_KEY`, then `company_key::resolve` (1a); `token_configured` checks both (1a); `fan_out` (4a) |
| `composio/byok/key` | raw string, e.g. `ak-not-a-real-key` | `store_api_key`, direction-ordered (1a). Set: `BYOK_KEY_KEY`, then `LEGACY_API_KEY_KEY` (the same value), then `MODE_KEY = byok`. Clear: `MODE_KEY = managed`, then `BYOK_KEY_KEY = ""`, then `LEGACY_API_KEY_KEY = ""`. | `resolve_access` Byok arm: new, then `LEGACY_API_KEY_KEY`, with no other fallback (1a); `stored_api_key` (1a) |
| `composio/token` | legacy, same value as the new key | **still written as a mirror for one release** (1a); stopping the mirror is a later release, not #2306 | read fallback |
| `composio/api_key` | legacy, same value as the new key | **still written as a mirror for one release** (1a); stopping the mirror is a later release | read fallback |
| `composio/mode` | `"managed"` or `"byok"` | unchanged (Q11) | unchanged |
| `composio/defaults` | `{"gmail":"ca_123"}` | unchanged | unchanged |

**Gotcha for 1a:** today `TOKEN_KEY` and `API_KEY_KEY` are re-exported by
`src/harness/built_in/composio.rs:104-105` and imported by
`src/server/ops/composio.rs:950` and `:2489`, and by harness tests. Every one
of those sites moves with the rename. The address mapping is **never
crossed**:

- `tinyhumans` is paired only with `token`;
- `byok` is paired only with `api_key`.

---

## 5. Search (1b: endpoint in the row, dual-written for one release)

| Key | Value after 1b | Writers | Readers |
|---|---|---|---|
| `search/providers` | `[{"slug":"searxng","enabled":true,"endpoint":"https://searx.example"}]`. `IndexEntry` gains `#[serde(default, skip_serializing_if = "Option::is_none")] endpoint: Option<String>`. Never add `deny_unknown_fields`. | `put_provider_locked`: `Some` sets `entry.endpoint`; `None` **keeps** the stored endpoint (merge, never replace) | `list_providers`: `entry.endpoint`, else `search/provider/<slug>/endpoint`, else `search/endpoint` for entry zero |
| `search/provider/<slug>/endpoint` | unchanged string | **still written with the row for one release**; cleared on delete | read fallback |
| `search/provider/<slug>/key`, `search/default`, `search/provider`, `search/api_key`, `search/endpoint` | unchanged | unchanged | unchanged |

---

## 6. The agent record (3a): gains `provider`

| Record | Field added | Serde |
|---|---|---|
| `Agent` (`src/company/types.rs:634`) | `pub provider: Option<String>`, after `harness` (`:670`) | `#[serde(default)]` |
| `AgentFile` (`src/company/agent_file.rs`) | `provider: Option<String>`, mapped like `model` (`:293`) | `#[serde(default)]` |
| `OverlayAgent` (`src/ports/types.rs:3410`) | `pub provider: Option<String>` | `#[serde(default, skip_serializing_if = "Option::is_none")]` |
| `AgentOverride` (`ports/types.rs:3493`) | `pub provider: Option<String>`; `Some("")` means cleared | same |

```toml
# company.toml, hand-authored. A slug added only in the console fails closed at the first turn (F5).
[[agent]]
id = "researcher"
role = "Researcher"
harness = "built_in"
provider = "anthropic"
model = "test-model-large"
```

```json
{"overlay_agent_edits":{"web_search":{"provider":"tinyhumans","model":"acme/test-model"}}}
{"overlay_agent_edits":{"web_search":{"provider":"","model":""}}}
```

The second line is the stored cleared pair.

**The pairing rule:**

- On a `built_in` harness, `provider` and `model` must both be set, or both be
  unset.
- On an `acp` harness, `model` keeps its ACP meaning and `provider` is refused.

**Writers:**

- a hand-written manifest or `agents/<id>.toml`;
- `PATCH {scope}/team/{agent_id}` `{"provider":"…","model":"…"}` (3a, admin
  only, double option: `null` clears), sent by the editor in one request (3b).

**Readers:**

- `build.rs:1159-1162` calls `deps.provider.pinned(&ModelChoice)` →
  `HarnessModel::pinned` (3a);
- `resolve_for_turn(pin)`;
- the builder's `configured` also counts agents whose pair resolves;
- `AgentDetailDto.provider`;
- the rebuild fingerprint.

---

## 7. Environment variables

Columns below distinguish the **interim** state (after 2a/2d, before 6a — the
window in which a partial deploy of this branch could sit) from the **final**
state (after 6a, the last slice).

| Variable | Interim (after 2a/2d, before 6a) | Final (after 6a) |
|---|---|---|
| `OPENCOMPANY_INFERENCE_MODEL` | **Not removed by 6a.** The fallback model for a company with no full default and no agent pair (2d): the legacy path sends it, or a configured real id, else fails closed with "choose a model". Ignored whenever a chosen model exists (2b). Hosted tenants need the manager to inject it until each company sets a default. E2E fixture hosts set it. | Unchanged from interim. |
| `OPENCOMPANY_INFERENCE_URL` | Still outranks `PLATFORM_BASE_URL`. After 2a it must name the proxy path (`https://api.tinyhumans.ai/agent-integrations/openrouter`, or staging's equivalent), or be unset. | **Removed.** Nothing reads it. The base URL is always the constant (the proxy, after 2a's gated commit 5). A manager that used to inject a custom URL loses that lever — see [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md). |
| `OPENCOMPANY_INFERENCE_KEY` | Checked first, ahead of the instance-identity token source. | **Removed.** `hosted_endpoint_from_env`'s credential is `TinyhumansTokenSource::from_env` only. |
| `TINYHUMANS_TOKEN_FILE` | Instance-identity tier 1 (projected file), ahead of `TINYHUMANS_API_KEY`. | **Removed.** `TinyhumansTokenSource` has one tier left: `TINYHUMANS_API_KEY`. |
| `TINYHUMANS_API_KEY` | Instance-identity tier 2 (static), unchanged since before the rework. | **Unchanged, and now the only instance-level TinyHumans credential.** Still the last-resort step of both managed chains (item 10 is not handled); still the config-file `tinyhumans_api_key` path. |
| `OPENCOMPANY_COMPOSIO_BACKEND_URL` | Explicit Composio backend override, ahead of `TINYHUMANS_API_URL`. | **Removed.** Composio's backend URL is `TINYHUMANS_API_URL`, else the `DEFAULT_BACKEND_URL` constant. |
| `TINYHUMANS_API_URL` | Unchanged. | Unchanged, and now the only URL override for the Composio backend. |

Continued in [target-state-part2.md](target-state-part2.md).
