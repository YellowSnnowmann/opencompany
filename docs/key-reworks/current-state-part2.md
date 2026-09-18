# Current state: every key the rework touches (part 2 of 2)

This continues [current-state.md](current-state.md), and the same rules apply:

- the code was read at `upstream/main @ fcfb3e1bc` on 2026-09-14;
- a `file:line` with no marker was opened, and anything not opened is marked
  *(inferred)*.

---

## 1. Composio keys

Router: `src/server/ops/composio.rs:279-293`.

### 1.1 `composio/token` (credential)

```rust
// src/company/composio.rs:26
pub const TOKEN_KEY: &str = "composio/token";
```

- **What it actually is:** despite its name, a **TinyHumans bearer**. It is
  sent to the managed backend (`DEFAULT_BACKEND_URL = "https://api.tinyhumans.ai"`,
  `composio.rs:43`) and outranks `tinyhumans/key` for Composio only.
- **Value:** a raw string, trimmed on write. Example: `th-not-a-real-key`.

**Writer:** `store_token` (`composio.rs:66-74`), called by
`PUT …/composio/token` `{"token":"…"}`.

- Handler: `set_token` (`ops/composio.rs:653-684`).
- Journal entries: `credential_set` and `credential_cleared`.
- Console: `setComposioToken` (`frontend/src/api/composio.ts:358`), called from
  `ComposioSection.tsx:368` (clear) and `:390` (save).

**Readers:**

- `resolve_credential` (`composio.rs:98-115`) reads the key at `:103`. When the
  key is not a value it falls to `company_key::resolve` at `:113`. It is called
  from:
  - `resolve_access`, Managed arm (`composio.rs:398-400`);
  - `effective_status` (`ops/composio.rs:612`);
  - the harness catalog read (`src/harness/built_in/composio.rs:363`).
- `token_configured` (`composio.rs:140-146`) is called from
  `src/server/ops/capabilities.rs:474` (`composioTokenConfigured`).
- The constant is re-exported to the harness at
  `harness/built_in/composio.rs:104-105`, and read by the tests there
  (`:2933`, `:3035`, `:3048`).

### 1.2 `composio/api_key` (credential)

```rust
// composio.rs:181
pub const API_KEY_KEY: &str = "composio/api_key";
```

- **What it is:** the company's own Composio `ak_…` key. It is sent as
  `x-api-key` to `DIRECT_BASE_URL = "https://backend.composio.dev"`
  (`composio.rs:192`). Example: `ak-not-a-real-key`.

**Writer:** `store_api_key` (`composio.rs:320-347`), which writes the key
**and** `composio/mode` in a direction-ordered pair:

- **Set:** key first, then `mode = byok`.
- **Clear:** `mode = managed` first, then the key as `""`.

It is called by `PUT …/composio/api-key`, handled by `set_api_key`
(`ops/composio.rs:719-770`):

- a probe runs unless the key is blank or `skip_verify` is set;
- journal entries: `composio_byok_set` and `composio_byok_cleared`;
- console: `setComposioApiKey` (`api/composio.ts:388`), called from
  `ComposioSection.tsx:319` and `:385`.

**Readers:**

- `resolve_access`, Byok arm (`composio.rs:401-404`). There is **no
  fallback**: with no key it warns and withholds tools (`:406-412`).
- `stored_api_key` (`ops/composio.rs:949-966`, read at `:961`), used by
  `test_api_key` (`:910`).

### 1.3 `composio/mode` (setting)

- **Values:** `"managed"` (`MANAGED_MODE`, `composio.rs:184`) or `"byok"`
  (`BYOK_MODE`, `:187`). Unset reads as managed (`load_mode`, `:285-291`).
- **Writer:** only `store_api_key` (above).
- **Readers:**
  - `resolve_access` (`:398`);
  - `stored_api_key` (`ops/composio.rs:952-958`);
  - `effective_status`, through `access_for` (`ops/composio.rs:566-570`);
  - the harness (`harness/built_in/composio.rs:330`).

### 1.4 `composio/defaults` (setting)

- **Value:** a JSON map of toolkit to connection id. Example:
  `{"gmail":"ca_123"}`.
- **Parsing:** a blob that will not parse reads as empty and is logged
  (`load_defaults`, `composio.rs:448`).
- **Store functions:** `set_default` (`:484`), `clear_default` (`:501`),
  `forget_connection` (`:517`) and `save_defaults` (`:533`).
- **Writers:** `PUT` and `DELETE …/composio/connections/{connection_id}/default`
  (`ops/composio.rs:291-292`), handled by `set_default` (`:1346`) and
  `clear_default` (`:1368`). A disconnect forgets the connection
  (`ops/composio.rs:1314`).
- **Readers:**
  - `harness/built_in/composio.rs:354`;
  - `connections_impl` (`ops/composio.rs:1149`);
  - `drop_dangling_defaults` (`:1196`).

The keys rework does not touch this key.

---

## 2. Search keys

- **Router:** `src/server/ops/search.rs:147-156`.
- **Index lock:** `index_guard` (`search/store.rs:97`).
- **The one derivation of the active provider:** `candidates` and
  `resolve_effective_provider` (`src/company/search/mod.rs:138-175`). Three
  callers use it: `status_of` (`ops/search.rs:263`), the capabilities panel,
  and `search_byo.rs:137-160`.

### 2.1 `search/providers` (setting)

```rust
// src/company/search/store.rs:151-155
struct IndexEntry {
    slug: String,
    #[serde(default = "yes")]
    enabled: bool,
}
```

- **Example:** `[{"slug":"brave","enabled":true},{"slug":"searxng","enabled":false}]`.
- **Parsing:** an index that will not parse is an **error**, not an empty
  list (`list_providers`, `search/store.rs:203-211`).

**Writers:** everything goes through `save_index` (`:264-282`, write at `:281`),
called from:

- `put_provider` (`:290`) and `claim_provider` (`:318`);
- `put_provider_locked` (`:406-442`);
- `set_enabled` (`:444`);
- `delete_provider` (`:470`) and `delete_all_providers` (`:489`);
- `select_provider` (`:650`).

**Routes that end up writing it:**

| Route | Handler |
|---|---|
| `POST …/search/providers` | `connect_provider`, `ops/search.rs:493` |
| `PUT …/search/providers/{slug}` | `update_provider`, `:634` (`update_endpoint_if_present` at `:660`, `set_enabled` at `:672`) |
| `DELETE …/search/providers/{slug}` | `remove_provider`, `:679` |
| `PUT …/search` (legacy save) | `put_search`, `:884` |
| `DELETE …/search/key` | `delete_search`, `:1042` |

**Readers:** `list_providers` (`search/store.rs:199-262`), called from
`candidates` (`search/mod.rs:143`) and from the routes above.

### 2.2 `search/provider/<slug>/key` (credential)

- **Writers:**
  - `store_provider_key` (`search/store.rs:569-594`) writes the key at `:575`
    and clears `search/api_key` for entry zero at `:581`;
  - `store_key_if_connected` (`:381`), called from `connect_provider` and from
    `replace_key` (`ops/search.rs:709`, `PUT …/search/providers/{slug}/key`);
  - cleared by `delete_provider_locked` (`search/store.rs:527`).
- **Readers:**
  - `load_provider_key` (`:595-612`), which falls back to `search/api_key` at
    `:604`;
  - `provider_key_configured` (`:613`), used by `candidates`
    (`search/mod.rs:144`);
  - `search_byo.rs:160`;
  - `test_provider` (`ops/search.rs:811`).

### 2.3 `search/provider/<slug>/endpoint` (setting)

- **Value:** the instance URL of a self-hosted provider, for example
  `https://searx.example`. It is not a secret (`search/mod.rs:56-59`).
- **Writer:** written only when `Some`:

  ```rust
  // search/store.rs:413-421
  if let Some(endpoint) = provider.endpoint.as_deref() {
      write(company, secrets, &provider_endpoint_key(&provider.slug), endpoint).await?;
  }
  ```

  It is reached through `put_provider` and `update_endpoint_if_present`
  (`:346`). Cleared on delete (`:527`).
- **Readers:** `list_providers` reads the per-slug key at `:229`. For entry
  zero it falls back to the flat `search/endpoint` at `:251`. The value lands
  in `SearchProvider.endpoint` (`:146`).

### 2.4 `search/default` (setting)

- **Value:** a bare slug, for example `"brave"`. There is no model to store,
  and this key **stays a bare slug** after the rework.
- **Store functions:** load (`search/store.rs:622-627`), set (`:705-711`),
  clear (`:714-716`), and `set_default_if_connected` (`:687`).
- **Writers:**
  - `set_default`, `PUT …/search/default` (`ops/search.rs:755`), which sets at
    `:768` and clears at `:774`;
  - `put_search` (`:962`);
  - `delete_search` (`:1061`);
  - `delete_provider_locked` (`search/store.rs:515-567`).
- **Readers:**
  - `resolve_effective_provider` (`search/mod.rs:173`);
  - `status_of` (`ops/search.rs:280`);
  - `put_search` (`:909`);
  - `search_byo.rs:139`.

### 2.5 Legacy entry zero: `search/provider`, `search/api_key`, `search/endpoint`

- **`search/provider`** (`PROVIDER_SECRET`, `search/mod.rs:50`):
  - read by `entry_zero_slug` (`search/store.rs:187-193`);
  - cleared **last** by `delete_provider_locked` (`:546`), and by
    `delete_search` (`ops/search.rs:1056-1060`).
- **`search/api_key`** (`API_KEY_SECRET`, `search/mod.rs:54`):
  - read as the fallback at `search/store.rs:604`;
  - cleared at `:581`, `:546` and `ops/search.rs:1056`.
- **`search/endpoint`** (`ENDPOINT_SECRET`, `search/mod.rs:59`):
  - read at `search/store.rs:222` (the synthesised row) and `:251` (after the
    slug is indexed);
  - cleared at `:546` and `ops/search.rs:1056`.

---

## 3. The managed fallback chains

### 3.1 LLM managed chain: first match wins

The chain is used only by a **proxied** declaration, one of these four:

- the legacy runtime blob or the manifest naming `managed`;
- a keyless `openrouter`;
- the env default (`resolve_legacy_scoped` step 3, `inference.rs:1544-1589`);
- a `managed` route or boot step 4 (`managed_decl`, `inference.rs:1740-1759`).

An **indexed row is never proxied**: `decl_for_indexed` (`inference.rs:1413-1443`)
states `proxied = false`.

| Step | Source | Where |
|---|---|---|
| 1 | `provider/tinyhumans/key` | `load_managed_key`, `inference.rs:941-946` |
| 2 | `inference/key`, only for the default harness and when `legacy_slot_is_managed` | `inference.rs:956-962` (`inference/store.rs:502-512`) |
| (env-default arm only) | `provider/openrouter/key`, then `inference/key` | `load_inference_key_scoped(…, DEFAULT_PROVIDER)`, `inference.rs:1567-1571` *(line range inferred from the arm at `:1544-1589`)* |
| 3 | `tinyhumans/key` | `managed_identity`, `inference.rs:1116-1137`. It returns early unless `proxied && !had_key` (`:1123-1125`) and reads the key at `:1127`. |
| 4 | Instance identity | `resolve_endpoint` (`inference.rs:644-714`) puts `EnvDefault.credential` at `:666` (managed) and `:684` (keyless openrouter). `EnvDefault` is built by `builder.rs:3050-3060` and `platform_default` (`ops/inference.rs:792-807`) from `hosted_endpoint_from_env` (`provider.rs:182-195`): `OPENCOMPANY_INFERENCE_KEY`, else `TinyhumansTokenSource::from_env` (`credentials.rs:207-246`), meaning `TINYHUMANS_TOKEN_FILE` **if that path exists**, else `TINYHUMANS_API_KEY`. |
| 5 | `Credential::None` | Boot treats `None` as not configured and uses the echo brain (`builder.rs:3087-3104`). A turn errors. |

- **What the console reports:** `ManagedSource` (`inference.rs:1045-1097`):
  `provider_key`, `company_account`, `instance` or `none`. `managed_state`
  (`ops/inference.rs:951-990`) builds it.
- **Correction to the key map page:** its order "`TINYHUMANS_TOKEN_FILE`, else
  `OPENCOMPANY_INFERENCE_KEY`" is wrong. `OPENCOMPANY_INFERENCE_KEY` is checked
  first (`provider.rs:184`).

### 3.2 Composio managed chain, when `composio/mode` is managed

| Step | Source | Where |
|---|---|---|
| 1 | `composio/token` | `resolve_credential`, `composio.rs:103` |
| 2 | `tinyhumans/key` | `company_key::resolve`, `company_key.rs:128-129` |
| 3 | Instance identity: `TINYHUMANS_TOKEN_FILE`, else `TINYHUMANS_API_KEY`. `OPENCOMPANY_INFERENCE_KEY` is **not** consulted. | `TinyhumansTokenSource::from_env`, passed in at `builder.rs:3258-3270` and `harness/built_in/mod.rs:3336-3350` |
| 4 | `Credential::None`: no tools | `company_key.rs:131` |

In BYOK mode only `composio/api_key` is read (`composio.rs:401-404`).

---

## 4. How a model is chosen today

1. **The agent's model string** is set in the roster build:

   ```rust
   // src/harness/built_in/build.rs:1159-1162
   let model = deps
       .model_override
       .clone()
       .unwrap_or_else(|| model_for_tier(manifest_agent.tier.as_deref()));
   ```

   `model_override` is `OPENCOMPANY_INFERENCE_MODEL`. It is read at
   `provider.rs:155` and carried by `builder.rs:3062-3065`.
2. **Tier mapping** (`build.rs:188-204`):

   | Agent tier | Tier sent |
   |---|---|
   | `orchestrator` | `agentic-v1` |
   | `frontend` | `agentic-v1` |
   | `agentic` | `agentic-v1` |
   | `reasoning` | `reasoning-v1` |
   | `vision` | `vision-v1` |
   | anything else | `chat-v1` |

   The tier list is `INFERENCE_TIERS` (`src/company/types.rs:80`).
3. **Per turn,** `TenantProvider::resolve(tier)` (`provider.rs:2096-2133`)
   calls `resolve_effective_for_tier` (`inference.rs:1618-1732`), which acts on
   the route for that tier:
   - **unset row:** the primary, else the legacy chain;
   - **`managed`:** `managed_decl`, refused while the switch is off;
   - **`<slug>:<model>`:** `decl.models.insert(tier, model)`, so the route
     beats the row's map.

   It then attaches a discovered `TierVocabulary` via `turn_vocabulary`
   (`:2124-2131`). With no declaration the error is
   `"no inference provider is configured for this company"` (`:2107`).
4. **The wire model** is chosen in `request_plan` (`provider.rs:1652-1668`),
   which calls `model_for_tier(abstract_model, &decl.models, decl.vocabulary())`
   (`inference.rs:360-378`). That function tries, in order:
   - the row's override map;
   - a `Concrete` catalog substitution from `DEFAULT_TIER_MODELS`, quoted here
     as today's code (`inference.rs:175-180`: `chat-v1 → anthropic/claude-sonnet-5`,
     `reasoning-v1 → openai/gpt-5.6-sol-pro`,
     `agentic-v1 → anthropic/claude-opus-5`, `vision-v1 → qwen/qwen3.8-max`).
     Slice 2d removes it because it guesses ids;
   - otherwise the bare tier name.
5. **The add route** writes the one chosen model to all four tiers
   (`tier_overrides`, `providers.rs:680-688`).
6. **The setup brain** (`src/harness/roster_build.rs:188-212`) is not a company
   turn:
   - URL: `OPENCOMPANY_INFERENCE_URL`, else `DEFAULT_TINYHUMANS_INFERENCE_URL`
     (`provider.rs:55`, which is `…/openai/v1`);
   - model: the wizard's model, else `_MODEL`, else
     `DEFAULT_HOSTED_MODEL = "chat-v1"` (`provider.rs:58`).
7. **Internal passes** (title, triage, planning and others) pass no tier, so
   they fall back to `chat-v1`. The provider-model plan (§3.6) lists their
   `file:line`s; they were not re-opened here.

---

## 5. The agent record today: no provider field anywhere

| Record | Fields | Where |
|---|---|---|
| `Agent` (`src/company/types.rs:634`) | `tier` (`:661`, "hint; never selects a model"), `harness` (`:670`), `model` (`:684`, ACP harness only) | `company.toml` `[[agent]]` |
| `AgentFile` (`src/company/agent_file.rs`) | `harness` (`:88`), `model` (`:102`), mapped at `:278` and `:293` | `agents/<id>.toml` |
| `OverlayAgent` (`src/ports/types.rs:3410-3453`) | `model` (`:3445`), `harness` (`:3452`), each `#[serde(default, skip_serializing_if = "Option::is_none")]` | `CompanyRecord.overlay_agents` |
| `AgentOverride` (`ports/types.rs:3493-3562`) | `model` (`:3556`), `harness` (`:3561`). `Some("")` is the stored "cleared" form. | `CompanyRecord.overlay_agent_edits` |

- **Merging:**
  - `upsert_agent_override` copies the fields (`ports/types.rs:5577-5582`);
  - the effective merge filters out empty strings (`:5681-5686`);
  - the "has an override" check is at `:5872`.
- **Validation:**
  - the manifest refuses `model` on a harness that is not `acp`
    (`src/company/manifest.rs:1043-1076`, message at `:1066-1073`);
  - `PATCH {scope}/team/{agent_id}` (`src/server/ops/team_agent.rs:104`) takes
    `EditAgent.model` (`:564`) and `harness` (`:575`) as double options, and its
    cross-check at `:833-880` refuses a model that is not on `acp` (`:852-856`).

---

## 6. Local runtimes

`LOCAL_RUNTIMES` (`src/company/inference/catalogue.rs:374-417`) has three
entries:

| Slug | Default endpoint | `needs_key` | Auth |
|---|---|---|---|
| `ollama` | `http://localhost:11434` | false | none |
| `lmstudio` | none | false | none |
| `omlx` | none | false | Bearer only when a key exists |

- **Storage:** a local runtime is an ordinary `inference/providers` row with
  its own `base_url`. Its `provider/<slug>/key` is usually empty.
- **Auth:** `auth_style_for` (`catalogue.rs:799`).
- **Routes:** `local` and `local:<model>` pick the first enabled local row
  (`resolve.rs:199`).
- **"Set" today:** the row exists. A blank key says nothing either way.

## 7. CLI logins

`CLI_LOGINS` (`catalogue.rs:459-472`) has two entries:

- `claude-code`, stored under `claude-code`, with no probe;
- `codex`, stored under `openai`, with a probe.

Both are refused on this host:

```rust
// providers.rs:760-770 (plan_add)
if let Some(cli) = catalogue::cli_login(kind) {
    return Err(invalid(format!(
        "{} is a credential held by a command-line tool on someone's own machine. \
         This host cannot reach one.", cli.label)));
}
```

- **Console:** `CLI_LOGINS_REACHABLE = false` (`frontend/src/inference/connect.ts:220`),
  so `AddProviderDialog.tsx:108` passes an empty list.
- **Routes:** the route form `claude-code[:model]` (`resolve.rs:198`) is
  refused by `route_is_servable` (`providers.rs:2049-2087`).
- **Keys:** CLI logins store no key.
- **Not the same thing:** the ACP harness (an agent running on a local CLI) is
  a separate feature with no stored credential.

---

## 8. Environment variables

| Variable | Read at | Used for |
|---|---|---|
| `TINYHUMANS_TOKEN_FILE` | `credentials.rs:66` (`TOKEN_FILE_ENV`), used by `from_env` at `:207-212` | The projected hosted token. It wins over the static key only if the path exists (`:223-246`). Also feeds `credential_available_in` (`src/app/types.rs:256`). |
| `TINYHUMANS_API_KEY` | `credentials.rs:70`; `provider.rs:223` (media); `src/bin/opencompany.rs:2154` | Static instance identity. The bin read feeds `/spec` `cycles_available`. |
| `OPENCOMPANY_INFERENCE_KEY` | `provider.rs:184`; `src/bin/opencompany.rs:2153` | Instance inference credential, checked **before** the token source (LLM only) |
| `OPENCOMPANY_INFERENCE_URL` | `provider.rs:192`; `roster_build.rs:193` | Env default and setup-brain base URL. Default `…/openai/v1` (`provider.rs:55`). |
| `OPENCOMPANY_INFERENCE_MODEL` | `provider.rs:155`; `roster_build.rs:201` | `model_override`, which flattens every agent to one model string |
| `OPENCOMPANY_COMPOSIO_BACKEND_URL` | `composio.rs:32`; `builder.rs:3256`; `harness/built_in/mod.rs:3345` | Composio managed backend URL override |
| `TINYHUMANS_API_URL` | `composio.rs:38`; `src/app/config.rs:827-831`; `bin/opencompany.rs:2162`; `ops/composio.rs:211,591,1081` | Shared API base. Composio falls back to it (`backend_url_or_default`, `composio.rs:54`). |

`OPENHUMAN_WORKSPACE` is required for boot and is not part of this rework
(README decision Q16).

---

## 9. PR #2305

PR #2305 was closed without merging on 2026-09-14. Nothing from it is on
`fcfb3e1bc` or on this branch.

- **No `tinyhumans` catalogue row exists today.** Slice 2a adds it
  (`phase-2a-tinyhumans-on-proxy.md`).
- **Two facts on `fcfb3e1bc` that such a row will meet:**
  - an indexed row is resolved with `proxied = false`
    (`inference.rs:1413-1443`), so it never falls back to `tinyhumans/key` or
    the instance identity (`inference.rs:1123-1125`);
  - `store::delete_provider` clears `provider/<slug>/key`
    (`inference/store.rs:634-644`), the same slot `PUT …/inference/managed/key`
    writes for slug `tinyhumans`.

---

## Gotchas

- **The store has get and set only.** A clear writes `""`. Never plan a
  rename, delete or transaction.
- **`put_provider` refuses a slug equal to entry zero's.** A managed
  `inference/config` is entry zero with slug `tinyhumans`.
- **The TinyHumans catalogue is enveloped and paged**
  (`{success, data:{data,total,limit,offset}}`, `limit` up to 500, follow
  `total`). Any id it returns is valid; make no assumption about which ids it
  contains. The proxy answers 400 to tier names.
- **A hand-minted TinyHumans key lacks Composio's `connections` scope.** Only
  grant-minted or attested hosted keys carry it.
- **`OPENHUMAN_WORKSPACE` is required for boot.**
- **Frontend has three typecheck gates.** `scripts/ci/assert-design-tokens.sh`
  rejects raw hex. Do not run cargo locally; CI verifies by head SHA. Never put
  a real key on disk.

## Done when (for this reference)

Every `file:line` above still matches `upstream/main @ fcfb3e1bc`. Check a
constant with `git grep -n '<const>' fcfb3e1bc -- src`. A slice that starts
that edits a listed line re-verifies it first and updates this file in
the same commit.
