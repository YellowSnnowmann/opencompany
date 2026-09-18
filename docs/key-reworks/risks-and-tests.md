# Risks and the tests that prove each is handled

Part of [the keys rework](README.md). Every row names the risk, the slice that
handles it, and the test that proves it. A risk with no proving test is a stop
point, not a known gap. Test names are the ones the slice files specify; where a
slice spells a name differently, the slice file wins — update this table in the
same commit.

Code references are on `upstream/main @ fcfb3e1bc` (2026-09-14).

## How verification works on this branch

- `cargo fmt --all -- --check` runs locally. Clippy, `cargo test`, the gated
  `Rust (openhuman, tinymemory)` job (name per `.github/workflows/ci.yml`) and
  both Console E2E lanes run on CI only (the machine is shared).
- CI is verified **by head SHA**:
  `gh api repos/tinyhumansai/opencompany/commits/<sha>/check-runs`. Require
  zero failures and zero pending. The Console E2E lanes spawn late, so an early
  all-green reading is not a result. A PR that is `CONFLICTING` runs no CI.
- The frontend has three separate typecheck gates: `npm run typecheck`,
  `npm run typecheck:unit`, `npm run typecheck:e2e`. Name which ran.
- A UI slice is verified only in a real browser, light and dark, with a
  screenshot, at its real cap (a large catalog served by a mock, e.g. 500 ids,
  the 15-agent roster), against a host started on a claimed port with a fresh
  data dir and fake keys only.

---

## 1. Hosted tenants

| Risk | Handled by | Proof |
|---|---|---|
| A hosted company with no default loses its brain in 2b/2c | New arms are added in front; the legacy source is unchanged | 2b `a_hosted_company_with_no_default_resolves_exactly_as_before` |
| After 2d a hosted company with no default and no injected model fails every turn | D-legacy: `OPENCOMPANY_INFERENCE_MODEL` is the no-default model; manager-side deploy gate ([not-handled.md](not-handled.md)); clear error, never echo, never a tier name | 2d test: env default + `OPENCOMPANY_INFERENCE_MODEL=acme/test-model` sends that id; env default without it fails with the "choose a model" sentence |
| An injected `/openai/v1` URL is silently rewritten | D-proxy: used as given | 2a test: an env default URL is sent to exactly as injected |
| Hosted Composio stops working | 1a keeps managed resolution falling through to `company_key::resolve`; item 10 not handled | 1a blank-new-and-legacy fall-through test; the unchanged harness Composio chain tests |
| `TINYHUMANS_TOKEN_FILE`, `OPENCOMPANY_INFERENCE_KEY` reads removed; a hosted tenant with no `provider/tinyhumans/key` loses managed inference/embeddings/search | 6a (operator-accepted risk, 2026-09-15); the fallback chains themselves (item 10) stay not handled | 6a done-when list; the risk is written out in the PR description and in `phase-6a-remove-env-vars.md` §"the hosted-tenant risk, stated plainly" |

## 2. E2E hosts

| Risk | Handled by | Proof |
|---|---|---|
| Fixture hosts (`frontend/playwright.config.ts:214-226`, `OPENCOMPANY_INFERENCE_URL=http://…/v1`) stop answering once tiers are not sent | 2d adds `OPENCOMPANY_INFERENCE_MODEL` to both fixture hosts with an id the mock accepts | Both Console E2E lanes green by head SHA on the 2d commit |
| The same fixture hosts (and the Composio fixture at `:242`) stop being reachable at all once 6a deletes the `OPENCOMPANY_INFERENCE_URL` / `_KEY` / `OPENCOMPANY_COMPOSIO_BACKEND_URL` reads | 6a switches every fixture host from an env-injected URL to a stored provider row (LLM) and a stored `TINYHUMANS_API_URL`-reachable mock (Composio) — see `phase-6a-remove-env-vars.md` §"E2E hosts" | Both Console E2E lanes, and the Composio e2e specs, green by head SHA on the 6a commit |
| `OPENCOMPANY_INFERENCE_MODEL` beats a chosen model | `chosen_model` wins in `request_plan` | 2b `a_full_default_sends_its_model_whatever_the_tier` (with `model_override = "stub-model"`) |
| E2E specs that call `PUT …/composio/token` break | 1a keeps route paths and bodies | The three Composio e2e specs pass unchanged |

## 3. TinyHumans on the proxy (2a)

| Risk | Handled by | Proof |
|---|---|---|
| Two TinyHumans rows (a `tinyhumans` row plus the legacy Managed row) | One-row rule in status and console | 2a "fresh company, add TinyHumans → exactly one row" (host status test, unit test, e2e) |
| Adding TinyHumans over a legacy managed `inference/config` duplicates or writes | Refused before any write | 2a add-over-legacy-managed-config refusal test |
| The model step is skipped and health stays `unchecked` | Probe classifies the catalog as needs-a-model; probe result recorded | 2a probe classification + health write tests; 2c e2e add flow |
| The envelope is misread as zero models | Parser errors on `success:false` or an OpenAI-shaped body; pages to `total` | 2a `paged_catalog` tests (envelope, paging, cap, dedupe, unknown fields) |
| A 503 from the catalog reads as "no models" | Failure memo kept | 2a 503-remembered, 401/403-not-memoized tests |
| Extra response keys (`openhuman`, `service_tier`, `usage.cost`) or an `openhuman`-only final stream frame break a turn | Parser ignores unknown keys; stream tolerance if streaming is used | 2a parser tests |
| A new key is invented for the Managed model | D-set; do-not-do list | Section 11 review check |

## 4. Entry zero

| Risk | Handled by | Proof |
|---|---|---|
| A default naming entry zero changes behaviour | `decl_for_choice(EntryZero)` resolves through the legacy scoped chain with the chosen model | 2b `a_full_default_naming_entry_zero_keeps_the_legacy_chain_with_the_chosen_model` |
| Setting a default rewrites `inference/config` | 2c writes only `inference/default` and the row | 2c store test: `inference/config` byte-identical after set-default |
| The account-key flow keeps creating entry zero | 4a: `finish_link` stops writing `inference/config` | 4a `finish_link_no_longer_writes_inference_config` |
| Search entry zero loses its SearXNG address | 1b keeps the per-slug and flat fallbacks, merges on update | 1b omitted-endpoint-survives test |

## 5. Existing routes

| Risk | Handled by | Proof |
|---|---|---|
| A partial table (e.g. only `reasoning-v1`) moves all spend to one provider | F7: all four tier rows present and equal | 5a `a_single_row_is_not_copied`, `mixed_rows_are_not_copied` |
| An existing default is overwritten | Copy-if-empty: only `Unset` qualifies | 5a `a_full_or_bare_slug_default_is_never_overwritten` |
| A company routed only to Managed lands on echo | 5a banner + "choose a model" error; `managed` routes carry no model and are not copied | 5a `managed_rows_carry_no_model_and_are_not_copied`; 5b replacement for `a_company_routed_to_managed_resolves_rather_than_landing_on_echo` |
| Carry-over write fails and boot dies | Errors logged, never fatal | 5a `a_failing_write_leaves_boot_running` |
| Routing removed before carry-over | Order: 5a pushed and green before 5b | 5b `a_stored_routes_blob_changes_nothing_about_a_turn` |
| Console renders a Routing tab against a 404 | 5b removes backend and console together | 5b `the_routes_api_is_gone`; e2e `the inference page has no routing tab` |

## 6. Key rotation and the account-key fan-out (Q6, Q7)

| Risk | Handled by | Proof |
|---|---|---|
| A key pasted on the LLM or Composio page is overwritten | Q7: write only when empty or equal to the old account key | 4a state-matrix test (custom value kept per slot) |
| Rotating the account key strands copies on the old value (#2266) | Q7 equal-to-old rule | 4a rotation test |
| Clearing the account key leaves copies of a revoked key | Q7 clear rule | 4a clear test |
| Two admins save at once | Per-company lock | 4a concurrent-saves test |
| A rejected key leaves a default pointing at a dead provider | Q6 rollback of what this request wrote | 4a auth-failure rollback test |
| A failed write mid-plan half-applies | Ordered writes; each failure reported | 4a failing-writer test per step |
| A key appears in a response, log or journal | Values compared in memory only | 4a no-key-anywhere test |
| Composio legacy address read crosswise | 1a fixed mapping | 1a `a_byok_value_is_never_presented_as_the_tinyhumans_bearer`, `a_tinyhumans_value_is_never_presented_as_the_byok_key` |
| Rolling back past 1a loses a Composio key saved on the new binary | D-mirror | 1a `a_write_mirrors_to_the_legacy_address_for_one_release` |

## 7. Composio scope

| Risk | Handled by | Proof |
|---|---|---|
| A hand-minted account key copied into `composio/tinyhumans/key` is refused by Composio (`connections` only on grant-minted or attested keys) | 4a note (or skip, decided in 4a with evidence) | 4a Composio-slot note test |
| The account key lands in the BYOK slot | 4a never touches `composio/byok/key` or `composio/mode` | 4a matrix asserts both unchanged |
| The key-grant path is deleted with the dead UI | Q10: 1c removes only UI; `/credential/link/*` and `use-redeem-key-grant.ts` stay | 1c done-when: `useRedeemKeyGrant` still mounted by `ApiKeyView.tsx`; link rows in the auth matrix unchanged |

## 8. Models, vision and internal passes

| Risk | Handled by | Proof |
|---|---|---|
| A tier name reaches the wire on any path | D-no-tier | 2d tests: new path, legacy entry zero, manifest map, env default, internal passes, setup brain — each asserts the wire model is not in `INFERENCE_TIERS` |
| Internal passes send `chat-v1` | `chosen_model` / legacy real id; never a tier | 2d `internal_passes_on_a_full_default_send_the_default_model` |
| A guessed default id (`DEFAULT_TIER_MODELS`) is sent | 2d removes the guessed substitution | 2d test: a keyless-model provider with no chosen model fails closed |
| A tier name is saved as a model | `check_model_id` refuses `INFERENCE_TIERS` names | 2c `a_default_model_may_not_be_a_tier_name_or_contain_spaces` |
| A provider is saved with no model | 2c refuses before any write | 2c `an_add_without_a_model_is_refused_before_anything_is_written` |
| A row holding different ids per tier is silently collapsed | `ModelOnRow::Ambiguous`, never picked | 2b `a_row_with_two_distinct_models_is_ambiguous_never_picked` |
| A vision turn on a text-only default | Not rerouted; the provider's refusal surfaces naming the default | 2d vision refusal test |
| The TinyHumans picker drops, filters or invents ids | Picker lists exactly the catalog ids read host-side, all pages to `total`; no vendor or name filter | 2c paged-catalog unit test; browser check of a large catalog served by a mock (e.g. 500 ids) |

## 9. Agent pairs

| Risk | Handled by | Proof |
|---|---|---|
| A pair names a deleted or disabled provider and silently moves spend | F6 | 3a `a_pin_naming_a_gone_provider_fails_closed_without_falling_back` |
| A company whose only inference is a pair boots echo | `configured` counts pairs | 3a `a_company_whose_only_inference_is_a_pin_boots_the_harness_brain` |
| Editing a pair does not rebuild | Fingerprint hashes `provider` | 3a `pinning_saves_both_and_rebuilds` |
| A member pins an agent | Admin gate | 3a `a_member_cannot_pin` |
| Two agents on two providers reach the wrong endpoint | Per-agent `TenantProvider` sibling | 3a `two_agents_pinned_to_two_providers_reach_two_endpoints` |
| The ACP model path changes | `provider` refused on ACP | 3a `an_acp_agent_rejects_a_provider` |

## 10. Search endpoint (1b)

| Risk | Handled by | Proof |
|---|---|---|
| A toggle or key replace wipes the SearXNG address | Merge on update | 1b toggle/select/second-connect test |
| Rolling back loses the address | D-mirror | 1b index-and-per-slug write test |
| An old index blob without `endpoint` fails to parse | `#[serde(default)]` | 1b old-blob parse test |

## 11. Store invariants (every slice)

| Invariant | Proof |
|---|---|
| No new secret-store key | Review: `git diff upstream/main -- src | grep -n '^+.*: &str = "'` lists only `composio/tinyhumans/key` and `composio/byok/key` (the rename targets) and no `inference/…` key |
| Nothing renames or deletes a stored value | Every carry-over is copy-if-empty or mirror-on-write; each slice's carry-over tests |
| No write on a read path | Status and resolver tests use a store double that fails on any write |
| The auth matrix moves only where a slice says so | `tests/snapshots/auth-matrix.txt` diff limited to 4c's new routes and 5b's removed routes |
