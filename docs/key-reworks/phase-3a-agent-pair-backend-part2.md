# Phase 3a — agent pair backend (part 2)

Continues [phase-3a-agent-pair-backend.md](phase-3a-agent-pair-backend.md).
Same revision (`upstream/main @ fcfb3e1bc`, 2026-09-14) and the same rule:
re-find every line by its excerpt after slices 2a–2d.

## 4.6 Boot (`builder.rs:3087-3104`)

After `configured` is computed:

```rust
                        let configured = configured
                            || any_agent_pair_resolves(
                                &id,
                                &self.manifest,
                                &overlay_agents,
                                &overlay_agent_edits,
                                secrets.as_ref(),
                            )
                            .await;
```

`overlay_agents` / `overlay_agent_edits` are bound at `:2702`. New private fns in `builder.rs`:

- Pure helper `agent_pairs(manifest, edits, overlays) -> Vec<(String, ModelChoice)>`:
  for each `manifest.agents` entry, start from `(provider, model)`, apply the
  edit row for that id with the §4.1 merge rule (`Some("")` clears); skip agents
  whose `manifest.harness_for(id)` is `acp`; then add every `OverlayAgent` with
  both fields non-empty whose bound harness is not `acp`.
- `any_agent_pair_resolves`: for each pair, `store::get_provider`; `Some(row)`
  with `row.enabled` → return `true`. For a missing or disabled one, log
  `tracing::warn!(company = %id, agent = %agent_id, provider = %slug, "agent pair names a provider this company does not have or has switched off; that agent's turns will fail until it is fixed")`.
  A `get_provider` error is a `warn!` and counts as not resolving. Never fails boot.

## 5. Ordered edit list

Commit after step 6 (record), step 9 (validation + route) and step 15
(resolver + boot), each only after `cargo fmt --all -- --check` is clean. Push
each commit.

1. `src/company/types.rs`: add `Agent.provider`; rewrite the `model` doc (§4.1).
2. `src/company/agent_file.rs`: `AgentFile.provider` and `provider: file.provider`.
3. `src/ports/types.rs`: `OverlayAgent.provider`, `AgentOverride.provider`,
   upsert copy, merge, `retain_nonempty_agent_edits`.
4. `git grep -nE "\b(Agent|ManifestAgent|OverlayAgent) \{$" -- src tests`; add
   `provider: None` to every literal that lacks `..`.
5. `src/harness/built_in/mod.rs`: `overlay_agent_to_manifest` carries
   `provider`; three fingerprint hashes; `is_avatar_only` (`:4957-4965`) gains
   `&& edit.provider.is_none()` (G3).
6. `src/store/conformance.rs`: `sample_overlay_agents[0].provider = Some("anthropic")`
   (the other two `None`); `sample_agent_overrides[0].provider = Some("anthropic")`.
7. `src/company/manifest.rs`: the §4.2 loop; update the existing test (§7).
8. `src/server/ops/team_agent.rs`: §4.3 steps 1–9.
9. `src/harness/lanes.rs:215-219`: rewrite the `agent_models_on` doc sentence
   "`CompanyManifest::validate` already confirmed every override sits on an
   `acp`-bound agent" to: "a `built_in` agent's `model` is its pair's model and
   never reaches this map, because the map is built only for `acp` harness ids
   and an agent enters it only when bound to that id". **No code change.**
10. `src/harness/built_in/provider.rs`: `HarnessModel::pinned(agent_id, choice)` default.
11. `AgentPin { agent_id, choice }`; `TenantProvider.pin: Option<AgentPin>`, `pin: None` in `new`.
12. `impl HarnessModel for TenantProvider { fn pinned … }`.
13. `TenantProvider::resolve`: the agent-naming pin check, then pass
    `self.pin.as_ref().map(|p| p.choice.clone())` to `resolve_for_turn` (§4.4).
14. `src/harness/built_in/build.rs`: `pin` + `chat_model`, calling
    `deps.provider.pinned(&manifest_agent.id, choice)` (§4.5).
15. `src/runtime/builder.rs`: `agent_pairs`, `any_agent_pair_resolves`,
    `configured` (§4.6).
16. Tests (§7), then docs: `docs/spec/runtime/agents.md` and
    `docs/spec/runtime/manifest.md` gain the `provider` key with the §6 example;
    keep each file ≤ 500 lines.

## 6. Data carry-over

**None.** An absent `provider` means no pair; every stored record and manifest
deserializes exactly as today. No secret-store key is added or written. Nothing
runs at boot except the read-only `configured` check.

`companies/<name>/company.toml` before:

```toml
[[agent]]
id = "researcher"
role = "Researcher"

[[agent]]
id = "web_search"
role = "Web search"

[[agent]]
id = "writer"
role = "Writer"
```

After (researcher and web-search pinned; writer unchanged → company default):

```toml
[[agent]]
id = "researcher"
role = "Researcher"
provider = "anthropic"
model = "test-model-large"

[[agent]]
id = "web_search"
role = "Web search"
provider = "anthropic"
model = "test-model-small"

[[agent]]
id = "writer"
role = "Writer"
```

`CompanyRecord.overlay_agents` entry (a console-added teammate), after a pin:

```json
{"id": "sam_reyes", "name": "Sam Reyes", "role": "Web search",
 "provider": "anthropic", "model": "test-model-small"}
```

`CompanyRecord.overlay_agent_edits` row for the manifest researcher — pinned,
then cleared (`Some("")` = cleared, kept distinct from never-edited):

```json
{"agent_id": "researcher", "provider": "anthropic", "model": "test-model-large"}
{"agent_id": "researcher", "provider": "", "model": ""}
```

(Field spelling follows the existing `model` / `harness` keys on the same
structs; the stores serialize the structs whole.)

## 7. Tests

All Rust tests use fake credentials only: `sk-not-a-real-key`.

**`src/company/agent_file.rs`** (beside `:511`)
- `a_per_file_teammate_carries_its_provider` — parse
  `role = "Researcher"\nprovider = "anthropic"\nmodel = "test-model-large"\n`;
  assert both fields on the `Agent`.

**`src/ports/types.rs`** (beside `:8760`, `:8901`, `:8953`)
- Extend `agent_override_is_empty_only_when_nothing_is_set`: a row with only
  `provider: Some("")` survives `retain_nonempty_agent_edits`.
- `an_override_carries_and_clears_the_provider` — `upsert_agent_override` with
  `provider: Some("anthropic"), model: Some("test-model-large")`; assert
  `effective_agent("ceo")` has both; upsert `provider: Some(""), model: Some("")`;
  assert both `None`; assert an upsert carrying only `name` leaves `provider` alone.
- `an_overlay_agent_round_trips_its_provider` — `serde_json` round trip of an
  `OverlayAgent` with `provider`; and a JSON without the key deserializes to `None`
  and re-serializes with no `provider` key.

**`src/store/conformance.rs`** — the extended samples (step 6). Every backend's
conformance run then proves the field round-trips.

**`src/harness/built_in/mod.rs`**
- `a_provider_edit_moves_the_overlay_and_override_fingerprints` — two edit sets
  identical except `provider`; assert `overlay_fingerprint` and
  `override_fingerprint` differ; same for two `OverlayAgent` lists.
- `overlay_agent_to_manifest_carries_the_provider`.

**`src/company/manifest.rs`** (beside `:3053`)
- Update `agent_model_follows_the_harness_level_models_own_doctrine`: the
  `built_in` case now expects `"names a `model` but no `provider`"`.
- `a_built_in_agent_may_pin_provider_and_model_together` — `kind = "built_in"`,
  `provider = "anthropic"`, `model = "test-model-large"`; no problem mentions
  `provider` or `model`.
- `a_built_in_agent_with_model_but_no_provider_is_refused`.
- `a_built_in_agent_with_provider_but_no_model_is_refused`.
- `an_acp_agent_may_not_name_a_provider` — local ACP harness, `provider` set;
  expect "which brings its own provider".
- `a_provider_slug_that_is_not_a_slug_is_refused` — `provider = "Anthropic API"`.
- `a_pair_on_a_manifest_with_no_harness_section_is_validated` — no `[[harness]]`,
  `provider` without `model`; expect the refusal (G2).

**`src/server/ops/team_agent.rs`** (`mod tests` `:2193`; helpers
`state_with_manifest` `:2368`, `patch_agent` `:2452`, `add_overlay` `:2496`,
`ROSTER` `:2211` is all built-in). Seed providers with `store::put_provider`
(`store.rs:531`) on the state's secret store — one enabled `anthropic`, one
disabled `groq`.
- `pinning_a_built_in_agent_needs_an_existing_enabled_provider` — PATCH writer
  `{"provider":"nope","model":"m"}` → 400 naming `nope`; `{"provider":"groq","model":"m"}`
  → 400 "switched off"; `{"provider":"anthropic"}` alone → 400 "Choose a model".
- `pinning_saves_both_and_rebuilds` — PATCH writer
  `{"provider":"anthropic","model":"test-model-small"}` → 200, body has both;
  GET re-reads both; `overlay_fingerprint_of` (`mod.rs:3666`) changed; do the same
  for an overlay teammate from `add_overlay`.
- `clearing_the_pin_returns_to_the_default` — PATCH `{"provider":null,"model":null}`
  → 200, both absent in the DTO; the stored override holds `Some("")` for both.
- `a_member_cannot_pin` — `send_as` with `member_cookie`, body `{"provider":"anthropic","model":"x"}` → 403.
- `an_acp_agent_rejects_a_provider` — `ACP_ROSTER`, overlay teammate on `laptop`,
  `{"provider":"anthropic","model":"x"}` → 400 "ACP harness".
- `a_name_edit_does_not_revalidate_a_disabled_pin` — pin, then disable the
  provider, then PATCH `{"name":"x"}` → 200.
- `the_agent_detail_editable_list_offers_provider_to_an_admin_only`.
- Existing `a_model_override_is_refused_off_an_acp_harness` (`:3949`): update
  the expected text to "names a model but no provider".

**`src/company/inference.rs`** (resolver). Slice 2b already has
`a_pin_outranks_a_full_default` and `a_pin_naming_a_gone_provider_fails_closed_without_falling_back`;
do not duplicate them. Add:
- `a_pin_outranks_a_company_with_no_default`.
- `a_pin_outranks_a_named_harness_inference` — `HarnessScope::named("side").declaring_own_inference(true)`; the pin still wins (Q5).
- `a_pin_naming_a_disabled_provider_fails_closed`.
- `a_pin_naming_entry_zero_keeps_the_legacy_chain_with_its_model`.

**`src/harness/built_in/provider.rs`** (loopback pattern at `:4134`)
- `two_agents_pinned_to_two_providers_reach_two_endpoints` — two
  `TcpListener::bind("127.0.0.1:0")` mocks that record the request body and
  `Authorization` header and answer a minimal chat completion. `put_provider` two
  rows, kind `openai_compatible`, slugs `anthropic` and `research-box`, base URLs
  `http://127.0.0.1:<port>/v1`, both keys `sk-not-a-real-key` (written with the
  store's provider-key setter that `load_provider_key`, `store.rs:729`, reads).
  `let base = TenantProvider::new(company, secrets, Inference::default(), None);`
  `let a = base.pinned("researcher", &choice("anthropic","test-model-large")).unwrap();`
  `let b = base.pinned("web_search", &choice("research-box","test-model-small")).unwrap();`
  Invoke each once. Assert mock A saw only `"model":"test-model-large"`, mock B
  only `"test-model-small"`, both with `Bearer sk-not-a-real-key`, and that `base`
  itself (no default) errors "choose a model".
- `a_pinned_turn_naming_a_gone_provider_names_the_agent` — no `anthropic` row, a
  full default present; `base.pinned("researcher", &choice("anthropic","test-model-large"))`
  invoked → `Err` whose text contains ``agent `researcher` is set to `anthropic`, which this company does not have.``
  and `Team → researcher → Model`; the default's mock receives no request.
- `a_pinned_turn_naming_a_switched_off_provider_names_the_agent` — `anthropic`
  row with `enabled: false` → text contains ``agent `researcher` is set to`` and `which is switched off`.
- `the_default_pinned_is_none_for_a_double` — a test `HarnessModel` returns `None`.

**`src/runtime/builder.rs`** (`#[cfg(test)]` `:4638`)
- `agent_pairs_apply_edits_and_skip_acp_agents` — pure helper: manifest pair,
  cleared by `Some("")` edit; overlay pair; acp-bound agent ignored.
- `a_company_whose_only_inference_is_a_pin_boots_the_harness_brain` — manifest
  `[[agent]] id="researcher" provider="anthropic" model="test-model-large"`, no
  `[inference]`, no env default; seed the `anthropic` row into the secret store
  the builder opens (copy how existing builder tests seed inference); build;
  assert `runtime.cognition().path != "echo"` (the echo check is at
  `src/server/ops/inference.rs:4247`).
- `a_pin_naming_a_missing_provider_keeps_the_echo_brain` — same without the row → `"echo"`.

**`tests/auth_matrix.rs`** — no route added; the snapshot must not move.

## 8. Console / UI

None in this slice. The DTO gains `provider`; the console ignores unknown keys
until slice 3b. Error strings in §4.3 are shown verbatim by the 3b editor, so keep
them sentence-cased and ending with a full stop.

## 9. Must not touch

- The ACP model path: `LocalAcpAgent`'s model map (`lanes.rs:90-114`,
  `agent_models_on` `:220`) — comment only (step 9).
- `[harness.inference]` scoping: `HarnessScope`, `harness_configures_itself`
  (`inference.rs:1367-1381`), `built_in_lane` (`lanes.rs:405-445`).
- Internal passes (title, triage, planning, selector, confine, payload extract,
  workflow build, judge, profile draft) keep `deps.provider` → default (Q13).
- Routes: no new route, no auth change; `PATCH …/team/{agent_id}` only gains a field.
- `EDITABLE_FIELDS_MEMBER`, `Agent.tier` (still never selects a model).
- The inference page, `inference/default`, `inference/providers` writers.

## 10. Done when

- `cargo fmt --all -- --check` clean locally.
- Pushed; CI verified by head SHA:
  `gh api repos/tinyhumansai/opencompany/commits/<sha>/check-runs` shows zero
  failures **and** zero pending, including `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, the gated `Rust (openhuman, tinycortex)` lane, and both Console
  E2E lanes (which boot `companies/e2e_harness`, whose agents set no pair).
- Every §7 test exists and ran (name visible in the CI log).
- No browser check in this slice (no UI); 3b owns screenshots.

## 11. Gotchas

- **G1 — typo fails at the first turn, not at boot.** The manifest cannot see
  console provider slugs (F5), so `provider = "antropic"` loads clean. The
  builder logs a warning naming the agent (§4.6); the turn errors naming the
  agent and the slug. There is no fallback to the default (F6).
- **G2 — `harness_for` with no `[[harness]]`.** `None => {}` skips "unknown
  harness". Confirm what `harness_for` returns on a manifest that declares no
  harness (e.g. `companies/e2e_harness`). If it is `None`, treat "no harness
  declared and none named" as the implicit `built_in` default, or the pair rule
  never runs on the most common manifest.
- **G3 — `is_avatar_only`.** `overlay_fingerprint` skips avatar-only edits
  (`mod.rs:5005-5007`), and the predicate (`:4957-4965`) enumerates every field
  except `avatar`. Without `&& edit.provider.is_none()` a provider-only edit
  reads as avatar-only and never rebuilds. The fingerprint test in §7 covers it.
- **G4 — `Some("")` means cleared** in `AgentOverride`; `None` means never
  edited. Hash the `Option`, never the filtered value.
- **G5 — the rebuild fingerprint must include `provider`**, or the PATCH saves
  and nothing changes until restart. `routing_changed` must include it too.
- **G6 — no catalog assumptions.** A pair is `{provider, model}` exactly as
  chosen; "researcher on `anthropic` / `test-model-large`" uses the `anthropic`
  row and its key. Never filter, prefer or reject a model id by vendor or name,
  and do not special-case `tinyhumans`.
- **G7 — a deleted or disabled provider** named by a pair errors on that agent's
  turn, with no fallback. `TenantProvider::resolve` checks the `AgentPin` first,
  so the message names the agent (use case 4); the boot warning (§4.6) names it
  too. `resolve_for_turn` sees only the `ModelChoice` and is not changed.
  "Team → … → Model" uses the agent **id** (`researcher`), because `pinned`
  carries the id and not the display role; phase-3.md use case 4 writes the role
  (`Researcher`). Carrying the role would need one more `pinned` argument.
- **G8 — telemetry.** `HarnessPool::run` reads `telemetry_provider_id` /
  `telemetry_model` live. Find which `Arc` it reads (`git grep -n
  "telemetry_provider_id()" src/harness`). If it is `deps.provider` rather than
  the agent's chat model, a pinned agent's usage is attributed to the default's
  slug; fix by reading the agent's own model, and add a test.
- **G9 — kind switches.** Switching a pinned built-in agent to an ACP harness
  must send `provider: null` (and `model: null` unless an ACP model is chosen);
  switching an ACP agent with a model to built-in must send a pair or
  `model: null`. The route refuses otherwise — 3b's editor sends these.
- **G10 — env model override.** `OPENCOMPANY_INFERENCE_MODEL` never overrides a
  pin: `chosen_model` wins in `request_plan` (2b).
