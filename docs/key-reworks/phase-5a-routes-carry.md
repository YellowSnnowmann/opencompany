# Phase 5a — routes carry-over into an empty default, plus a banner

Slice 5a of the keys rework (issue #2306). Read [README.md](README.md) first: the
decisions (Q14, F7, D-legacy) and the naming contract are fixed there. The next
slice, [phase-5b-routing-removal.md](phase-5b-routing-removal.md), deletes
routing, and it must not start until this slice is pushed and green.

- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14). Every `file:line`
  below was opened on that commit. **Phase 5 runs after 2a–2d, 3a–3b and 4a–4c
  are committed on this branch**, so every line number will have moved. Find
  each site by the quoted code, not by the number. (PR #2305 was closed on
  2026-09-14; nothing here depends on it.)
- **Depends on names from earlier slices:** `store::ModelChoice { provider, model }`,
  `store::DefaultChoice { Unset, ProviderOnly(String), Full(ModelChoice) }`,
  `store::load_default`, `store::set_default_choice` (2b); `check_model_id` and the
  `defaultChoice` status field (2c). Before writing code, open those
  definitions and confirm their signatures. The code below assumes
  `load_default(&CompanyId, &dyn SecretStore) -> Result<DefaultChoice>` and
  `set_default_choice(&CompanyId, &dyn SecretStore, &ModelChoice) -> Result<()>`.
  If a signature differs, adapt the call and keep the behaviour.

## 1. Goal

At boot, copy a routing table whose four tier rows all name the same
`provider:model` into an **empty** `inference/default`. Anything else keeps its
routes and gets a console banner. Routes are still read after this slice.

Dump item 2 (remove `inference/routes`), decision Q14, flag F7.

## 2. Files

| File | Current site on `fcfb3e1bc` | Change |
|---|---|---|
| `src/company/inference/routes_carry.rs` | does not exist | **new**: the carry, the status helper, the route parser |
| `src/company/inference.rs` | module list `:28-32` (`pub mod resolve;` at `:31`) | add `pub mod routes_carry;` |
| `src/company/inference/store.rs` | `ROUTES_KEY` `:871`; `load_routes` `:885-899`; `save_routes` `:907-920`; unset rows dropped `:912-916` | **read only**. Nothing changes here |
| `src/runtime/builder.rs` | `model_override` `:3065-3068`; `configured = inference::resolve_effective(` `:3087-3102`; `with_secrets` `:1414`; tests `mod test` `:4638`; harness-boot test pattern `:9034-9047` | call the carry before `configured` |
| `src/server/ops/inference.rs` | `InferenceStatusDto` `:233-355` (`routes` field `:345-352`); `routing_table` `:545-555`; `effective_status_with` `:840-927` (`let routes = routing_table(…)` `:877`, filled `:901`, `:923`) | add `routes_not_carried` |
| `frontend/src/api/inference.ts` | `InferenceStatus.routes?` `:186-198` | add `RouteNotCarried`, `routesNotCarried?` |
| `frontend/src/inference/routes-not-carried.ts` | does not exist | **new**: pure copy builder |
| `frontend/src/inference/RoutesNotCarriedBanner.tsx` | does not exist | **new**: the banner |
| `frontend/src/inference/ProvidersTab.tsx` | dead-end paragraph `:378-383` (`data-testid="inference-providers-dead-end"`) | render the banner |
| `frontend/test/unit/inference-routes-banner.test.ts` | does not exist | **new** |
| `frontend/test/e2e/inference.spec.ts` | `deleting a provider removes its row and resets the routes that named it` `:253-295` | add one test after it |

**Earlier slices edit these files.** 2b and 2c edit `src/company/inference.rs`,
`store.rs`, `ops/inference.rs` and `ProvidersTab.tsx`; 2c adds `defaultChoice`
to the status DTO right beside where this slice adds `routesNotCarried`. Re-open
each file and re-find the anchors quoted in §3 before editing.

## 3. Current code (quoted so it can be found)

The boot branch that makes a company routed only to Managed "configured"
(`src/company/inference.rs:1295-1315`):

```rust
    let routes = store::load_routes(company, secrets).await?;
    if resolve::any_route_is_managed(&routes) && store::managed_enabled(company, secrets).await? {
        let decl = managed_decl(company, secrets, env_default, scope).await?;
        if decl.credential.configured() {
            return Ok(Some(decl));
        }
    }
```

Rows that are unset are dropped when saved (`store.rs:912-916`), which is why F7
exists:

```rust
    let stored: StoredRoutes = routes
        .iter()
        .filter(|(_, route)| !matches!(route, ProviderRef::Default))
```

The route grammar the stored strings use (`resolve.rs:182-205`): `""` or
`default` is unset, `managed` is Managed, `slug:model` splits on the **first**
colon, and `local` / `claude-code` are slug-less categories.

The builder's configured check (`src/runtime/builder.rs:3083-3102`):

```rust
                        let configured = inference::resolve_effective(
                            &id,
                            &effective_manifest,
                            env_default.as_ref(),
                            secrets.as_ref(),
                        )
```

The status DTO's routing field (`src/server/ops/inference.rs:352`) and its fill
(`:877`):

```rust
    routes: BTreeMap<String, String>,
    // …
    let routes = routing_table(runtime).await?;
```

## 4. Target code

### 4.1 `src/company/inference/routes_carry.rs` (new, complete)

It is self-contained on purpose. It reads `inference/routes` raw and parses it
with its own copy of the grammar, so it compiles unchanged after 5b deletes
`resolve::ProviderRef` and `store::load_routes`. Match the `Result` import to
the one `store.rs` uses in its `use` block.

```rust
//! Boot-time carry-over of a unanimous routing table into an empty default
//! (issue #2306, phase 5a; decision Q14, flag F7).
//!
//! Copy-if-empty and nothing else: it never clears the routes, never writes a
//! provider row, never touches Managed, and never overwrites a default. So a
//! re-run, a crash half-way, or two builds racing are all harmless — the only
//! write is the same value into a slot that was empty.
//!
//! Reads `inference/routes` raw with its own copy of the route grammar, so it
//! survives phase 5b, which deletes every other routing reader.

use std::collections::BTreeMap;

use crate::company::types::INFERENCE_TIERS;
use crate::error::{OpenCompanyError, Result};
use crate::ports::SecretStore;
use crate::ports::types::{CompanyId, SecretValue};

use super::store::{self, DefaultChoice, ModelChoice, ROUTES_KEY};

/// What one stored route string names, read the way the routing table wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteTarget {
    /// `""` or `default`.
    Unset,
    /// `managed`. Carries no model.
    Managed,
    /// `local…` or `claude-code…`: a category, not a provider row.
    Category(String),
    /// A slug with no model (`acme`, or `acme:` with nothing after the colon).
    ProviderOnly(String),
    /// `acme:test-model`. The model is everything after the first colon.
    ProviderModel { provider: String, model: String },
}

/// Parses one stored route string. Total: every string is some target.
pub fn parse_route(raw: &str) -> RouteTarget {
    let raw = raw.trim();
    if raw.is_empty() || raw == "default" {
        return RouteTarget::Unset;
    }
    if raw == "managed" {
        return RouteTarget::Managed;
    }
    let (slug, model) = match raw.split_once(':') {
        Some((slug, model)) => (slug.trim(), model.trim()),
        None => (raw, ""),
    };
    match slug {
        "local" | "claude-code" => RouteTarget::Category(slug.to_string()),
        _ if model.is_empty() => RouteTarget::ProviderOnly(slug.to_string()),
        _ => RouteTarget::ProviderModel {
            provider: slug.to_string(),
            model: model.to_string(),
        },
    }
}

/// Why a stored table was not copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotCarried {
    /// `inference/default` is a bare slug or a full choice already.
    DefaultAlreadySet,
    /// The blob is not a JSON object of strings.
    Unreadable,
    /// F7: these tiers have no row (the store drops unset rows).
    MissingTiers(Vec<String>),
    /// This row is not `provider:model` (managed, a category, or no model).
    NotAProviderModel { tier: String, route: String },
    /// Every tier has a `provider:model` row, but they differ.
    Disagree,
    /// The rows agree on a slug this company does not have.
    ProviderMissing(String),
    /// The rows agree on a provider that is switched off.
    ProviderDisabled(String),
    /// `check_model_id` refused the model the rows agree on.
    ModelRefused { model: String, reason: String },
}

/// What [`carry_routes_into_default`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CarryOutcome {
    /// One `set_default_choice` write of this value.
    Copied(ModelChoice),
    /// Routes exist and were left alone.
    NotEligible(NotCarried),
    /// No routes stored (absent, blank, `{}`, or only unset rows).
    Nothing,
}

/// One stored row, for the status banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRoute {
    pub tier: String,
    pub route: String,
}

/// The raw table: `None` when absent or blank, `Err(Unreadable)` when not JSON.
async fn read_stored(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<Option<std::result::Result<BTreeMap<String, String>, NotCarried>>> {
    let Some(SecretValue(raw)) = secrets.get(company, ROUTES_KEY).await? else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(
        serde_json::from_str::<BTreeMap<String, String>>(&raw).map_err(|_| NotCarried::Unreadable),
    ))
}

/// Rows that name something, tiers first in `INFERENCE_TIERS` order, then any
/// other key alphabetically.
fn set_rows(table: &BTreeMap<String, String>) -> Vec<StoredRoute> {
    let mut rows: Vec<StoredRoute> = table
        .iter()
        .filter(|(_, route)| parse_route(route) != RouteTarget::Unset)
        .map(|(tier, route)| StoredRoute { tier: tier.clone(), route: route.trim().to_string() })
        .collect();
    let rank = |tier: &str| INFERENCE_TIERS.iter().position(|t| *t == tier).unwrap_or(usize::MAX);
    rows.sort_by(|a, b| rank(&a.tier).cmp(&rank(&b.tier)).then_with(|| a.tier.cmp(&b.tier)));
    rows
}

/// Copies a unanimous four-row table into an empty default. Boot/build only —
/// never call this from a read path.
pub async fn carry_routes_into_default(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<CarryOutcome> {
    let table = match read_stored(company, secrets).await? {
        None => return Ok(CarryOutcome::Nothing),
        Some(Err(reason)) => return Ok(CarryOutcome::NotEligible(reason)),
        Some(Ok(table)) => table,
    };
    if set_rows(&table).is_empty() {
        return Ok(CarryOutcome::Nothing);
    }
    // An unreadable default is an error, never "empty": overwriting it would
    // be the one outcome copy-if-empty exists to rule out.
    if !matches!(store::load_default(company, secrets).await?, DefaultChoice::Unset) {
        return Ok(CarryOutcome::NotEligible(NotCarried::DefaultAlreadySet));
    }

    let missing: Vec<String> = INFERENCE_TIERS
        .iter()
        .filter(|tier| table.get(**tier).is_none_or(|r| parse_route(r) == RouteTarget::Unset))
        .map(|tier| (*tier).to_string())
        .collect();
    if !missing.is_empty() {
        return Ok(CarryOutcome::NotEligible(NotCarried::MissingTiers(missing)));
    }

    let mut agreed: Option<(String, String)> = None;
    for tier in INFERENCE_TIERS {
        let raw = &table[*tier];
        let RouteTarget::ProviderModel { provider, model } = parse_route(raw) else {
            return Ok(CarryOutcome::NotEligible(NotCarried::NotAProviderModel {
                tier: (*tier).to_string(),
                route: raw.trim().to_string(),
            }));
        };
        match &agreed {
            None => agreed = Some((provider, model)),
            Some(first) if *first == (provider.clone(), model.clone()) => {}
            Some(_) => return Ok(CarryOutcome::NotEligible(NotCarried::Disagree)),
        }
    }
    let (provider, model) = agreed.expect("INFERENCE_TIERS is non-empty");

    match store::get_provider(company, secrets, &provider).await? {
        None => return Ok(CarryOutcome::NotEligible(NotCarried::ProviderMissing(provider))),
        Some(row) if !row.enabled => {
            return Ok(CarryOutcome::NotEligible(NotCarried::ProviderDisabled(provider)));
        }
        Some(_) => {}
    }
    // 2c's shared validation (`store::check_model_id`, returns the trimmed id).
    let model = match store::check_model_id(&model) {
        Ok(id) => id,
        Err(reason) => {
            return Ok(CarryOutcome::NotEligible(NotCarried::ModelRefused {
                model,
                reason: reason.to_string(),
            }));
        }
    };

    let choice = ModelChoice { provider, model };
    store::set_default_choice(company, secrets, &choice).await?;
    tracing::info!(
        company = %company,
        provider = %choice.provider,
        model = %choice.model,
        "carried the routing table into the default"
    );
    Ok(CarryOutcome::Copied(choice))
}

/// The rows the console banner lists, or `None` when there is nothing to say:
/// no set rows, or the default is already a full choice. A pure read.
///
/// `Err` on an unreadable routes blob or default; the status route logs it and
/// sends `null`, so a key nothing else reads cannot break the status page.
pub async fn routes_not_carried(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<Option<Vec<StoredRoute>>> {
    let table = match read_stored(company, secrets).await? {
        None => return Ok(None),
        Some(Err(_)) => {
            return Err(OpenCompanyError::Store(
                "inference routes are not valid JSON".to_string(),
            ));
        }
        Some(Ok(table)) => table,
    };
    let rows = set_rows(&table);
    if rows.is_empty() {
        return Ok(None);
    }
    if matches!(store::load_default(company, secrets).await?, DefaultChoice::Full(_)) {
        return Ok(None);
    }
    Ok(Some(rows))
}
```

`store::check_model_id` is defined by 2c in `store.rs`, directly after
`check_provider_name`. A stored route like `acme:chat-v1` is refused there
(tier name), which gives `ModelRefused`. If `is_none_or` is unavailable on the
pinned toolchain, use `map_or(true, …)`.

### 4.2 The builder call (`src/runtime/builder.rs`, before `let configured`)

Insert immediately after the `model_override` binding (`:3065-3068`) and before
the comment `// Is any inference source configured` (`:3070`):

```rust
                        // Phase 5a (issue #2306): copy a unanimous routing table
                        // into an empty default before asking whether anything is
                        // configured. Copy-if-empty, so a re-run is harmless, and
                        // never fatal: a failed carry leaves the company exactly as
                        // it would have booted without it.
                        match inference::routes_carry::carry_routes_into_default(
                            &id,
                            secrets.as_ref(),
                        )
                        .await
                        {
                            Ok(inference::routes_carry::CarryOutcome::NotEligible(reason)) => {
                                tracing::info!(
                                    company = %id,
                                    reason = ?reason,
                                    "routing table not carried into the default"
                                );
                            }
                            Ok(_) => {}
                            Err(err) => tracing::warn!(
                                company = %id,
                                error = %err,
                                "carrying the routing table into the default failed; boot continues"
                            ),
                        }
```

It runs on every build of the company, which includes `POST …/inference/restart`
(`ops/inference.rs:1161`, through `rebuild_company`) and the rebuild after a
config save.

### 4.3 Status DTO (`src/server/ops/inference.rs`)

Add beside `InferenceStatusDto`:

```rust
/// One routing row the default did not absorb, for the console banner.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteNotCarriedDto {
    /// The stored tier key (`chat-v1`).
    tier: String,
    /// The stored route string (`acme:test-model`, `managed`, `local:local-vision-model`).
    route: String,
}
```

Add a field to `InferenceStatusDto`, directly after `routes` (`:352`). Do not
add `skip_serializing_if`: the wire value is `null` when there is nothing to say.

```rust
    /// Routing rows the default did not absorb (phase 5a): non-null when
    /// `inference/routes` names anything and the default is not a full
    /// `{provider, model}`. Drives the "Routing is going away" banner.
    routes_not_carried: Option<Vec<RouteNotCarriedDto>>,
```

In `effective_status_with`, after `let routes = routing_table(runtime).await?;`
(`:877`):

```rust
    let routes_not_carried =
        match inference::routes_carry::routes_not_carried(runtime.id(), secrets).await {
            Ok(rows) => rows.map(|rows| {
                rows.into_iter()
                    .map(|r| RouteNotCarriedDto { tier: r.tier, route: r.route })
                    .collect()
            }),
            Err(err) => {
                tracing::warn!(
                    company = %runtime.id(),
                    error = %err,
                    "could not read the routing rows for the banner"
                );
                None
            }
        };
```

In both struct literals (`Some(d)` at `:886-903` and `None` at `:904-925`), add
`routes_not_carried,` after `routes,`. The first literal moves it, so write
`routes_not_carried: routes_not_carried.clone(),` there only if the compiler
requires it; the `match` has two arms, so a move in each is fine as written.


Continued in [phase-5a-routes-carry-part2.md](phase-5a-routes-carry-part2.md): §4.4–§4.6
(TypeScript, copy builder, banner), §5 edit list, §6 data, §7 tests, §8 UI,
§9 must not touch, §10 done when, §11 gotchas.
