# Phase 1c — remove verified dead code

Slice 1c of the keys rework (issue #2306). Read [README.md](README.md) first.
Code read at `upstream/main @ fcfb3e1bc` (2026-09-14).

## 1. Goal

Delete the console code that nothing renders and fix one dangling Rust doc
link, and delete nothing that is still wired. Dump item 17. Decisions: **Q16 = A**
(the embedded OpenHuman runtime and its `OPENHUMAN_*` names stay; remove only
verified dead code), **Q10** (keep the key-grant backend `/credential/link/*` and
the redeem hook), **F8** (CLI logins as LLM providers are out of scope).

Every candidate below records the command that proved it and what that
command printed on `fcfb3e1bc`. **Re-run each command on `upstream/main`
before deleting.** If any result differs (a new importer, a new caller), stop
and report instead of deleting.

## 2. Summary of what this slice does

| # | Candidate | Verdict |
|---|---|---|
| C1 | `frontend/src/views/connections/CompanyCredentialCard.tsx` (285 lines) | **delete**; reword 6 comments |
| C2 | `frontend/src/views/connections/ConnectTinyHumansButton.tsx` (89 lines) + `frontend/test/unit/connect-tinyhumans-button.test.ts` (145 lines) | **delete**; port 3 redeem tests |
| C2b | `startCredentialLink` + `CredentialLinkStart` in `frontend/src/api/credential.ts:114-137` | **delete** |
| C3 | `frontend/src/views/connections/HubAccountLinks.tsx` (64 lines) + `frontend/test/unit/inference-hub-account-links.test.ts` (82 lines) | **delete** (orphaned by C1) |
| C4 | dangling `hosted_embeddings_from_env` link, `src/harness/built_in/provider.rs:166-181`, `:294-295` | **reword doc** |
| C5 | `RunnerDispatch` (`src/runner/`) | **leave** — designed, feature-gated, tested, documented |
| C6 | CLI login rows (`CLI_LOGINS`, `CLI_LOGINS_REACHABLE = false`) | **leave** — F8 |
| C7 | `OPENHUMAN_*` names | **leave** — Q16 = A |
| keep | `finishCredentialLink`, `use-redeem-key-grant.ts`, `/credential/link/start` + `/finish` routes | **keep** — Q10 |

## 3. C1 — `CompanyCredentialCard.tsx`

**Evidence.**

```
git grep -n "CompanyCredentialCard" upstream/main -- . ':!vendor'
```

On `fcfb3e1bc` this printed 9 lines: the definition
(`CompanyCredentialCard.tsx:52`) and 8 comments. **No `import` of it anywhere**,
including tests, `scripts/` and `docs/`. `ComposioView.tsx:78-83` and
`page-section-heading-level.test.ts:53-58` already say "it has no caller
left" (since #2279).

Its own imports (`CompanyCredentialCard.tsx:1-18`) are all still used elsewhere
(`getCompanyCredential`, `setCompanyCredential`, `CompanyCredentialStatus` by
`ApiKeyView.tsx:9-12`; ui components everywhere) **except**
`ConnectTinyHumansButton` (C2) and `HubAccountLinks` (C3).

**Edits.**

1. `git rm frontend/src/views/connections/CompanyCredentialCard.tsx`.
2. Reword each comment reference (text only; no code change):

| File:line | Current | Replace with |
|---|---|---|
| `frontend/src/lib/section-load.ts:7-9` | "`CompanyCredentialCard` already draws the right distinction one directory over; this is that rule, extracted so every section routes through the same decision." | "The company-credential card (deleted in #2306) drew this distinction first; this is that rule, extracted so every section routes through the same decision." |
| `frontend/src/product-scope.ts:58-60` | "…started deciding its own visibility from the host's answer; see `CompanyCredentialCard`. What is left here is the route, and the route is now offered." | "…started deciding its own visibility from the host's answer (that card has since been deleted, #2306). What is left here is the route, and the route is now offered." |
| `frontend/src/views/connections/ApiKeyView.tsx:135-136` | "host (issue tracked alongside `CompanyCredentialCard`'s identical guard):" | "host (the retired company-credential card carried the same guard):" |
| `frontend/src/views/connections/ComposioView.tsx:78-83` | "…`CompanyCredentialCard` used to sit here… so the card has no caller left." | keep the paragraph; change the first sentence's symbol to "The company-credential card used to sit here" and the last clause to "so the card had no caller left and was deleted (#2306)." |
| `frontend/src/views/connections/SectionUnreachable.tsx:11-12` | "Modelled on `CompanyCredentialCard`'s error card." | "Modelled on the error card of the company-credential card (deleted in #2306)." |
| `frontend/test/unit/page-section-heading-level.test.ts:53-58` | "`CompanyCredentialCard` was the other and no longer renders here: … so it has no caller left, and nothing here pins it" | "The company-credential card was the other and no longer renders here: … so it had no caller left and was deleted (#2306); nothing here pins it" |

`frontend/test/unit/inference-hub-account-links.test.ts:20` also names the card;
that file is deleted in C3, so it needs no rewording.

**Leave these prose mentions of "the company-credential card" alone** (not
symbol references; historical explanations, and editing them risks changing
what a test comment justifies): `onboarding-gate-integration-credential.test.ts:346`,
`product-scope-hidden-surfaces.test.ts:369`, `test/e2e/connections-authority.spec.ts:241-246`.
The id `#company-credential` is still live in `AccountKeyDialog.tsx:81-83`.

## 4. C2 — `ConnectTinyHumansButton.tsx` and its test

**Evidence.**

```
git grep -n "ConnectTinyHumansButton" upstream/main -- . ':!vendor'
```

On `fcfb3e1bc`: `CompanyCredentialCard.tsx:17` (import), `:217` (render),
its own definition `ConnectTinyHumansButton.tsx:41`, comments at
`HubAccountLinks.tsx:22` and `use-redeem-key-grant.ts:19`, and the test
`connect-tinyhumans-button.test.ts:9,47,110`. **The only production renderer is
the card from C1.**

```
git grep -n "connect-tinyhumans" upstream/main -- frontend
```

On `fcfb3e1bc`: the button's `data-testid` (`ConnectTinyHumansButton.tsx:78`),
its own test (`:60`), and three absence assertions in
`frontend/test/unit/api-key-view.test.ts:272,282,291`
(`expect(…'[data-testid="connect-tinyhumans"]').toBeNull()`). No e2e spec uses it.

**Edits.**

1. `git rm frontend/src/views/connections/ConnectTinyHumansButton.tsx`.
2. `git rm frontend/test/unit/connect-tinyhumans-button.test.ts`.
3. **Keep** the three absence assertions in `api-key-view.test.ts:272,282,291`.
   They still pass and guard against the button coming back on the Account page.
4. **Port the redeem coverage.** The deleted test's second `describe`
   ("coming back finishes the exchange", `:99-145`) is the only unit coverage of
   `useRedeemKeyGrant` redeeming exactly once, and of redeeming nothing after a
   cancel. `api-key-view.test.ts:484-525` covers "finishes even when the
   credential read fails" only. Create
   `frontend/test/unit/use-redeem-key-grant.test.ts`:

```ts
// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { captureKeyLink } from "@/lib/pending-key-link";
import { useRedeemKeyGrant } from "@/views/connections/use-redeem-key-grant";

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  captureKeyLink(null, false);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.restoreAllMocks();
});

function client(post: unknown) {
  return {
    scopeFor: (company: string | null) =>
      company ? `/api/v1/companies/${company}` : "/api/v1/company",
    post,
  } as unknown as OpenCompanyClient;
}

function Harness(props: { client: OpenCompanyClient; onConnected?: () => void }) {
  useRedeemKeyGrant(props.client, "acme", props.onConnected);
  return null;
}

async function mount(props: { client: OpenCompanyClient; onConnected?: () => void }) {
  await act(async () => {
    root.render(createElement(Harness, props));
  });
}
```

   Then three `it` blocks, bodies copied from the deleted file with
   `createElement(ConnectTinyHumansButton, …)` replaced by `mount({ client: client(post), onConnected })`:

| Test name | Setup | Assert |
|---|---|---|
| `redeems a captured grant exactly once` | `post = vi.fn(async () => ({ status: { configured: true, source: "company", notice: "" }, note: "connected" }))`; `captureKeyLink({ state: "s1", code: "c1" }, false)`; `onConnected = vi.fn()` | `post` called once with `("/api/v1/companies/acme/credential/link/finish", { state: "s1", code: "c1" })`; `onConnected` called once |
| `redeems nothing when the person cancelled on the hub` | `post = vi.fn()`; `captureKeyLink(null, true)` | `post` not called |
| `redeems nothing on an ordinary load` | `post = vi.fn()` | `post` not called |

The deleted file's first two `describe` blocks ("appears only where it could
work", "starting a grant leaves for the hub") test the button and
`startCredentialLink` only; they are not ported.

### C2b — `startCredentialLink` and `CredentialLinkStart`

**Evidence.**

```
git grep -n "startCredentialLink\|CredentialLinkStart" upstream/main -- . ':!vendor'
```

On `fcfb3e1bc`: `credential.ts:121,132,135,136` (definitions) and
`ConnectTinyHumansButton.tsx:6,55` (the only caller). After C2 there is no
caller.

**Edits.** In `frontend/src/api/credential.ts` delete lines `114-137` (the
`CredentialLinkStart` doc comment and interface, then the `startCredentialLink`
doc comment and function) and one of the two blank lines left behind.

**Keep:**

- `finishCredentialLink` (`credential.ts:139-157`), called by
  `use-redeem-key-grant.ts:5,38`, which `ApiKeyView.tsx:48,202` mounts
  unconditionally. In its doc at `:144-145` replace "so the card that called
  this can repaint from it either way" with "so the page that redeemed it can
  repaint from it either way". Leave the sentence about what the host stores;
  phase 4a changes that behaviour and its doc.
- `hubLink?: boolean` on `CompanyCredentialStatus` (`credential.ts:63`). The
  host still sends it, and `account-row.test.ts` and `api-key-view.test.ts`
  fixtures set it. It is a wire field, not dead code.
- The Rust routes `POST …/credential/link/start` and `…/finish`
  (`src/server/ops/company_key.rs:112-113`, handlers `:303`, `:430`), their
  tests (`company_key/test.rs:682-851`), `tests/auth_matrix.rs:436-437` and
  `tests/snapshots/auth-matrix.txt`. Q10: grant keys are the only keys besides
  hosted tokens that carry Composio's connections scope. `/start` has no console
  caller after this slice, **on purpose**; do not remove it and do not update
  the auth-matrix snapshot.
- `frontend/src/lib/pending-key-link.ts` (used by `App` and the hook).

**Comment rewording.** `frontend/src/views/connections/use-redeem-key-grant.ts:17-19`:
"The Account page calls it at the top of `ApiKeyView`; the Apps card gets it
through `ConnectTinyHumansButton`." → "The Account page calls it at the top of
`ApiKeyView`, which is its only caller." (`HubAccountLinks.tsx:22` goes with C3.)

## 5. C3 — `HubAccountLinks.tsx` (orphaned by C1)

Not in the rundown's list; it becomes dead the moment C1 lands.

**Evidence.**

```
git grep -n 'views/connections/HubAccountLinks"' upstream/main -- frontend
```

On `fcfb3e1bc`: `CompanyCredentialCard.tsx:18` and
`test/unit/inference-hub-account-links.test.ts:29`. No other importer. The
Account page draws its own links from `status.account`
(`ApiKeyView.tsx:260`, `:384`, `:473-476`) and does not use the component, so
deleting it removes no rendered UI.

**Edits.**

1. `git rm frontend/src/views/connections/HubAccountLinks.tsx`.
2. `git rm frontend/test/unit/inference-hub-account-links.test.ts`. Its first
   `describe` tests only the component. Its second ("the LLM page does not carry
   them") reads `ProvidersTab`, `RoutingTab`, `ProviderList` sources and asserts
   the string `HubAccountLinks` is absent. With the component gone that is
   vacuous, and `RoutingTab` is deleted in slice 5b.
3. **Keep** the `HubAccountLinks` **interface** in `frontend/src/api/credential.ts:80-85`.
   It types `CompanyCredentialStatus.account`, which `ApiKeyView` reads. Same
   name, different thing. Do not delete it.

If the operator prefers to keep the component, skip C3 entirely. C1 then leaves
it with one test importer and no production importer, which is the situation
item 17 asks to remove.

## 6. C4 — the dangling `hosted_embeddings_from_env` doc link

**Evidence.**

```
git grep -n "hosted_embeddings_from_env" upstream/main -- . ':!vendor'
```

On `fcfb3e1bc`: one hit, `src/harness/built_in/provider.rs:169`, inside a doc
comment. `src/harness/` has no `embeddings` module (`ls src/harness/`).
`git log -S hosted_embeddings_from_env --format='%h %ad %s' --date=short` shows
it introduced in `1ef04132b` (2026-07-31, #201), with its occurrence count
changing in `103e2746b` (2026-08-23, "feat(memory): remove embedded tinycortex
backend") and `9d13688b3` (2026-08-24). No CI step runs `cargo doc` or sets
`RUSTDOCFLAGS` (`git grep -n "RUSTDOCFLAGS\|cargo doc\|broken_intra_doc" -- .github Cargo.toml src/lib.rs`
printed nothing), which is why the broken link was never flagged.

`hosted_endpoint_from_env` callers today: `provider.rs:150`
(`harness_inference_from_env`) and `:313` (`PlatformCredentialStatus::resolve`).

**Current** `provider.rs:166-171`:

```rust
/// Resolve the shared hosted-endpoint `(credential, base_url)` pair every hosted
/// TinyHumans surface addresses — the **one** credential path both chat
/// inference ([`harness_inference_from_env`]) and embeddings
/// ([`hosted_embeddings_from_env`](crate::harness::embeddings::hosted_embeddings_from_env))
/// resolve against, so a rotation or a per-tenant key reaches both without a
/// second, drifting resolution.
```

**Target** `:166-171`:

```rust
/// Resolve the shared hosted-endpoint `(credential, base_url)` pair that managed
/// TinyHumans chat inference addresses — the **one** credential path both
/// [`harness_inference_from_env`] and [`PlatformCredentialStatus::resolve`] read,
/// so a rotation or a per-tenant key reaches both without a second, drifting
/// resolution.
```

**Current** `:180-181`:

```rust
/// The embeddings client POSTs to `{base_url}/embeddings`, the chat client to
/// `{base_url}/chat/completions` — the same OpenAI-compatible surface.
```

**Target** `:180-181`:

```rust
/// The chat client POSTs to `{base_url}/chat/completions`, an OpenAI-compatible
/// surface.
```

**Current** `:294-295`: `/// Managed chat inference and embeddings resolved` /
`/// ([`hosted_endpoint_from_env`]).` → **Target**: `/// Managed chat inference
resolved` / `/// ([`hosted_endpoint_from_env`]).`

Doc comments only. No code, no test. Do not change `TINYHUMANS_TOKEN_FILE`
wording at `:176` (item 18 is not handled here).

## 7. C5 — `RunnerDispatch`: leave it

**Evidence** (all on `fcfb3e1bc`):

- `git grep -n "RunnerDispatch" -- . ':!vendor'` → `src/runner/dispatch.rs:114,122,165`
  (definition, `impl AcpAgent`), its tests `dispatch.rs:273,275,376`, the
  re-export `src/runner/mod.rs:39`, a doc comment `src/company/types.rs:65`, and
  two specs: `docs/spec/runtime/harnesses-acp.md:44`,
  `docs/spec/runtime/external-harnesses-ui.md:223`. No runtime caller.
- Feature-gated, not orphaned: `src/lib.rs:77-78` (`#[cfg(feature = "runner")] pub mod runner;`).
  `scripts/ci/feature-lanes.txt:61` classifies `runner` as `partial`, tested in the
  `acp,runner,tinymemory` lane, so its tests run in CI.
- The transport is part of the manifest contract: `ACP_TRANSPORTS = &["local", "runner"]`
  (`src/company/types.rs:68`), validated in `src/company/manifest.rs:1057`, `:1125`,
  handled in `src/server/ops/team_agent.rs:869` and its test `:4256-4270`.
  `src/harness/lanes.rs:101-107` resolves it to a clear
  "this build has no runner transport wired yet" reason.
- Documented as designed and deliberately deferred: `harnesses-acp.md:23-28`
  (config shape), `:42-46` ("nothing wires it into `lanes::build`, so a
  `runner`-transport harness resolves `unavailable`"),
  `external-harnesses-ui.md:221-232` ("Deliberately out of scope … written and
  unit-tested but wired to nothing").
- `gh issue list --repo tinyhumansai/opencompany --search RunnerDispatch --state all`
  → #1245, #1903, #1524, all CLOSED, none about wiring or removing the runner.
  `--search "runner in:title"` → #1885, #434, #590, #681, none related.

**Recommendation: no change in this slice.** It is a designed transport with a
manifest value, validation, a feature flag, a CI lane, and specs that already
say it is unwired on purpose. It is not a key, provider or credential surface.
Deleting it would change the manifest contract (`transport = "runner"` would
become a validation error), `ACP_TRANSPORTS`, `Cargo.toml` features,
`feature-lanes.txt`, three specs and `team_agent` tests. That is a product
decision about remote runners, not dead-code removal. Report it to the operator
as a possible separate issue; do not file it from this slice.

## 8. C6 — CLI login rows: leave them (F8)

`frontend/src/inference/connect.ts:220` `export const CLI_LOGINS_REACHABLE = false;`,
`:223` `CLI_LOGINS_UNAVAILABLE`, used at `AddProviderDialog.tsx:22,107-109`;
`frontend/src/inference/catalogue.ts:315` `CLI_LOGINS`;
`src/company/inference/catalogue.rs:459` `CLI_LOGINS`, with a Rust/TS parity test
at `catalogue.rs:1552-1554` and a count test at `:1080`. Rendered but switched
off, and the host refuses the kinds. F8 puts CLI logins as LLM providers out of
scope for #2306. **Do not remove** in this slice. The rundown's "remove the CLI
login rows and the claude-code route form" belongs with routing removal (5b), if
anywhere.

## 9. C7 — `OPENHUMAN_*` names: all stay (Q16 = A)

```
git grep -n -o 'OPENHUMAN_[A-Z_]*' upstream/main -- . ':!vendor' ':!docs'
```

On `fcfb3e1bc` (counts): `OPENHUMAN_WORKSPACE` 23, `OPENHUMAN_WORKSPACE_ENV` 13,
`OPENHUMAN_USAGE_META_KEY` 6, `OPENHUMAN_URL` 6 (as `OPENCOMPANY_OPENHUMAN_URL`),
`OPENHUMAN_AGENT_TURN_TIMEOUT_SECS` 4, `OPENHUMAN_DEV_PORT` 4,
`OPENHUMAN_SUBDIR` 2, `OPENHUMAN_CARGO_INSTALL_ROOT` 2, `OPENHUMAN_TOKEN` 1 (as
`OPENCOMPANY_OPENHUMAN_TOKEN`).

| Name | Where | Why it stays |
|---|---|---|
| `OPENHUMAN_WORKSPACE`, `OPENHUMAN_WORKSPACE_ENV`, `OPENHUMAN_SUBDIR` | `src/app/journal.rs:55,58,182-192`; `src/app/boot.rs:113,184`; `crates/opencompany-app/src/main.rs:16-20`; `crates/opencompany-tui/src/lib.rs:73-81`; `docs/spec/runtime/workspace-layout.md:50-81`; `AGENTS.md:161` | The vendored runtime reads it to place its journal. Unset, it defaults under `$HOME`, which is read-only in a tenant container, and boot aborts (issue #446, `CLAUDE.md`). |
| `OPENHUMAN_USAGE_META_KEY` = `"openhuman_usage_meta"` | `src/harness/built_in/provider.rs:74-75,714,719,2753,2802` | Wire key for backend-charged cost; must match OpenHuman's. Renaming loses metering. |
| `OPENHUMAN_AGENT_TURN_TIMEOUT_SECS` | `src/harness/built_in/mod.rs:2225,2315,8538,8582`; `docs/spec/runtime/harnesses.md:236`; `docs/plans/hivemind-handoff.md:58` | The vendored runtime's own setting; our error message names it so an operator can act. |
| `OPENCOMPANY_OPENHUMAN_URL`, `OPENCOMPANY_OPENHUMAN_TOKEN` | `src/bin/opencompany.rs:884,898,901`; `src/app/config.rs:903`; `src/app/doctor.rs:167,361`; `src/openhuman/http_client.rs:8`; `docs/spec/runtime/config.md:120`; `docs/spec/integrations/openhuman.md:74` | Optional attach path to a running `openhuman-core serve`. Q16 = A keeps it (B would remove it). |
| `OPENHUMAN_DEV_PORT`, `OPENHUMAN_CARGO_INSTALL_ROOT` | `src/openhuman/launcher.rs:93,97,263,266,458,725` | Desktop launcher dev settings. Q16 = A keeps them. |

No edit. A blanket find-and-delete on `OPENHUMAN_` is explicitly forbidden.

## 10. Data carry-over

None. No stored key, route or wire shape changes.

## 11. Tests summary

- **Delete:** `frontend/test/unit/connect-tinyhumans-button.test.ts`,
  `frontend/test/unit/inference-hub-account-links.test.ts`.
- **Add:** `frontend/test/unit/use-redeem-key-grant.test.ts` (three tests, §4).
- **Unchanged and must pass:** `frontend/test/unit/api-key-view.test.ts`
  (including the absence assertions at `:272,282,291` and the redeem test at
  `:497`), `account-row.test.ts`, `page-section-heading-level.test.ts`.
- **Rust:** none added. `company_key` tests and the auth matrix stay as they are.

## 12. Console / UI

No visible change. None of the deleted components is rendered on any page on
`fcfb3e1bc` (C1–C3 evidence). No copy, route or testid that a page renders
changes. The Account page still mounts `useRedeemKeyGrant`. A browser check is
not needed for this slice; the three Console E2E lanes exercise the Account and
Apps pages.

## 13. Order of edits

1. Re-run every evidence command in §3–§6 on `upstream/main`. Stop on any new hit.
2. C1 delete + six comment edits.
3. C2 deletes; add `use-redeem-key-grant.test.ts`; `use-redeem-key-grant.ts:17-19` comment.
4. C2b: delete `credential.ts:114-137`; reword `:144-145`.
5. C3 deletes.
6. Confirm no importer is left:
   `git grep -n "CompanyCredentialCard\|ConnectTinyHumansButton\|startCredentialLink\|CredentialLinkStart\|views/connections/HubAccountLinks" -- frontend`
   must print **no `import` lines**. Only reworded comments (no backticked
   symbol) may remain.
7. In `frontend/`: `npm run typecheck`, `npm run typecheck:unit`,
   `npm run typecheck:e2e` (`package.json:16-18`, three separate tsconfigs),
   then `npx vitest run test/unit/use-redeem-key-grant.test.ts test/unit/api-key-view.test.ts`.
8. Commit: `Remove the unrendered company-credential card and its link button`.
9. C4 doc edits in `provider.rs`; `cargo fmt --all -- --check`.
   Commit: `Fix the dangling embeddings doc link in the hosted provider`.
10. Push.

## 14. Must not touch

- `use-redeem-key-grant.ts` code, `finishCredentialLink`, `pending-key-link.ts`,
  `ApiKeyView.tsx` code (comment at `:135-136` only), `AccountKeyDialog.tsx`.
- `src/server/ops/company_key.rs` and its tests, `tests/auth_matrix.rs`,
  `tests/snapshots/auth-matrix.txt`.
- `src/runner/`, `ACP_TRANSPORTS`, manifest validation, `feature-lanes.txt`.
- `CLI_LOGINS`, `CLI_LOGINS_REACHABLE`, `AddProviderDialog.tsx`.
- Every `OPENHUMAN_*` name, `TINYHUMANS_TOKEN_FILE` (item 18, not handled).
- The `HubAccountLinks` interface in `credential.ts:80-85`.
- `vendor/`.

## 15. Done when

- No importer is left for any deleted file (§13 step 6 prints no `import`).
- `npm run typecheck`, `npm run typecheck:unit` and `npm run typecheck:e2e` are
  each clean locally, named separately in the PR description.
- On the pushed head SHA (`gh api "repos/tinyhumansai/opencompany/actions/runs?head_sha=$SHA"`,
  then that run's `/jobs`): `Console` (runs all three typechecks and
  `npm test`, `ci.yml:1613-1687`), `Console E2E`, `Console E2E (live brain)`,
  `Console E2E (first run)`, `Rust`, `Rust (openhuman, tinymemory)` and
  `Markdown line cap` are green, with zero failures **and** zero pending. The
  E2E lanes start late; wait for them.
- `git grep -n "hosted_embeddings_from_env" -- src` prints nothing.
- The PR description lists C5, C6 and C7 as deliberately kept, each with a
  one-line reason.

## 16. Gotchas

- **The redeem hook is not dead.** A grant started anywhere (including a
  bookmarked hub URL) returns to the Account page, and `ApiKeyView.tsx:202`
  spends it. Deleting the hook or `finishCredentialLink` drops a single-use
  credential with nothing on screen to retry.
- **`HubAccountLinks` is two things.** The component (deleted) and the
  interface in `api/credential.ts` (kept). A careless search-and-delete removes
  the type that `CompanyCredentialStatus.account` needs, and `typecheck` fails.
- **`typecheck` alone is not enough.** A deleted test file can leave a broken
  import only `typecheck:unit` sees; an e2e helper only `typecheck:e2e` sees.
- **`/credential/link/start` loses its last console caller.** That is Q10, not
  an oversight. Do not "tidy" the route, its auth-matrix row or the snapshot.
- **`RunnerDispatch` looks dead to a grep that ignores features.** Its tests run
  only in the `acp,runner,tinymemory` lane. It is gated, not orphaned.
- The rundown (item 17, "How to do it") suggests deleting or wiring
  `RunnerDispatch` and removing the CLI-login rows in this PR. This brief
  overrides that with the evidence in §7–§8 and F8.
