# Target state: every key after phase 5b (part 2 of 2)

This continues [target-state.md](target-state.md).

- The code reference is `upstream/main @ fcfb3e1bc` (2026-09-14).
- Slice numbers follow the 2026-09-14 renumbering: 2a is TinyHumans on the
  proxy, 2b the default's shape, 2c model required, 2d no tier on the wire.
  6a (added 2026-09-15) removes four environment variables; it is the last
  slice.
- Every example value is fake.

---

## 1. Keys that stop being read or change shape

In the examples below, a JSON object stands for the store: each property is a
secret-store key and its value. **No row here renames, deletes, or clears a
value at boot.**

### 1.1 `inference/default`: bare slug becomes JSON (2b, 2c)

Rule: **converge-on-write**. The value is rewritten only when an admin sets a
default, and that write always carries a model (Q2).

| State | Stored | `load_default()` | Resolution |
|---|---|---|---|
| Before, and unchanged until someone sets a default | `{"inference/default":"openrouter"}` | `ProviderOnly("openrouter")` | legacy sources (the `primary` marker, as today). The model follows the 2d rule: `OPENCOMPANY_INFERENCE_MODEL` or a configured real id, else "choose a model". |
| After `POST …/providers/tinyhumans/default {"model":"acme/test-model"}` | `{"inference/default":"{\"provider\":\"tinyhumans\",\"model\":\"acme/test-model\"}"}` | `Full` | step 3: sends `acme/test-model` to the proxy |
| Cleared by deleting or disabling that provider | `{"inference/default":""}` | `Unset` | legacy sources, with the 2d model rule |

**Status and console:**

- A bare slug shows the banner "Your default provider has no model. Choose
  one."
- The banner itself never blocks a turn; the 2d model rule decides.

**Rollback.** A binary from before 2b reads the JSON as a slug that matches no
row, so `resolve::primary` falls back to the first enabled provider
(*inferred*; state this in the PR body).

### 1.2 `inference/providers` row: one model under four keys stays the encoding (2b)

Rule: **nothing is rewritten.** `Provider::model()` collapses on read.

| Stored `models` | `Provider::model()` | Status |
|---|---|---|
| `{"agentic-v1":"local-test-model","chat-v1":"local-test-model","reasoning-v1":"local-test-model","vision-v1":"local-test-model"}` | `One("local-test-model")` | `model: "local-test-model"`, `modelAmbiguous: false` |
| `{}` | `None` | `model: null`; the "Needs a model" chip |
| `{"chat-v1":"test-model-mini","agentic-v1":"test-model"}` | `Ambiguous(["test-model","test-model-mini"])` | `model: null`, `modelAmbiguous: true`; never guessed |

**What changes the stored map:**

- A save with a model writes the uniform four-key map through
  `uniform_models(Some(id))`, which 2d renames from `tier_overrides`.
- A single-key `{"model": …}` shape is **not** used, for two reasons:
  - a rollback binary maps tiers through `models` (`inference.rs:360-378`) and
    would send bare tier names;
  - the 2d legacy rule reads configured ids from this map.

### 1.3 `composio/token` becomes `composio/tinyhumans/key` (1a)

Rule: **dual-write for one release, reads new-first with a legacy fallback,
never crossed.**

| Step | Store | `resolve_credential` presents |
|---|---|---|
| Before 1a | `{"composio/token":"th-not-a-real-key"}` | `th-not-a-real-key` |
| After deploy, before any save | same | `th-not-a-real-key` (legacy fallback) |
| `PUT …/composio/token {"token":"th-not-a-real-key-2"}` | `{"composio/tinyhumans/key":"th-not-a-real-key-2","composio/token":"th-not-a-real-key-2"}`, new address written first | `th-not-a-real-key-2` |
| Save whose legacy mirror write fails | `{"composio/tinyhumans/key":"th-not-a-real-key-2","composio/token":"th-not-a-real-key"}`; the route returns an error | `th-not-a-real-key-2` (the new address outranks) |
| Clear (`{"token":""}`) | `{"composio/tinyhumans/key":"","composio/token":""}` | `company_key::resolve` (next chain step) |
| Clear whose legacy write fails | `{"composio/tinyhumans/key":"","composio/token":"th-not-a-real-key-2"}`; the route returns an error | `th-not-a-real-key-2` until a retry clears both |
| A later release (not #2306) | stops the mirror write; the fallback read stays until nothing needs it | — |

**Rollback:** a binary from before 1a reads `composio/token`, which holds the
same value.

### 1.4 `composio/api_key` becomes `composio/byok/key` (1a)

| Step | Store, in write order |
|---|---|
| Before | `{"composio/mode":"byok","composio/api_key":"ak-not-a-real-key"}` |
| Save `ak-not-a-real-key-2` | 1 `composio/byok/key`, 2 `composio/api_key` (the same value), 3 `composio/mode="byok"` → `{"composio/mode":"byok","composio/byok/key":"ak-not-a-real-key-2","composio/api_key":"ak-not-a-real-key-2"}` |
| Save whose mirror write (2) fails on a managed company | the mode write never runs; the company stays `managed`, and the new key is inert |
| Clear | 1 `composio/mode="managed"`, 2 `composio/byok/key=""`, 3 `composio/api_key=""` |

- **BYOK with both addresses blank** still withholds tools. It never falls
  back to the managed tiers (`composio.rs:401-412`).
- **`fan_out` never writes** `composio/mode` or either BYOK address (4a).

### 1.5 `search/provider/<slug>/endpoint` moves into `search/providers[].endpoint` (1b)

Rule: **merge-on-write into the row and, for one release, also write the
per-slug key.** Reads try the row, then the per-slug key, then the flat key.

| Step | Store |
|---|---|
| Before | `{"search/providers":"[{\"slug\":\"searxng\",\"enabled\":true}]","search/provider/searxng/endpoint":"https://searx.example"}` |
| After deploy, before any save | unchanged. `list_providers` reads `entry.endpoint`, which is `None`, then the per-slug key. |
| `PUT …/search/providers/searxng {"endpoint":"https://searx2.example"}` | `{"search/providers":"[{\"slug\":\"searxng\",\"enabled\":true,\"endpoint\":\"https://searx2.example\"}]","search/provider/searxng/endpoint":"https://searx2.example"}` |
| Toggle `{"enabled":false}` with no endpoint sent | the stored endpoint is **kept** in the row, and the per-slug key is untouched |
| Delete | the per-slug key is cleared, as today (`search/store.rs:527`); the row is gone |

- **Entry zero** (a flat `search/endpoint` whose slug is not yet indexed)
  still reads `search/endpoint` (`search/store.rs:222`, `:251`).
- **Not part of #2306:** a later release stops writing the per-slug key.

### 1.6 `inference/routes`: unread after 5b, carried only when unanimous (5a)

Rule: **copy-if-empty at boot.**

- `routes_carry::carry_routes_into_default` runs once per company build,
  before `resolve_effective`.
- Its errors are logged and never fail boot.

**It writes a default only when all of these hold** (F7, Q14):

- `load_default()` is `Unset`;
- the routes blob is non-empty;
- all four `INFERENCE_TIERS` (`src/company/types.rs:80`) are present;
- all four parse to the same `<slug>:<model>`, with a non-empty model;
- `get_provider(slug)` exists and is enabled.

**It never** clears routes, writes a row, or touches managed.

| Stored before boot | Default after boot | Why |
|---|---|---|
| `inference/default` = `""`; routes `{"agentic-v1":"acme:test-model","chat-v1":"acme:test-model","reasoning-v1":"acme:test-model","vision-v1":"acme:test-model"}`, `acme` enabled | `{"provider":"acme","model":"test-model"}` | unanimous |
| `""`; `{"reasoning-v1":"acme:test-model"}` | `""` plus the banner | three tiers are absent (F7) |
| `""`; the rows name different models | `""` plus the banner | the rows differ |
| `""`; all four are `"managed"` | `""` plus the banner | a managed route carries no model |
| `""`; all four are `"acme"`, with no model | `""` plus the banner | no model |
| `"acme"` (bare slug) or a full JSON default | unchanged | not `Unset` |
| any of the above on a second boot | unchanged | idempotent |

In every case the stored `inference/routes` value **stays**. After 5b nothing
but `routes_carry` reads it.

### 1.7 `inference/config`: `finish_link` stops writing it (4a); still read

| Step | Store |
|---|---|
| Before 4a, after `finish_link` | `{"tinyhumans/key":"th-not-a-real-key","inference/config":"{\"provider\":\"managed\",\"base_url\":null,\"models\":{}}"}` |
| After 4a, after `finish_link` (empty slots) | `{"tinyhumans/key":"th-not-a-real-key","composio/tinyhumans/key":"th-not-a-real-key","composio/token":"th-not-a-real-key","provider/tinyhumans/key":"th-not-a-real-key"}`, plus a `tinyhumans` row when none exists, plus a default when `Unset` and a model was sent. **No `inference/config` write.** |
| A company that already has `inference/config={managed}` | the value **stays**. It is still entry zero, slug `tinyhumans`, resolved by the legacy sources. `put_provider` still refuses a `tinyhumans` row for it, so the fan-out copies only the keys. |

- **Contradiction with "never written again":** the legacy
  `PUT …/inference` route (`set_config`, `ops/inference.rs:1010`, write at
  `:1032`) **still writes** `inference/config`. No console component calls it
  on `fcfb3e1bc`. Unless a slice retires that route, "never written again"
  holds only for `finish_link` and the fan-out.

### 1.8 Tiers (2d): no stored value changes

- After 2d no tier name reaches the wire.
- The tier code that remains (`model_for_tier`, `TierVocabulary`,
  `DEFAULT_TIER_MODELS`) is legacy-only (F3), or deleted where 2d says so.
- The tier keys inside stored `models` maps stay, as the encoding in §1.2.

---

## 2. Keys that stay exactly as today, and why

| Key | Why it stays |
|---|---|
| `tinyhumans/key` | It is the source for the fan-out and reuse copies. It remains a **fallback** in the managed LLM chain (step 3) and the Composio chain (step 2), because item 10 (dropping those fallbacks) is not handled: hosted tenants store no key. See [not-handled.md](not-handled.md). |
| `inference/managed/enabled` | It gates the legacy managed chain and boot step 4. Removing it before item 10 would restart platform spend for companies that switched Managed off (`inference.rs:1340-1357`). Item 3 is not handled. |
| `inference/key` | Entry zero's own credential and managed chain step 2. It converges only when entry zero is re-saved (`inference/store.rs:706-720`). |
| `inference/config` | The entry-zero row and a legacy source. It is read by `entry_zero`, `resolve_legacy_scoped` and `harness_configures_itself`. |
| `inference/health` | Not routing. The provider rows and the fan-out probe use it. |
| `harness/<id>/inference/config`, `…/key` | A named harness's own config. It ranks below the agent pair (Q5). |
| `provider/<slug>/key` | Already one credential slot per provider (`inference/store.rs:200-202`). |
| `composio/mode` | Q11: an admin can hold both keys and switch. The direction-ordered write keeps a failed switch inert. |
| `composio/defaults` | Unrelated to credentials. |
| `search/provider/<slug>/key` | Already per provider. |
| `search/default` | There is no model to store, so a bare slug is correct. |
| `search/provider`, `search/api_key`, `search/endpoint` | Search entry zero. It converges on save, as today. |

---

## 3. Item 15: what "set" means for each provider kind

**The one rule** (D-set): a provider is **set** if and only if
`store::list_providers` returns a row for its slug.

- "A row" means an index row in `inference/providers`, or the synthesised
  entry zero from `inference/config`.
- `provider/<slug>/key` holds only a credential. A blank key never answers
  "set" or "not set" by itself, because a clear is also `""`.
- A set provider serves a turn only when all of these hold:
  - the row is enabled;
  - a model is chosen, by the agent pair or the default;
  - its key is present, if the kind needs one.

| Kind | "Set" after the rework | Credential | Can it serve as the default or an agent pair? | Notes |
|---|---|---|---|---|
| Cloud (e.g. `openrouter`, `anthropic`) | a row with `kind` equal to the catalogue slug | `provider/<slug>/key`. The add route refuses a kind that needs a key when none is sent. | yes, with `{provider: slug, model}` | The model is picked when the provider is added (2c). A 500-id catalogue is the layout's real cap. |
| Local (`ollama`, `lmstudio`, `omlx`) | a row with `kind` equal to the runtime slug and the `base_url` that was typed | usually none. `omlx` may use `provider/omlx/key`. | yes. Example: `{"provider":"ollama","model":"local-test-model"}` | "Local" means the host's localhost. The `local[:model]` route form ends with 5b. |
| TinyHumans | a row with slug `tinyhumans` (the 2a catalogue row), or entry zero with slug `tinyhumans` when `inference/config` is `managed` | `provider/tinyhumans/key`. The index row never reads `tinyhumans/key` or the instance identity. | yes, with a model from `…/agent-integrations/openrouter/models`, e.g. `{"provider":"tinyhumans","model":"acme/test-model"}`. Any id the list returns is valid; tier names answer 400. | The legacy Managed chain is **not** a provider in this sense. Its status stays `ManagedSource`. |
| CLI login (`claude-code`, `codex`) | **never** on a server. `plan_add` refuses the kind (`providers.rs:760-770`) and `CLI_LOGINS_REACHABLE = false`. | none | no (F8, out of scope) | An agent on an ACP harness that runs a CLI uses that CLI's own sign-in, outside provider and model. |

---

## 4. Gotchas

- **Copy-if-empty with no rotation rule** is the #2266 stale-copy bug. Always
  apply the Q7 table in target-state.md §2.
- **`put_provider` refuses a `tinyhumans` row** when entry zero is managed
  (`inference/store.rs:542-549`). Slice 2a needs a host test for it, and the
  fan-out must treat that refusal as "row already present".
- **After 2a, the Managed row's key route and the `tinyhumans` row share
  `provider/tinyhumans/key`.** Removing one removes the other's credential.
- **An injected `OPENCOMPANY_INFERENCE_URL` outranks the constants.** Moving
  only the constants leaves hosted tenants on `/openai/v1`.
- **After 2d, a no-default company with no `OPENCOMPANY_INFERENCE_MODEL` and
  no configured real id fails closed.** Hosted tenants need the manager to
  inject the variable.
- **A hand-minted account key authenticates at Composio but is refused there**
  (no `connections` scope). Only grant-minted or attested keys work.
- **Never** write on a read path, copy the rotating hosted token into a slot,
  or copy `TINYHUMANS_API_KEY` into a company.
- **Frontend has three typecheck gates.** `scripts/ci/assert-design-tokens.sh`
  rejects raw hex. No cargo locally; CI verifies by head SHA. Never a real key
  on disk.

## 5. Must not touch

- The `SecretStore` port: no list, delete or rename is added.
- `OPENHUMAN_WORKSPACE` and the other `OPENHUMAN_*` contract names (Q16).
- The managed chains' fallback steps themselves — `tinyhumans/key`,
  `inference/key`, `company_key::resolve` — and `TINYHUMANS_API_KEY` (item 10
  is not handled; `TINYHUMANS_API_KEY` is not one of the four variables 6a
  removes and stays as the chains' last step).
- `TINYHUMANS_TOKEN_FILE`, `OPENCOMPANY_INFERENCE_KEY`,
  `OPENCOMPANY_INFERENCE_URL` and `OPENCOMPANY_COMPOSIO_BACKEND_URL` in every
  slice **except** 6a, which is the only slice that touches them. Removing any
  of the four earlier stops phases 2a–5b's own fixture hosts and manager
  guidance from being read.

## 6. Done when

- Every key in target-state.md §§2-7 has exactly the writers and readers
  listed, on the branch head after 5b.
- Check the new Composio constants with
  `git grep -n '"composio/tinyhumans/key"\|"composio/byok/key"' -- src`: each
  appears only in `src/company/composio.rs` and in tests.
- Check that 5b removed every other routes reader with
  `git grep -n 'ROUTES_KEY\|load_routes\|save_routes' -- src`: the only hits
  are `store.rs` (the constant) and `routes_carry.rs`.
- Check that 2a moved the URL constants with
  `git grep -n 'openai/v1' -- src/company/inference.rs src/harness/built_in/provider.rs`:
  no hit on either constant.
- Each carry-over example in §1 is covered by a named test in its slice file.
