# Phase 6a — remove four environment variables (part 1 of 2)

Slice 6a of the keys rework (issue #2306), added by the operator on
2026-09-15 (dump item 23, which supersedes item 17's `OPENHUMAN_*` part and
brings item 18 into scope). Read [README.md](README.md) and
[phase-6.md](phase-6.md) first.

- **Code read at:** `upstream/main @ fcfb3e1bc`. Every `file:line` in both
  parts of this slice was re-verified line by line against that commit on
  2026-09-15, after `git fetch upstream main` confirmed it is still the tip.
- **Runs after:** 2a (including its gated commit 5, §5.9 "step 3b"), 2d, 5b.
  Do not start this slice on a branch that has not landed all three — see §0.
- **Removes exactly:** `OPENCOMPANY_INFERENCE_KEY`, `OPENCOMPANY_INFERENCE_URL`,
  `OPENCOMPANY_COMPOSIO_BACKEND_URL`, `TINYHUMANS_TOKEN_FILE` (and the
  `tinyhumans_token_file` config field that only ever came from it).
- **Keeps, untouched:** every `OPENHUMAN_*` name (Q16);
  `TINYHUMANS_API_KEY` and its `config.toml` twin `tinyhumans_api_key`
  (`src/app/config.rs:922-928`), which become the **only** instance-level
  TinyHumans credential; `TINYHUMANS_API_URL`; `OPENCOMPANY_INFERENCE_MODEL`;
  `OPENCOMPANY_MEDIA_KEY` / `_MEDIA_BACKEND_URL`; `OPENCOMPANY_SEARCH_BACKEND_URL`.
- **Continues in:** [part 2](phase-6a-remove-env-vars-part2.md) — config and
  doctor, Composio, the E2E replacement, docs, tests, must-not-touch, done-when,
  gotchas, **what breaks and how it is handled** (§14), commit order (§15).

## 0. Read first: why order is load-bearing

This slice removes an escape hatch the earlier slices still use while they
land. Out of order, a company that resolves on the commit before 6a gets a 400
or an outage on the commit after it, with no variable left to unblock it.

**`OPENCOMPANY_INFERENCE_URL` is not only a URL override.** It feeds
`hosted_endpoint_from_env` (`src/harness/built_in/provider.rs:182-195`), which
builds `EnvDefault` (`src/company/inference.rs:459-466`, constructed at
`src/runtime/builder.rs:3050-3060`) — the lowest-precedence source
`resolve_legacy_scoped` tries in its own right (its step 3,
`inference.rs:1544-1589`). A company that configured nothing, and a keyless
`openrouter` declaration (`resolve_endpoint`, `inference.rs:644-714`), land on
it today.

Once 6a removes the variable, the constant is the only base URL left for that
arm. So:

1. **2a must have landed, including its gated commit 5** (part 2 §5.9 of 2a:
   `PLATFORM_BASE_URL` and `DEFAULT_TINYHUMANS_INFERENCE_URL` move to
   `https://api.tinyhumans.ai/agent-integrations/openrouter`). Without it, 6a
   strands that arm on `/openai/v1` with nothing able to repoint it.
2. **2d must have landed** — the proxy answers 400 to a tier name.
3. **5b should have landed** — not for correctness, but 6a edits
   `resolve_legacy_scoped`'s neighbourhood, which 5b also reshapes.

Check before §15's first commit, and stop and report if either check fails:

```bash
git grep -n "fn resolve_for_turn" -- src/company/inference.rs          # 2b present
git grep -n 'agent-integrations/openrouter"' -- src/company/inference.rs src/harness/built_in/provider.rs   # 2a commit 5 present
git grep -n 'openai/v1"' -- src/company/inference.rs src/harness/built_in/provider.rs                      # must print nothing
```

**The E2E consequence is the largest piece of work.** The live-brain lane
points the host at `mock-brain.mjs` with `OPENCOMPANY_INFERENCE_URL` +
`OPENCOMPANY_INFERENCE_KEY` (`frontend/playwright.config.ts:214-228`). Part 2
§8 replaces that with a stored `custom` provider row seeded once by
`frontend/test/e2e/global-setup.ts`.

## 1. Files (every read site, verified)

Production reads of the four names:

| File:line | Symbol | Reads |
|---|---|---|
| `src/harness/built_in/provider.rs:182-195` | `hosted_endpoint_from_env` | `OPENCOMPANY_INFERENCE_KEY` (:184), `OPENCOMPANY_INFERENCE_URL` (:192) |
| `src/harness/roster_build.rs:127-212` | `InternalPass::for_setup` (first-run wizard pass) | `OPENCOMPANY_INFERENCE_URL` (:192-194) when the wizard typed a managed key |
| `src/bin/opencompany.rs:2153-2157` | `serve`'s manual `AppConfig` build | `OPENCOMPANY_INFERENCE_KEY` (:2153) |
| `src/company/credentials.rs:66, 207-212, 223-236` | `TOKEN_FILE_ENV`, `TinyhumansTokenSource::from_env` / `from_parts` | `TINYHUMANS_TOKEN_FILE` |
| `src/app/config.rs:715-719, 745-758, 779, 930-938, 977` | `RuntimeConfig.tinyhumans_token_file`, `credential_source`, `Debug`, `resolve()` | `TINYHUMANS_TOKEN_FILE` via `TOKEN_FILE_ENV` |
| `src/app/doctor.rs:87-91, 106, 132, 150-155` | `value_of`, `FIELDS`, the `credential_source` layer, `cycles.needs` | `tinyhumans_token_file`, `TOKEN_FILE_ENV` |
| `src/harness/built_in/provider.rs:287-381` | `PlatformCredentialStatus` (`projected_tier` :292-293, `resolve` :305-316, `boot_warning` :343-381) | `TokenTier::ProjectedFile`, `TOKEN_FILE_ENV` |
| `src/server/ops/connections_read.rs:54, 128-145` | `HostConnectRoutes.attested` | `TokenTier::ProjectedFile` via `from_env` |
| `src/company/composio.rs:28-32, 45-61` | `COMPOSIO_BACKEND_URL_ENV`, `backend_url_or_default` | `OPENCOMPANY_COMPOSIO_BACKEND_URL` — part 2 §7 |
| `src/harness/built_in/composio.rs:104, 307-326, 371` | re-export, `TenantComposio::resolve(backend_url_env, …)` | part 2 §7 |
| `src/harness/built_in/mod.rs:3345` · `src/runtime/builder.rs:3255-3256` | `resolve_composio` · boot Composio | part 2 §7 |
| `src/server/ops/composio.rs:209-212, 586-593, 1080` | `catalog_cache_key`, `effective_status`, `resolve_tenant` | part 2 §7 |

Doc-comment-only mentions (reword in the same commit as the code they sit on):
`src/harness/built_in/provider.rs:131-146, 166-181, 241-246`,
`src/company/inference.rs:452-465, 3042`,
`src/company/inference/catalogue.rs:753`,
`src/server/ops/inference.rs:238-243, 776-790, 866-871`,
`src/server/setup.rs:233-238, 244-248, 1309-1312`,
`src/company/credentials.rs:1-52`, `src/app/types.rs:240-270`,
`src/harness/built_in/mod.rs:3324-3327`, `src/runtime/builder.rs:3139-3148` (the issue #110 comment above boot's Composio resolve).

Frontend and fixtures: `frontend/playwright.config.ts:198-243`,
`frontend/test/e2e/global-setup.ts`, `frontend/test/e2e/capabilities.ts:51-55`,
`frontend/test/e2e/mock-brain.mjs:119-126, 1074-1089`,
`frontend/test/e2e/live-brain-proxy.mjs:185-215`,
`frontend/test/unit/mock-brain.test.ts` — part 2 §8.

Docs and examples: part 2 §9. Tests: part 2 §10.

Not affected (verified): `docker-compose.yml:28` and `.env.example:24` pass only
`TINYHUMANS_API_KEY`; `deploy/entrypoint.sh` and `.github/workflows/` name none
of the four.

## 2. Current code

`hosted_endpoint_from_env` (`provider.rs:182-195`):

```rust
pub(crate) fn hosted_endpoint_from_env(env: &dyn EnvSource) -> Option<(Credential, String)> {
    let credential = match env
        .get("OPENCOMPANY_INFERENCE_KEY")
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
    {
        Some(key) => Credential::from_value(key),
        None => Credential::from_source(Arc::new(TinyhumansTokenSource::from_env(env)?)),
    };
    let base_url = env
        .get("OPENCOMPANY_INFERENCE_URL")
        .unwrap_or_else(|| DEFAULT_TINYHUMANS_INFERENCE_URL.to_string());
    Some((credential, base_url))
}
```

`InternalPass::for_setup`, the typed-key arm (`roster_build.rs:188-202`):

```rust
let typed = credential
    .map(str::trim)
    .filter(|key| !key.is_empty())
    .map(|key| {
        let base_url = env
            .get("OPENCOMPANY_INFERENCE_URL")
            .unwrap_or_else(|| DEFAULT_TINYHUMANS_INFERENCE_URL.to_string());
```

`TinyhumansTokenSource::from_env` (`credentials.rs:207-212`):

```rust
pub fn from_env(env: &dyn EnvSource) -> Option<Self> {
    Self::from_parts(
        env.get(TOKEN_FILE_ENV).as_deref().map(Path::new),
        env.get(API_KEY_ENV).as_deref(),
    )
}
```

`serve`'s `/spec` credential (`src/bin/opencompany.rs:2153-2157`):

```rust
let tinyhumans_credential = std::env::var("OPENCOMPANY_INFERENCE_KEY")
    .or_else(|_| std::env::var("TINYHUMANS_API_KEY"))
    .ok()
    .filter(|value| !value.trim().is_empty())
    .map(opencompany::ports::types::SecretValue);
```

`HostConnectRoutes::resolve` (`connections_read.rs:139-145`):

```rust
fn resolve(env: &dyn EnvSource) -> Self {
    Self {
        attested: TinyhumansTokenSource::from_env(env).map(|source| source.tier())
            == Some(TokenTier::ProjectedFile),
    }
}
```

## 3. Target code: inference

```rust
/// Resolve the shared hosted-endpoint `(credential, base_url)` pair the managed
/// env-default arm addresses.
///
/// The credential is the platform token source — a static
/// [`API_KEY_ENV`](crate::company::credentials::API_KEY_ENV) — or nothing:
/// **nothing configured ⇒ `None`**. The base URL is always
/// [`DEFAULT_TINYHUMANS_INFERENCE_URL`]. `OPENCOMPANY_INFERENCE_KEY`,
/// `OPENCOMPANY_INFERENCE_URL` and the projected token file were removed in the
/// keys rework (issue #2306, phase 6a); a deployment that needs another
/// endpoint adds a provider row instead.
pub(crate) fn hosted_endpoint_from_env(env: &dyn EnvSource) -> Option<(Credential, String)> {
    let credential = Credential::from_source(Arc::new(TinyhumansTokenSource::from_env(env)?));
    Some((credential, DEFAULT_TINYHUMANS_INFERENCE_URL.to_string()))
}
```

`env` stays a parameter: `from_env` still reads `TINYHUMANS_API_KEY` through it.

`for_setup` (`roster_build.rs:192-194`) — the wizard's typed managed key goes to
the constant:

```rust
.map(|key| {
    let base_url = DEFAULT_TINYHUMANS_INFERENCE_URL.to_string();
```

`harness_inference_from_env`'s doc (`provider.rs:131-146`): delete the
credential and url bullets and the paragraph about `OPENCOMPANY_INFERENCE_KEY`;
the intro becomes "Resolve a [`HostedProvider`] configuration (and its model
override) from the environment. The credential is the static
`TINYHUMANS_API_KEY` or nothing; the URL is always
[`DEFAULT_TINYHUMANS_INFERENCE_URL`]." Keep the model bullet. Apply the same
two-bullet cut to `hosted_endpoint_from_env`'s doc (:166-181) and
`search_backend_from_env`'s credential bullet (:241-246: "the static
`TINYHUMANS_API_KEY`").

`EnvDefault` (`inference.rs:459-466`):

```rust
#[derive(Clone, Debug)]
pub struct EnvDefault {
    /// Always [`DEFAULT_TINYHUMANS_INFERENCE_URL`] in production — no
    /// environment override exists after phase 6a. Tests may set any value.
    pub base_url: String,
    /// The platform token source over a static `TINYHUMANS_API_KEY`.
    pub credential: Credential,
}
```

Keep `base_url` as a field: `ops/inference.rs`'s staging tests build an
`EnvDefault` by hand (`staging_platform`, :2464-2472) and stay valid.

`serve`'s `/spec` credential (`src/bin/opencompany.rs:2150-2157`):

```rust
// Hosted-brain credential, resolved the way the harness resolves it
// (`harness_inference_from_env`) so `/spec`'s `cycles_available` reflects
// whether cognition can run. Only the static platform key remains (#2306 6a).
let tinyhumans_credential = std::env::var("TINYHUMANS_API_KEY")
    .ok()
    .filter(|value| !value.trim().is_empty())
    .map(opencompany::ports::types::SecretValue);
```

`resolve_serve_base_url("TINYHUMANS_API_URL", …)` just below (:2161-2167) is
unchanged.

`PlatformCredentialStatus` (`provider.rs:287-381`): delete the `projected_tier`
field (:292-293) and its computation (:307-309, :312). In `boot_warning`, delete
the `self.projected_tier && !self.media` arm (:356-362); change the `use` at
:344 to `use crate::company::credentials::API_KEY_ENV;`; the no-identity message
(:347-353) becomes:

```rust
"no platform credential resolved: managed inference, embeddings, web_search and \
 media generation are ALL unwired for every company on this deployment \
 (fail-closed). Set a static {API_KEY_ENV}, or give each company a provider key."
```

and the partial message's tail (:377-380, the `See {TOKEN_FILE_ENV} / {API_KEY_ENV}.` text) becomes `"See {API_KEY_ENV}."`.

## 4. Target code: connection routes

`HostConnectRoutes` answered "is this a platform-projected pod?" and nothing
else can answer that after 6a. Delete the `attested` field (struct at :132-136), make
`resolve` return `Self {}`, and delete the `if self.attested` arm of `route`
(:159-161). `route` is now: stored → `Static`, else `None`. Drop `TokenTier`
from the `use` at :54 and rewrite the doc at :87-106 to the two-step rule. The
**behaviour change** this carries is written out in part 2 §14.

## 5. Target code: `TinyhumansTokenSource` becomes static-only

**Decision (not left to the implementer):** keep `CredentialSource::Attested`
and its `"attested"` wire spelling (the `CredentialSource` enum and `as_str`, `credentials.rs:115-150`) — the console's
`CompanyCredentialSource` type (`frontend/src/api/credential.ts:36`) and its
readers (`frontend/src/lib/connections.ts:421`,
`frontend/src/lib/provider-grid.ts:436`) keep compiling and no `frontend/src`
file changes. Production never constructs `Attested` after 6a; say so on the
variant's doc. **Delete** `TokenTier` entirely (`credentials.rs:82-113`, the
re-export at `src/company/mod.rs:179`).

Delete from `src/company/credentials.rs`:

- `TOKEN_FILE_ENV` (:63-66), `MAX_CACHE_WINDOW` (:72-76), `TTL_FRACTION` (:78-80);
- `Cached` (:153-157) and the `Tier` enum (:159-168);
- `projected_file` (:179-187), `tier` (:259-265), `token_file` (:272-278);
- `current_at` (:332-…) — `current` returns the value directly;
- `cache_window` (:533), `unverified_jwt_exp` (:545), `base64url_decode` (:561) —
  `git grep -n "unverified_jwt_exp\|cache_window\|base64url_decode" src` shows no
  caller outside this file;
- the module doc (:1-52), replaced by the three-sentence doc below.

Target:

```rust
//! How this instance obtains its TinyHumans credential: a static
//! [`API_KEY_ENV`] value held for the life of the process. The platform-projected
//! token-file tier was removed in the keys rework (issue #2306, phase 6a); a
//! hosted company now needs its own `provider/tinyhumans/key`. Nothing here ever
//! renders the token.

pub const API_KEY_ENV: &str = "TINYHUMANS_API_KEY";

pub struct TinyhumansTokenSource {
    token: String,
}

impl TinyhumansTokenSource {
    pub fn static_key(token: impl Into<String>) -> Self {
        Self { token: token.into() }
    }

    /// `None` when [`API_KEY_ENV`] is unset or blank (fail closed).
    pub fn from_env(env: &dyn EnvSource) -> Option<Self> {
        Self::from_parts(env.get(API_KEY_ENV).as_deref())
    }

    pub fn from_parts(api_key: Option<&str>) -> Option<Self> {
        let key = api_key?.trim();
        (!key.is_empty()).then(|| Self::static_key(key))
    }

    pub fn credential_source(&self) -> CredentialSource {
        CredentialSource::Static
    }

    pub fn describe(&self) -> String {
        "static (set)".to_string()
    }

    pub fn hash_identity<H: std::hash::Hasher>(&self, hasher: &mut H) {
        use std::hash::Hash;
        self.token.hash(hasher);
    }

    /// Kept as a no-op so `Credential::invalidate` (:458-462) and the 401
    /// handling that calls it (`provider.rs:1562`, `:1949`) need no edit.
    pub fn invalidate(&self) {}

    pub async fn current(&self) -> Result<String> {
        Ok(self.token.clone())
    }
}
```

`source_of_parts` (:240-257) is deleted: its only production caller is
`RuntimeConfig::credential_source` (part 2 §6), which no longer needs it.
`Debug` for the source prints `TinyhumansTokenSource { tier: "static" }` and
never the token.

Continued in [part 2](phase-6a-remove-env-vars-part2.md).
