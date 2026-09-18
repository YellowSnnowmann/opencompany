# Phase 2a — TinyHumans on the proxy, part 3

Parts: [1](phase-2a-tinyhumans-on-proxy.md) · [2](phase-2a-tinyhumans-on-proxy-part2.md) · [3](phase-2a-tinyhumans-on-proxy-part3.md).

This continues part 2; section numbers carry on from it. `file:line` references
are on `upstream/main @ fcfb3e1bc` (2026-09-14).

**Fixtures.** Model ids in tests and examples are fake ids served by a mock
catalog (`acme/test-model`, `acme/other-model`). The key is `th-not-a-real-key`.

## 8. Tests

### 8.1 Rust

**`src/company/inference/paged_catalog.rs`:**

| Test | Assert |
|---|---|
| `the_page_path_asks_for_the_maximum_page` | `page_path(0) == "/models?limit=500&offset=0"` |
| `a_page_is_unwrapped_and_unknown_fields_are_ignored` | body with `object`, `pricing`, `supports_tools`, `input_modalities`, `display_name` → ids `acme/test-model`, `acme/other-model`, `name`, `context_length`, `raw_len`, `total` |
| `every_id_is_kept_as_given` | ids `acme/test-model`, `x`, `a:b:c` come back unchanged, in order |
| `an_openai_shaped_body_is_an_error_not_an_empty_page` | `{"data":[{"id":"acme/test-model"}]}` → `Err` containing "envelope" |
| `success_false_is_an_error_carrying_the_reason` | `Err` contains the backend `error` text |
| `a_malformed_entry_is_dropped_but_still_advances_paging` | ids `42` and `"   "` dropped; string `context_length` → `None`; `raw_len` counts all |
| `paging_follows_total_and_stops_there` | two pages, total 3: `At(2)` then `Done` |
| `a_clamped_limit_costs_requests_not_models` | pages of 1, total 2: `At(1)` then `Done` |
| `an_empty_page_ends_a_read_whose_total_is_never_reached` | `Done` |
| `a_page_with_no_total_is_the_whole_answer` | `Done` |
| `duplicates_across_pages_are_kept_once` | once each, listing order |
| `the_page_bound_reports_truncation_instead_of_looping` | 20 pushes with total 1,000,000 → `Truncated { read: 20, total: 1_000_000 }` |

**`src/company/inference/catalogue.rs`:**

- `the_catalogue_ships_the_counts_the_plan_names` (:1077): 26 becomes 27.
- Rename `managed_owns_its_slug_even_though_it_is_not_a_catalogue_row`
  (:1257-1271) to `tinyhumans_owns_its_slug_and_is_a_catalogue_row`. Keep its
  `is_reserved_slug` asserts; the last assert becomes
  `cloud_provider("tinyhumans").is_some()`.
- New `tinyhumans_is_a_bearer_row_on_the_proxy`: endpoint,
  `auth_style_for("tinyhumans") == Bearer`, placeholder `th-...`.
- New `catalog_shape_is_paged_only_for_tinyhumans_or_the_proxy_path`:
  - Paged: `("tinyhumans", proxy)`, `(" tinyhumans ", "")`, and
    `("openrouter", "https://api.tinyhumans.ai/agent-integrations/openrouter/")`.
  - OpenAi: `("openrouter", "https://api.tinyhumans.ai/openai/v1")`,
    `("custom", "http://127.0.0.1:8099/v1")`, and every other `CLOUD_PROVIDERS`
    row with its own endpoint.
- Re-run unchanged:
  - `anthropic_is_the_only_non_bearer_entry_in_the_catalogue` (:1651);
  - `every_entry_has_a_parseable_endpoint_and_a_known_auth_style` (:1084);
  - the mirror tests (:1478…);
  - the `endpoint_is_chat_completions_only` asserts (:1184-1196). That function
    has no caller outside tests, per `git grep`.

**`src/company/inference/probe.rs`:**

- **`a_paged_probe_reads_every_page_with_the_bearer`.** A loopback `axum` on
  `127.0.0.1:0` serves `/agent-integrations/openrouter/models`: offset 0 returns
  `acme/test-model`, `acme/other-model` with total 3; later offsets return
  `acme/third-model`. Call
  `probe_models(base, Some("th-not-a-real-key"), Bearer, LOCAL_OFFERED, PagedEnvelope)`.
  Assert the 3 ids come back in order, there were 2 requests with queries
  `limit=500&offset=0` and `limit=500&offset=2`, and both carried
  `Authorization: Bearer th-not-a-real-key`.
- **`a_paged_probe_that_gets_an_openai_body_is_unknown_not_auth`:**
  `class == ProbeClass::Unknown`.

**`src/server/inference_models.rs`:** add the helper
`spawn_proxy_catalog(respond: fn(usize) -> (u16, String)) -> (String, Seen)`.
It serves `/agent-integrations/openrouter/models` and records each query and
`Authorization` header.

| Test | Setup | Assert |
|---|---|---|
| `the_paged_catalog_is_read_to_total_with_the_bearer` | 2 pages | ids, `name`, `context_length`; 2 requests; bearer on each |
| `a_large_catalog_is_read_in_pages_of_500` | mock serves 1,200 ids `acme/model-0001`… by offset/limit | 3 requests (offsets 0, 500, 1000); 1,200 models; every served id present |
| `a_503_before_the_snapshot_loads_is_memoized_not_empty` | 503 `{"success":false,…}` | `Err` contains "503"; `catalog_cache_scoped(&shaped_endpoint(&base, PagedEnvelope), Some(scope)).lookup_failure(now).is_some()` |
| `a_401_or_403_on_the_paged_catalog_is_not_memoized` | loop 401, 403 | `Err`; `lookup_failure(now).is_none()` |
| `an_openai_shaped_answer_is_not_read_as_the_paged_catalog` | 200 `{"data":[{"id":"acme/test-model"}]}` | error contains "envelope" |
| `a_paged_failure_never_names_the_endpoint_credential` | base with `alice:hunter2@` | no "hunter2", no "alice" |
| `an_oversized_page_is_refused_not_buffered` | body `PAGE_BODY_CAP + 1` bytes | error contains "larger than" |
| `one_url_read_in_two_shapes_is_two_cache_slots` | — | `shaped_endpoint(B, OpenAi) == cache_key(B)`, and it differs from the Paged slot |

**`src/company/inference.rs`: `a_tinyhumans_row_resolves_direct_on_its_own_key_and_model`.**

- **Setup:** `store::put_provider` the draft from
  `catalogue::cloud_provider("tinyhumans")`, with the four tiers mapped to
  `acme/test-model`; set the key at `provider_key_key("tinyhumans")`.
- **Assert:** `base_url` is the proxy; `!decl.is_proxied()`; the bearer is the
  fake key;
  `model_for_tier("agentic-v1", &decl.models, decl.vocabulary()) == "acme/test-model"`.
- **Second case:** only `tinyhumans/key` (the account) and no row key. The
  row's bearer is `None` (D-set: no fallback).

**`src/harness/built_in/provider.rs`: `a_proxy_reply_with_extra_top_level_and_usage_keys_parses`.**

- **Payload:** `model: "acme/test-model"`, `choices[0].message.content = "pong"`,
  top-level `service_tier` and `openhuman: {billing, usage}`, and `usage` with
  `prompt_tokens: 12`, `completion_tokens: 3`, `cost`, `is_byok`.
- **Assert** through `model_response_from_payload`: text `pong`, input 12,
  output 3.

There is no stream test (part 2 §5.9).

**`src/server/ops/inference.rs`** (helpers `home()` :1407, `state_with_company`
:1476, `send` :1812, `runtime_with`):

| Test | Setup and call | Assert |
|---|---|---|
| `adding_tinyhumans_over_a_legacy_managed_config_is_refused_and_writes_nothing` | `PUT /api/v1/company/inference {"provider":"managed"}`; then `POST …/inference/providers {"kind":"tinyhumans","key":FAKE,"model":"acme/test-model"}` | 400 with "TinyHumans is already connected. Edit the existing row rather than adding a second one."; body has no `FAKE`; `GET` shows one `tinyhumans`, `origin == "entryZero"`, no `health`, `managed.legacyRow == false` |
| `adding_tinyhumans_without_a_key_is_refused` | `{"kind":"tinyhumans","model":"acme/test-model"}` | 400 "TinyHumans needs an API key."; no `tinyhumans` provider |
| `adding_tinyhumans_without_a_model_is_refused_before_any_write` | `{"kind":"tinyhumans","key":FAKE}` | 400 "Choose a model for TinyHumans."; no `tinyhumans` provider; `managed.source == "none"` |
| `a_listed_tinyhumans_row_hides_the_legacy_managed_row` | `runtime_with`; `store::put_provider` the row; set its key; `effective_status_with(&runtime, None, false)` | one `tinyhumans`, `origin == "indexed"`; `managed.configured`; `source == "provider_key"`; `!legacy_row` |
| `an_account_key_only_company_shows_the_legacy_managed_row` | only `company_key::KEY_KEY` set | no `tinyhumans` provider; `source == "company_account"`; `legacy_row` |
| `a_company_with_nothing_shows_no_legacy_row` | nothing | `!configured`; `!legacy_row` |

**`src/server/ops/inference/providers.rs`** (tests :2110):
`restore_previous_key_puts_back_the_replaced_key_and_none_does_nothing`.

1. Set the slot to `"th-new-not-a-real-key"`.
2. `restore_previous_key(…, Some("th-not-a-real-key"))`: the slot now reads the
   old value.
3. `restore_previous_key(…, None)`: the slot is unchanged.

An end-to-end rollback cannot run as a host test, because the TinyHumans
endpoint is a fixed internet URL. List it in the PR as an intentionally untested
edge.

**Commit 5 check:** `git grep -n 'openai/v1' -- src` shows only the part 2 §5.9
"stay" hits.

### 8.2 Frontend unit (`frontend/test/unit/`)

**`inference-catalogue.test.ts`:**

- `toHaveLength(26)` becomes 27.
- New "lists TinyHumans once, as a bearer row on the proxy": one `tinyhumans`
  entry, proxy endpoint, `auth === "bearer"`, `isReservedSlug("tinyhumans")`.
- The non-bearer list stays `["anthropic"]`.

**`inference-managed-row.test.ts`:**

- Delete `describe("where managed is offered")` and its `offersManaged` /
  `MANAGED_OPTION_SLUG` imports.
- Add `describe("TinyHumans is offered once, as its catalogue row")`:
  - `addOptions([]).cloud` has exactly one `value === "tinyhumans"`, with
    `{label: "TinyHumans", detail: "api.tinyhumans.ai"}`;
  - it is hidden when `providers` holds `tinyhumans` with `origin` `"indexed"`,
    and when it holds `origin` `"entryZero"`.
- Add `describe("showsLegacyManagedRow")`:
  - configured with `legacyRow: false` → false;
  - configured with `legacyRow: true` → true;
  - configured with `legacyRow` absent → true;
  - not configured → false.

**`inference-connect.test.ts`:**

- The `CLOUD_PROVIDERS.length - 1` assert (:48) is unchanged.
- Add `credentialAsk("tinyhumans")` →
  `{title: "Connect TinyHumans", needsKey: true, needsEndpoint: false, keyPlaceholder: "th-..."}`.
- Add `probeEndpoint("tinyhumans")` → the proxy URL.
- Add `asksForModel`:
  - `("tinyhumans", {ok: true, needsModel: false})` → true;
  - `("tinyhumans", {ok: false})` → true;
  - `("openrouter", {ok: true, needsModel: false})` → false;
  - `("openrouter", {ok: true, needsModel: true})` → true;
  - `("openrouter", {ok: false})` → false.

**New `inference-tinyhumans-one-row.test.ts`.** Render with
`renderToStaticMarkup(createElement(ProviderList, props))`, as
`inference-hub-account-links.test.ts` does. The `Provider` fixture fills every
required field of `frontend/src/inference/types.ts`. Handlers are `() => {}`,
`testState` returns `{ kind: "idle" }`, and `routingState` returns `null`.

- **"a company that added TinyHumans renders exactly one TinyHumans row"**
  - Props: providers `[tinyhumans, origin "indexed"]`; managed
    `{source: "provider_key", configured: true, legacyRow: false, baseUrl}`.
  - Assert: exactly one `data-testid="inference-provider-tinyhumans"`, and no
    `data-testid="inference-provider-managed"`.
- **"a company with only an account key renders the legacy Managed row"**
  - Props: providers `[]`; managed
    `{source: "company_account", configured: true, legacyRow: true}`.
  - Assert: contains `inference-provider-managed`, and not
    `inference-provider-tinyhumans`.

### 8.3 E2E (`frontend/test/e2e/inference.spec.ts`)

Add `test("TinyHumans is added through its catalogue row and shows as exactly one row")`.

The real probe goes to `api.tinyhumans.ai`, so this test stubs **the console's
API calls only**, with `page.route` (pattern: `brain-virtualization.spec.ts:48`).

1. **Stub the probe.** `**/api/v1/company/inference/probe` →
   `{ok: true, modelCount: 500, needsModel: false, models: ["acme/model-0001" … "acme/model-0499", "acme/test-model"]}`.
   That is a large catalog served by the mock, at the probe's real cap
   (`PROBE_CATALOGUE_LIMIT`, `providers.rs:268`). `needsModel: false` proves the
   step appears for TinyHumans regardless.
2. **Stub the add.** On `POST **/api/v1/company/inference/providers`:
   - record the body;
   - `const real = await (await route.fetch({ url: <origin>/api/v1/company/inference, method: "GET" })).json();`
   - push a `tinyhumans` provider (`origin: "indexed"`, `keyConfigured: true`,
     tiers → `acme/test-model`, `health: {state: "ok", at}`);
   - set `real.managed.legacyRow = false` if `real.managed` exists;
   - fulfill `{status: real, note: "TinyHumans is connected and answering."}`;
   - fulfill later `GET **/api/v1/company/inference` with `real`.
3. **Open the dialog.** `openInference(page)`,
   `choose(page, "cloud", "TinyHumans")`, fill `#inference-connect-key` with
   `th-not-a-real-key`, click `inference-connect-submit`.
4. **Assert the model step.** `inference-connect-model` is visible, and
   `#inference-connect-model-options option` has count 500.
5. **Choose.** Fill `acme/test-model` and submit.
6. **Assert the request.** The recorded body has `kind: "tinyhumans"`,
   `model: "acme/test-model"`, `key: "th-not-a-real-key"`.
7. **Assert the page.** `inference-provider-tinyhumans` count is 1 and
   `inference-provider-managed` count is 0.

The test's comment should say it is console-only; the host is covered by §8.1.
The existing test "Managed is a connected row only when its chain actually
resolves" (:96-127) is unchanged: on the live-brain lane nothing is listed, so
`legacyRow` is true.

## 9. Console / UI

- **Add dialog:** a Cloud entry "TinyHumans", detail `api.tinyhumans.ai`. The
  "Managed (TinyHumans)" entry is gone.
- **Connect dialog:** title "Connect TinyHumans", placeholder `th-...`, and the
  replace note `inference-connect-replaces-key` when a Managed key exists.
- **Model step:** `inference-connect-model` with datalist
  `inference-connect-model-options`. It lists exactly what the probe returned
  and accepts any typed id.
- **Row:** `inference-provider-tinyhumans`, label "TinyHumans", sub-line
  `api.tinyhumans.ai`, health `inference-provider-tinyhumans-health`.
- **Legacy row:** `inference-provider-managed`, only when
  `showsLegacyManagedRow`.
- **Tokens:** existing classes only (`text-muted-foreground`). No raw hex;
  `scripts/ci/assert-design-tokens.sh` must pass.

## 10. Must not touch

- **The managed chain:** `load_managed_key` (`inference.rs:936`),
  `managed_source` (:1079), `managed_identity` (:1116), `resolve_endpoint`
  (:644-694), `decl_for_probe`, `is_managed_choice` (:399), `normalize_provider`
  (:382).
- **Legacy managed routes and state:** `set_managed_key`
  (`providers.rs:1697-1777`), `set_managed_enabled` (:1461), `test_managed`
  except its one shape argument, and `inference/managed/enabled`.
- **Routing:** `resolve.rs`, the routes handlers, `auto_route_sole_provider`,
  `managed_parked_tiers`.
- **Tiers:** `DEFAULT_TIER_MODELS`, `TierVocabulary`, `model_for_tier`,
  `tier_overrides`, and `needs_an_explicit_model` for other kinds.
- **Store:** `store.rs` (`put_provider`, `list_providers`,
  `legacy_slot_is_managed`).
- **Other surfaces:** Composio, the Account page, `company_key.rs`,
  `TINYHUMANS_TOKEN_FILE`, `vendor/`.
- **E2E hosts:** the `frontend/playwright.config.ts` env, `mock-brain.mjs`,
  `live-brain-proxy.mjs`.
- **Other work:** every other file in `docs/key-reworks/`, and any worktree you
  do not hold.

## 11. Done when

1. Commits 1–4 are pushed.
2. **Rust:** `cargo fmt --all -- --check` passes locally. Clippy and tests pass
   on CI, checked **by head SHA**:
   - `gh api "repos/tinyhumansai/opencompany/actions/runs?head_sha=$SHA"` for
     the run id;
   - `gh api …/actions/runs/$RUN/jobs` shows zero failures and zero pending;
   - both Console E2E lanes have completed.

   Never trust `gh pr checks`.
3. **Frontend:** `npm run typecheck`, `npm run typecheck:unit` and
   `npm run typecheck:e2e` are each run and named;
   `scripts/ci/assert-design-tokens.sh` passes.
4. **Browser evidence,** on a port from `local/wt ports --take`, checked with
   `--verify`, in light and dark, with screenshots:
   - the TinyHumans model step showing a large catalog served by the mock;
   - the Connected card with exactly one TinyHumans row;
   - an account-key-only company showing the legacy Managed row.

   No real key touches disk.
5. **PR description:** lists the untested edge (§8.1) and says there is no
   streaming path.
6. **E2E fixture hosts:** keep `OPENCOMPANY_INFERENCE_URL=http://…/v1`; no
   change needed.
7. **Commit 5:** either absent (the PR says so) or present with the operator's
   §12.1 answer quoted.

## 12. Gotchas and stop points

### 12.1 STOP before commit 5 (3b)

Ask the operator. Once the constants flip, legacy managed traffic reaches the
proxy carrying tier names, and the proxy refuses a tier name as a model id. That
traffic is:

- a company with no row that resolves through the managed chain;
- the setup wizard Managed Test (`chat-v1`);
- the setup brain (`DEFAULT_HOSTED_MODEL`);
- every hosted tenant without an injected `OPENCOMPANY_INFERENCE_URL`.

Options:

- **(A) Hold commit 5 until slice 2d gives the legacy path a model.**
  Recommended; it matches D-legacy.
- **(B) Flip now, and have the manager inject
  `OPENCOMPANY_INFERENCE_URL=https://api.tinyhumans.ai/openai/v1` for hosted
  tenants.** Self-hosted and docker-dev instances on `TINYHUMANS_API_KEY` still
  break.
- **(C) Flip now and accept the 400s.**

**Manager-side prerequisite, in every case:** hosted tenants run on whatever
`OPENCOMPANY_INFERENCE_URL` the manager injects, used verbatim. Moving them to
the proxy needs the proxy URL **and** a model source (2d), and that is a change
in `opencompany-microservices`.

### 12.2 The row resolves direct

`is_managed_choice("tinyhumans")` is **true** (`inference.rs:399-401`). If a
row's kind went through `resolve_endpoint` or `decl_for_probe`, it would get the
platform URL and the chain's identities. Indexed rows go through
`decl_for_indexed`, which sets `proxied = false` (:1431-1437). Do not add
`is_managed_choice` checks on the row path.

### 12.3 One key slot, two readers

The add writes `provider/tinyhumans/key`, which is also step 1 of the legacy
chain, so `managed.configured` becomes true. Consequences:

- `auto_route_sole_provider` does not route (`providers.rs:577`);
- unset routes resolve to the primary provider;
- explicit `managed` routes use the chain on the same key.

Removing the row clears the slot (`delete_provider`). Item 12 owns what happens
next.

### 12.4 The model step never depends on catalog content

The content-based backstop (`providers.rs:442`, `:668-670`) stays for other
kinds. For TinyHumans, the host refuses a model-less add (part 2 §5.6 b) and the
console always asks (`asksForModel`). Do not add any rule that reads model ids
by name.

### 12.5 The probe cap

`PROBE_CATALOGUE_LIMIT` is 500 (`providers.rs:268`). This is an existing cap and
it is unchanged. A larger catalog offers its first 500 ids in sorted order, and
the field still accepts any typed id. A paged success body is capped at 4 MiB,
not the probe's 64 KiB (`probe.rs:653`).

### 12.6 Metering is not handled

The row has `proxied = false`. Whether `usage.cost` and `openhuman.billing` are
metered for it is out of scope (`cost.rs:87`, `:229`). Report it to the main
session for `not-handled.md`; do not edit that file.

### 12.7 Row order is load-bearing

`the_console_mirror_lists_the_same_cloud_providers` (`catalogue.rs:1478`)
compares the tables row by row. Put `tinyhumans` last in **both**.

### 12.8 What is verified and what is inferred

- **Verified live on 2026-09-14** (parent session): the envelope and paging;
  OpenAI-compatible chat, where extra keys can be ignored; a 400 for a tier
  name.
- **Verified by reading `fcfb3e1bc`:** every `file:line` in this slice.
- **Inferred:** that hosted tenants have no injected `OPENCOMPANY_INFERENCE_URL`
  (from `CLAUDE.md`), and the entry-zero "two rows today" case.

---

Parts: [1](phase-2a-tinyhumans-on-proxy.md) · [2](phase-2a-tinyhumans-on-proxy-part2.md) · [3](phase-2a-tinyhumans-on-proxy-part3.md). Start of the slice: [part 1](phase-2a-tinyhumans-on-proxy.md).
