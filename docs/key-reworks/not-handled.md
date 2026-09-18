# Not handled in this PR

Part of [the keys rework](README.md). Each entry says what was asked, why it is
not in this PR, what breaks if it is done anyway, and what would unblock it.
Code references are on `upstream/main @ fcfb3e1bc` (2026-09-14).

An implementer must **not** start any of these in `feat/key-reworks`. If a
slice seems to need one, stop and report.

---

## Item 10 — drop the managed fallback chains (LLM and Composio)

**Asked.** Managed resolves only from its own key: no `inference/key`, no
`tinyhumans/key`, no instance identity. The same for Composio.

**Today.** The managed LLM chain is `provider/tinyhumans/key` → legacy
`inference/key` (only when entry zero is managed, `load_managed_key`,
`src/company/inference.rs:936`) → `tinyhumans/key` (`managed_identity`,
`inference.rs:1116`) → the instance identity (`hosted_endpoint_from_env`,
`src/harness/built_in/provider.rs:182`, reading `OPENCOMPANY_INFERENCE_KEY`,
then `TINYHUMANS_TOKEN_FILE`, then `TINYHUMANS_API_KEY`) → nothing. Composio's
managed chain is `composio/token` → `company_key::resolve`
(`src/company/company_key.rs:123`: `tinyhumans/key`, then the instance
identity) → nothing.

**After slice 6a**, the instance-identity step of both chains shrinks: it
reads only `TINYHUMANS_API_KEY` (`OPENCOMPANY_INFERENCE_KEY` and
`TINYHUMANS_TOKEN_FILE` are no longer read anywhere). The chain still has a
step, it is just one variable shorter — item 10 itself (dropping this step,
and the `tinyhumans/key` step before it) remains not handled for the reasons
below.

**Why not here.**
- **Hosted tenants store no key.** A hosted pod's only credential is the
  projected token file the kubelet rewrites in place with a 600-second expiry
  (`src/company/credentials.rs:3-17`). It cannot be copied into a secret slot:
  a copy is dead within minutes, and it would put a platform credential into
  the tenant's database.
- **Nothing in this repo writes a company key for a hosted tenant.** Removing
  the last step hands every hosted tenant the echo brain and no Composio tools
  on the next deploy, silently — `RuntimeBuilder` reads `None` as "nothing
  configured".
- **Only a grant-minted or attested key carries Composio's `connections`
  scope.** A key a person mints does not, so "paste a key" is not a hosted
  replacement for Composio.
- **Docker development and both Console E2E lanes rode the env default**
  (`frontend/playwright.config.ts:220-226` set `OPENCOMPANY_INFERENCE_URL` and
  `OPENCOMPANY_INFERENCE_KEY` on the fixture hosts) until slice 6a moved them
  to a stored provider row instead — see
  [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md) §"E2E hosts".

**What would unblock it.** A manager-side change (Q4 option B): at provision
time the manager (or backend) mints a per-tenant key that carries `inference`
and `connections`, and writes it into `provider/tinyhumans/key` and
`composio/tinyhumans/key` for that tenant. Then, in a later PR in this repo: a
copy-if-empty carry-over from `tinyhumans/key` into both slots behind a
one-release flag, removal of `managed_identity`'s `tinyhumans/key` step, the
managed-gated `inference/key` read, and `company_key::resolve` in Composio,
with the E2E fixtures moved to a stored key.

**What this PR does instead.** Phase 4a copies the account key into both
TinyHumans slots going forward (empty-or-equal-to-old, Q7), so the explicit
path exists and is used first. The fallbacks stay behind it, unchanged.

---

## Item 18 — remove `TINYHUMANS_TOKEN_FILE` — now handled (slice 6a)

**Asked.** Stop reading `TINYHUMANS_TOKEN_FILE` and wire everything that used
it to the normal per-provider key.

**Originally deferred here** alongside item 10 (same blocker: a hosted tenant
holds no stored key, so deleting the read would remove the brain and Composio
from every hosted company at once, and the value cannot be "wired" into a
stored key because it rotates every few minutes).

**2026-09-15: the operator decided to remove it anyway**, together with
`OPENCOMPANY_INFERENCE_KEY`, `OPENCOMPANY_INFERENCE_URL` and
`OPENCOMPANY_COMPOSIO_BACKEND_URL`, accepting the hosted-tenant risk rather than
waiting on item 10's manager-side per-tenant key. `TINYHUMANS_API_KEY` is
**not** one of the four and stays as the sole remaining instance-level
TinyHumans credential. See [phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md)
for every read site, what replaces each, what breaks, and the risk written out
in full. Item 10 itself (dropping the fallback *chains*, as opposed to these
four specific variable reads) is still not handled — see below.

---

## Item 3 — remove `inference/managed/enabled`

**Asked.** No Managed on/off switch.

**Today.** `MANAGED_ENABLED_KEY` (`src/company/inference/store.rs:779`), absent
reads as on. It gates the step-4 boot branch, the managed-route refusal, and
`refuse_a_managed_fallback_that_is_switched_off` (`inference.rs:1340`), and is
written by `POST …/inference/managed/enabled`.

**Why not here.** It depends on item 10. Until the fallback chain is gone,
this switch is the **only** thing honouring "off" on the fallback path. A
company that switched Managed off to stop platform spend would start spending
again the moment the read is deleted.

**What would unblock it.** Item 10. After it, Managed is used only when the
default or an agent pair names `tinyhumans` (a normal row, which already has
the generic per-row `enabled` flag), so "off" means "disable that row" or "pick
another default". Then delete the reads, the route and the switch; the stored
value stays, unread.

---

## Item 17 — the `OPENHUMAN_*` names (Q16 = A)

**Asked.** Remove everything prefixed `OpenHuman_*`, and everything unwired.

**What this PR does.** Phase 1c removes only verified dead code:
`CompanyCredentialCard.tsx`, the buttonless `ConnectTinyHumansButton` entry,
and the dangling `hosted_embeddings_from_env` doc link. `RunnerDispatch` is
decided with evidence in [phase-1c-dead-code.md](phase-1c-dead-code.md).

**Why the names stay.** Every `OPENHUMAN_*` name outside `vendor/` is wired:
- `OPENHUMAN_WORKSPACE` tells the embedded runtime where its journal lives.
  `serve` derives `<data-dir>/openhuman` and exports it; without it the runtime
  defaults to `$HOME`, which is read-only in a tenant container, and **boot
  aborts** (issue #446, `docs/spec/runtime/storage.md`).
- `OPENHUMAN_USAGE_META_KEY` is the wire key carrying backend-charged cost; it
  must match the vendored runtime's spelling or metering silently loses cost.
- `OPENHUMAN_AGENT_TURN_TIMEOUT_SECS` is the vendored runtime's own setting.
- `OPENCOMPANY_OPENHUMAN_URL` / `…_TOKEN` are the optional attach path;
  `OPENHUMAN_DEV_PORT` and `OPENHUMAN_CARGO_INSTALL_ROOT` are desktop launcher
  dev settings.

**What would unblock more.** An operator decision for Q16 option B (drop the
attach path and launcher settings) or C (replace the embedded runtime). Both
are separate projects, not key work.

---

## CLI logins as LLM providers (F8)

**Today.** `CLI_LOGINS` (`src/company/inference/catalogue.rs`) lists Claude Code
and Codex; the console shows them as "Not available on this host"
(`CLI_LOGINS_REACHABLE = false`, `frontend/src/inference/connect.ts`), and the
host refuses those kinds in `plan_add`.

**Why not here.** A CLI login is a credential in a dotfile on the machine a
person types on; OpenCompany runs on a server, so nothing on the host holds
one. Making them providers needs a credential transport, not a key rename. The
ACP harness (an agent running on a local CLI in the desktop app) is unaffected:
it keeps its own sign-in and its own `model` field.

**What would unblock it.** A design for getting a CLI login's credential onto
the host (or running the CLI beside it), then CLI rows in `inference/providers`
under the same D-set rule.

---

## A separate background model (Q13)

**Asked (suggested in the rundown).** An optional "background model" for
internal passes (title, triage, planning, selector, confine, payload extract,
workflow build, judge, profile draft).

**Decision.** Not built. Internal passes use the company default model. They
carry no agent, so an agent pair never applies to them.

**Cost of the decision.** Internal passes cost what the default model costs.
With an expensive default, titles and triage get dearer. A vision task on a
text-only default fails with the provider's own refusal.

**What would unblock it.** An operator request. It would be one more field in
the `inference/default` JSON (`{"provider","model","background":{…}}`), which
Q1's JSON shape was chosen to allow — no new key.

---

## Rewriting an injected `OPENCOMPANY_INFERENCE_URL`, and the manager's side

**In scope (2a, 2d).** `PLATFORM_BASE_URL` and `DEFAULT_TINYHUMANS_INFERENCE_URL`
move to `https://api.tinyhumans.ai/agent-integrations/openrouter`, and no tier
name is sent on any path.

**Not in this repo.** An `OPENCOMPANY_INFERENCE_URL` the manager injects is used
exactly as given — no path rewriting (D-proxy; #2305's URL-origin derivation is
on the do-not-do list). Hosted tenants therefore need two manager-side changes
before a build containing 2d is deployed to them:
- inject `OPENCOMPANY_INFERENCE_URL=https://api.tinyhumans.ai/agent-integrations/openrouter`
  (or the staging origin with the same path);
- inject `OPENCOMPANY_INFERENCE_MODEL` with a model id the catalog returns, so a
  hosted company with no default has a real model.

**Why.** Without them a hosted company with no default either keeps calling
`/openai/v1` with a real id (not a supported combination) or, with no model at
all, fails every turn with "choose a model". The fail-closed error is the
operator's chosen behaviour; the outage is avoidable only on the manager side.

**What would unblock removing the env model.** Every hosted company holding a
full default (for example written at provisioning, alongside item 10's per-tenant
key).

**After slice 6a** (the last slice, landing after 5b), `OPENCOMPANY_INFERENCE_URL`
is no longer read anywhere in this repo — see
[phase-6a-remove-env-vars.md](phase-6a-remove-env-vars.md). Everything above
describes the interim window from 2a through 5b, while the variable is still
read. Once 6a lands, the manager's job changes: it no longer injects a URL at
all (the constants alone decide the base, and after 2a's gated commit 5 they
already name the proxy); it still needs to give each hosted tenant a
`provider/tinyhumans/key` (or another provider's key) for managed inference to
work at all, because 6a also removes the instance-identity URL override that
used to carry a per-tenant `OPENCOMPANY_INFERENCE_KEY`.

---

## Rolling back a stored value

Nothing in this PR renames or deletes a stored key, and nothing clears a value
it did not itself write in the same request (the Q6 rollback restores the
previous value exactly). There is therefore no "undo migration" to handle:
rolling back the binary leaves stored values that the old binary either reads
as before or ignores. The per-slice rollback notes are in each phase overview.
