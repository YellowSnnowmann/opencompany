# Phase 2b — `inference/default` holds `{provider, model}`; the resolver sends a chosen model

Slice **2b** of the [keys rework](README.md). Continued in
[phase-2b-default-shape-part2.md](phase-2b-default-shape-part2.md) (resolver
target code, edit list, carry-over, tests, must-not-touch, done-when, gotchas).

- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14). Every `file:line`
  below is on that commit.
- **Runs after:** slice 2a ([phase-2a-tinyhumans-on-proxy.md](phase-2a-tinyhumans-on-proxy.md))
  is committed on `feat/key-reworks`. PR #2305 was closed on 2026-09-14 and is
  not a gate. Its diff is only a reference for what not to do.
- **What 2a changes under this slice:** 2a adds a `tinyhumans` catalogue row
  (`https://api.tinyhumans.ai/agent-integrations/openrouter`, bearer, paged
  catalogue parser). It moves `PLATFORM_BASE_URL` (`inference.rs:152`) and
  `DEFAULT_TINYHUMANS_INFERENCE_URL` (`provider.rs:55`) to that proxy path. It
  adds stream tolerance to response parsing and the rule that the legacy Managed
  row and a `tinyhumans` row cannot both exist. It may add arguments to
  `turn_vocabulary` / `catalog_models`, and it may shift lines in every file
  below. **Find each function by the quoted code, never by line number.**
- **Source plan:** `provider-model-plan-2026-09-14.md` §1.1, §1.2, §3, slice A1.
  There is no `proxied_model`, and no `inference/managed/models` key.

---

## 1. Goal

The store and the resolver learn three things: a company default of
`{"provider":"…","model":"…"}`, a row's single model, and a decl's
`chosen_model`. When a full default exists, every turn sends that model. This
slice has no **writer** for the JSON shape (slice 2c adds one), so every
existing company behaves exactly as before it.

- **Dump items:** 1 (the default stores provider and model); 16 (provider plus
  model, no tiers on the new path).
- **Q1:** the default is JSON. A bare slug reads as provider-only and is never
  rewritten.
- **D-legacy / decision 8:** with no full default, today's behaviour is kept
  byte for byte. Slice 2d later removes tiers from the wire.
- **F6:** a default that names a missing or disabled provider fails closed.
- In 2b, `pin` is always `None`. Slice 3a supplies it.

## 2. Files

| File | What changes |
|---|---|
| `src/company/inference/store.rs` | New: `ModelChoice`, `DefaultChoice`, `StoredDefault`, `parse_default`, `load_default`, `set_default_choice`, `ModelOnRow`, `model_on_row`, `Provider::model`. `load_default_slug` becomes a wrapper. |
| `src/company/inference.rs` | New: `InferenceDecl.chosen_model` and its accessors, `NO_MODEL_CHOSEN`, `ChoiceSource`, `decl_for_choice`, `full_default_decl`, `resolve_choice`, `resolve_for_turn`. One new step in `resolve_effective_scoped`. |
| `src/harness/built_in/provider.rs` | `request_plan` picks the chosen model. `TenantProvider::resolve` calls `resolve_for_turn` and skips the catalogue read. |
| `docs/modules/inference/data-model.md` | `:159-162` describe the new value shape. |

Nothing else changes. In particular, `src/server/ops/inference.rs` and
`src/server/ops/inference/providers.rs` are **not edited**:

- Their four `load_default_slug` callers (`ops/inference.rs:505`,
  `providers.rs:1326`, `:1397`, `:1433`) still compile and still compare the
  provider slug, through the wrapper.
- The wrapper is what makes `clear_default_if_marked` (`providers.rs:1324-1345`)
  compare `.provider` on a JSON default.

## 3. Current code

`src/company/inference/store.rs:811-861` (the default section):

```rust
pub const DEFAULT_PROVIDER_KEY: &str = "inference/default";

pub async fn load_default_slug(company: &CompanyId, secrets: &dyn SecretStore)
    -> Result<Option<String>> {
    let Some(SecretValue(raw)) = secrets.get(company, DEFAULT_PROVIDER_KEY).await? else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
}

pub async fn set_default_slug(company, secrets, slug: &str) -> Result<()>   // :841-853, writes slug.trim()
pub async fn clear_default_slug(company, secrets) -> Result<()>             // :857-861, writes ""
```

The index parse error this slice copies (`store.rs:395-397`):

```rust
serde_json::from_str(&raw).map_err(|e| {
    OpenCompanyError::Store(format!("inference provider index is not valid JSON: {e}"))
})
```

- `Provider` has `models: BTreeMap<String, String>` (`store.rs:174-175`).
- `impl Provider` (`:188-216`) holds `key_key` and `legacy_key_key`.
- Entry zero's record copies `inference/config.models` in
  `provider_from_runtime` (`store.rs:429-460`; the copy is at `:453`).

`src/company/inference.rs:488-514`, the decl:

```rust
#[derive(Clone, Debug)]
pub struct InferenceDecl {
    pub provider: String,
    pub base_url: String,
    pub models: BTreeMap<String, String>,
    pub source: InferenceSource,
    credential: Credential,
    proxied: bool,
    vocabulary: Option<TierVocabulary>,
}
```

It is built by struct literal in exactly six places, all in `inference.rs`:

- `:729-737`, in `decl_for_probe`;
- `:1434-1442`, in `decl_for_indexed`;
- `:1489-1497`, `:1533-1541` and `:1580-1588`, in `resolve_legacy_scoped`;
- `:1750-1758`, in `managed_decl`.

Find them with `git grep -n "vocabulary: None" -- src/company`.

`resolve_effective_scoped`, step 0 (`inference.rs:1246-1251`):

```rust
if scope.is_default || !harness_configures_itself(company, secrets, scope).await? {
    let providers = store::list_providers(company, secrets).await?;
    if let Some(decl) = decl_for_primary(company, secrets, &providers).await? {
        return Ok(Some(decl));
    }
}
```

`resolve_effective_for_tier` (`inference.rs:1618-1732`) is today's turn
resolver. Its entry-zero route arm (`:1712-1720`) is the rule `decl_for_choice`
reuses:

```rust
store::ProviderOrigin::EntryZero => {
    let flat = HarnessScope::default_harness(&scope.id);
    match resolve_legacy_scoped(company, manifest, env_default, secrets, &flat).await? {
        Some(decl) => decl,
        None => return Ok(None),
    }
}
```

`src/harness/built_in/provider.rs:1668` (`request_plan`):

```rust
let model = inference::model_for_tier(abstract_model, &decl.models, decl.vocabulary());
```

`src/harness/built_in/provider.rs:2096-2133` (`TenantProvider::resolve`):

```rust
async fn resolve(&self, tier: &str) -> anyhow::Result<InferenceDecl> {
    let decl = inference::resolve_effective_for_tier(
        &self.company, &self.manifest, self.env_default.as_ref(),
        self.secrets.as_ref(), &self.scope, tier,
    )
    .await
    .map_err(|e| anyhow::anyhow!("resolving inference config: {e}"))?
    .ok_or_else(|| anyhow::anyhow!("no inference provider is configured for this company"))?;
    *self.slug.write().unwrap() = decl.telemetry_slug();
    let bearer = decl.bearer().await.ok().flatten();
    let vocabulary = crate::server::inference_models::turn_vocabulary(
        &decl.base_url, bearer.as_deref(), Some(&self.catalog_scope()),
        crate::company::inference::catalogue::auth_style_for(&decl.provider),
        // (slice 2a may have added an argument here; keep it)
    )
    .await;
    Ok(decl.with_vocabulary(vocabulary))
}
```

`invoke` (`provider.rs:2152-2173`) calls `self.resolve(model)` with
`request.model`, or `DEFAULT_HOSTED_MODEL = "chat-v1"` (`:58`) when there is
none, then calls `request_plan(&decl, model, …)`.

## 4. Target code

### 4.1 `src/company/inference/store.rs`

**(a)** Add `model()` **inside** the existing `impl Provider` block. Put
`ModelOnRow` and `model_on_row` directly after that block (after `:216`):

```rust
impl Provider {
    // … key_key, legacy_key_key unchanged …

    /// This row's one model, read without guessing. See [`model_on_row`].
    pub fn model(&self) -> ModelOnRow {
        model_on_row(&self.models)
    }
}

/// A provider row's single model, as read from its `models` map.
///
/// The map is a storage encoding (the same id under every tier key), not a
/// selection. Two different ids is a row nobody chose one model for, and it is
/// reported, never resolved by picking one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelOnRow {
    /// No non-blank value at all.
    None,
    /// Exactly one distinct non-blank id (trimmed).
    One(String),
    /// Two or more distinct ids, trimmed, sorted ascending, de-duplicated.
    Ambiguous(Vec<String>),
}

/// Collapses a tier-keyed `models` map to [`ModelOnRow`]. Entry zero uses the
/// same function, because its record carries `inference/config.models`.
pub fn model_on_row(models: &BTreeMap<String, String>) -> ModelOnRow {
    let mut distinct: Vec<String> = models
        .values()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
        .collect::<std::collections::BTreeSet<&str>>()
        .into_iter()
        .map(str::to_string)
        .collect();
    match distinct.len() {
        0 => ModelOnRow::None,
        1 => ModelOnRow::One(distinct.remove(0)),
        _ => ModelOnRow::Ambiguous(distinct),
    }
}
```

**(b)** In the default section (`:811-861`), keep `DEFAULT_PROVIDER_KEY`,
`set_default_slug` and `clear_default_slug` exactly as they are. Add these items
between `DEFAULT_PROVIDER_KEY` and `load_default_slug`:

```rust
/// A provider slug and the one model to send it: the only shape the new
/// resolution path sends. Serialized field order is `provider`, then `model`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    /// A slug from [`list_providers`], which includes entry zero.
    pub provider: String,
    /// The id that provider's API accepts. Non-empty after trim.
    pub model: String,
}

/// `inference/default`, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultChoice {
    /// Missing, `""`, or only whitespace.
    Unset,
    /// A bare slug (every value stored before 2c), or JSON with a blank or
    /// absent `model`. Resolves exactly as a bare slug always has.
    ProviderOnly(String),
    /// JSON with a non-blank provider and a non-blank model.
    Full(ModelChoice),
}

impl DefaultChoice {
    /// The provider this default names, if any.
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Unset => None,
            Self::ProviderOnly(slug) => Some(slug.as_str()),
            Self::Full(choice) => Some(choice.provider.as_str()),
        }
    }

    /// The full pair, only when both halves are present.
    pub fn full(&self) -> Option<&ModelChoice> {
        match self {
            Self::Full(choice) => Some(choice),
            _ => None,
        }
    }
}

/// The JSON read shape. `model` is optional so a provider-only JSON value is
/// representable; unknown fields are ignored (no `deny_unknown_fields`).
#[derive(Deserialize)]
struct StoredDefault {
    provider: String,
    #[serde(default)]
    model: Option<String>,
}

/// The parse rules for `inference/default`, in order:
///
/// 1. Trim. Empty ⇒ [`DefaultChoice::Unset`].
/// 2. Does not start with `{` ⇒ the whole trimmed value is a slug ⇒
///    [`DefaultChoice::ProviderOnly`]. (A slug is `[a-z0-9-]`.)
/// 3. Starts with `{` ⇒ serde into [`StoredDefault`]. Invalid JSON, a missing
///    `provider`, or a non-string `provider` ⇒ `OpenCompanyError::Store`.
/// 4. `provider` blank after trim ⇒ `OpenCompanyError::Store`.
/// 5. `model` absent, `null`, or blank after trim ⇒ `ProviderOnly(provider)`.
/// 6. Otherwise ⇒ `Full { provider: trimmed, model: trimmed }`.
pub fn parse_default(raw: &str) -> Result<DefaultChoice> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(DefaultChoice::Unset);
    }
    if !trimmed.starts_with('{') {
        return Ok(DefaultChoice::ProviderOnly(trimmed.to_string()));
    }
    let stored: StoredDefault = serde_json::from_str(trimmed).map_err(|e| {
        OpenCompanyError::Store(format!("inference default is not valid JSON: {e}"))
    })?;
    let provider = stored.provider.trim();
    if provider.is_empty() {
        return Err(OpenCompanyError::Store(
            "inference default names no provider".to_string(),
        ));
    }
    match stored.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => Ok(DefaultChoice::Full(ModelChoice {
            provider: provider.to_string(),
            model: model.to_string(),
        })),
        None => Ok(DefaultChoice::ProviderOnly(provider.to_string())),
    }
}

/// Reads and parses `inference/default`. Never writes: a bare slug stays a
/// bare slug on disk (Q1).
pub async fn load_default(company: &CompanyId, secrets: &dyn SecretStore) -> Result<DefaultChoice> {
    let Some(SecretValue(raw)) = secrets.get(company, DEFAULT_PROVIDER_KEY).await? else {
        return Ok(DefaultChoice::Unset);
    };
    parse_default(&raw)
}

/// Writes a full default as **one** JSON value, e.g.
/// `{"provider":"tinyhumans","model":"acme/test-model"}`. One write, so a
/// provider can never be paired with another provider's model.
pub async fn set_default_choice(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    choice: &ModelChoice,
) -> Result<()> {
    let provider = choice.provider.trim();
    let model = choice.model.trim();
    if provider.is_empty() || model.is_empty() {
        return Err(OpenCompanyError::InvalidRequest(
            "a default needs both a provider and a model".to_string(),
        ));
    }
    let raw = serde_json::to_string(&ModelChoice {
        provider: provider.to_string(),
        model: model.to_string(),
    })
    .map_err(|e| OpenCompanyError::Store(format!("serializing the inference default: {e}")))?;
    secrets.set(company, DEFAULT_PROVIDER_KEY, SecretValue(raw)).await
}
```

**(c)** `load_default_slug` keeps its signature and doc. Add one sentence to the
doc: "A JSON default answers its `provider`." Its body becomes:

```rust
    Ok(load_default(company, secrets)
        .await?
        .provider()
        .map(str::to_string))
```

### 4.2 `src/company/inference.rs`, decl

**(a)** Add a last field to `InferenceDecl`, after `vocabulary` (`:513`):

```rust
    /// The model the new resolution path chose: a full company default's, or
    /// (3a) an agent pair's. `None` on every legacy arm. Set only through
    /// [`with_chosen_model`](Self::with_chosen_model).
    chosen_model: Option<String>,
```

Add `chosen_model: None,` to all six struct literals listed in §3.

**(b)** Add two methods to `impl InferenceDecl`, directly after
`with_vocabulary` (`:583-587`):

```rust
    /// The model a turn sends, when the new path chose one.
    pub fn chosen_model(&self) -> Option<&str> {
        self.chosen_model.as_deref()
    }

    /// Attaches the chosen model. Never call this on a legacy arm.
    #[must_use]
    pub fn with_chosen_model(mut self, model: String) -> Self {
        self.chosen_model = Some(model);
        self
    }
```

The resolver functions are in
[part 2 §4.3](phase-2b-default-shape-part2.md#43-srccompanyinferencers-resolver).
