# Keys rework

How a company's keys, providers and models are stored and chosen, reworked in
one PR (issue #2306). This folder is the plan **and** the implementation
brief: every slice file is written so that an implementer with no other
context can execute it exactly.

- **Requested by:** the operator, 2026-09-14 (dump items 1–20).
- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14, #2304 merged).
  Every `file:line` in this folder is on that commit unless marked otherwise.
  Line numbers drift; every slice also quotes the code so it can be found.
- **PR #2305 is closed** (2026-09-14) and is **not** a dependency. What it was
  meant to do — TinyHumans as an ordinary provider on the
  `/agent-integrations/openrouter` proxy — is slice 2a here. Its diff is read
  only as a record of what not to do (see 2a's do-not-do list). Never merge from
  its branch `feat/managed-openrouter-proxy`; never use worktree `oc-t9-wt-2`.

## The goal in plain words

1. **A provider is a provider plus one model.** OpenRouter + `acme/test-model`,
   Anthropic + `test-model-small`, Ollama + `local-test-model`, TinyHumans +
   `acme/test-model`.
2. **No tier name is ever sent as a model.** `chat-v1`, `reasoning-v1`,
   `agentic-v1`, `vision-v1` never go on the wire. Only a real model id does.
3. **TinyHumans ("Managed") is one ordinary row** on
   `https://api.tinyhumans.ai/agent-integrations/openrouter`: model list at
   `…/models` (paged envelope), chat at `…/chat/completions`.
4. **A company has one default `{provider, model}`**, stored in the existing
   `inference/default` key.
5. **Each agent may have its own `{provider, model}` pair**, stored on the agent's
   existing record (e.g. the researcher on `anthropic` / `test-model-large`, the
   web-search agent on `anthropic` / `test-model-small` — a larger model for
   research, a smaller one for search, as the operator described (item 19)).
6. **Resolution:** agent pair → company default → a clear "choose a model" error.
   A pair or default naming a missing or switched-off provider fails closed; it
   never falls through (F6).
7. **"Is it set?" has one rule:** a provider is set ⇔ it has a row in
   `inference/providers`. `provider/<slug>/key` holds only a credential.
8. **Key names say what they hold:** `composio/token` → `composio/tinyhumans/key`,
   `composio/api_key` → `composio/byok/key`; the search endpoint moves into the
   `search/providers` record.
9. **One account-key save sets up TinyHumans for LLM and Composio**, never
   overwriting a key someone set on those pages.
10. **Routing goes away**, after a carry-over that copies a unanimous routing
    table into an empty default.

## Order of work

Each row is one commit (or a short run of commits) on branch
`feat/key-reworks`, pushed as soon as it is verified. Do them top to bottom.

| # | Slice file | Dump items / decisions |
|---|---|---|
| 0 | this folder (docs only) | item 20 |
| 1a | [phase-1a-composio-keys.md](phase-1a-composio-keys.md) (+ [part 2](phase-1a-composio-keys-part2.md)) | 7, 8; Q11, Q12 |
| 1b | [phase-1b-search-endpoint.md](phase-1b-search-endpoint.md) | 9 |
| 1c | [phase-1c-dead-code.md](phase-1c-dead-code.md) | 17 (dead code only); Q10, Q16 |
| 2a | [phase-2a-tinyhumans-on-proxy.md](phase-2a-tinyhumans-on-proxy.md) | 13b, 14, 15; Q3 |
| 2b | [phase-2b-default-shape.md](phase-2b-default-shape.md) | 1, 16; Q1 |
| 2c | [phase-2c-model-required.md](phase-2c-model-required.md) (+ [part 2](phase-2c-model-required-part2.md)) | 1, 4, 14; Q2 |
| 2d | [phase-2d-no-tier-on-the-wire.md](phase-2d-no-tier-on-the-wire.md) | 16 (no tiers); F3; D-legacy |
| 3a | [phase-3a-agent-pair-backend.md](phase-3a-agent-pair-backend.md) (+ [part 2](phase-3a-agent-pair-backend-part2.md)) | 13c, 19; Q5; F5, F6 |
| 3b | [phase-3b-agent-pair-editor.md](phase-3b-agent-pair-editor.md) | 13c, 19 |
| 4a | [phase-4a-account-key-fanout.md](phase-4a-account-key-fanout.md) | 6, 11; Q6, Q7, Q10 |
| 4b | [phase-4b-account-dialog.md](phase-4b-account-dialog.md) | 5, 6; Q9 |
| 4c | [phase-4c-reuse-banner.md](phase-4c-reuse-banner.md) | 12; Q8 |
| 5a | [phase-5a-routes-carry.md](phase-5a-routes-carry.md) | 2; Q14; F7 |
| 5b | [phase-5b-routing-removal.md](phase-5b-routing-removal.md) | 2, 13a |
| 6a | [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md) | 18 (now handled); D-env-cleanup |

Phase overviews (what, where, use cases, handled / not handled, rollback):
[phase-1.md](phase-1.md) · [phase-2.md](phase-2.md) · [phase-3.md](phase-3.md) ·
[phase-4.md](phase-4.md) · [phase-5.md](phase-5.md). Slice 6a has no separate
overview file; [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md) is
both.

Reference: [current-state.md](current-state.md) + [part 2](current-state-part2.md)
(every key today) · [target-state.md](target-state.md) +
[part 2](target-state-part2.md) (every key after) ·
[not-handled.md](not-handled.md) · [risks-and-tests.md](risks-and-tests.md).

**Stop points — report instead of guessing:** a slice needs a new secret-store
key; a decision below turns out impossible or dangerous in code; a carry-over
would clear or overwrite a stored value; slice 2d is ready to deploy but the
manager has not confirmed it injects `OPENCOMPANY_INFERENCE_MODEL` and the proxy
`OPENCOMPANY_INFERENCE_URL` for hosted tenants (the merge itself is not blocked;
the deploy is); slice 6a is ready to deploy but the manager has not confirmed
every hosted tenant has a `provider/tinyhumans/key` (or another provider's key)
set — after 6a there is no env-var fallback left for managed inference,
embeddings or search (merge is not blocked; the deploy is).

## Decisions (taken; do not re-open)

The operator delegated these on 2026-09-14 ("just take the decision and make
the changes"). The Q-numbers are the questions in the Keys Rework Rundown.

| Id | Decision |
|---|---|
| D-model | Every provider = provider + one chosen model. Company default `{provider, model}`; optional agent pair; resolution agent pair → default → "choose a model" error. |
| D-no-tier | A tier name is never sent as a model on the wire, on any path. |
| D-set | A provider is set ⇔ a row in `inference/providers`. `provider/<slug>/key` holds only a credential. Applies to cloud, local and TinyHumans. **No new secret-store keys.** |
| D-proxy | TinyHumans model list and chat use `/agent-integrations/openrouter`, never `/openai/v1`. No URL-origin rewriting of an injected `OPENCOMPANY_INFERENCE_URL`: the manager injects the proxy URL. |
| D-legacy | A company with no full default and no agent pair keeps its resolution **source** (entry zero, manifest `[inference]`, env default, managed chain). After 2d its wire model is `OPENCOMPANY_INFERENCE_MODEL` or a configured real id; with neither, its turns fail closed with "choose a model". |
| Q1 | `inference/default` is JSON `{"provider":"…","model":"…"}`. A stored bare slug reads as "provider chosen, model not chosen" and is never rewritten. |
| Q2 | Setting a default always requires a model. |
| Q3 | TinyHumans is a normal row in `inference/providers` (slug `tinyhumans`, slice 2a). Exactly one TinyHumans row is ever shown. |
| Q5 | The agent pair lives on the agent's existing record (`Agent`, `OverlayAgent`, `AgentOverride`). Order: agent pair → named-harness `[harness.inference]` → company default. |
| Q6 | Account-key save whose inference health check says `auth`: keep the account key and the Composio copy; roll back the inference copy and the default **this request** wrote. |
| Q7 | A derived slot is written when it is empty **or equal to the old account key**; any other value is left. Clearing the account key clears copies still equal to it. |
| Q8 | Removing the TinyHumans key while it is the default (or an agent pair): keep the default, warn in the dialog, show the reuse banner. |
| Q9 | The account-key dialog shows one conditional line naming only the slots the save will fill, and the host's after-save note. |
| Q10 | Keep the key-grant backend (`/credential/link/*`) and the redeem hook; `finish_link` runs the same fan-out and stops writing `inference/config`. |
| Q11 | Keep `composio/mode`. |
| Q12 | `composio/byok/key` (not `byo`). |
| Q13 | Internal passes (title, triage, planning, …) use the default model. A separate background model is **not** built. |
| Q14 | Routes are copied into an empty default only when all four tier rows are present and identical; otherwise a banner. |
| Q16 | `OPENHUMAN_*` names stay (embedded runtime contract). Only verified dead code is removed. |
| D-env-cleanup | The operator decided (2026-09-15) to remove exactly four environment variables — `OPENCOMPANY_INFERENCE_KEY`, `OPENCOMPANY_INFERENCE_URL`, `OPENCOMPANY_COMPOSIO_BACKEND_URL`, `TINYHUMANS_TOKEN_FILE` — in slice 6a, accepting the hosted-tenant risk that item 10 had deferred item 18 on. `TINYHUMANS_API_KEY`, `TINYHUMANS_API_URL` and every `OPENHUMAN_*` name are **not** in scope and stay exactly as they are. |
| F3 | Tier code is quarantined, not deleted, only where the legacy path still maps a tier hint to a configured real id (2d names each place). |
| F6 | A pair or default naming a missing or disabled provider fails closed, with no fallback. |
| F7 | Route carry-over requires all four tier rows present and equal. |
| F8 | CLI logins as LLM providers are out of scope. |
| D-mirror | A renamed address (1a Composio keys, 1b search endpoint) is written at both the new and the legacy address for one release, so a rolled-back binary keeps working. Reads are new-first. Stopping the mirror is a later release. |

### Gaps found by the test planner, closed 2026-09-15 (orchestrator, operator-delegated)

The operator delegated these to the orchestrator, who delegated the build to
the implementing agents ("just take the decision and make the changes"). They
close gaps the test planner found in the slice files above; where one narrows
or supersedes an earlier row (Q13), the later row wins.

| Id | Decision |
|---|---|
| D-first-default (X1) | The first provider + model added always becomes the company default, in the backend, with no opt-out. If a default already exists, adding never changes it. |
| D-managed-toggle (X3) | TinyHumans has no special on/off switch. A `tinyhumans` row's enabled state is the same per-row toggle every provider has, and it follows the in-use contract (`usedBy`, 409, `confirmInUse`). `inference/managed/enabled` is read only for the legacy Managed row's own chain, and every such read is marked `DEPRECATED(keys-rework #2306)`. |
| D-key-without-row (X5) | `provider/tinyhumans/key` set with no `tinyhumans` row (e.g. the account-key fan-out, 4a) is **not** "set" (D-set is unchanged: set ⇔ a row exists). The status DTO says so explicitly rather than reading as connected/healthy — see `ManagedDto.needs_model` (2a). |
| D-legacy-writes (X6) | `PUT …/inference/managed/key` and the legacy `PUT …/inference` (`set_config`) stay, for a manager or CLI that still calls them. Both handlers are marked `DEPRECATED(keys-rework #2306)` naming their replacement; the console stops calling them (Agent C); each keeps a passing back-compat test. |
| D-never-clear-default (X14) | Disabling or deleting the provider a default or an agent pair names never clears or rewrites `inference/default` or the pair. This **supersedes** the pre-rework `clear_default_if_marked` calls on delete/disable, which are removed (2c). Turns fail closed with the D-copy sentence; status exposes that the default (or pair) points at a missing/off provider so the console can show a banner. |
| D-names-in-errors (X7) | A user-facing turn-failure sentence names the agent's **display name**, not its id. |
| D-attribution (X8) | Usage and cost book to the provider and model that actually served the turn, including a pinned agent's own pair (3a gotcha G8). Additive `provider`/`model` fields are added to a run/turn record where one exists and the addition is additive. |
| D-copy (X9) | One shared set of turn-failure and save-refusal sentences, defined once (`src/company/inference/copy.rs`) and asserted by name in tests. See that module for the exact text. |
| D-internal-passes (X12) | Internal passes (title, triage, planning, …) resolve: 1) the company default; 2) else the agent pair of the turn they serve; 3) else a non-essential pass degrades gracefully (e.g. title falls back to the message text) with a `warn!` log, and an essential pass fails with the D-copy "no model is chosen" sentence. Never a tier string, never another provider. **Supersedes Q13**, which had internal passes on the default only with no fallback. |
| D-no-silent-echo (X13) | A company that had inference configured (routes, entry zero, or a provider row) never silently boots the echo brain once routing is removed (5b). If the routes table unanimously names one provider but its values are tier strings rather than real ids, 5a carries `DefaultChoice::ProviderOnly(slug)` into an empty default (not `Full`) — status then shows "choose a model" rather than nothing. Otherwise the 5a banner covers it. Turns fail closed with a D-copy sentence, never an echo reply. |

`D-attribution`'s pinned-agent telemetry fix (G8) and `D-internal-passes`'s
per-turn agent-pair fallback (step 2) are the two items in this table with the
widest blast radius across the harness turn path; see the PR body for what
shipped versus what is recorded as a follow-up.

## Shared naming contract

Every slice uses these names. Do not invent synonyms.

| Name | Kind | Slice | Meaning |
|---|---|---|---|
| `composio::TINYHUMANS_KEY_KEY` = `"composio/tinyhumans/key"` | const | 1a | TinyHumans bearer for managed Composio |
| `composio::BYOK_KEY_KEY` = `"composio/byok/key"` | const | 1a | company's own Composio `ak_…` key |
| `composio::LEGACY_TOKEN_KEY` = `"composio/token"` | const | 1a | fallback + mirror for `TINYHUMANS_KEY_KEY` only |
| `composio::LEGACY_API_KEY_KEY` = `"composio/api_key"` | const | 1a | fallback + mirror for `BYOK_KEY_KEY` only |
| `composio::load_tinyhumans_key` / `composio::load_byok_key` | fns | 1a | new-first reads with a fixed, never-crossed mapping |
| `IndexEntry.endpoint` | field | 1b | search endpoint inside `search/providers` |
| `CatalogShape { OpenAi, PagedEnvelope }`, `catalog_shape_for(kind)` | enum / fn | 2a | per-kind model-list shape |
| `paged_catalog` module | module | 2a | parser + paging for the TinyHumans envelope |
| `store::ModelChoice { provider, model }` | struct | 2b | a full provider + model pair |
| `store::DefaultChoice { Unset, ProviderOnly(String), Full(ModelChoice) }` | enum | 2b | parsed `inference/default` |
| `store::load_default` / `store::set_default_choice` | fns | 2b | read / one-write of the JSON default |
| `ModelOnRow { None, One(String), Ambiguous(Vec<String>) }`, `Provider::model()` | enum / fn | 2b | a row's single model, never guessed |
| `InferenceDecl::chosen_model()` / `with_chosen_model()` | fns | 2b | the model the new path sends |
| `resolve_for_turn(…, pin: Option<ModelChoice>, legacy_hint)` | fn | 2b | agent pair → default → legacy |
| `check_model_id` | fn | 2c | shared model-id validation |
| `defaultChoice`, `providers[].model`, `providers[].modelAmbiguous` | DTO fields | 2c | status wire shape |
| `legacy_tiers` module | module | 2d | the tier-hint → configured-id lookups the legacy path still needs |
| `Agent.provider`, `OverlayAgent.provider`, `AgentOverride.provider` | fields | 3a | agent pair's provider slug |
| `HarnessModel::pinned(agent_id, &ModelChoice)` | trait fn | 3a | per-agent provider instance; errors name the agent |
| `company_key::fan_out` | fn | 4a | account-key copies under a per-company lock |
| `routes_carry::carry_routes_into_default` | fn | 5a | boot-time copy-if-empty of unanimous routes |

If a slice file names something differently, the slice file is wrong: fix it
to match this table in the same commit.

## Glossary

- **Provider** — a row in `inference/providers` (cloud, local runtime, or
  TinyHumans), with its credential at `provider/<slug>/key` when it needs one.
- **Model** — the id a provider's API accepts (`acme/test-model`,
  `test-model-small`, `local-test-model`). Never a tier name.
- **Tier** — `chat-v1`, `reasoning-v1`, `agentic-v1`, `vision-v1`. After this
  rework only a *hint* (on an agent, or as a key in a legacy map), never a model.
- **Default** — the company's `{provider, model}` in `inference/default`.
- **Agent pair** — an agent's own `{provider, model}` on its record.
- **Entry zero** — the pre-list single-provider config (`inference/config` +
  `inference/key`), shown as the first row and never migrated.
- **Legacy Managed row** — the LLM-page row that renders when the managed
  fallback chain resolves (account key or instance identity) and no
  `tinyhumans` row exists.
- **Legacy path** — resolution for a company with no full default and no agent
  pair: entry zero, manifest `[inference]`, env default, managed chain, routes
  (until 5b).
- **Copy-if-empty** — the only carry-over shape used: write a new address only
  when it is empty; never clear or overwrite. The store has no list, rename,
  delete or transaction.

## Rules for the implementer

- Rust: `cargo fmt --all -- --check` locally only. Clippy and tests run on CI;
  verify by head SHA (`gh api repos/tinyhumansai/opencompany/commits/<sha>/check-runs`),
  zero failures **and** zero pending.
- Frontend: three separate typecheck gates (`npm run typecheck`,
  `typecheck:unit`, `typecheck:e2e`); `scripts/ci/assert-design-tokens.sh`
  rejects raw hex.
- A UI change is verified only in a real browser, light and dark, screenshot.
- Never a real credential on disk. Tests use `th-not-a-real-key`,
  `ak-not-a-real-key`, `sk-not-a-real-key`.
- Never assume what any provider's catalog contains. Any id returned by
  `GET /agent-integrations/openrouter/models` is valid and is what the operator
  picks; code never hardcodes, filters, prefers or rejects model ids by vendor
  or name. It lists what the endpoint returns (paged: `limit` up to 500, follow
  `total`) and sends what was chosen. Examples and tests use obviously fake ids
  (`acme/test-model`, `acme/other-model`, `test-model-small`).
- Every Markdown file ≤ 500 lines. Absolute dates.
