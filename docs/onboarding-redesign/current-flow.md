# Current flow, as traced

Everything on this page was read from the running code, not inferred. Source:
`frontend/src/views/setup/SetupWizard.tsx` and
`crates/opencompany-core/src/server/setup.rs`, `upstream/main` post-#2338.

## Step visibility

`STEPS` (`SetupWizard.tsx:105-111`): `power, business, signin, account,
advanced, review` — six possible steps. `visibleSteps` (`:665-674`) hides:

- `power` when the host already supplies inference (`tested.kind ===
  "hosted"`)
- `account` when no sign-in mode requires an admin email
- `advanced` when the host has no advanced settings to ask about

A host with a required sign-in and no advanced settings shows five steps; one
with neither `account` nor `advanced` needed shows four — which is what the
"step 1 of 4" in the operator's own screenshot was.

## The six steps

**1. Model (`power`, `PowerStep`, `:1506-1889`).** "What should your team
think with?" — provider dropdown, API key field, Test connection button, a
"Sign in with TinyHumans" link-out. Clicking Test connection sets
`tested = { kind: "testing" }`, then `POST /api/v1/setup/inference/test`
(handler `test_inference`, `server/setup.rs`) makes a **live probe call** —
one real chat turn sent through the actual provider path — and returns
`ok`/`model` or a summarized error; the raw key is used and discarded, never
stored by this call. `tested.kind` must land on `ok`, `skipped` (the "no
model" choice), or `hosted` (the host already supplies inference) to advance
— `problem()` (`:998-1027`) blocks Next otherwise, with "test the connection
first" or "that connection did not work" depending on which.

**2. Business (`BusinessStep`, `:2362`).** "What kind of company are you
setting up?" — a template `<select>` if the host has any, else free-text
`industry`; `teamHint`/`automate` follow-ups only when a model was tested.
Gate: template chosen, or industry text filled.

**3. Sign-in (`SignInStep`, `:1352`).** "How should people sign in?" — buttons
per offered `auth_mode`. No blocking gate of its own; feeds step 4's
validation.

**4. You (`AccountStep`, `:1503`, conditional).** Admin email. Gate:
`adminEmailProblem(email, requiresSignIn(...))` (`:1068`) — a valid admin
address, only when sign-in requires one.

**5. Advanced (`AdvancedStep`, `:2302`, conditional).** Host-level config
fields (e.g. `bind`), validated server-side on submit.

**6. Review (`ReviewStep`, `:1989`).** Entering this step fires `design()`
(`:730`) → `POST /api/v1/setup/roster` → backend `propose_roster`
(`server/setup.rs:1525`) — either `template_proposal()` (no tested model) or
the tested model designing a roster. Shows a spinner, then the roster plus an
**editable company name**. Button: "Build my company" (`:1157-1162`).
Design failure: `designError` + Retry, stays on Review.

## Submit

`submit()` (`:782`) → `POST /api/v1/setup` with `SetupInput` (changed
`fields`, `name`, `admin_email`, `company` [designed roster, wins] or
`template` [slug]). Backend `apply` → `apply_inner`
(`server/setup.rs:724-753, 900-1035`):

- Serialized process-wide via `APPLY_LOCK`.
- Validates every field, writes `config.toml` — all-or-nothing, nothing live
  touched before this.
- Branches: a designed company → `manifest_from_setup` →
  `desktop::seed_generated_company` (`desktop.rs:423`) → `register()` (boots
  the runtime) → optionally `company::inference::store_key` (see below); a
  template → `desktop::seed_company_with` (`desktop.rs:375`); neither → no
  seed, existing company rebuilt in place if `auth_mode` changed.

Submit failure: `saveError`, stays on Review, no partial state.

## What finishing the Model step actually writes

Traced directly, not inferred from the PR description. Finishing the wizard
with a TinyHumans key tested writes exactly two things, both at final submit,
**not** at Test connection (which discards the key it tests):

1. **`inference/key`** in the secret store — the raw key, via
   `company::inference::store_key()` (`inference.rs:952-967`, `KEY_KEY =
   "inference/key"`). Write-only, never read back to the console.
2. **The manifest's `inference` block** — `provider`, `base_url`, `models` —
   into `config.toml` (`setup.rs:962-965`).

That is the whole write. **No `provider/tinyhumans/key` row. No Composio. No
search.** This is a completely separate mechanism from the Connections →
Account fan-out (`company_key.rs`'s own, differently-named `store_key`) —
same word, two unrelated functions in two different files. See
[reuse-mapping.md](reuse-mapping.md) for why this matters.

## The post-build checklist

After a successful submit, before this redesign, the operator lands on "Let's
get your company running" — three items completable in any order (Name your
company / Connect an integration / Run an automation), each with its own
gate, plus a "Skip setup" escape hatch. Removed entirely by this redesign —
see [removed.md](removed.md).
