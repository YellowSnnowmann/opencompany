# Phase 5b — routing removed (backend and console together)

Slice 5b of the keys rework (issue #2306). Read [README.md](README.md) and
[phase-5a-routes-carry.md](phase-5a-routes-carry.md) first. This file covers the
backend, the behaviour analysis and the data. The console, the tests, the auth
matrix and the docs are in [phase-5b-routing-removal-part2.md](phase-5b-routing-removal-part2.md).

- **Code read at:** `upstream/main @ fcfb3e1bc` (2026-09-14). Every `file:line`
  was opened on that commit, and every line has moved by the time this slice
  runs, so find each site by the quoted code.
- **Starts only when:** 2a–2d, 3a–3b, 4a–4c and **5a** are committed, and 5a's
  head SHA is green on CI (both Console E2E lanes included). PR #2305 was closed
  on 2026-09-14; nothing here depends on it.
- **Names from earlier slices:** `resolve_for_turn`, `NO_MODEL_CHOSEN`, and the
  full-default step in `resolve_effective_scoped` (2b,
  `phase-2b-default-shape.md`); `defaultChoice` and `store::check_model_id` (2c);
  the legacy wire-model rule (2d); the agent pin (3a); `routes_carry` (5a).
- **One commit run, one push.** The backend deletions and the console deletions
  land together: the routes API answers 404 after this, so a console that still
  calls it cannot ship separately.

## 1. Goal

Nothing selects a provider by workload any more. After this slice the stored
`inference/routes` value is read only by `routes_carry` (5a), for the banner.

Dump items 2 and 13(a); decision Q14.

## 2. Files (backend)

| File | Site on `fcfb3e1bc` | Change |
|---|---|---|
| `src/company/inference.rs` | `resolve_effective` doc `:1177-1186`; `resolve_effective_scoped` `:1213-1318`, step-4 routes branch `:1262-1315`; `refuse_a_managed_fallback_that_is_switched_off` `:1320-1357`; `decl_for_indexed` doc `:1407-1412`; `resolve_legacy_scoped` doc `:1445-1452`; `resolve_effective_for_tier` doc+fn `:1594-1732`; `managed_decl` `:1734-1759`; 2b's `resolve_for_turn` (inserted before `:1594`) | delete step 4, `resolve_effective_for_tier`, `managed_decl`; step 4 of `resolve_for_turn` calls `resolve_effective_scoped` + refuse |
| `src/company/inference/resolve.rs` | module doc `:1-46`; `Workload` `:53-135`; `tier_label` `:137-140`; `ProviderRef` `:142-259`; `Routes` `:262`; `Resolution` `:264-296`; `routing_targets` `:298-301`; `primary` `:303-346`; `provider_for_workload` `:348-393`; `resolve_by_category` `:395-420`; `RoutingMode` `:422-441`; `any_route_is_managed` `:443-470`; `infer_routing_mode` `:472-520`; `scrub_removed` `:522-565`; `routes_served_by` `:567-599`; `route_names` `:601-627`; `orphaned_routes` `:629-665`; tests `:667-1295` | keep `primary` and its three tests only |
| `src/company/inference/store.rs` | `use super::resolve::{ProviderRef, Routes};` `:65`; `ROUTES_KEY` `:863-871`; `StoredRoutes` `:873-881`; `load_routes` `:883-899`; `save_routes` `:901-920`; tests `:1535-1591`, `:1695-1741` | delete all but `ROUTES_KEY` |
| `src/server/ops/inference/providers.rs` | imports `:60`, `:66`; router `:106` (comment `:97-99`); `ProviderMutation.affected_tiers` `:300-303`; add-path auto-route `:506-520`; `auto_route_sole_provider` `:522-633`; `is_the_only_thing_that_can_answer` `:635-652`; `delete_provider` scrub `:1107-1141`, note `:1162-1176`; `set_enabled` doc `:1184-1197`, parking `:1208-1258`, note `:1259-1275`; `parked_tiers` `:1343-1376`; `managed_parked_tiers` `:1378-1420`; `is_primary` `:1422-1437`; `set_managed_enabled` `:1452-1495`; routes section `:1932-2107`; tests `:2114-2200`, `:2322-2420` | delete routing; keep every provider route |
| `src/server/ops/inference.rs` | `InferenceStatusDto.routes` `:345-352`; `routing_table` + doc (directly above `:545`) `:545-555`; `let routes = routing_table` `:877`; `routes,` `:901`, `:923`; `managed_resolves` `:929-943` | delete; keep `routes_not_carried` (5a) |
| `src/harness/built_in/provider.rs` | `TenantProvider::resolve` doc `:2088-2095`, call `:2096-2107` (2b replaced the call with `resolve_for_turn`) | drop the `legacy_hint` argument if §4.1 removes it; reword the doc |
| `tests/auth_matrix.rs`, `tests/snapshots/auth-matrix.txt` | rows `:595-596`; counts `:1743`, `:1751`, `:1754`, `:1773-1774`; snapshot rows `:694-700`, `:1296-1302`, `:2878-2884`, `:3123-3129` | part 2 §3 |

## 3. Current code (anchors)

The routes API (`providers.rs:106`):

```rust
        .merge(scoped("/inference/routes", get(get_routes).put(put_routes)))
```

The turn resolver's legacy step as 2b wrote it (`phase-2b-default-shape-part2.md`):

```rust
    resolve_effective_for_tier(company, manifest, env_default, secrets, scope, legacy_hint)
        .await?
        .ok_or_else(|| OpenCompanyError::Config(NO_MODEL_CHOSEN.to_string()))
```

The tail of `resolve_effective_scoped` (`inference.rs:1257-1318`):

```rust
    let legacy = resolve_legacy_scoped(company, manifest, env_default, secrets, scope).await?;
    if legacy.is_some() {
        return Ok(legacy);
    }

    // 4. The routing table naming `managed` — after the legacy chain's steps
    …
    let routes = store::load_routes(company, secrets).await?;
    if resolve::any_route_is_managed(&routes) && store::managed_enabled(company, secrets).await? {
        let decl = managed_decl(company, secrets, env_default, scope).await?;
        if decl.credential.configured() {
            return Ok(Some(decl));
        }
    }

    Ok(None)
}
```

The mutation DTO's routing field (`providers.rs:300-303`):

```rust
    /// Tiers whose route this change moved or parked, so the console can say
    /// which rows changed rather than leaving the operator to notice.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    affected_tiers: Vec<String>,
```

## 4. Target code (backend)

### 4.1 `resolve_for_turn` step 4 (`src/company/inference.rs`)

Replace the `resolve_effective_for_tier(…)` tail quoted in §3 with:

```rust
    // Legacy path (D-legacy): no pin, no full default, or a named harness that
    // configures itself. Exactly the resolution a company without routes had
    // before phase 5b — see phase-5b-routing-removal.md §5.
    let decl = resolve_effective_scoped(company, manifest, env_default, secrets, scope).await?;
    refuse_a_managed_fallback_that_is_switched_off(company, secrets, decl)
        .await?
        .ok_or_else(|| OpenCompanyError::Config(NO_MODEL_CHOSEN.to_string()))
```

In the doc comment, step 4 becomes: `4. Otherwise ⇒ [`resolve_effective_scoped`], then [`refuse_a_managed_fallback_that_is_switched_off`]; `None` becomes [`NO_MODEL_CHOSEN`].`

Then run `git grep -n "legacy_hint" src`. If the parameter has no other use
inside `resolve_for_turn`, delete it from the signature, and delete the
argument at every `git grep -n "resolve_for_turn(" src` call site (production
and tests). An unused parameter fails `clippy -D warnings`. Note in the PR body
that the README contract's `legacy_hint` argument is gone.

### 4.2 `resolve_effective_scoped` tail

Replace everything from `let legacy = resolve_legacy_scoped(` to the function's
closing brace with:

```rust
    resolve_legacy_scoped(company, manifest, env_default, secrets, scope).await
}
```

Keep the comment block above it (`:1253-1256`, "The switched-off-Managed refusal
used to sit here…"). In the `resolve_effective` doc (`:1179-1181`), replace
`**provider list > runtime > manifest > env-default > a routing table that names `managed`**`
with `**full default (2b) > provider list > runtime > manifest > env-default**`.

### 4.3 Deletions in `src/company/inference.rs`

1. `resolve_effective_for_tier`, with its whole doc comment (`:1594-1732`).
2. `managed_decl`, with its doc (`:1734-1759`). Check first: `git grep -n "managed_decl" src`
   must show only this definition and the two callers being deleted.
3. Then grep each symbol whose last caller may have gone, and delete it if the
   grep shows only its definition: `LEGACY_MANAGED`, `HarnessScope::default_harness`
   (2b's `decl_for_choice` probably still uses it; keep it if so),
   `use … resolve` items.
4. Doc comments that name deleted items, reworded:
   - `refuse_a_managed_fallback_that_is_switched_off` (`:1322-1325`): replace
     "The switch is honoured on the explicit `managed` route in [`resolve_effective_for_tier`],
     but an **unset** row does not take that branch: it falls through here," with
     "A turn that falls through to the legacy chain lands here,".
   - `decl_for_indexed` (`:1409-1412`): replace "the unrouted path above, which
     reaches it through the primary, and a routing row that names this provider
     by slug" with "the primary, and a full default or agent pair naming this
     provider".
   - `resolve_legacy_scoped` (`:1448-1452`): replace "a routing row naming **entry zero**"
     with "a default or pair naming **entry zero**".

### 4.4 `src/company/inference/resolve.rs` (whole file after the edit)

```rust
//! The provider a company without a full default falls through to. Pure: no
//! IO, no async.
//!
//! Per-workload routing lived here until the keys rework removed it (issue
//! #2306, phase 5b). [`primary`] remains: the legacy resolution path and the
//! status route both use it to name the provider an unset default resolves
//! through.

use super::store::Provider;

/// The provider an unset default falls through to.
///
/// … keep the existing doc from `:305-338`, deleting only the paragraph that
/// begins "A marked provider may be **disabled** or **gone**" down to "because
/// those are choices with a workload attached." and replacing it with:
/// "A marked provider that is disabled or gone falls back to first-enabled."
pub fn primary<'a>(providers: &'a [Provider], marked: Option<&str>) -> Option<&'a Provider> {
    if let Some(marked) = marked.map(str::trim).filter(|s| !s.is_empty())
        && let Some(provider) = providers.iter().find(|p| p.slug == marked && p.enabled)
    {
        return Some(provider);
    }
    providers.iter().find(|p| p.enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use crate::company::inference::store::{ProviderId, ProviderOrigin};

    // `provider(slug, kind, enabled)` helper, verbatim from `:672-683`.
    // Keep `with_no_marker_the_primary_is_the_first_enabled_provider` (`:724`),
    // `a_marked_default_wins_over_list_order` (`:736`) and
    // `a_stale_marker_falls_back_rather_than_stranding_the_company` (`:753`),
    // only if their bodies call nothing but `primary` and `provider`. If one
    // builds `Routes`, rewrite it to call `primary` directly with the same
    // providers and marker, and keep the name.
}
```

Every other item and test in the file is deleted, including the `routes()` test
helper and the `catalogue` import.

### 4.5 `src/company/inference/store.rs`

- Delete `use super::resolve::{ProviderRef, Routes};` (`:65`), `StoredRoutes`,
  `load_routes` and `save_routes` (`:873-920`).
- Keep the constant, with this doc instead of `:865-870`:

```rust
/// The [`SecretStore`] key that held the routing table. Routing was removed in
/// the keys rework (phase 5b): nothing resolves from it, nothing writes or
/// clears it, and only [`routes_carry`](super::routes_carry) reads it, raw, for
/// the carry-over and the console banner.
pub const ROUTES_KEY: &str = "inference/routes";
```

- Delete these tests: `a_company_with_no_routes_reads_an_empty_table` (`:1536`),
  `routes_round_trip_through_the_grammar_an_operator_types` (`:1542`),
  `an_unset_route_is_dropped_rather_than_stored_as_empty` (`:1564`),
  `an_unreadable_routes_blob_is_an_error_rather_than_silently_empty` (`:1580`),
  `a_routing_write_does_not_store_what_it_was_handed` (`:1696`),
  `a_dropped_routing_write_is_visible_on_the_read_back` (`:1724`). Keep
  `FailsWriting`: `:1323` still uses it.

### 4.6 `src/server/ops/inference/providers.rs`

1. **Router:** delete `.merge(scoped("/inference/routes", get(get_routes).put(put_routes)))`.
   Reword the comment at `:97-99` to: "Per provider: the model step needs the
   list of the provider it is choosing for."
2. **`ProviderMutation`:** delete `affected_tiers` and its doc. Then delete
   every `affected_tiers: …,` initializer (`git grep -n "affected_tiers" src`;
   on `fcfb3e1bc` at `:519`, `:1071`, `:1179`, `:1280`, `:1315`, `:1492`, `:1775`,
   plus any 2c or 4a added).
3. **Add:** delete the `auto_route_sole_provider` match (`:506-513`) and pass
   `note` straight through. Delete `auto_route_sole_provider` with its doc
   (`:522-633`) and `is_the_only_thing_that_can_answer` (`:635-652`).
4. **Delete provider:** delete the scrub computation (`let mut routes = …` through
   `let reset = resolve::scrub_removed(…);`), its comment block, and the
   `if !reset.is_empty() { store::save_routes(…) }` write. The note becomes
   `format!("{} is disconnected and its key is cleared.", provider.label)`. Reword
   the marker comment: "The marker goes with the record: a marker naming a
   provider that is gone is a default nobody can see and nobody chose."
5. **Set enabled:** delete `was_primary`, `parked_tiers(…)` and the unset-row
   loop. Keep the default clearing exactly as 2c left it. Shape:

```rust
    if !body.enabled {
        clear_default_if_marked(runtime, &provider.slug).await;
    }
    let note = if body.enabled {
        format!("{} is on.", provider.label)
    } else {
        format!("{} is off.", provider.label)
    };
```

   In the doc (`:1184-1197`), replace the routes paragraph with: "Disabling keeps
   the provider's endpoint, label and credential, so switching it back on is a
   switch rather than a re-configuration."
6. Delete `parked_tiers`, `managed_parked_tiers` and `is_primary`, after
   `git grep -n "is_primary\|parked_tiers" src` shows no other caller.
7. **Managed switch:** keep the route, the store write and the body. Delete only
   the parked-tier computation. Notes: on → `"Managed is on."`, off →
   `"Managed is off. Its credential is untouched."`.
8. Delete the routes section (`:1932-2107`): `RoutesDto`, `get_routes`,
   `PutRoutes` (`git grep -n "struct PutRoutes"`), `put_routes`,
   `route_is_servable`, `has_category`, `mode_name`.
9. **Imports:** remove `managed_resolves` from `use super::{…}` (`:66`). Remove
   `resolve` from `:60` if `grep -n "resolve::" providers.rs` is empty. Remove
   `catalogue::Category` uses if they are gone.
10. **Tests:** delete `a_cloud_route_must_name_a_provider_this_company_holds`
    (`:2130`), `the_slug_less_kinds_are_gated_too` (`:2146`),
    `a_category_that_is_present_serves_its_slug_less_route` (`:2174`),
    `managed_and_unset_name_no_record_and_are_always_servable` (`:2187`), the
    helpers `empty` / `routed_to` (`:2322-2335`),
    `a_sole_provider_with_no_managed_and_no_routes_is_unambiguous` (`:2337`),
    `managed_being_available_makes_it_a_decision_rather_than_a_certainty` (`:2352`),
    `a_table_that_names_anything_is_left_alone` (`:2364`),
    `a_second_enabled_provider_makes_it_a_choice` (`:2386`) and
    `only_enabled_providers_count_and_it_must_be_this_one` (`:2401`). Delete the
    `provider()` helper (`:2114`) if nothing else calls it.

### 4.7 `src/server/ops/inference.rs`

- Delete `routes: BTreeMap<String, String>` and its doc (`:345-352`), leaving
  5a's `routes_not_carried` in place.
- Delete `routing_table` and the doc comment directly above it.
- Delete `let routes = routing_table(runtime).await?;` and both `routes,` lines.
- Delete `managed_resolves` and its doc (`:929-943`); `managed_state` stays.
- Keep `BTreeMap` in the imports: `models` still uses it.

### 4.8 `src/harness/built_in/provider.rs`

In `TenantProvider::resolve`'s doc (`:2088-2095`), delete the sentence from "the
company's routing table routes per workload" to "the next turn has to honour
it." and write: "`tier` is the abstract tier this turn carries; resolution no
longer depends on it, and 2d decides what it means on the wire." If §4.1 removed
`legacy_hint`, drop the argument from this call.

## 5. D-legacy: what a company with no full default and no pin resolves to

After 5b, step 4 is `resolve_effective_scoped` minus the routes branch, then
`refuse_a_managed_fallback_that_is_switched_off`. Before 5b it was
`resolve_effective_for_tier(…, legacy_hint)`. The following is **verified by
reading `fcfb3e1bc`**. 2b's additions do not change it: 2b's full-default step
is skipped when the default is not `Full`, and a company that reaches step 4 has
no `Full` default, or a harness that owns its inference (the same guard at
`:1246`).

**Identical for every company whose `inference/routes` is absent, blank, `{}`,
or holds only unset rows:**

1. **A tier with no row of its own** (not one of the four): `resolve_effective_for_tier`
   already returns `resolve_effective_scoped` + refuse (`:1626-1631`). The only
   code 5b removes from that path is step 4, guarded by `any_route_is_managed(&routes)`
   (`:1296`), which is false with no rows.
2. **One of the four tiers, no row:** `provider_for_workload` gives `Resolution::Primary`
   (`resolve.rs:358-363`). That arm (`:1644-1660`) is `decl_for_primary` under the
   guard `scope.is_default || !harness_configures_itself`; a `Some` returns
   unrefused, and a `None` returns legacy + refuse. `resolve_effective_scoped`
   (`:1246-1260`) has the same guard and the same `decl_for_primary`, then
   legacy. The one difference is that refuse now also sees the `decl_for_primary`
   result, and that is a no-op there: `decl_for_indexed` hard-codes
   `proxied = false` (`:1432`), and refuse errors only when `decl.is_proxied()`
   (`:1348`).
3. **Per source**, then:
   - **Entry zero** (`inference/config`): `decl_for_primary` returns `None` for an
     `EntryZero` primary (`:1400-1403`), so legacy step 1 answers (`:1461-1498`)
     on both paths.
   - **Manifest `[inference]`:** legacy step 2 (`:1501-1542`) on both.
   - **Env default** (`OPENCOMPANY_INFERENCE_URL`, `/openai/v1`): legacy step 3
     (`:1554-1589`) on both, refused on both when proxied with Managed switched off.
   - **Nothing:** `None` → `NO_MODEL_CHOSEN` on both.
4. **Boot** (`builder.rs:3087`, `resolve_effective`): removing step 4 changes
   nothing when no row is `managed`.
5. **The wire model** is decided in `request_plan` by 2d's rule. 5b does not touch it.

**Unchanged for companies with routes too:** a pinned agent never reaches step 4
(step 1), and a company with a full default never reaches it either (step 3). A
unanimous table 5a carried already has a full default.

**Companies whose behaviour changes** are therefore exactly those with no full
default, no pin on the agent in question, and a stored table that names
something. That is the population whose status carries a non-null
`routesNotCarried`, so every one of them already sees the 5a banner.

| Stored rows (no full default) | Before 5b | After 5b | What they see |
|---|---|---|---|
| Any row `managed`; nothing else resolves (no enabled indexed provider, no `inference/config`, no manifest, no env default); switch on; a managed credential | boot: harness brain via step 4 (`:1311-1313`); routed turns on `managed_decl` | boot: echo brain; a turn would fail with `NO_MODEL_CHOSEN` | banner rows `→ Managed`; status `cognition: "echo"`, `restartRequired: false`; the fix is adding TinyHumans (2a) with a model, making it the default, then restarting |
| Row `managed` for tier T; something else resolves | T's turns on the managed chain (TinyHumans bill) | T's turns on the primary or legacy source | banner; T's spend moves off Managed |
| Row `managed`, Managed switched off | T fails: "routed to Managed, which is switched off" (`:1673-1680`) | T resolves via primary or legacy; refused only if that source is proxied | banner |
| `slug:model` or `slug` rows 5a did not carry (partial, mixed, bare slug, disabled or missing provider) | T goes to that provider, with that model; a missing or disabled provider fails closed (`:1685-1695`) | every tier goes to the primary or legacy source | banner |
| `local:` / `claude-code:` rows | first enabled provider of that category (`resolve.rs:386-391`) | primary or legacy | banner |
| Row naming entry zero, turn on a named harness | entry zero at company scope (`:1712-1720`) | the harness carve-out applies | banner |
| Hosted tenant (env default) with rows | legacy step 3 always resolved, so step 4 never ran at boot; only routed turns differ | env default, unless an indexed primary exists | banner |

## 6. Data

Nothing is written, cleared or migrated by 5b. Before and after, byte for byte:

```text
inference/routes    {"agentic-v1":"managed","chat-v1":"acme:test-model"}   (any stored value)
inference/default   ""  |  "acme"  |  {"provider":"acme","model":"test-model"}
inference/providers unchanged
```

The only reader of `inference/routes` left is `routes_carry` (the carry at build,
and the banner in status). A hand-edited value is not validated anywhere any more.

## 7. Ordered edit list

Do these in order, push once, and verify by head SHA.

1. Confirm 5a is green. Run `git grep -n "load_routes\|save_routes\|resolve_effective_for_tier\|any_route_is_managed\|provider_for_workload\|ROUTABLE_WORKLOADS\|ProviderRef\|RoutingMode\|managed_resolves\|affected_tiers\|scrub_removed\|routes_served_by\|orphaned_routes\|tier_label" src tests`
   and keep the list: it is every site you will touch.
2. §4.1–§4.3 (`inference.rs`), then its tests (part 2 §2.1).
3. §4.4 (`resolve.rs`), §4.5 (`store.rs`).
4. §4.6 (`providers.rs`), §4.7 (`ops/inference.rs`), then their tests (part 2 §2.2–§2.3).
5. §4.8 (`provider.rs`).
6. Auth matrix (part 2 §3).
7. Re-run the step 1 grep. It must print only `routes_carry.rs` and `ROUTES_KEY`.
   `cargo fmt --all -- --check`.
8. Console (part 2 §1), then the frontend tests (part 2 §2.4–§2.5). Run
   `git grep -n "getRoutes\|putRoutes\|saveRoutes\|RoutingTab\|WorkloadModelDialog\|routingBadge\|routingState\|affectedTiers\|from \"./routing\"\|@/inference/routing" frontend`
   and expect no output.
9. Docs (part 2 §4). Then the three typecheck gates and `scripts/ci/assert-design-tokens.sh`.
10. Commit `Remove per-workload routing` and push. Check the browser in light and dark.
