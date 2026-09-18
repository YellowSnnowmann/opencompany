# Phase 2a — TinyHumans is a provider row on the proxy

This is the first inference slice of the keys rework (issue #2306). It does
what closed PR #2305 set out to do, without the parts that went wrong.

- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14). Every `file:line`
  was reopened on that commit on 2026-09-14.
- **Continues in:** [part 2](phase-2a-tinyhumans-on-proxy-part2.md) (§5.6–§7:
  add rules, status, 3b, console, edits, carry-over) and
  [part 3](phase-2a-tinyhumans-on-proxy-part3.md) (§8–§12: tests, UI,
  must-not-touch, done-when, gotchas).
- **Next slices:** [2b](phase-2b-default-shape.md) ·
  [2c](phase-2c-model-required.md) · [2d](phase-2d-no-tier-on-the-wire.md).
- **#2305 is closed.** Do not merge, cherry-pick or copy its branch
  (`feat/managed-openrouter-proxy`, `f4c42ea48`). All the code this slice needs
  is written out here.

## 0. Read first: step 3b is gated

Step 3b moves `PLATFORM_BASE_URL` and `DEFAULT_TINYHUMANS_INFERENCE_URL` to the
proxy, which moves all **legacy** managed traffic there too. On `fcfb3e1bc`
that traffic sends tier names as model ids, and the proxy refuses a tier name
(checked 2026-09-14: `chat-v1` gets a 400).

- **Tier names are sent by** the setup probe and the setup brain
  (`DEFAULT_HOSTED_MODEL = "chat-v1"`, `src/harness/built_in/provider.rs:58`,
  `:2245-2252`; `src/harness/roster_build.rs:201-208`), and by every turn with
  no override (`model_for_tier`, `src/company/inference.rs:360-378`).
- **Hosted tenants use the constant** unless `OPENCOMPANY_INFERENCE_URL` is set
  (`provider.rs:188-197`). The list of manager-injected variables in `CLAUDE.md`
  does not include it. That is inferred; the manager code was not read.
- **README** goal 9 and decision D-legacy keep the env default on `/openai/v1`.

**Everything else in this slice is safe without 3b.** The new `tinyhumans` row
reaches the proxy through its own catalogue endpoint, whatever the constants
say. 3b is in part 2 §5.9. It is the **last** commit, and it runs only after the
operator answers part 3 §12.1.

E2E hosts are unaffected either way: they set
`OPENCOMPANY_INFERENCE_URL=http://…/v1` (`frontend/playwright.config.ts:221`,
`:226`).

## 1. Goal

TinyHumans becomes an ordinary row in `inference/providers` with slug
`tinyhumans`. It is added through the same dialog as OpenRouter: type the key,
the probe runs, pick a model from whatever the endpoint lists (or type one),
save with health recorded.

Every model-list and chat call **that row** makes goes to
`https://api.tinyhumans.ai/agent-integrations/openrouter`. The LLM page shows
exactly one TinyHumans row. The legacy managed chain and its routes are
unchanged.

**Dump items** (`/Volumes/T9/oc-runs/operator-dump-2026-09-14.md`):

- **13b** "move everything to /agent integration/open router": the row does it
  now; the constants follow in gated 3b.
- **14** "same flow as OpenRouter: add the key … select the model".
- **15** one "is it set?" rule: a row means set (D-set), and the model lives in
  the row's `models` field (Q3).

**Model ids.** Any id returned by `GET /agent-integrations/openrouter/models` is
valid. The code lists exactly what the endpoint returns and sends exactly what
was chosen. It never hardcodes, filters, prefers or rejects an id by vendor or
by name.

**Use cases.** The "Today" column is read from code, not checked in a browser.

| Company | Today on `fcfb3e1bc` | After this slice |
|---|---|---|
| Fresh company, adds TinyHumans | "Managed (TinyHumans)" → `PUT …/inference/managed/key`; no probe, model or record | "TinyHumans" catalogue option → key → the endpoint's model list → one row with `models` and health |
| Only the account key `tinyhumans/key` | Legacy Managed row "Billed to this company's TinyHumans account" | Unchanged |
| Legacy key at `provider/tinyhumans/key`, no row | Legacy Managed row | Unchanged until TinyHumans is added. The add replaces the key and the legacy row goes (part 2 §7) |
| Entry zero `inference/config = {provider: managed}` | Entry-zero "Managed" row **plus** the legacy row when the chain resolves (`ProviderList.tsx:253`, `:319`) | Entry-zero row only; adding TinyHumans gets a 400 before any write |
| Hosted tenant, nothing configured | Legacy Managed row "Billed to whoever runs this server" | Unchanged (3b is gated) |

## 2. Do not do this (lessons from #2305)

1. **No new secret-store keys.** #2305's `inference/managed/models` was
   rejected. The model goes in the row's `models`.
2. **No Managed-only model dialog, new draft-probe route, `proxied_model` rule,
   or URL-origin derivation module.** `OPENCOMPANY_INFERENCE_URL` is used as
   given.
3. **No `catalog_shape` field on every `CloudProvider` row.** That means 27 Rust
   rows plus the TS mirror the mirror test parses. One function instead (§5.1).
4. **Never two TinyHumans rows** (part 2 §5.7, §5.10).
5. **Never skip the model step, and never leave health `unchecked`.** A
   TinyHumans add without a key or without a model is refused before any write
   (part 2 §5.6), so the probe always runs. The step must not depend on catalog
   content.
6. **Never a tier name from the new row.** `tier_overrides` writes the chosen id
   to all four tiers (`providers.rs:680-688`), and `model_for_tier` returns it
   (`inference.rs:365-367`). Slice 2d owns the general rule.
7. **No assumptions about which ids the catalog holds,** in code, docs or tests.
   Tests use fake ids served by a mock catalog (`acme/test-model`,
   `acme/other-model`) and the fake key `th-not-a-real-key`.
8. **Leave these alone:** the managed chain, the legacy managed routes, and
   `inference/managed/enabled` (items 3 and 10 are not handled here).
9. **Do not flip the constants** before the §0 gate is answered.

## 3. Files (anchors on `fcfb3e1bc`)

| File | Change | Anchors |
|---|---|---|
| `src/company/inference/catalogue.rs` | `CatalogShape`, `TINYHUMANS_PROXY_PATH`, `catalog_shape_for`, row, docs, tests | `CloudProvider` :77-92; `CLOUD_PROVIDERS` :110 (last row `modelscope`); `auth_style_for` :799-810; `INTERNAL_SLUGS` :986-1000; tests :1077, :1257-1271, :1478 |
| `src/company/inference/paged_catalog.rs` | **new** pure parser | — |
| `src/company/inference.rs` | `pub mod paged_catalog;`; constant (3b) | modules ~:28-32; `PLATFORM_BASE_URL` :152; `MANAGED_SLUG` :416 |
| `src/company/inference/probe.rs` | `shape` param, paged loop, `probe_get`, `read_capped_to`, `ProbeFailure::unreadable` | `ProbeFailure` impl ~:694-725; `probe_models` :774-876; `read_capped` :1000; test call :1373 |
| `src/server/inference_models.rs` | `shape` on 4 fns, paged fetch, shaped cache key | `discover_models` :298-383; `fetch_catalog` :390-431; `cache_key` :451; `catalog_models` :536-617; `discovered_vocabulary` :631; `turn_vocabulary` :666; test calls :718, :766, :801, :971 |
| `src/server/ops/inference/providers.rs` | shape at 5 reads; key and model required for TinyHumans; restore a replaced key | `add_provider` :309-521 (dup :343-348, key write :352-362, rollbacks :389-391/:443/:481, health :452/:489); `plan_add` cloud :717-726; probes :1550, :1805, :1889; catalog :1642 |
| `src/server/ops/inference.rs` | shape in `resolved_endpoint`/`test_config`; `ManagedDto.legacy_row` | `resolved_endpoint` :141-167; `list_models` :171/:192; `ManagedDto` :366-390; `effective_status_with` :876-878; `managed_state` :951-990; `test_config` :1342 |
| `src/harness/built_in/provider.rs` | shape at turn vocabulary; parser test; constant (3b) | `DEFAULT_TINYHUMANS_INFERENCE_URL` :55; call :2125-2130; `model_response_from_payload` :953 |
| `src/server/setup.rs` | shape at wizard discovery | :1296 |
| `frontend/src/inference/*`, `frontend/src/api/inference.ts` | console (part 2 §5.10) | part 2 |
| `docs/modules/inference/catalogue.md` | 27 rows | :19, :50, :52-56 |
| `docs/spec/runtime/config.md`, `docs/modules/openhuman/README.md` | default URL (3b only) | :122, :81 |

## 4. Current code (excerpts)

The reader expects one OpenAI-shaped page (`probe.rs:853`). The parser fails on
the envelope because there `data` is an object (`inference_models.rs:222-226`,
`:427`):

```rust
let url = format!("{base}/models{}", catalogue::catalog_query(base));
struct RegistryResponse { #[serde(default)] data: Vec<serde_json::Value> }
```

The duplicate check already covers an entry zero whose slug is `tinyhumans`
(`providers.rs:343-348`):

```rust
} else if existing.iter().any(|p| p.slug == plan.slug) {
    return Err(ApiError(OpenCompanyError::InvalidRequest(format!(
        "{} is already connected. Edit the existing row rather than adding a second one.",
        plan.label))));
```

The add backstop at `providers.rs:442` depends on catalog **content**
(`needs_an_explicit_model`, `:668-670`, via `TierVocabulary::from_catalog_ids`),
so it cannot guarantee a model step. An indexed row resolves direct with
`proxied = false` (`inference.rs:1431-1440`), and `normalize_provider` passes
`tinyhumans` through (`:382-387`).

## 5. Target code (Rust)

### 5.1 `catalogue.rs`

Above `pub enum AuthStyle` (:46):

```rust
/// The shape a provider's `GET {base}/models` answers in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CatalogShape {
    /// `{"data":[{"id":…}]}`, one response.
    OpenAi,
    /// `{"success":true,"data":{"data":[…],"total":N,"limit":L,"offset":O}}`, paged.
    /// See [`super::paged_catalog`].
    PagedEnvelope,
}

/// The path the TinyHumans OpenRouter proxy is served under.
pub const TINYHUMANS_PROXY_PATH: &str = "/agent-integrations/openrouter";
```

Above `pub fn auth_style_for` (:799):

```rust
/// Which catalog shape to read at `base_url` for a provider of `kind`.
///
/// `tinyhumans` always pages. Any other kind pages only when its endpoint's path
/// ends in [`TINYHUMANS_PROXY_PATH`]: the legacy managed declaration has kind
/// `openrouter`, and after step 3b (or with `OPENCOMPANY_INFERENCE_URL` set to
/// the proxy) its base is the proxy. A read-only path check; no URL is built.
pub fn catalog_shape_for(kind: &str, base_url: &str) -> CatalogShape {
    if kind.trim() == super::MANAGED_SLUG
        || base_url.trim().trim_end_matches('/').ends_with(TINYHUMANS_PROXY_PATH)
    {
        CatalogShape::PagedEnvelope
    } else {
        CatalogShape::OpenAi
    }
}
```

Append as the **last** `CLOUD_PROVIDERS` entry, after `modelscope`:

```rust
    CloudProvider {
        slug: "tinyhumans",
        label: "TinyHumans",
        endpoint: "https://api.tinyhumans.ai/agent-integrations/openrouter",
        auth: AuthStyle::Bearer,
        key_placeholder: Some("th-..."),
    },
```

Docs to update:

- `:94`: "The 26 hosted providers" becomes "The 27 hosted providers".
- Module doc `:40-44`: "TinyHumans is a row (slug `tinyhumans`): its OpenRouter
  proxy, bearer key, paged catalog. The legacy managed *chain* keeps its own
  auth path (`super::PLATFORM_BASE_URL`)."
- `INTERNAL_SLUGS` doc `:986-999`: add "`tinyhumans` is also a cloud row now; it
  stays listed so reservation does not depend on the table."

### 5.2 `paged_catalog.rs` (new)

Register `pub mod paged_catalog;` in `src/company/inference.rs` next to
`pub mod catalogue;`.

```rust
//! The TinyHumans proxy's paged model catalog:
//! `{"success":true,"data":{"object":"list","data":[…],"total":N,"limit":L,"offset":O}}`.
//! Pure; no I/O. Readers: `probe::probe_models`, `inference_models::discover_models`.
use std::collections::HashSet;

/// Page size requested. The backend clamps `limit` to `[1, 500]`.
pub const PAGE_LIMIT: usize = 500;
/// Most pages one read follows, so a `total` never reached cannot loop.
pub const MAX_PAGES: usize = 20;
/// Largest success body read for one page.
pub const PAGE_BODY_CAP: usize = 4 * 1024 * 1024;

pub fn page_path(offset: usize) -> String { format!("/models?limit={PAGE_LIMIT}&offset={offset}") }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry { pub id: String, pub name: Option<String>, pub context_length: Option<u64> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPage {
    pub entries: Vec<CatalogEntry>,
    /// Entries the page carried, usable or not. Paging advances by this.
    pub raw_len: usize,
    pub total: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextPage { At(usize), Done, Truncated { read: usize, total: usize } }

/// One page. `Err` (never an empty page) on: not JSON; `success: false`; no object
/// `data`; no array `data.data` — so a plain `{"data":[…]}` body is an error.
/// Every id is kept exactly as given. Unknown fields are ignored.
pub fn parse_page(body: &str) -> Result<CatalogPage, String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| format!("the model catalog was not JSON: {e}"))?;
    if value.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        let reason = value.get("error").or_else(|| value.get("message"))
            .and_then(serde_json::Value::as_str).unwrap_or("no reason given");
        return Err(format!("the model catalog reported a failure: {reason}"));
    }
    let Some(data) = value.get("data").filter(|d| d.is_object()) else {
        return Err("the model catalog was not in the `{success, data}` envelope".to_string());
    };
    let Some(raw) = data.get("data").and_then(serde_json::Value::as_array) else {
        return Err("the model catalog envelope carried no `data` list".to_string());
    };
    let text = |e: &serde_json::Value, k: &str| e.get(k).and_then(serde_json::Value::as_str)
        .map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let entries = raw.iter().filter_map(|e| Some(CatalogEntry {
        id: text(e, "id")?,
        name: text(e, "display_name").or_else(|| text(e, "name")),
        context_length: e.get("context_length").and_then(serde_json::Value::as_u64),
    })).collect();
    let total = data.get("total").and_then(serde_json::Value::as_u64)
        .and_then(|t| usize::try_from(t).ok());
    Ok(CatalogPage { entries, raw_len: raw.len(), total })
}

/// Pages collected, deduplicated by id, in listing order.
#[derive(Debug, Default)]
pub struct Collector { seen: HashSet<String>, entries: Vec<CatalogEntry>, offset: usize, pages: usize }

impl Collector {
    pub fn offset(&self) -> usize { self.offset }
    /// Stops on: an empty page; reaching `total`; no `total`; [`MAX_PAGES`].
    pub fn push(&mut self, page: CatalogPage) -> NextPage {
        self.pages += 1;
        for entry in page.entries {
            if self.seen.insert(entry.id.clone()) { self.entries.push(entry); }
        }
        if page.raw_len == 0 { return NextPage::Done; }
        self.offset += page.raw_len;
        match page.total {
            Some(total) if self.offset < total && self.pages >= MAX_PAGES =>
                NextPage::Truncated { read: self.offset, total },
            Some(total) if self.offset < total => NextPage::At(self.offset),
            _ => NextPage::Done,
        }
    }
    pub fn finish(self) -> Vec<CatalogEntry> { self.entries }
}
```

Then run `cargo fmt`. The tests are in part 3 §8.1.

### 5.3 `probe.rs`

1. Import `super::catalogue::{self, CatalogShape}` and `super::paged_catalog`.
2. In `impl ProbeFailure` (~:694), add a constructor that can never be `auth`,
   so it can never roll back a key:
   `fn unreadable(raw: String) -> Self { Self { class: ProbeClass::Unknown, raw } }`.
3. Rename `read_capped(response)` (:1000) to `read_capped_to(response, cap)`,
   using `cap` in place of `PROBE_BODY_CAP`.
4. Change the signature to
   `probe_models(base_url, credential, auth, policy, shape: CatalogShape)`.
5. Move `:853-875` (from `let request = apply_auth(…)` to the non-success
   `return Err`) into
   `async fn probe_get(client: &reqwest::Client, url: &str, auth: catalogue::AuthStyle, credential: Option<&str>, success_cap: usize) -> Result<String, ProbeFailure>`.
   It builds `named` from `url`, reads the body with `success_cap` on success
   and `PROBE_BODY_CAP` otherwise, and returns `Ok(body)`.
6. After the client is built (:848):

```rust
if shape == CatalogShape::PagedEnvelope {
    let mut collector = paged_catalog::Collector::default();
    loop {
        let page_url = format!("{base}{}", paged_catalog::page_path(collector.offset()));
        let body = probe_get(&client, &page_url, auth, credential, paged_catalog::PAGE_BODY_CAP).await?;
        let page = paged_catalog::parse_page(&body).map_err(|e|
            ProbeFailure::unreadable(format!("{}: {e}", catalogue::redact_endpoint(&page_url))))?;
        if !matches!(collector.push(page), paged_catalog::NextPage::At(_)) { break; }
    }
    return Ok(collector.finish().into_iter().map(|e| e.id).collect());
}
let body = probe_get(&client, &url, auth, credential, PROBE_BODY_CAP).await?;
Ok(parse_model_ids(&body))
```

The redirect policy's `origin` is the first URL, and every page shares it, so
the same-origin guard still holds.

### 5.4 `inference_models.rs`

1. Import `CatalogShape` and `paged_catalog::{self, NextPage}`.
2. Move `fetch_catalog`'s auth/send/status block (:405-426) into
   `async fn send_classified(client, url, bearer, auth) -> Result<reqwest::Response, DiscoveryError>`.
   `fetch_catalog` calls it and keeps its `RegistryResponse` parse.
3. Add `shape: CatalogShape` as the last parameter of `discover_models`,
   `catalog_models`, `discovered_vocabulary` and `turn_vocabulary`, and pass it
   through.
4. In `discover_models`, after the client is built (:351) and before
   `scoped_catalog_path`, add
   `if shape == CatalogShape::PagedEnvelope { return fetch_paged_catalog(&client, base, bearer, auth).await; }`.
5. Add the paged fetch:

```rust
async fn fetch_paged_catalog(client: &reqwest::Client, base: &str, bearer: Option<&str>,
    auth: AuthStyle) -> Result<Vec<InferenceModel>, DiscoveryError> {
    let mut collector = paged_catalog::Collector::default();
    loop {
        let url = format!("{base}{}", paged_catalog::page_path(collector.offset()));
        let named = catalogue::redact_endpoint(&url);
        let mut response = send_classified(client, &url, bearer, auth).await?;
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| DiscoveryError::endpoint(
            format!("reading the model catalog from {named} failed: {e}")))? {
            if body.len() + chunk.len() > paged_catalog::PAGE_BODY_CAP {
                return Err(DiscoveryError::endpoint(format!(
                    "a model catalog page from {named} is larger than 4 MiB")));
            }
            body.extend_from_slice(&chunk);
        }
        let body = String::from_utf8(body).map_err(|e| DiscoveryError::endpoint(
            format!("model catalog from {named} was not UTF-8: {e}")))?;
        let page = paged_catalog::parse_page(&body).map_err(|e| DiscoveryError::endpoint(
            format!("model catalog from {named} was invalid: {e}")))?;
        match collector.push(page) {
            NextPage::At(_) => {}
            NextPage::Done => break,
            NextPage::Truncated { read, total } => {
                tracing::warn!(base = %catalogue::redact_endpoint(base), read, total,
                    "model catalog has more pages than one read follows");
                break;
            }
        }
    }
    Ok(collector.finish().into_iter()
        .map(|e| InferenceModel { id: e.id, name: e.name, context_length: e.context_length })
        .collect())
}
```

6. Add a shaped cache key below `cache_key` (:451):

```rust
fn shaped_endpoint(base_url: &str, shape: CatalogShape) -> String {
    match shape {
        CatalogShape::OpenAi => cache_key(base_url),
        CatalogShape::PagedEnvelope => format!("{}\u{2}paged", cache_key(base_url)),
    }
}
```

In `catalog_models` (:549), use
`catalog_cache_scoped(&shaped_endpoint(base_url, shape), authenticated_scope)`.
Leave these as they are: the 60 s failure memo, 401/403 not being memoized
(`FetchError::Credential`), an empty catalog counting as a failure, the timeout,
the sort, and `evict_company_catalogs`.

### 5.5 Callers pass the shape

| Caller | Argument added |
|---|---|
| `providers.rs:412` `add_provider` | `catalogue::catalog_shape_for(&plan.kind, &provider.base_url)` |
| `providers.rs:1550` `test_managed` | `catalogue::catalog_shape_for(inference::LEGACY_MANAGED, &base_url)` |
| `providers.rs:1642` `list_provider_models`, `:1805` `test_provider` | `catalogue::catalog_shape_for(&provider.kind, &provider.base_url)` |
| `providers.rs:1889` `probe_draft` | `catalogue::catalog_shape_for(kind, body.base_url.trim())` |
| `ops/inference.rs:141-167` `resolved_endpoint` | return a 4-tuple ending in `catalogue::catalog_shape_for(&decl.provider, &decl.base_url)`; destructure at :171, pass at :192 |
| `ops/inference.rs:1342` `test_config` | `catalogue::catalog_shape_for(&decl.provider, &decl.base_url)` |
| `provider.rs:2125` `TenantProvider::resolve` | `crate::company::inference::catalogue::catalog_shape_for(&decl.provider, &decl.base_url)` |
| `setup.rs:1296` `probe_inference` | `crate::company::inference::catalogue::catalog_shape_for(&req.provider, &decl.base_url)` |
| tests `probe.rs:1373`, `inference_models.rs:718`, `:766`, `:801`, `:971` | `CatalogShape::OpenAi` |

`cargo build --all-targets` names any caller this table missed.

---

Parts: [1](phase-2a-tinyhumans-on-proxy.md) · [2](phase-2a-tinyhumans-on-proxy-part2.md) · [3](phase-2a-tinyhumans-on-proxy-part3.md). Next: [part 2](phase-2a-tinyhumans-on-proxy-part2.md), from §5.6.
