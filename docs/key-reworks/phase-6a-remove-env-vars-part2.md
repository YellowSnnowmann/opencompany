# Phase 6a — remove four environment variables (part 2 of 2)

Parts: [1](phase-6a-remove-env-vars.md) · [2](phase-6a-remove-env-vars-part2.md).
Continues part 1; section numbers carry on from it. Code read at
`upstream/main @ fcfb3e1bc` (2026-09-14).

## 6. Target code: `RuntimeConfig`, doctor, `AppConfig`

**`src/app/config.rs`.** Delete the `tinyhumans_token_file` field
(:715-719) and its `Debug` line (:779). Its doc said this was "when the
platform hands this instance a rotating, audience-bound identity instead of a
static key" — after 6a that identity no longer exists on this instance, full
stop, so there is nothing to hold.

`credential_source` (:745-758) simplifies from a delegated match to a direct
one:

```rust
/// Whether a static TinyHumans credential is configured. The projected-file
/// tier was removed in phase 6a (issue #2306): a hosted tenant now needs a
/// company-level `provider/tinyhumans/key` (2a) instead of an instance-wide
/// identity.
pub fn credential_source(&self) -> crate::company::CredentialSource {
    if self.tinyhumans_credential.is_some() {
        crate::company::CredentialSource::Static
    } else {
        crate::company::CredentialSource::None
    }
}
```

`credential_available` (:731-733, :741-743) is unchanged in shape — it still
calls `credential_source() != CredentialSource::None` — only the private
plumbing under it changes.

In `resolve()` (:930-982), delete the `tinyhumans_token_file` build
(:930-938) entirely, and delete `tinyhumans_token_file` from the `RuntimeConfig { … }`
literal (:977).

**`src/app/doctor.rs`.** Delete `"tinyhumans_token_file"` from `FIELDS`
(:106) and its arm in `value_of` (:87-91). In `report`'s
`CREDENTIAL_SOURCE_FIELD` layer match, delete the
`CredentialSource::Attested => prov.layer("tinyhumans_token_file")` arm
(:132) — if part 1 §5 kept `Attested` typed-but-unreachable, fold it into the
`None` arm instead of deleting the match case, so the match stays exhaustive
without a wildcard:

```rust
crate::company::CredentialSource::Attested | crate::company::CredentialSource::Company => None,
crate::company::CredentialSource::Static => prov.layer("tinyhumans_credential"),
crate::company::CredentialSource::None => None,
```

The `cycles` capability's `needs` text (:150-155) currently names both env
vars:

```rust
format!("needs {} (hosted) or {}", crate::company::credentials::TOKEN_FILE_ENV, crate::company::credentials::API_KEY_ENV)
```

becomes:

```rust
format!("needs {}", crate::company::credentials::API_KEY_ENV)
```

**Tests to delete** (`src/app/doctor.rs` test module, :193-371): both
`TOKEN_FILE_ENV`-based tests — `projected_token_file_reports_the_attested_tier`
and `a_token_file_that_does_not_exist_does_not_report_attested` — since there
is no path-existence check left to test. In `cycles_unavailable_without_credential`
(:220-236), delete the `cycles.needs.contains("TINYHUMANS_TOKEN_FILE")` half
of the assertion and the `value(&report, "tinyhumans_token_file")` line.

**`src/app/config.rs` test module** (around :1490-1600): delete every test
that sets `TOKEN_FILE_ENV` and asserts `tinyhumans_token_file` or
`CredentialSource::Attested` — `git grep -n "TOKEN_FILE_ENV\|tinyhumans_token_file" src/app/config.rs`
after the edits above must show only production call sites this section did
not delete (there should be none left; an empty result is correct).

**`src/app/types.rs`.** `credential_available_in` (:255-257) and
`credential_source_in` (:261-267) both call `TinyhumansTokenSource::from_env(env)`,
whose signature does not change (part 1 §5 keeps `from_env(env: &dyn EnvSource)`).
Only the body they delegate to changed, so **no edit is needed here** — verify
with `cargo build` rather than assuming; if `credential_source_in`'s
`None if self.tinyhumans_credential.is_some() => CredentialSource::Static`
arm becomes unreachable because `TinyhumansTokenSource::source_of_parts` no
longer takes a `has_static_credential` bool the same way, adjust the call to
match part 1 §5's simplified signature.

## 7. Target code: `OPENCOMPANY_COMPOSIO_BACKEND_URL`

**`src/company/composio.rs`.** Delete `COMPOSIO_BACKEND_URL_ENV` (:32) and its
doc (:28-32). `backend_url_or_default` (:54-61) drops its `env_override`
parameter:

```rust
/// The effective Composio backend URL: [`TINYHUMANS_API_URL_ENV`] (the
/// tenant's shared backend base) if set, else [`DEFAULT_BACKEND_URL`]. The
/// explicit per-surface override (`OPENCOMPANY_COMPOSIO_BACKEND_URL`) was
/// removed in phase 6a (issue #2306): Composio now always follows the
/// tenant's shared API base, the same way media and search already do.
pub fn backend_url_or_default(api_url: Option<String>) -> String {
    api_url
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_BACKEND_URL.to_string())
}
```

**Every call site drops its first argument** (`git grep -n "backend_url_or_default(" upstream/main -- . ':!vendor'`
on `fcfb3e1bc` lists them all):

| Call site | Before | After |
|---|---|---|
| `src/harness/built_in/composio.rs:371` | `backend_url_or_default(backend_url_env, api_url_env)` | `backend_url_or_default(api_url_env)`; delete the `let url = env.get(composio::COMPOSIO_BACKEND_URL_ENV);` above it (`harness/built_in/mod.rs:3345`, propagated in at `composio.rs:307-311`) |
| `src/harness/built_in/mod.rs:3345` (`resolve_composio`) | reads `COMPOSIO_BACKEND_URL_ENV` then passes `url` in | delete the read; drop the argument at the call |
| `src/runtime/builder.rs:3256` | same pattern | same |
| `src/server/ops/composio.rs:209-211` (`effective_status`) | `backend_url_or_default(env.get(COMPOSIO_BACKEND_URL_ENV), api_url)` | `backend_url_or_default(api_url)` |
| `src/server/ops/composio.rs:590-621` (`access_for`) | same pattern | same |
| `src/server/ops/composio.rs:1080-1114` | same pattern | same |

**Tests.** `composio.rs:624-658` tests `backend_url_or_default` directly with
various `(env_override, api_url)` pairs — rewrite each to drop the first
argument; the ones that tested the override winning
(`backend_url_or_default(Some(x), Some(y)) == x`) are deleted outright, since
there is no override left to test. `ops/composio.rs:1742-1758`, `:3060-3063`,
`:3569-3571`, `:3606-3608` set `COMPOSIO_BACKEND_URL_ENV` on a test env to
point at a mock backend — **these must switch to setting
`TINYHUMANS_API_URL_ENV`** (`composio.rs:38`) instead, which is unaffected by
this slice and already exists for exactly this purpose (staging Composio
following staging). Re-run each test after the swap; the doc comment at
`ops/composio.rs:1742-1745` ("In composio builds that test repoints
`COMPOSIO_BACKEND_URL_ENV` at a mock backend") needs the same rename.

## 8. The E2E fixture replacement

**The problem** (part 1 §0): `frontend/playwright.config.ts:214-228` points
the host's managed-inference resolution at a local `mock-brain.mjs` or
`live-brain-proxy.mjs` fixture with `OPENCOMPANY_INFERENCE_KEY` /
`OPENCOMPANY_INFERENCE_URL`. Once both are gone, `hosted_endpoint_from_env`
can never resolve to a local address — it is always the real
`api.tinyhumans.ai` proxy or nothing.

**The replacement.** After 2a, a `tinyhumans` **provider row** (in
`inference/providers`, not the managed/env-default arm) stores its own
`base_url` and never touches any of the four removed variables — it resolves
`proxied = false` through `decl_for_indexed` (`inference.rs:1413-1443`), the
same as every other indexed provider. The fixture setup adds one, pointed at
the local mock/live-brain server, instead of setting environment variables:

```
POST /api/v1/company/inference/providers
{ "kind": "tinyhumans", "baseUrl": "http://127.0.0.1:<mock-brain-port>",
  "key": "mock-brain", "model": "acme/test-model" }
```

- The URL is the **whole fixture origin**, not the `/agent-integrations/openrouter`
  suffix a real TinyHumans row would use — `mock-brain.mjs` and
  `live-brain-proxy.mjs` serve at their own paths today; confirm on the
  branch whether they need a route added to mirror the proxy's `/models` and
  `/chat/completions` paths, or whether the add route's `baseUrl` can point
  straight at the fixture's existing paths unmodified.
- This must run once per spec (or per `beforeAll`) that today relies on
  `managesLiveLlm` / `managesFixtures` selecting the host env — not once
  globally, because `inference/providers` is per-company and E2E specs each
  provision their own company. Find every affected spec with
  `git grep -rl "managesLiveLlm\|managesFixtures\|LIVE_LLM\|MOCK_BRAIN" frontend/test/e2e`
  and add the provider-row POST (or a shared helper wrapping it) to each
  one's setup.
- `frontend/playwright.config.ts:214-228`'s `inferenceEnv` block, and the
  `OPENCOMPANY_INFERENCE_KEY` / `OPENCOMPANY_INFERENCE_URL` lines inside it,
  are deleted once every affected spec has been moved to the provider-row
  setup — not before, or those specs lose their credential entirely and fall
  back to the echo brain, which is a silent pass-with-wrong-behaviour, not a
  failure. Verify each spec would fail loudly (a snapshot mismatch, not a
  silent pass) before deleting the env block, by running it once against the
  echo brain deliberately and confirming it fails.
- This is real implementation work, not a mechanical rename, and it touches
  every spec relying on the fixture — size it as its own sub-task inside 6a's
  commit sequence (§9), and if it turns out larger than the rest of 6a
  combined, report that to the operator rather than shipping a partial
  migration that leaves some specs silently on the echo brain.

**Composio's fixture has no equivalent replacement.** Composio has no
per-company provider-row mechanism — its backend URL is instance-wide. §7's
switch to `TINYHUMANS_API_URL_ENV` is the only option that exists today, and
it is coarser: it repoints the **whole host's** TinyHumans API base, not just
Composio. This is safe for the Composio E2E fixture specifically because that
fixture already runs its own isolated host process per Playwright project
(`managesComposio`, `playwright.config.ts:231`) — nothing else on that host
needs the real `api.tinyhumans.ai` base during those specs. **Confirm this by
running the Composio E2E project after the swap and checking every assertion
that reads a TinyHumans-hosted URL** (hub links, billing) still passes; if one
does not, report it rather than special-casing around it.

## 9. Docs

| File | Edit |
|---|---|
| `docs/spec/runtime/config.md` | Delete the `TINYHUMANS_TOKEN_FILE` row from the precedence table (§"Where the credential comes from", :41-44) — the source becomes one tier, so collapse the sentence to "The runtime holds a single static credential, `TINYHUMANS_API_KEY`." Delete the `TINYHUMANS_TOKEN_FILE`, `OPENCOMPANY_INFERENCE_KEY` and `OPENCOMPANY_INFERENCE_URL` rows from the Reference table (:113, :121-122). Delete `TINYHUMANS_TOKEN_FILE` from the Precedence code block (:69). Rewrite the "Credential reality vs contract" and "Where the credential comes from" sections' hosted-tenant framing: they currently promise a hosted tenant "stores no secret at all" via the projected file — after 6a this is no longer true, and the doc must say so plainly, pointing at not-handled.md's `TINYHUMANS_TOKEN_FILE` entry (§10) for what a hosted tenant needs instead. |
| `docs/modules/inference/credentials.md` | Full rewrite of the precedence chain (env row, :19; the diagram at :85; the numbered chain at :137) — drop the "instance identity: `TINYHUMANS_TOKEN_FILE`, else `TINYHUMANS_API_KEY`" framing to "instance identity: `TINYHUMANS_API_KEY`". Drop `OPENCOMPANY_INFERENCE_URL` from :46. |
| `docs/modules/inference/current-state.md` | :25-26 — the same collapse |
| `docs/modules/openhuman/README.md` | :80-81 — the key/url table rows lose their env-var column entries; state the base URL is fixed at the constant |
| `docs/modules/composio/data-model.md` | :79 — drop the `OPENCOMPANY_COMPOSIO_BACKEND_URL` clause, keep the `TINYHUMANS_API_URL` fallback |
| `docs/gitbooks/developers/configuration.md` | :75-76 — delete both rows |
| `companies/hive_math_lab/{README.md,company.toml}`, `companies/retail_co/{README.md,company.toml}`, `companies/vending_machine_co/{README.md,company.toml}`, `companies/openhuman_demo/{README.md,company.toml}` | Every example command that sets `OPENCOMPANY_INFERENCE_KEY` or `OPENCOMPANY_INFERENCE_URL` (`git grep -n "OPENCOMPANY_INFERENCE_KEY\|OPENCOMPANY_INFERENCE_URL" companies`) needs rewriting to the new mechanism: a `provider/tinyhumans/key` row set up through the console or the provider API, not an environment variable. Rewrite each example command; do not just delete the line, or the example stops demonstrating anything. |
| `docs/plans/hivemind-handoff.md` | :57, :83 — same rewrite; this is a grading/dev doc, not a shipped feature, so confirm with the operator whether it still needs an equivalent before spending time on it |
| `examples/live_company_turn.rs` | :10, :73-74 — doc comment and the runtime error message both name `OPENCOMPANY_INFERENCE_KEY` / `_URL`; reword to name `TINYHUMANS_API_KEY` and the fixed proxy URL |
| `CLAUDE.md` | Not edited by the implementer (per repo convention — the manager-injection list is an operator/superproject concern); flag in the PR body that `opencompany-manager` no longer needs to inject `OPENCOMPANY_INFERENCE_KEY`, `OPENCOMPANY_INFERENCE_URL` or `OPENCOMPANY_COMPOSIO_BACKEND_URL` for any tenant, and that `TINYHUMANS_TOKEN_FILE` no longer does anything if it still injects it |

## 10. Tests summary

- **Delete:** every test listed in §5 (part 1), §6 and §7 above.
- **Add:** none beyond what those sections already specify (a rewritten
  `backend_url_or_default` test per remaining branch in §7's table; a
  `hosted_endpoint_from_env`/`harness_inference_from_env` test asserting the
  base URL is always `DEFAULT_TINYHUMANS_INFERENCE_URL` regardless of any env
  var set, and the credential is `None` with no `TINYHUMANS_API_KEY`).
- **New:** `a_key_env_var_no_longer_overrides_the_hosted_url_or_credential` —
  set `OPENCOMPANY_INFERENCE_KEY`, `OPENCOMPANY_INFERENCE_URL` and
  `TINYHUMANS_TOKEN_FILE` (pointing at a real, valid file) on a `MapEnv`, and
  assert `hosted_endpoint_from_env` returns `None` (no `TINYHUMANS_API_KEY`)
  or, with `TINYHUMANS_API_KEY` also set, returns exactly
  `(Credential::Source(..), DEFAULT_TINYHUMANS_INFERENCE_URL)` — proving the
  three removed variables are inert, not merely unread by convention.
- **Rust:** `cargo fmt --all -- --check` locally; clippy and `cargo test` on
  CI, verified by head SHA per the standing rule.
- **Frontend:** all three typecheck gates, named. The E2E provider-row
  migration (§8) needs both Console E2E lanes green on the final push.

## 11. Must not touch

- `TINYHUMANS_API_KEY`, `TINYHUMANS_API_URL`, `OPENCOMPANY_INFERENCE_MODEL` —
  none of the three is in scope. Do not fold their reads into this slice's
  cleanup even where they sit beside a deleted line.
- Every `OPENHUMAN_*` name (Q16, phase 1c, unchanged).
- `company_key::resolve` and the managed chains' `tinyhumans/key` /
  `provider/tinyhumans/key` steps — item 10 is still not handled; 6a removes
  environment reads, not secret-store keys or chain steps that do not depend
  on the four variables.
- `inference/managed/enabled` (item 3, still not handled).
- Any provider **other** than `tinyhumans` in the E2E provider-row migration —
  §8's fixture is scoped to the inference fixture specs only.
- `vendor/`.

## 12. Done when

- `git grep -n 'OPENCOMPANY_INFERENCE_KEY\|OPENCOMPANY_INFERENCE_URL\|OPENCOMPANY_COMPOSIO_BACKEND_URL\|TINYHUMANS_TOKEN_FILE' -- . ':!vendor' ':!docs/key-reworks'` prints nothing outside comments explaining the removal (`not-handled.md` history, this slice's own files, and the "gotchas" prose below are outside `src`/`frontend`/`companies` and are fine).
- Every §10 test exists and CI is green on the head SHA, zero failures and
  zero pending, both Console E2E lanes included.
- The §8 E2E migration covers every spec the §8 grep found — re-run that grep
  after the migration; a spec still matching `managesLiveLlm`/`managesFixtures`
  without a provider-row POST in its own setup is not done.
- The docs in §9 are updated; `docs/spec/runtime/config.md`'s hosted-tenant
  framing no longer promises a projected-file credential.
- The PR body states plainly, for the operator and for `opencompany-manager`:
  which four variables are gone, that a hosted tenant now needs
  `provider/tinyhumans/key` set (through the console, or provisioned the same
  way item 10's future per-tenant key would be) or it has no managed brain and
  no managed Composio tools, and that the manager no longer needs to inject
  any of the four.

## 13. Gotchas

- **`TinyhumansTokenSource::from_env` still takes `env: &dyn EnvSource`.** Do
  not drop the parameter even though it now reads only one variable — every
  caller passes a test double or `ProcessEnv`, and keeping the signature
  avoids touching every call site for a change that is purely internal.
- **`CredentialSource::Attested` and `TokenTier::ProjectedFile`.** Part 1 §5
  recommends keeping them typed but unreachable for this slice. If a later
  reviewer asks "why is this dead", the answer is the PR body note this
  slice's done-when requires, not a code comment that will rot.
- **The Composio fixture's `TINYHUMANS_API_URL` swap is coarser than the
  variable it replaces.** It is safe only because that E2E project's host is
  isolated (§8's last paragraph). Do not reuse the same swap for a
  non-isolated host without re-checking that claim.
- **A hosted tenant with only the projected-file tier loses cognition,
  embeddings, web search and media generation the moment 6a ships to it** —
  `search_backend_from_env` and the embeddings resolver (referenced but not
  ported, per phase 1c's C4) share `TinyhumansTokenSource`, so this is not an
  inference-only regression. Say so in the PR body in exactly those terms; do
  not let "managed inference" stand in for all four surfaces.
- **This slice does not add a manager-side per-tenant key.** That is item
  10's unblock condition (not-handled.md), not something 6a does on its own.
  6a removing `TINYHUMANS_TOKEN_FILE` makes the *need* for that manager
  change immediate rather than eventual; it does not satisfy it.
