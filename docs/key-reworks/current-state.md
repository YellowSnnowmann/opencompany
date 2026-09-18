# Current state: every key the rework touches (part 1 of 2)

This is what the code does **today**, before the keys rework. It covers every
per-company secret-store key the rework reads, writes or retires. Nothing here
is a plan; the plan is in the slice files, and the end state is in
[target-state.md](target-state.md).

- **Code read:** `upstream/main @ fcfb3e1bc` (2026-09-14), in worktree
  `oc-t9-wt-3`. Every `file:line` below was opened on that commit on
  2026-09-14, unless it is marked *(inferred)*. Paths are relative to the repo
  root. `src/company/inference/store.rs` is written `inference/store.rs`, and
  `src/server/ops/inference/providers.rs` is written `providers.rs`.
- **Part 2** ([current-state-part2.md](current-state-part2.md)) covers
  Composio, Search, the two managed fallback chains step by step, how a model
  is chosen, the agent record, local runtimes, CLI logins and environment
  variables. PR #2305 was closed without merging on 2026-09-14, so nothing
  from it is part of this state.
- **Route prefix.** Every route below is mounted per company through
  `scoped(…)`. Host tests call both `/api/v1/companies/acme/…` and
  `/api/v1/company/…`. `…/credential` means either form.
- **Kinds.**
  - **Credential**: write-only; no route returns it.
  - **Setting**: not secret; the status routes return it.
  - **Status**: something learnt, not something configured.
  - **Legacy**: still read, but no longer the main address.

---

## 0. How the store behaves (applies to every key)

1. **Get and set only.** The `SecretStore` port has no list, no rename, no
   delete and no transaction. A key is read by its exact name. The design note
   sits at `inference/store.rs:33`.
2. **A clear is a write of `""`.** Every reader trims the value and treats
   blank as unset. Example: `load_default_slug` at `inference/store.rs:826-835`,
   and the search store's `read` at `src/company/search/store.rs:163-169`.
3. **Lists are JSON index blobs**: `inference/providers` and `search/providers`.
   Each credential lives in its own key beside the index, because nothing can
   enumerate `provider/*`.
4. **Entry zero.** The config from before the lists existed stays in its flat
   keys and is synthesised as the first row. It is never migrated
   (`inference/store.rs:420-460`, `search/store.rs:187-193`).
5. **Convergence on write.** Writing the new address issues a `""` write to
   the legacy address in the same call (`inference/store.rs:697-721`,
   `search/store.rs:569-594`). Nothing is copied at boot and nothing is written
   on a read path.

## 1. Every key at a glance

| Key | Constant (`file:line`) | Kind | Details |
|---|---|---|---|
| `tinyhumans/key` | `company_key::KEY_KEY`, `src/company/company_key.rs:64` | credential | §2 |
| `inference/providers` | `store::PROVIDER_INDEX_KEY`, `inference/store.rs:70` | setting | §3.1 |
| `provider/<slug>/key` | `store::provider_key_key`, `inference/store.rs:77-79` | credential | §3.2 |
| `inference/default` | `store::DEFAULT_PROVIDER_KEY`, `inference/store.rs:814` | setting | §3.3 |
| `inference/routes` | `store::ROUTES_KEY`, `inference/store.rs:871` | setting | §3.4 |
| `inference/health` | `store::HEALTH_KEY`, `inference/store.rs:925` | status | §3.5 |
| `inference/managed/enabled` | `store::MANAGED_ENABLED_KEY`, `inference/store.rs:779` | setting | §3.6 |
| `inference/config` | `inference::RUNTIME_CONFIG_KEY`, `src/company/inference.rs:49` | setting, legacy | §3.7 |
| `inference/key` | `inference::KEY_KEY`, `src/company/inference.rs:53` | credential, legacy | §3.8 |
| `harness/<id>/inference/config`, `…/key` | `runtime_config_key` / `harness_key_key`, `inference.rs:66-80` | setting / credential | §3.9 |
| `composio/token` | `composio::TOKEN_KEY`, `src/company/composio.rs:26` | credential | part 2 §1 |
| `composio/api_key` | `composio::API_KEY_KEY`, `composio.rs:181` | credential | part 2 §1 |
| `composio/mode` | `composio::MODE_KEY`, `composio.rs:172` | setting | part 2 §1 |
| `composio/defaults` | `composio::DEFAULTS_KEY`, `composio.rs:427` | setting | part 2 §1 |
| `search/providers` | `search::store::PROVIDER_INDEX_KEY`, `search/store.rs:111` | setting | part 2 §2 |
| `search/provider/<slug>/key` | `provider_key_key`, `search/store.rs:121-123` | credential | part 2 §2 |
| `search/provider/<slug>/endpoint` | `provider_endpoint_key`, `search/store.rs:126-128` | setting | part 2 §2 |
| `search/default` | `search::store::DEFAULT_PROVIDER_KEY`, `search/store.rs:118` | setting | part 2 §2 |
| `search/provider` | `search::PROVIDER_SECRET`, `src/company/search/mod.rs:50` | setting, legacy | part 2 §2 |
| `search/api_key` | `search::API_KEY_SECRET`, `search/mod.rs:54` | credential, legacy | part 2 §2 |
| `search/endpoint` | `search::ENDPOINT_SECRET`, `search/mod.rs:59` | setting, legacy | part 2 §2 |

---

## 2. Account: `tinyhumans/key`

```rust
// src/company/company_key.rs:64
pub const KEY_KEY: &str = "tinyhumans/key";
```

- **Value:** the raw key string, trimmed on write (`store_key`,
  `company_key.rs:69-73`). Example: `th-not-a-real-key`. A clear writes `""`.
- **Kind:** credential. It is also a fallback rung in both managed chains
  (part 2 §3).

**Writers** (router at `src/server/ops/company_key.rs:110-115`):

| Route | Handler | What it writes |
|---|---|---|
| `PUT …/credential` `{"key":"…"}` | `set_key`, `ops/company_key.rs:243-281` | `store_key` at `:249`. Nothing else: no copy into LLM or Composio. Journals `company_key_set` or `company_key_cleared`. |
| `POST …/credential/link/finish` `{"state","code"}` | `finish_link`, `ops/company_key.rs:446-512` | `store_key`, then `save_runtime_config` at `:486` writing `inference/config = {"provider":"managed"}` (§3.7). Together they span `:477-496`. |
| `POST …/credential/link/start` | `start_link`, `:308` | nothing in the store |

The second write in `finish_link`, quoted:

```rust
// src/server/ops/company_key.rs:486-496
crate::company::inference::save_runtime_config(
    runtime.id(),
    runtime.secrets().as_ref(),
    &crate::company::inference::RuntimeInference {
        provider: "managed".to_string(),
        base_url: None,
        models: Default::default(),
    },
)
```

**Readers:**

- `company_key::load` (`company_key.rs:91-96`). It never falls through to
  another credential. Its callers:
  - `key_configured` (`:77`);
  - `resolve` (`:123-133`);
  - `managed_identity` (`inference.rs:1127`);
  - `managed_state` (`ops/inference.rs:964`);
  - `get_billing` (`ops/company_key.rs:562`).
- `company_key::resolve` (`company_key.rs:123-133`) returns the company key,
  else the instance token source, else `Credential::None`. Its callers:
  - `composio::resolve_credential` (`composio.rs:113`);
  - `effective_status` (`ops/company_key.rs:197-230`), which reports the
    winning tier as `source`.

**Console:**

- `setCompanyCredential` (`frontend/src/api/credential.ts:106`) is called from
  `ApiKeyView.tsx:226`, and from `CompanyCredentialCard.tsx:112`, a component
  that nothing imports.
- `finishCredentialLink` (`credential.ts:147`) is called from
  `use-redeem-key-grant.ts:38`.
- `startCredentialLink` (`credential.ts:132`) has no reachable button. See
  rundown baseline 6 (`dump-rundown-2026-09-14.md`).

---

## 3. LLM keys

### 3.1 `inference/providers` (setting)

The index of every provider **except entry zero**. It holds no credentials.

```rust
// inference/store.rs:223-236
struct StoredProvider {
    id: ProviderId,
    slug: String,
    label: String,
    kind: String,
    base_url: String,
    #[serde(default)]
    models: BTreeMap<String, String>,
    #[serde(default = "enabled_default")]
    enabled: bool,
}
```

Example value, as the add route writes it: one model under all four tier
keys.

```json
[{"id":"prv_…","slug":"openrouter","label":"OpenRouter","kind":"openrouter",
  "base_url":"https://openrouter.ai/api/v1",
  "models":{"agentic-v1":"acme/test-model","chat-v1":"acme/test-model",
            "reasoning-v1":"acme/test-model","vision-v1":"acme/test-model"},
  "enabled":true}]
```

**Store functions:**

- `put_provider` (`inference/store.rs:531-589`) refuses a slug equal to entry
  zero's (`:542-549`).
- `set_enabled` (`:590`).
- `delete_provider` (`:618-671`) clears `provider/<slug>/key` first
  (`:634-644`), then saves the index.

**Writer routes** (router at `providers.rs:70-119`):

| Route | Handler | Writes |
|---|---|---|
| `POST …/inference/providers` `{kind,label?,baseUrl?,key?,model?,addAnyway?}` (`AddProvider`, `providers.rs:132-161`) | `add_provider`, `:309` | The key first (`secrets.set(provider_key_key)` at `:354-360`). Then `put_provider` at `:371`, with `models: tier_overrides(asked_model)`. Health goes through the `record_health` helper (`:878`). Auto-route goes through `auto_route_sole_provider` (`:562`, which calls `save_routes` at `:629`). A catalog with no tier or shipped ids and no model is rolled back (`:442-451`, `needs_an_explicit_model` at `:668-670`). |
| `PUT …/inference/providers/{slug}` `{label?,baseUrl?,models?,key?}` (`EditProvider`, `:178-190`) | `edit_provider`, `:907` | Refuses entry zero (`:917`). `store_provider_key` at `:1012`. `put_provider` at `:1017`, with `models: body.models.unwrap_or(existing.models)`. `forget_health` at `:1057`. |
| `DELETE …/inference/providers/{slug}` | `delete_provider`, `:1090` | Refuses entry zero (`:1099-1105`). Route scrub (`:1109`). `store::delete_provider` at `:1134`. `save_routes` at `:1138`. `forget_health` at `:1142`. `clear_default_if_marked` at `:1154`. |
| `POST …/inference/providers/{slug}/enabled` `{enabled}` | `set_enabled`, `:1198` | `store::set_enabled` at `:1214`. When disabling, `clear_default_if_marked` at `:1234`. |

**Readers:**

- `store::list_providers` (`inference/store.rs:467-488`) returns entry zero
  first, then the index rows. Its callers:
  - step 0 of `resolve_effective_scoped` (`inference.rs:1213-1318`);
  - `resolve_effective_for_tier` (`:1631`);
  - `provider_list` (`ops/inference.rs:493`);
  - `is_primary` (`providers.rs:1428`);
  - `legacy_slot_is_managed` (`inference/store.rs:502-512`).
- `store::get_provider` (`:514-525`) is called by `require_provider`
  (`providers.rs:1440-1452`). That function answers 404 for a slug with no
  row, which today includes `tinyhumans` unless entry zero is managed.

**Console:** `addProvider` (`frontend/src/api/inference.ts:489`),
`editProvider` (`:498`), `deleteProvider` (`:516`) and `setProviderEnabled`
(`:534`).

### 3.2 `provider/<slug>/key` (credential)

```rust
// inference/store.rs:77-79
pub fn provider_key_key(slug: &str) -> String {
    format!("provider/{slug}/key")
}
```

- **Value:** a raw string, trimmed on read (`load_provider_key`,
  `inference/store.rs:729-752`).
- **Examples:** `provider/openrouter/key = sk-not-a-real-key`, and
  `provider/tinyhumans/key = th-not-a-real-key`.
- **Addresses:** `Provider::key_key` (`:200-202`) is always this address.
  `legacy_key_key` (`:210-215`) answers `inference/key` for entry zero only.

**Writers:**

- `add_provider` writes it directly (`providers.rs:354-360`).
- `store_provider_key` (`inference/store.rs:697-721`) writes it and clears
  entry zero's legacy slot. It is called from `edit_provider`: `:1012` for the
  save and `:1036` to restore on rollback.
- `PUT …/inference/managed/key` `{"key":"…"}`, handled by `set_managed_key`
  (`providers.rs:1697-1777`):
  - writes `provider/tinyhumans/key` at `:1715-1722`;
  - clears `inference/key` at `:1735-1741`, but **only** when
    `legacy_slot_is_managed` (`:1711`);
  - the console calls it as `setManagedKey` (`api/inference.ts:643`) through
    `saveManagedKey` (`use-inference.ts:234`).
- Cleared by `store::delete_provider` (`inference/store.rs:634-644`) and by
  `clear_orphaned_key` (`providers.rs:855`).

**Readers:**

- `load_provider_key` is called by:
  - `decl_for_indexed` (`inference.rs:1418`);
  - `provider_key_configured` (`inference/store.rs:760-770`);
  - the catalog and test routes: `list_provider_models` (`providers.rs:1629`)
    and `test_provider` (`:1789`).
- `load_managed_key` (`inference.rs:936-963`) reads `provider/tinyhumans/key`
  at `:941-946`. That is managed chain step 1.
- `load_inference_key_for` (`inference.rs:1007-1037`) reads
  `provider/<credential_slug>/key` for the legacy runtime and manifest arms.

### 3.3 `inference/default` (setting)

- **Value:** a bare slug. Example: `openrouter`.
- **What it decides:** only which provider is the **primary**. It never
  decides the model.
- **Unset:** a missing key or `""`.

```rust
// inference/store.rs:826-835
pub async fn load_default_slug(company: &CompanyId, secrets: &dyn SecretStore)
    -> Result<Option<String>> {
    let Some(SecretValue(raw)) = secrets.get(company, DEFAULT_PROVIDER_KEY).await? else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
}
```

**Writers:**

- `POST …/inference/providers/{slug}/default` takes **no body**. It is handled
  by `set_default` (`providers.rs:1294-1317`), which:
  - refuses a disabled provider (`:1301-1306`);
  - calls `set_default_slug` at `:1307` (store function at
    `inference/store.rs:841-853`).
- `clear_default_if_marked` (`providers.rs:1324-1345`) reads the value at
  `:1326` and calls `clear_default_slug` at `:1328` (store function at
  `inference/store.rs:857-861`). It runs on delete (`:1154`) and on disable
  (`:1234`).
- **Console:** `setDefaultProvider` (`api/inference.ts:661`), called through
  `makeDefault` (`use-inference.ts:233`). It sends `{}`.

**Readers:**

- `decl_for_primary` (`inference.rs:1393-1405`, reading at `:1398`) passes the
  value to `resolve::primary` (`src/company/inference/resolve.rs:339-346`).
  The marked provider wins when it exists and is enabled; otherwise the first
  enabled provider wins.
- `provider_list` (`ops/inference.rs:505`) uses it for `isDefault`.
- `managed_parked_tiers` (`providers.rs:1397`) and `is_primary` (`:1433`).

### 3.4 `inference/routes` (setting)

- **Value:** a JSON map from tier to a route string. Unset rows are dropped
  when saving (`save_routes`, `inference/store.rs:907-920`).
- **Grammar:** parsed by `ProviderRef::parse` (`resolve.rs:182-205`):
  - `managed`;
  - `<slug>` or `<slug>:<model>`;
  - `local` or `local:<model>`;
  - `claude-code` or `claude-code:<model>`.

```json
{"agentic-v1":"managed","chat-v1":"openrouter:acme/test-model","reasoning-v1":"openrouter"}
```

**Writers:**

- `PUT …/inference/routes`, handled by `put_routes` (`providers.rs:1980-2048`,
  save at `:2004`). Console: `putRoutes` (`api/inference.ts:714`), called from
  `use-inference.ts:263`.
- The auto-route on first add (`:629`).
- The scrub on delete (`:1138`).

**Readers:** `load_routes` (`inference/store.rs:885-903`), called from:

- step 4 of `resolve_effective_scoped` (`inference.rs:1295`);
- `resolve_effective_for_tier` (`:1633`);
- `routing_table` (`ops/inference.rs:545-555`), which fills the status
  `routes` field (`:877`);
- `get_routes` (`providers.rs:1957`);
- disable parking (`:1243`), `parked_tiers` (`:1365`) and
  `managed_parked_tiers` (`:1391`);
- `put_routes` (`:2020`).

### 3.5 `inference/health` (status)

- **Value:** a JSON map of slug to `ProviderHealth {state, at}`
  (`inference/store.rs:935-940`). Example:
  `{"tinyhumans":{"state":"ok","at":"2026-09-14T10:00:00Z"}}`.
- **Parsing:** a blob that will not parse reads as empty (`load_health`,
  `:951-963`).
- **Latching:** a write happens only when the state changes (`record_health`,
  `:976-1000`).
- **Writers:**
  - the `record_health` helper (`providers.rs:878-905`), used by add, test
    provider and test managed;
  - `forget_health` (`inference/store.rs:1002`), called on edit
    (`providers.rs:1057`) and on delete (`:1142`).
- **Readers:** `provider_list` (`ops/inference.rs:500`) and `managed_state`
  (`:969`).

### 3.6 `inference/managed/enabled` (setting)

- **Value:** `"true"` or `"false"`.
- **Unset:** absent reads as on, and anything except a literal `false` is on
  (`managed_enabled`, `inference/store.rs:782-791`).
- **Writer:** `POST …/inference/managed/enabled` `{enabled}`, handled by
  `set_managed_enabled` (`providers.rs:1461-1500`, store write at `:1467`).
  Console: `setManagedEnabled` (`api/inference.ts:567`) through `setManagedOn`
  (`use-inference.ts:235`).
- **Readers:**
  - the boot step 4 gate (`inference.rs:1296`);
  - `refuse_a_managed_fallback_that_is_switched_off` (`:1340-1357`, read at
    `:1348`);
  - the managed route arm (`:1673`);
  - `managed_state` (`ops/inference.rs:985`).

### 3.7 `inference/config`: entry zero (setting, legacy)

```rust
// src/company/inference.rs:471-480
pub struct RuntimeInference {
    pub provider: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub models: BTreeMap<String, String>,
}
```

- **Example:** `{"provider":"managed","base_url":null,"models":{}}`.
- **Scoped form:** for a named harness the key is
  `harness/<id>/inference/config` (`runtime_config_key`, `inference.rs:66-71`).

**Writers:**

- `PUT …/inference` `{provider, baseUrl?, models?, key?}` (`SetInference`,
  `ops/inference.rs:568-580`), handled by `set_config` (`:1010`). It calls
  `save_runtime_config` at `:1032`.
  - The console client `setInference` exists (`api/inference.ts:342`), but no
    component calls it on `fcfb3e1bc`.
  - The `setInference(…)` calls in `frontend/src/setup/SetupDialog.tsx:305,315`
    are a React state setter, not this client.
- `finish_link` (`ops/company_key.rs:486`).
- Cleared by `DELETE …/inference`, handled by `revert_config`
  (`ops/inference.rs:1107`, `clear_runtime_config` at `:1113`).

**Readers:** `load_runtime_config_scoped` (`inference.rs:828-847`, get at
`:833`), called from:

- `entry_zero` (`inference/store.rs:420-425`);
- step 1 of `resolve_legacy_scoped` (`inference.rs:1461`);
- `harness_configures_itself` (`:1378`).

**The entry-zero row** is built by `provider_from_runtime`
(`inference/store.rs:429-460`):

- Its slug is `credential_slug(provider)` (`:438`), so a `managed` or
  `tinyhumans` config becomes a row with slug **`tinyhumans`** and label
  `Managed`.
- `put_provider` then refuses an index row with that slug (`:542-549`).
- The delete route refuses entry zero (`providers.rs:1099-1105`).

### 3.8 `inference/key` (credential, legacy)

- **Value:** a raw string. It is entry zero's credential, and managed chain
  step 2.

**Writers:**

- `store_key` (`inference.rs:1140-1143`), called by `set_config`
  (`ops/inference.rs:1039`).
- `clear_key` (`inference.rs:1158-1163`), called by `revert_config`
  (`ops/inference.rs:1122`).
- Cleared by `store_provider_key` for entry zero
  (`inference/store.rs:706-720`).
- Cleared by `set_managed_key` when the slot is managed's
  (`providers.rs:1735-1741`).

**Readers:**

- `load_key_scoped` (`inference.rs:904`, get at `:910`).
- The legacy fallback in `load_provider_key` (`inference/store.rs:745-750`).
- `load_managed_key` step 2 (`inference.rs:956-962`). It runs only for the
  default harness, and only when `legacy_slot_is_managed` (`:959`).

### 3.9 `harness/<id>/inference/config` and `harness/<id>/inference/key`

- **Addresses:** `HarnessScope` (`inference.rs:88-138`) builds them with
  `config_key` (`:130`) and `key_key` (`:135`). The default harness keeps the
  flat keys (`:66-80`). The test at `:2556-2557` asserts
  `harness/deep/inference/key` and `harness/deep/inference/config`.
- **Readers:**
  - the same loaders, called with a named scope;
  - `harness_configures_itself` (`:1367-1381`);
  - `load_inference_key_for` (`:1007-1037`), where a named harness reads its
    own slot first.
- **Writers:** no HTTP route writes a named scope; `set_config` uses the
  default-scope `save_runtime_config`. *(Inferred: only tests write these.)*
  The rundown's research agent found no shipped manifest declaring
  `[harness.inference]` (not re-verified).

---

## Tests that pin today's behaviour (verified names)

- **`src/company/company_key.rs`:**
  - `the_company_key_outranks_the_instance_identity` (`:183`)
  - `no_key_and_no_instance_identity_fails_closed` (`:209`)
  - `clearing_the_key_falls_back_rather_than_stranding_the_company` (`:218`)
  - `a_blank_key_is_not_configuration` (`:240`)
  - `an_unreadable_store_never_borrows_another_identity` (`:263`)
  - `rotating_the_key_moves_the_fingerprint` (`:295`)
  - `the_key_is_write_only_in_every_rendering` (`:325`)
- **`src/company/inference.rs`:** the managed-chain block begins with the
  comment at `:3035-3045`, which lists steps 1 to 5.
- **`inference/store.rs`:** the `FailsWriting` store double at `:1054-1070`,
  used at `:1321`.

Continued in [current-state-part2.md](current-state-part2.md).
