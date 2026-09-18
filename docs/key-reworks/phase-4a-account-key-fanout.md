# Phase 4a — account-key fan-out (server)

- **Goal:** one `PUT …/credential {key, model?}` (and the grant's `finish_link`)
  stores the account key and, under a per-company lock, copies it into the
  Composio and LLM TinyHumans slots by the Q7 rule, adds the `tinyhumans` row
  when a model is known, sets the default only when none is set, and checks
  health.
- **Dump items:** 6, 11. **Decisions:** Q6, Q7, Q10, D-set (no new keys), D-legacy.
- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14). Runs after 1a
  (Composio key names), 2a (the `tinyhumans` catalogue row on the proxy, its paged
  catalog and provider test path), 2b (`store::load_default`,
  `store::set_default_choice`, `DefaultChoice`, `Provider::model`) and 2c
  (`store::check_model_id`, `tier_overrides(&str)`). Re-verify every line number.
- **Not handled here:** item 10. Until it lands the legacy chain still resolves
  `tinyhumans/key` for Composio (`src/company/composio.rs:98-115`) and for a
  managed turn with no default (`src/company/inference.rs:1116-1137`). So the
  copies change what is **stored**, not what a company with no default presents
  today. Do not touch `company_key::resolve`.

## 1. Files

| File | What changes |
|---|---|
| `src/company/company_key.rs` | `mod fan_out; mod types; pub use …` (keep `KEY_KEY` :64, `store_key` :69, `resolve` :123 unchanged) |
| `src/company/company_key/types.rs` (new) | `Slot`, `SlotOutcome`, `SkipReason`, `SlotReport`, `FanOutRequest`, `FanOutReport` |
| `src/company/company_key/fan_out.rs` (new) | `slot_guard`, `decide_copy`, `fan_out`, `InferenceProber`, `LiveProber`, `fan_out_note` |
| `src/company/company_key/fan_out/test.rs` (new) | store-level tests |
| `src/server/ops/company_key.rs` | `SetKey` :178, `MutationResponse` :171, `set_key` :243-277, `finish_link` :446-505, `CONSEQUENCE` :83-91, `SWITCH_NOTE` :50 |
| `src/server/ops/company_key/test.rs` | new route tests; three existing tests change (§7) |
| `src/server/ops/inference/providers.rs` | take `slot_guard` in `set_managed_key` :1697, and in `add_provider` :309 / `edit_provider` :907 / `delete_provider` :1090 when slug is `tinyhumans`; make `catalogue_offer` :274 `pub(crate)` |
| `src/server/ops/composio.rs` | take `slot_guard` in the 1a TinyHumans-key write (today `set_token` :653) |

## 2. Current code

`set_key` writes one slot and stops (`src/server/ops/company_key.rs:249-276`):

```rust
store_key(runtime.id(), runtime.secrets().as_ref(), &body.key).await.map_err(ApiError)?;
super::composio::evict_catalog_cache(runtime);
let change = if body.key.trim().is_empty() { "company_key_cleared" } else { "company_key_set" };
journal(&company, change).await?;
Ok(Json(MutationResponse { status: effective_status(&state, runtime).await?, note: SWITCH_NOTE.to_string() }))
```

`finish_link` also declares entry zero managed (`:486-496`), which Q10 retires:

```rust
crate::company::inference::save_runtime_config(runtime.id(), runtime.secrets().as_ref(),
    &crate::company::inference::RuntimeInference { provider: "managed".to_string(), base_url: None, models: Default::default() })
```

The lock pattern to copy is `INDEX_LOCKS` / `index_guard`
(`src/company/search/store.rs:88-108`). `store::put_provider` refuses a slug equal
to entry zero's (`src/company/inference/store.rs:542-549`), and entry zero from
`inference/config={provider:"managed"}` has slug `tinyhumans`
(`credential_slug`, `src/company/inference.rs:422-428`). `load_managed_key`
(`inference.rs:936-963`) reads `provider/tinyhumans/key`, then `inference/key`
only when `store::legacy_slot_is_managed`.

## 3. Target code

### 3.1 Types (`src/company/company_key/types.rs`)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Slot { Composio, Inference, Provider, Default, Health }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    AlreadyCurrent,        // slot already holds the new key
    CustomKey,             // slot holds a key that is neither empty nor the old account key
    AlreadyEmpty,          // clear, and the slot was empty
    RowExists,             // a `tinyhumans` row already exists
    LegacyManagedConfig,   // entry zero is managed (`inference/config`), put_provider refuses the slug
    NeedsModel,            // no model sent and none on the row
    DefaultAlreadySet,     // `inference/default` is ProviderOnly or Full
    InferenceNotWritten,   // the LLM key slot does not hold the new key
    InferenceRejected,     // the health probe answered `auth`
    KeyCleared,            // this request cleared the account key
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotOutcome {
    Filled, Rotated, Cleared, RolledBack,
    Kept(SkipReason), Skipped(SkipReason),
    Failed,                               // a store write failed; detail "store"
    HealthOk, HealthFailed(ProbeClass),   // health slot only
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotReport { pub slot: Slot, pub outcome: SlotOutcome }

pub struct FanOutRequest<'a> { pub key: &'a str, pub model: Option<&'a str> }

#[derive(Clone, Debug, Default)]
pub struct FanOutReport {
    pub slots: Vec<SlotReport>,   // always in order composio, inference, provider, default, health
    pub needs_model: bool,
    pub sets_default: bool,       // a model sent now would also become the default
    pub models: Vec<String>,      // catalog ids, only when needs_model; sorted, deduped, capped at 500
}
```

`FanOutReport` must never hold a key: no `String` field other than model ids.

Wire spelling of `SlotReport` (build a `SlotReportDto` in the ops module):
`outcome` is `filled | rotated | cleared | rolledBack | kept | skipped | failed | ok`
(health) and `detail` is the camelCase `SkipReason`, `"store"` for `Failed`, or
the probe class (`auth`, `endpoint`, …) for `HealthFailed`.

### 3.2 Functions (`src/company/company_key/fan_out.rs`)

```rust
static SLOT_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = …; // as INDEX_LOCKS
pub async fn slot_guard(company: &CompanyId) -> tokio::sync::OwnedMutexGuard<()>;    // as index_guard

pub enum CopyDecision { Write, Clear, Keep(SkipReason), Skip(SkipReason) }
/// Pure. `current`, `old_account`, `new` are already trimmed.
pub fn decide_copy(current: &str, old_account: &str, new: &str) -> CopyDecision;

#[async_trait]
pub trait InferenceProber: Send + Sync {
    async fn probe(&self, base_url: &str, key: &str) -> Result<Vec<String>, probe::ProbeFailure>;
}
pub struct LiveProber;  // the provider test path 2a wires for `tinyhumans`: probe::probe_models(base, Some(key),
                        //   catalogue::auth_style_for("tinyhumans"), probe::default_policy(), <2a's paged catalog shape>)

/// Err only when the pre-write reads or the account-key write fail (nothing
/// else has been written then). Every later failure is a `Failed` slot.
pub async fn fan_out(company: &CompanyId, secrets: &dyn SecretStore,
    request: FanOutRequest<'_>, prober: &dyn InferenceProber) -> Result<FanOutReport>;

/// Pure. The plain-words note (§3.4). Never contains a key.
pub fn fan_out_note(clearing: bool, report: &FanOutReport, model: Option<&str>) -> String;
```

`decide_copy` table (the whole Q7 rule):

| clearing? | `current` | decision |
|---|---|---|
| no | `== new` | `Keep(AlreadyCurrent)` |
| no | empty | `Write` → `Filled` |
| no | `== old_account` (old non-empty) | `Write` → `Rotated` |
| no | anything else | `Keep(CustomKey)` |
| yes | empty | `Skip(AlreadyEmpty)` |
| yes | `== old_account` | `Clear` → `Cleared` |
| yes | anything else | `Keep(CustomKey)` |

### 3.3 Request / response DTOs (`src/server/ops/company_key.rs`)

```rust
#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")]
struct SetKey { key: String, #[serde(default)] model: Option<String> }

#[derive(Debug, Serialize)] #[serde(rename_all = "camelCase")]
struct MutationResponse {
    status: CredentialStatusDto,
    note: String,
    slots: Vec<SlotReportDto>,
    needs_model: bool,
    sets_default: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")] models: Vec<String>,
}
```

First save on an empty company, no model (matrix row M1):

```json
{ "status": { "configured": true, "source": "company", "notice": "…", "hubLink": false },
  "note": "Key saved. Composio now uses this key. Choose a model to finish setting up TinyHumans for LLM.",
  "slots": [ {"slot":"composio","outcome":"filled"}, {"slot":"inference","outcome":"filled"},
             {"slot":"provider","outcome":"skipped","detail":"needsModel"},
             {"slot":"default","outcome":"skipped","detail":"needsModel"},
             {"slot":"health","outcome":"ok"} ],
  "needsModel": true, "setsDefault": true, "models": ["acme/test-model", "acme/other-model"] }
```

Second post `{"key":"th-not-a-real-key-2","model":"acme/test-model"}` (M11): `composio`
and `inference` are `kept` / `alreadyCurrent`, `provider` `filled`, `default`
`filled`, `health` `ok`, `needsModel: false`.

## 4. Ordered edits

1. Create `types.rs` and `fan_out.rs`; add `mod fan_out; mod types; pub use fan_out::{…}; pub use types::*;` to `company_key.rs`.
2. Implement `decide_copy` and its table test first.
3. Implement `fan_out` in exactly this order, holding `slot_guard` for the whole call:
   1. **Validate.** `model` trimmed; empty → `None`. `Some` with a clearing key → `InvalidRequest("A model cannot be chosen while removing the key.")`. `Some(raw)` → `let model = store::check_model_id(raw)?` (2c; its error is already `InvalidRequest`). Nothing written yet.
   2. **Read** (any error returns `Err`): `old_account` = trimmed `tinyhumans/key`; `composio_now` = trimmed `composio::TINYHUMANS_KEY_KEY`, else trimmed `composio::LEGACY_TOKEN_KEY` (1a's reader if it exports one); `providers = store::list_providers`; `legacy_managed = providers` has `origin == EntryZero && slug == MANAGED_SLUG`; `row = providers` entry with `origin == Indexed && slug == "tinyhumans"`; `inference_raw_new` = raw `provider/tinyhumans/key` (untrimmed, for exact restore); `legacy_owned = store::legacy_slot_is_managed`; `legacy_raw` = raw `inference::KEY_KEY` when `legacy_owned`; `inference_now` = trimmed `inference::load_managed_key(…, &HarnessScope::default())`; `default_raw` = raw `inference/default`; `default_now = store::load_default`.
   3. **Account key.** `store_key(new)`. Error → `Err`.
   4. **Composio.** `decide_copy(composio_now, old_account, new)`. `Write`/`Clear` go through 1a's TinyHumans-key write (new address = value, legacy `composio/token` = `""`). Error → `Failed`, continue. Never read or write `composio/mode` or `composio/byok/key`.
   5. **Inference key.** `decide_copy(inference_now, old_account, new)`. `Write`/`Clear`: set `provider/tinyhumans/key`; then, when `legacy_owned` and `legacy_raw` is non-empty, set `inference/key` to `""` (the `set_managed_key` convergence, `providers.rs:1735-1764`). Error on the first set → `Failed`; error on the legacy clear → `Failed` on a clear, logged-only on a write (same rule as `set_managed_key`). `holds_new` = outcome is `Filled`, `Rotated` or `Kept(AlreadyCurrent)`.
   6. **Clearing stops here:** `provider` and `default` are `Skipped(KeyCleared)`. `health` is `Cleared` after a successful `store::forget_health("tinyhumans")` when inference was `Cleared`, else `Skipped(KeyCleared)`. Rows and default are never touched by a clear (§6 C1).
   7. **Health, before any row or default write** (Q6 by construction). Skip with `LegacyManagedConfig` when `legacy_managed`; with `InferenceNotWritten` or `CustomKey` when not `holds_new`. Else `base = row.base_url` or `catalogue::cloud_provider("tinyhumans").endpoint`; `prober.probe(base, new)`. `Ok(ids)` → `store::record_health("tinyhumans","ok",now)` → `HealthOk`, keep `ids`. `Err(f)` → `record_health(f.class.as_str())` → `HealthFailed(f.class)`. A `record_health` error is logged at `warn` and changes no outcome.
   8. **Q6 rollback** when the class is `Auth`: if this request wrote the inference key, set `provider/tinyhumans/key` back to `inference_raw_new` (or `""` if absent) and, if it cleared `inference/key`, set that back to `legacy_raw`; outcome `RolledBack`. Keep the account key and the Composio copy. `provider` and `default` → `Skipped(InferenceRejected)`. `needs_model = false`. A restore error is logged at `error` naming the slot, never the value.
   9. **Provider row.** In order: `legacy_managed` → `Skipped(LegacyManagedConfig)`; not `holds_new` → `Skipped(InferenceNotWritten|CustomKey)`; `row` exists → `Kept(RowExists)`; `model` is `None` → `Skipped(NeedsModel)`, `needs_model = true`; else, with `cat = catalogue::cloud_provider("tinyhumans")` (2a), `store::put_provider(ProviderDraft { slug: "tinyhumans".into(), label: cat.label.into(), kind: "tinyhumans".into(), base_url: cat.endpoint.into(), models: tier_overrides(&model), enabled: true })` (the same draft 2c's `add_provider` builds) → `Filled`. Error → `Failed` (the key copy stays; it is the same stored state a `needsModel` answer leaves).
   10. **Default.** `default_now != Unset` → `Kept(DefaultAlreadySet)`. Provider `Filled` → `set_default_choice({provider:"tinyhumans", model})`. Provider `Kept(RowExists)` with `row.model() == ModelOnRow::One(m)` and no model sent → write `{tinyhumans, m}`; with `None`/`Ambiguous` → sent model if any, else `Skipped(NeedsModel)` and `needs_model = true`. Otherwise `Skipped(<provider's reason>)`. Error → `Failed`; the row stays.
   11. `sets_default = needs_model && default_now == Unset`. `models = catalogue_offer(ids)` only when `needs_model` and the probe succeeded.
4. `set_key`: parse `SetKey`, call `fan_out(…, &prober_for(runtime))`, evict the Composio catalog (as today) and `inference_models::evict_company_catalogs` when inference changed, journal (§3.5), answer `MutationResponse` with `note = fan_out_note(…)`. Delete `SWITCH_NOTE` once nothing uses it.
5. `prober_for`: `#[cfg(test)] mod prober_override` keyed by company id, a copy of `ops::composio::probe_override` (`composio.rs:845-870`) holding `Result<Vec<String>, ProbeClass>`; production is `LiveProber`.
6. `finish_link`: after `store_key` is replaced by `fan_out(FanOutRequest { key: &key, model: None }, …)`, **delete** the `save_runtime_config` block (`:480-496`) and rewrite the #2266 doc comment (`:432-445`) to say the grant runs the same fan-out and declares no provider (Q10). Keep `start_link`, the hub checks and single-use `take`.
7. Rewrite `CONSEQUENCE` (§3.4); keep `DEGRADED`.
8. Add `let _guard = company_key::slot_guard(runtime.id()).await;` as the first line of the handlers in §1 (only when slug is `tinyhumans` for add/edit/delete).

### 3.4 Copy

`fan_out_note` joins, in this order, the sentences that apply (fixed strings; no interpolation except the model id):

| Condition | Sentence |
|---|---|
| always | `Key saved.` / `Key removed.` |
| composio `Filled`/`Rotated` | `Composio now uses this key. A key you created by hand may lack the connections permission Composio needs.` |
| composio `Kept(CustomKey)` | `Composio keeps the key set on its own page.` |
| composio `Cleared` | `Composio's copy was removed too.` |
| inference `Kept(CustomKey)` | `LLM keeps the TinyHumans key set on its own page.` |
| provider `Filled` | `TinyHumans is set up for LLM with {model}.` |
| default `Filled` | `It is now the default for new work.` |
| `needs_model` | `Choose a model to finish setting up TinyHumans for LLM.` |
| provider `Skipped(LegacyManagedConfig)` | `LLM keeps this company's existing TinyHumans setup.` |
| health `HealthFailed(Auth)` | `TinyHumans rejected this key for LLM, so the LLM copy was not kept.` |
| health `HealthFailed(other)` | `probe::describe(class, "TinyHumans")` |
| inference `Cleared` | `LLM's copy was removed too; TinyHumans stays on the LLM page without a key.` |
| any `Failed` | `Some copies could not be saved — check the LLM and Composio pages.` |

New `CONSEQUENCE` (replaces the three sentences from "Where this company's models…" to "…leaves the choice of provider alone."): `Saving it also copies it to TinyHumans on the LLM page and to Composio wherever those hold no key of their own, and makes TinyHumans the default only when no default is set.`

**Composio scope check — decided: none.** Evidence: `set_token` explicitly declines to probe this bearer because "there is no cheap call here that distinguishes a bad bearer from a backend that is down" (`src/server/ops/composio.rs:692-697`); the only backend read, `harness::built_in::composio::list_catalog_toolkits` (`src/harness/built_in/composio.rs:1409`), is `#[cfg(feature = "composio")]`, needs a resolved `TenantComposio` rather than a candidate key, and a `403` is ambiguous between scope and WAF (`composio_probe.rs:28-31`). Before item 10 the copy presents the same bearer the fallback already presents. The note says so instead.

### 3.5 Journal

After `fan_out` returns, `ToolAccessChanged { change, toolkit: None, by }`: first `company_key_set` / `company_key_cleared` (unchanged), then one entry per slot whose outcome changed stored state: `company_key_{slot}_{filled|rotated|cleared|rolled_back}` with slot in `composio|inference|provider|default` (the row is `company_key_provider_filled`). No entry for `kept`, `skipped`, `failed` or health. A journal error still propagates as today (`a_journal_failure_after_the_key_is_stored_still_leaves_the_key_stored`).

## 6. Data carry-over matrix

`A` = `th-not-a-real-key` (old account key), `B` = `th-not-a-real-key-2` (new), `C` = `th-not-a-real-key-custom` (set on a page). `row(m)` = `tinyhumans` row with model `m` = `acme/test-model`. `default` shows `inference/default`. Prober answers `ok` unless stated. Nothing in this matrix ever writes `inference/config`, `composio/mode`, `composio/byok/key`, `inference/managed/enabled` or `inference/routes`.

| # | Before: `tinyhumans/key` · `composio/tinyhumans/key` · `provider/tinyhumans/key` · rows · default | Request | After (same order) and flags |
|---|---|---|---|
| M1 | `""` · `""` · `""` · none · unset | `{B}` | `B` · `B` filled · `B` filled · none · unset; `needsModel`, `setsDefault` |
| M2 | as M1 | `{B,m}` | `B` · `B` · `B` · `row(m)` filled · `{tinyhumans,m}` filled |
| M3 | `""` · `""` · `""` · `openrouter` · `{openrouter,x}` | `{B,m}` | `B` · `B` · `B` · `openrouter`, `row(m)` · kept |
| M4 | `""` · `""` · `""` · none · `"openrouter"` (bare) | `{B}` | `B` · `B` · `B` · none · kept; `needsModel`, not `setsDefault` |
| M5 | `A` · `A` · `A` · `row(m)` · `{tinyhumans,m}` | `{B}` | `B` · `B` rotated · `B` rotated · `row(m)` kept · kept |
| M6 | `A` · `C` · `C` · none · unset | `{B}` | `B` · `C` kept · `C` kept · none (skipped customKey) · unset; health skipped |
| M7 | `A` · `A` · `""` · `row(m)` · `{tinyhumans,m}` | `{B}` | `B` · `B` · `B` filled · `row(m)` · kept |
| M8 | `A` · `""` · `""` + `inference/config={managed}`, `inference/key=A` · unset | `{B}` | `B` · `B` · `B` rotated, `inference/key=""` · none (legacyManagedConfig) · unset; health skipped; no `needsModel` |
| M9 | `A` · `""` · `""` + entry zero `openrouter`, `inference/key=sk-not-a-real-key` · unset | `{B,m}` | `B` · `B` · `B` filled, `inference/key` untouched · `row(m)` · `{tinyhumans,m}` |
| M10 | as M1, prober `auth` | `{B}` | `B` · `B` · `""` rolledBack · none · unset; health failed `auth`; no `needsModel` |
| M11 | after M1 | `{B,m}` | `B` · `B` kept · `B` kept · `row(m)` filled · `{tinyhumans,m}` filled |
| M12 | `A` · `A` · `A` · `row(m)` · unset | `{B}` | `B` · `B` · `B` · `row(m)` · `{tinyhumans,m}` filled (row's model) |
| M13 | `""` · `""` + `composio/mode=byok`, `composio/byok/key=ak-not-a-real-key` · `""` · none · unset | `{B}` | composio `B` filled; mode and byok unchanged |
| M14 | `A` · legacy `composio/token=A` only · `A` · `row(m)` · kept | `{B}` | composio new address `B`, `composio/token=""` (rotated) |
| C1 | `B` · `B` · `B` · `row(m)` · `{tinyhumans,m}` | `{""}` | `""` · `""` cleared · `""` cleared · `row(m)` stays · stays; health forgotten |
| C2 | `B` · `C` · `C` · `row(m)` · stays | `{""}` | `""` · `C` kept · `C` kept |
| C3 | `B` · `""` · `""` · none · unset | `{""}` | `""` · skipped alreadyEmpty · skipped alreadyEmpty |

## 7. Tests

**`src/company/company_key/fan_out/test.rs`** — local `MemSecrets` (copy of `store.rs:1032-1051`), `FailsWriting { inner, failing_key }` (copy of `store.rs:1053-1070`), and `FakeProber { answer: Result<Vec<String>, ProbeClass>, calls: AtomicUsize }` building `ProbeFailure { class, raw: "fake".into() }`.

| Test | Setup → call → assert |
|---|---|
| `decide_copy_follows_the_q7_table` | every row of §3.2's table |
| `matrix_m1` … `matrix_m14`, `matrix_c1` … `matrix_c3` | seed the "Before" column → `fan_out` → assert every "After" value by raw `get`, each `SlotReport`, and the flags |
| `an_auth_probe_restores_the_llm_slots_exactly` | M8 with prober `auth` → `provider/tinyhumans/key` raw equals its raw before, `inference/key == A` |
| `a_non_auth_probe_failure_keeps_everything` | M2 with prober `endpoint` → row and default written, health `HealthFailed(Endpoint)` |
| `an_invalid_model_writes_nothing` | `{B, model:"chat-v1"}` → `Err`, every key unchanged, `calls == 0` |
| `a_model_with_a_clear_is_refused` | `{"", m}` → `Err`, nothing written |
| `failing_account_key_write_writes_nothing_else` | `failing_key = "tinyhumans/key"` → `Err`; composio and inference empty |
| `failing_composio_write_still_sets_up_llm` | `failing_key = "composio/tinyhumans/key"`, M2 → composio `Failed`, row and default written |
| `failing_inference_write_skips_row_default_and_probe` | `failing_key = "provider/tinyhumans/key"` → `Failed`; provider/default/health `Skipped(InferenceNotWritten)`; `calls == 0` |
| `failing_row_write_keeps_the_key_copy` | `failing_key = "inference/providers"`, M2 → provider `Failed`, `provider/tinyhumans/key == B`, default `Skipped` |
| `failing_default_write_keeps_the_row` | `failing_key = "inference/default"`, M2 → default `Failed`, row present |
| `failing_health_record_does_not_change_outcomes` | `failing_key = "inference/health"` → health `HealthOk` |
| `concurrent_saves_leave_every_copy_equal_to_the_account_key` | `Arc` store whose `set` sleeps 2 ms; seed `A/A/A`; 20 iterations of `join!(fan_out(B), fan_out(B-2))` → `composio == inference == tinyhumans/key`. Red-proof in the PR: without the guard this fails |
| `no_report_or_note_contains_a_key` | M5 → `format!("{report:?}")` and `fan_out_note(…)` contain neither `A` nor `B` |

**`src/server/ops/company_key/test.rs`** — `state_with_manifest` sets `prober_override::set(company, Ok(vec!["acme/test-model".into()]))` so no test dials the network.

- New: `put_credential_answers_slots_and_needs_model` (M1 by route: JSON shape of §3.3); `put_credential_with_a_model_adds_the_row_and_default` (M2; `GET …/inference` lists `tinyhumans` with `origin: "indexed"`); `put_credential_auth_probe_rolls_back_the_llm_copy` (override `Err(Auth)`); `put_credential_never_echoes_the_key` (M5 raw body and every journal `change` string lack both keys); `put_credential_journals_one_line_per_changed_slot` (M2 → `company_key_set`, `company_key_composio_filled`, `company_key_inference_filled`, `company_key_provider_filled`, `company_key_default_filled`); `finish_link_runs_the_fan_out_and_writes_no_inference_config` (after finish, `inference/config` is absent — `GET …/inference` has no `entryZero` row — and `managed.source == "provider_key"`).
- Change `the_key_round_trips_write_only_and_reports_the_company_tier` (:127): replace the `resolve through this same key`, `outranks it` and `leaves the choice of provider alone` assertions with `no key of their own` and `only when no default is set`.
- Change `setting_the_key_credentials_composio_with_no_composio_token` (:223): rename `setting_the_key_fills_the_composio_tinyhumans_key`; `credentialSource` is now the stored-token tier (`"static"` today; follow 1a if it renames the tier).
- Change `finishing_a_link_stores_the_minted_key_as_both_the_company_and_inference_credential` (:742): rename `finishing_a_link_copies_the_minted_key_without_declaring_a_provider`; drop the `keyConfigured == true` assertion (nothing declares entry zero any more).
- `clearing_the_key_reverts_to_the_degraded_state` and `a_pasted_composio_token_still_outranks_the_company_key` must pass unchanged (C1 and M6 in route form).
- `tests/auth_matrix.rs`: no change — no route path or authority moved.

## 8. Console / UI

None in 4a. `ApiKeyView.tsx:229-231` already toasts `result.note`, so the new note shows with no console change; the two-step flow is 4b.

## 9. Must not touch

`company_key::resolve` and every fallback (item 10); `inference/managed/enabled`; `TINYHUMANS_TOKEN_FILE`, `OPENCOMPANY_INFERENCE_KEY` and `hosted_endpoint_from_env` (item 18 is 6a's job, after this slice, not this one's); `composio/mode`, `composio/byok/key`; `inference/config` (never written again by this path); route paths and authority (`/credential` stays `Admin, Credential`); `start_link`, `get_billing`; `DEFAULT_TIER_MODELS` (never used to pick a model); `inference/routes`.

## 10. Done when

- Every §7 test exists and CI is green on the head SHA (zero failures, zero pending).
- `git grep -n "save_runtime_config" src/server/ops/company_key.rs` prints nothing.
- `git grep -n "composio/mode\|byok/key" src/company/company_key` prints nothing.
- No new secret-store key exists: `git grep -n 'const .*_KEY: &str' src/company/company_key` shows only `KEY_KEY`.
- The matrix in §6 is covered one test per row.

## 11. Gotchas

- **The row changes legacy resolution.** `resolve::primary` takes the first enabled provider for a company with no default, and 2a makes a `tinyhumans` row resolve directly like any added provider. That is why a row is only added with a model, and why M1 adds none.
- `put_provider` refuses the entry-zero slug with a `Store` error, not a skip — check `legacy_managed` first (M8).
- Do not call `store::check_slug`: `tinyhumans` is reserved (`is_reserved_slug`), and that check is for custom names.
- `load_managed_key` returns an untrimmed value; compare trimmed, restore raw.
- `delete_provider` clears `provider/<slug>/key`; 4a never deletes a row (the probe runs first).
- `record_health` latches per state; a repeated `ok` returns `false` and is not an error.
- A `ProviderOnly` default (bare slug, Q1) is **set** for this rule: never overwrite it (M4).
- Hold the std mutex only to clone the `Arc`, never across an await (`search/store.rs:92-96`).
- A key pasted by hand has no `connections` scope; a grant-minted key does. That is a note, not a skip.
- The probe runs inside the lock; a slow TinyHumans answer serialises saves for that company only.
- Names here are 2b's `store::load_default`, `store::set_default_choice(company, secrets, &ModelChoice)`, `DefaultChoice::{Unset, ProviderOnly, Full}`, `Provider::model() -> ModelOnRow`, and 2c's `store::check_model_id(&str) -> Result<String>` and `tier_overrides(&str)`. If those slices are edited later, use their names; do not add aliases.
- There is no Managed-specific path: the legacy Managed row (entry zero) and the `tinyhumans` row never both show (2a), which is why M8 adds no row.
