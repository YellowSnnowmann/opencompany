# Phase 1a — Composio storage keys renamed, dual-written for one release

Slice 1a of [the keys rework](README.md). Continued in
[phase-1a-composio-keys-part2.md](phase-1a-composio-keys-part2.md): write-failure
table, tests, console, docs, must-not-touch, done-when and gotchas.

- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14). Every `file:line`
  below was re-opened on that commit on 2026-09-14. Line numbers drift; every
  step also quotes the code so it can be found by text.
- **Needs #2305 merged?** No. This slice goes first (dump item 8).

## 1. Goal

Store the two Composio credentials at addresses that say what they hold —
`composio/token` becomes `composio/tinyhumans/key` and `composio/api_key`
becomes `composio/byok/key` — while every existing company keeps working:
reads fall back to the old address, and for one release every write also
mirrors to the old address so a rolled-back binary keeps working.

- **Dump items:** 7 (rename), 8 (rename before the account-key fan-out).
- **Decisions:** Q11 (keep `composio/mode`), Q12 (`byok`, not `byo`).
- **Also decided for this slice (2026-09-14, orchestrator):**
  - **HTTP routes and request bodies do not change.** `PUT …/composio/token
    {token}`, `PUT …/composio/api-key {apiKey?, skipVerify?}` and
    `POST …/composio/api-key/test` stay byte for byte. So `tests/auth_matrix.rs`,
    `tests/snapshots/auth-matrix.txt` and `frontend/src/api/composio.ts` do not
    move.
  - **Journal vocabulary does not change** (`credential_set`,
    `credential_cleared`, `composio_byok_set`, `composio_byok_cleared`).
  - **`composio/mode` and `composio/defaults` keep their addresses.**
  - **Dual-write for one release**, matching slice 1b: a write stores the same
    value at both addresses, a clear clears both. Stopping the legacy write is
    a later-release follow-up (part 2 §11), not this PR.

## 2. Files and symbols (verified on `fcfb3e1bc`)

| File | Line(s) | Symbol | What changes |
|---|---|---|---|
| `src/company/composio.rs` | 23-26 | `pub const TOKEN_KEY` | replaced by `TINYHUMANS_KEY_KEY` + `LEGACY_TOKEN_KEY` |
| `src/company/composio.rs` | 174-181 | `pub const API_KEY_KEY` | replaced by `BYOK_KEY_KEY` + `LEGACY_API_KEY_KEY` |
| `src/company/composio.rs` | 66-74 | `pub async fn store_token` | writes both addresses via `write_both` |
| `src/company/composio.rs` | 98-115 | `pub async fn resolve_credential` | reads through `load_tinyhumans_key` |
| `src/company/composio.rs` | 140-146 | `pub async fn token_configured` | reads through `load_tinyhumans_key` |
| `src/company/composio.rs` | 320-347 | `pub async fn store_api_key` | same direction order; key written to both addresses |
| `src/company/composio.rs` | 393-414 | `pub async fn resolve_access` (BYOK arm `:401-404`) | reads through `load_byok_key` |
| `src/company/composio.rs` | 172, 427 | `MODE_KEY`, `DEFAULTS_KEY` | **unchanged** |
| `src/company/composio.rs` | 618-1132 | `mod tests` | existing tests re-pointed, new tests added (part 2) |
| `src/server/ops/composio.rs` | 949-967 | `async fn stored_api_key` | reads through `load_byok_key` |
| `src/harness/built_in/composio.rs` | 101-106 | `pub use crate::company::composio::{…}` | re-exports renamed |
| `src/harness/built_in/mod.rs` | 538 | doc link to `composio::TOKEN_KEY` | link renamed (other doc comments: §5 step 8) |

Every use of the two old constants (`git grep -n 'TOKEN_KEY\|API_KEY_KEY'
fcfb3e1bc -- src tests`): writers `composio.rs:72, 333, 343`; readers
`composio.rs:103, 142, 401` and `ops/composio.rs:961`; re-export
`harness/built_in/composio.rs:104-105`; tests (part 2 §7.1); doc comments (§5
step 8).

There is no other string literal `"composio/…"` in `src`, `tests`,
`frontend/src`, `frontend/test` or `scripts` except the four `const`
definitions (`git grep -n '"composio/' -- src tests frontend/src frontend/test
scripts | grep -v /api/v1/`). No store backend, migration or export list names
these keys.

## 3. Current code (excerpts, `fcfb3e1bc`)

`src/company/composio.rs:26`, `:181`:

```rust
pub const TOKEN_KEY: &str = "composio/token";
pub const API_KEY_KEY: &str = "composio/api_key";
```

`src/company/composio.rs:66-74` — the managed token write:

```rust
pub async fn store_token(company: &CompanyId, secrets: &dyn SecretStore, token: &str) -> Result<()> {
    secrets
        .set(company, TOKEN_KEY, SecretValue(token.trim().to_string()))
        .await
}
```

`src/company/composio.rs:103-114` — the BYO tier of managed resolution:

```rust
    let byo = match secrets.get(company, TOKEN_KEY).await? {
        Some(SecretValue(token)) => Credential::from_value(token),
        None => Credential::None,
    };
    Ok(match byo {
        byo @ Credential::Value(_) => byo,
        _ => company_key::resolve(company, secrets, token_source).await?,
    })
```

`src/company/composio.rs:140-146`:

```rust
pub async fn token_configured(company: &CompanyId, secrets: &dyn SecretStore) -> Result<bool> {
    Ok(secrets.get(company, TOKEN_KEY).await?
        .map(|SecretValue(token)| !token.trim().is_empty())
        .unwrap_or(false))
}
```

`src/company/composio.rs:331-345` — direction-ordered mode write:

```rust
    if mode.is_byok() {
        secrets.set(company, API_KEY_KEY, SecretValue(api_key.to_string())).await?;
        secrets.set(company, MODE_KEY, SecretValue(mode.as_str().to_string())).await?;
    } else {
        secrets.set(company, MODE_KEY, SecretValue(mode.as_str().to_string())).await?;
        secrets.set(company, API_KEY_KEY, SecretValue(api_key.to_string())).await?;
    }
```

`src/company/composio.rs:399-405` — the BYOK arm never falls back:

```rust
    let credential = match mode {
        ComposioMode::Managed => resolve_credential(company, secrets, token_source).await?,
        ComposioMode::Byok => match secrets.get(company, API_KEY_KEY).await? {
            Some(SecretValue(key)) => Credential::from_value(key),
            None => Credential::None,
        },
    };
```

`src/server/ops/composio.rs:949-967`:

```rust
async fn stored_api_key(runtime: &CompanyRuntime) -> Result<Option<String>, ApiError> {
    use crate::company::composio::{API_KEY_KEY, load_mode};
    if !load_mode(runtime.id(), runtime.secrets().as_ref()).await.map_err(ApiError)?.is_byok() {
        return Ok(None);
    }
    let stored = runtime.secrets().get(runtime.id(), API_KEY_KEY).await.map_err(ApiError)?;
    Ok(stored
        .map(|crate::ports::types::SecretValue(key)| key.trim().to_string())
        .filter(|key| !key.is_empty()))
}
```

The error-handling pattern followed is `store_provider_key` in
`src/company/search/store.rs:569-591`: a failed legacy write is logged with key
names only and the error is **returned** (`return Err(err)`), not swallowed.

The port (`src/ports/secrets.rs:11-16`) has only `get` and `set`. There is no
delete: a clear is a `set` of `""`. `Credential::from_value`
(`src/company/credentials.rs:403-410`) already maps a blank value to
`Credential::None`.

## 4. Target code

### 4.1 Constants (replace `TOKEN_KEY` at `:23-26` and `API_KEY_KEY` at `:174-181`)

Names are the README's shared naming contract. Do not invent synonyms.

```rust
/// The TinyHumans bearer this company's **managed** Composio calls present — a
/// credential the *TinyHumans backend* recognises. Written by
/// `PUT …/composio/token` through [`store_token`]; read through
/// [`load_tinyhumans_key`]. Write-only over the API, never echoed.
pub const TINYHUMANS_KEY_KEY: &str = "composio/tinyhumans/key";

/// Pre-rename address of [`TINYHUMANS_KEY_KEY`] (issue #2306): its read fallback
/// only, never [`BYOK_KEY_KEY`]'s. For one release every write to that key also
/// stores the same value here, so a rolled-back binary keeps working.
pub const LEGACY_TOKEN_KEY: &str = "composio/token";

/// This company's **own** Composio API key (`ak_…`) — a credential *Composio*
/// recognises. Written by `PUT …/composio/api-key` through [`store_api_key`];
/// read through [`load_byok_key`]. Write-only over the API, never echoed.
///
/// Distinct from [`TINYHUMANS_KEY_KEY`], and not interchangeable with it: they
/// authenticate different hosts.
pub const BYOK_KEY_KEY: &str = "composio/byok/key";

/// Pre-rename address of [`BYOK_KEY_KEY`] (issue #2306): its read fallback only,
/// never [`TINYHUMANS_KEY_KEY`]'s. For one release every write to that key also
/// stores the same value here, so a rolled-back binary keeps working.
pub const LEGACY_API_KEY_KEY: &str = "composio/api_key";
```

`TOKEN_KEY` and `API_KEY_KEY` are **deleted**; no alias is kept (placement: §5
steps 1 and 3).

### 4.2 Private helpers (new, place directly after the constants block at `:26`)

```rust
/// Reads `key`, falling back to `legacy_key` only when `key` holds nothing.
///
/// "Holds nothing" is *absent or blank after trim*. The value returned is the
/// stored string exactly as stored (not trimmed) — callers keep their own
/// handling. Reads never write: nothing is migrated on read.
///
/// A read error on either address **propagates** (see [`resolve_credential`]:
/// an unreadable store must not change which account a call is attributed to).
/// The legacy address is not read at all when `key` holds a value.
///
/// Private on purpose: the only callers are [`load_tinyhumans_key`] and
/// [`load_byok_key`], which fix the mapping so the two pairs can never be
/// crossed.
async fn read_with_legacy(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    key: &'static str,
    legacy_key: &'static str,
) -> Result<Option<String>> {
    if let Some(SecretValue(value)) = secrets.get(company, key).await?
        && !value.trim().is_empty()
    {
        return Ok(Some(value));
    }
    Ok(secrets
        .get(company, legacy_key)
        .await?
        .map(|SecretValue(value)| value)
        .filter(|value| !value.trim().is_empty()))
}

/// Writes `value` to `key`, then the same `value` to `legacy_key`. `value` may
/// be `""`: a clear clears both.
///
/// The legacy write exists for **one release** (issue #2306): a rolled-back
/// binary reads only `legacy_key`, and must find what this binary stored. A
/// later release stops mirroring; that is a follow-up, not part of #2306.
///
/// Order is fixed: new address first, legacy second, so a failure between the
/// two leaves the new value in place and winning on read. The legacy write is
/// unconditional — no read first, so no check-then-write window. A failed
/// legacy write is logged (key names only, never a value) and **propagated**,
/// as `search::store::store_provider_key` does, so the caller reports a failed
/// write and an admin can retry.
async fn write_both(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    key: &'static str,
    legacy_key: &'static str,
    value: &str,
) -> Result<()> {
    secrets
        .set(company, key, SecretValue(value.to_string()))
        .await?;
    if let Err(err) = secrets
        .set(company, legacy_key, SecretValue(value.to_string()))
        .await
    {
        tracing::error!(
            company = %company,
            key = key,
            legacy_key = legacy_key,
            "[composio] wrote the credential's new address but not its legacy address; \
             reporting the write as failed so it can be retried: {err}"
        );
        return Err(err);
    }
    Ok(())
}
```

### 4.3 Public readers (new, directly after the helpers)

```rust
/// The stored managed-Composio TinyHumans bearer: [`TINYHUMANS_KEY_KEY`], else
/// [`LEGACY_TOKEN_KEY`]. `None` when both are absent or blank. Never reads
/// [`BYOK_KEY_KEY`] or [`LEGACY_API_KEY_KEY`].
pub async fn load_tinyhumans_key(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<Option<String>> {
    read_with_legacy(company, secrets, TINYHUMANS_KEY_KEY, LEGACY_TOKEN_KEY).await
}

/// The stored BYOK Composio API key: [`BYOK_KEY_KEY`], else
/// [`LEGACY_API_KEY_KEY`]. `None` when both are absent or blank. Never reads
/// [`TINYHUMANS_KEY_KEY`] or [`LEGACY_TOKEN_KEY`].
pub async fn load_byok_key(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<Option<String>> {
    read_with_legacy(company, secrets, BYOK_KEY_KEY, LEGACY_API_KEY_KEY).await
}
```

`load_tinyhumans_key` and `load_byok_key` are **new public names** not in the
README's naming contract. They are needed because `stored_api_key` lives in
another module and the mapping must stay fixed; do not rename them.

### 4.4 Rewritten functions

`store_token` (`:66-74`) — signature unchanged:

```rust
pub async fn store_token(company: &CompanyId, secrets: &dyn SecretStore, token: &str) -> Result<()> {
    write_both(company, secrets, TINYHUMANS_KEY_KEY, LEGACY_TOKEN_KEY, token.trim()).await
}
```

`resolve_credential` (`:103-106`) — only the first `let` changes:

```rust
    let byo = match load_tinyhumans_key(company, secrets).await? {
        Some(token) => Credential::from_value(token),
        None => Credential::None,
    };
```

`token_configured` (`:140-146`):

```rust
pub async fn token_configured(company: &CompanyId, secrets: &dyn SecretStore) -> Result<bool> {
    Ok(load_tinyhumans_key(company, secrets).await?.is_some())
}
```

`store_api_key` (`:320-347`) — signature and return unchanged; the body becomes:

```rust
    let api_key = api_key.trim();
    let mode = if api_key.is_empty() { ComposioMode::Managed } else { ComposioMode::Byok };
    if mode.is_byok() {
        // 1. BYOK_KEY_KEY = key   2. LEGACY_API_KEY_KEY = key   3. MODE_KEY = "byok"
        // The mode flip is LAST: until it lands the company is still on its old
        // route, so a failure at 1 or 2 leaves a managed company managed.
        write_both(company, secrets, BYOK_KEY_KEY, LEGACY_API_KEY_KEY, api_key).await?;
        secrets.set(company, MODE_KEY, SecretValue(mode.as_str().to_string())).await?;
    } else {
        // 1. MODE_KEY = "managed"   2. BYOK_KEY_KEY = ""   3. LEGACY_API_KEY_KEY = ""
        // The mode flip is FIRST: a failure at 2 or 3 leaves a managed company
        // holding a stale BYOK key, which managed resolution never reads.
        secrets.set(company, MODE_KEY, SecretValue(mode.as_str().to_string())).await?;
        write_both(company, secrets, BYOK_KEY_KEY, LEGACY_API_KEY_KEY, "").await?;
    }
    Ok(mode)
```

`resolve_access` BYOK arm (`:401-404`):

```rust
        ComposioMode::Byok => match load_byok_key(company, secrets).await? {
            Some(key) => Credential::from_value(key),
            None => Credential::None,
        },
```

The `if mode.is_byok() && !credential.configured()` warning at `:406-412` is
unchanged: BYOK with nothing at either BYOK address still withholds tools and
never falls back to managed.

`stored_api_key` (`src/server/ops/composio.rs:949-967`):

```rust
async fn stored_api_key(runtime: &CompanyRuntime) -> Result<Option<String>, ApiError> {
    use crate::company::composio::{load_byok_key, load_mode};

    if !load_mode(runtime.id(), runtime.secrets().as_ref())
        .await
        .map_err(ApiError)?
        .is_byok()
    {
        return Ok(None);
    }
    let stored = load_byok_key(runtime.id(), runtime.secrets().as_ref())
        .await
        .map_err(ApiError)?;
    Ok(stored
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty()))
}
```

Harness re-export (`src/harness/built_in/composio.rs:103-106`):

```rust
pub use crate::company::composio::{
    BYOK_KEY_KEY, COMPOSIO_BACKEND_URL_ENV, ComposioMode, DIRECT_BASE_URL, TINYHUMANS_API_URL_ENV,
    TINYHUMANS_KEY_KEY, backend_url_or_default, resolve_access, resolve_credential,
};
```

Do **not** re-export `LEGACY_TOKEN_KEY` / `LEGACY_API_KEY_KEY` from the harness;
tests that need them import `crate::company::composio::{LEGACY_…}` directly.

## 5. Ordered edit list

1. `src/company/composio.rs`: replace the `TOKEN_KEY` const and its doc
   (`:23-26`) with `TINYHUMANS_KEY_KEY` + `LEGACY_TOKEN_KEY` (§4.1).
2. Same file: add `read_with_legacy`, `write_both`, `load_tinyhumans_key`,
   `load_byok_key` (§4.2, §4.3) right after that block.
3. Same file: replace the `API_KEY_KEY` const and doc (`:174-181`) with
   `BYOK_KEY_KEY` + `LEGACY_API_KEY_KEY` (§4.1).
4. Same file: rewrite `store_token`, `resolve_credential`'s first `let`,
   `token_configured`, `store_api_key`'s body, and `resolve_access`'s BYOK arm
   (§4.4). Keep every existing doc paragraph; only rename keys inside them
   (step 8).
5. `src/server/ops/composio.rs:949-967`: rewrite `stored_api_key` (§4.4).
   The `use crate::company::composio::{…}` block at `:73-76` is unchanged
   (`store_api_key`, `store_token` keep their names).
6. `src/harness/built_in/composio.rs:103-106`: rename the re-exports (§4.4).
7. Tests: change existing ones, then add new ones —
   [part 2 §7](phase-1a-composio-keys-part2.md#7-tests).
8. Doc comments that name the old constants or addresses, each rewritten to
   the new name (write "`composio/tinyhumans/key` (read fallback
   `composio/token`)" the first time per module doc, the new name alone
   elsewhere):
   - `src/company/composio.rs:7` `[`TOKEN_KEY`]` → `[`TINYHUMANS_KEY_KEY`]`;
     `:90`, `:117`, `:123`, `:178`, `:203`, `:212`, `:256`, `:425`, `:560`
     same rename; `:256`, `:310`, `:382` `[`API_KEY_KEY`]` →
     `[`BYOK_KEY_KEY`]`. The `:382` sentence becomes "**BYOK** reads
     [`BYOK_KEY_KEY`] (falling back to [`LEGACY_API_KEY_KEY`]) and nothing
     else."
   - `src/harness/built_in/composio.rs:33` and `:298` `[`TOKEN_KEY`]` →
     `[`TINYHUMANS_KEY_KEY`]`.
   - `src/harness/built_in/composio_direct.rs:10`
     `[`API_KEY_KEY`](crate::company::composio::API_KEY_KEY)` →
     `[`BYOK_KEY_KEY`](crate::company::composio::BYOK_KEY_KEY)`.
   - `src/harness/built_in/mod.rs:538`
     `[`composio::TOKEN_KEY`](crate::harness::composio::TOKEN_KEY)` →
     `[`composio::TINYHUMANS_KEY_KEY`](crate::harness::composio::TINYHUMANS_KEY_KEY)`.
   - `src/server/ops/composio.rs:56`
     `[`TOKEN_KEY`](crate::company::composio::TOKEN_KEY)` →
     `[`TINYHUMANS_KEY_KEY`](crate::company::composio::TINYHUMANS_KEY_KEY)`;
     `:332` "`composio/token`" → "`composio/tinyhumans/key` (then
     `composio/token`)".
   - Storage-address prose (not routes): `src/company/company_key.rs:35`,
     `src/server/ops/company_key.rs:191`, `src/harness/built_in/build.rs:583`
     and `:623`, `src/harness/built_in/planning.rs:883` and `:1411`,
     `src/harness/built_in/planning/test.rs:1414`,
     `src/server/ops/capabilities.rs:90`, `:343`, `:1284`,
     `src/server/ops/company_key/test.rs:241`: "`composio/token`" →
     "`composio/tinyhumans/key`".
   - **Leave alone** every comment that names a **route**
     (`PUT …/composio/token`, `PUT …/composio/api-key`,
     `POST …/composio/api-key/test`), e.g. `ops/composio.rs:12, 43, 271, 644,
     702, 873, 884, 969`, `composio_probe.rs:225`.
9. Console comment edits — [part 2 §8](phase-1a-composio-keys-part2.md#8-consoleui).
10. Docs — [part 2 §8.1](phase-1a-composio-keys-part2.md#81-docs-to-update).
11. `cargo fmt --all`, then `cargo fmt --all -- --check`. Frontend:
    `npm run typecheck`, `npm run typecheck:unit`, `npm run typecheck:e2e`
    (comment-only edits, but all three gates run on CI). Commit, push, verify
    CI by head SHA (README "Rules for the implementer").

## 6. Data carry-over

**Rules.**

- **Read (both pairs, fixed mapping):** new address if non-empty after trim,
  else legacy address if non-empty after trim, else nothing.
  `TINYHUMANS_KEY_KEY ← LEGACY_TOKEN_KEY`, `BYOK_KEY_KEY ← LEGACY_API_KEY_KEY`.
  Never crossed. Reads never write. A read error on the new address propagates.
- **Write a value:** `set(new, value)`, then `set(legacy, value)` — the same
  value, for one release. Both errors propagate; the second is also logged
  (key names only).
- **Clear:** `set(new, "")`, then `set(legacy, "")`. Same error handling.
- **No boot-time migration.** Nothing copies or blanks on startup. A company
  that never saves again reads its legacy value forever, which is correct.
- **Rollback:** a pre-1a binary reads only the legacy address. After any save
  on this build that address holds the **same value** as the new one, so the
  old binary keeps working with no action.

**Why a blank new address still falls back to legacy.** The store cannot tell
"never written" from "cleared" on every backend (a clear is a `set` of `""`;
there is no delete). Under the write order above, a blank new address beside a
non-empty legacy one can only arise two ways:

1. A clear whose legacy `set` failed. The route already returned an error, so
   the admin knows the clear did not finish. Reading the legacy value keeps
   the page honest: the credential is still reported (for example
   `credentialSource: "static"`) until a retry clears both slots. A "blank new
   shadows legacy" rule would report "cleared" while the secret still sits at
   the legacy address.
2. A pre-1a binary (rollback) wrote the legacy address after a 1a binary had
   cleared the new one. Falling back honours that newer write.

In the set direction the new address is written first, so any failure leaves
the new value non-empty and winning. Hence: **read new if non-empty, else
legacy if non-empty.**

**Before/after stored values** (fake values; `—` = never written):

| # | Before | Action | After | Read result |
|---|---|---|---|---|
| 1 | `composio/token`=`th-not-a-real-key`; new `—` | none (boot, turns, status) | unchanged | tinyhumans key = `th-not-a-real-key` |
| 2 | as 1 | `PUT …/composio/token {"token":"th-not-a-real-key-2"}` | `composio/tinyhumans/key`=`th-not-a-real-key-2`; `composio/token`=`th-not-a-real-key-2` | `th-not-a-real-key-2` |
| 3 | as 2 | `PUT …/composio/token {"token":""}` | both `""` | none → `company_key::resolve` |
| 4 | `composio/tinyhumans/key`=`th-a`; `composio/token`=`th-b` | none | unchanged | `th-a` (new wins) |
| 5 | `composio/mode`=`byok`; `composio/api_key`=`ak-not-a-real-key`; new `—` | none | unchanged | BYOK key = `ak-not-a-real-key` |
| 6 | as 5 | `PUT …/composio/api-key {"apiKey":"ak-not-a-real-key-2"}` | `composio/byok/key`=`ak-not-a-real-key-2`; `composio/api_key`=`ak-not-a-real-key-2`; `composio/mode`=`byok` | `ak-not-a-real-key-2` |
| 7 | as 6 | `PUT …/composio/api-key {"apiKey":""}` | `composio/mode`=`managed`; both BYOK addresses `""` | managed chain |
| 8 | `composio/mode`=`managed`; `composio/api_key`=`ak-not-a-real-key` only | none | unchanged | managed chain; the `ak_…` value is **never** presented as the TinyHumans bearer |
| 9 | `composio/mode`=`byok`; `composio/token`=`th-not-a-real-key` only | none | unchanged | BYOK has no key → no tools, warning; never the `th_…` value |
| 10 | as 1 | token `PUT`, legacy `set` fails | new=`th-not-a-real-key-2`; legacy=`th-not-a-real-key`; route answers 500 | `th-not-a-real-key-2` (a pre-1a binary would still read the old value until a retry) |
| 11 | new=`th-a`; legacy=`th-b` | token clear, legacy `set` fails | new=`""`; legacy=`th-b`; route answers 500 | `th-b` until a retry succeeds |
| 12 | as 2 or 6 | roll back to a pre-1a binary | unchanged | old binary reads `composio/token` / `composio/api_key` = the saved value: keeps working |

`store_api_key` partial failures, write by write:
[part 2 §6.1](phase-1a-composio-keys-part2.md#61-store_api_key-partial-failures).
