# Phase 1b — the search endpoint moves into `search/providers`

Slice 1b of the keys rework (issue #2306). Read [README.md](README.md) first for
the decisions and the naming contract. Code read at `upstream/main @ fcfb3e1bc`
(2026-09-14). Line numbers drift; every step also quotes the code.

## 1. Goal

Store a search provider's instance address inside its row in the
`search/providers` index (new field `IndexEntry.endpoint`), keeping the old
per-slug key as a read fallback and, for one release, as a second write.
Dump item 9. No Q-number; the rundown's recommendation is adopted as written,
with the "write both for one release" rollback rule from rundown part 5 (PR2).

**Which key.** This is `search/providers` — the JSON **index** of connected
providers (`store::PROVIDER_INDEX_KEY`, `store.rs:111`). It is **not**
`search/provider` (`search::PROVIDER_SECRET`, `search/mod.rs:50`), which is the
legacy flat entry-zero slot holding one slug. The operator's wording named
`search/provider`; writing the endpoint there would be wrong.

## 2. Files

| File | Where (on `fcfb3e1bc`) | What |
|---|---|---|
| `src/company/search/store.rs` | module doc diagram `:14-26` | add `endpoint` to the index box |
| same | `provider_endpoint_key` `:125-128` | doc only: now a fallback + rollback copy |
| same | `IndexEntry` `:149-155` | add `endpoint` field |
| same | `list_providers` `:199-261` (loop `:228-258`, comment `:230-249`) | read entry first |
| same | `save_index` `:263-282` | write `endpoint` into each entry |
| same | `put_provider_locked` `:406-437` | merge endpoint; keep per-slug write |
| same | `select_provider` doc `:642-646` | doc only |
| `src/company/search/store_test.rs` | 1,197 lines | new tests, one renamed |
| `docs/modules/search/data-model.md` | storage diagram `:74-86` | add `endpoint` to the index |
| `docs/spec/runtime/search.md` | key table `:76-82` | add the index row |

Read but **not changed**: `src/company/search/mod.rs` (`ENDPOINT_SECRET` `:56-59`),
`src/server/ops/search.rs` (all callers below), `src/harness/built_in/search_byo.rs`
(reads endpoints only through `search::candidates` → `list_providers`, `:137`,
`:162`).

### Every caller of the functions this slice changes

`git grep -n "select_provider\|update_endpoint_if_present\|claim_provider\|put_provider\b" fcfb3e1bc -- src`
(outside `search/store.rs` and `store_test.rs`):

| Call | Endpoint passed |
|---|---|
| `ops/search.rs:523` `claim_provider` (connect) | `validate_draft(...)`: `Some(url)` for SearXNG, `None` for key providers |
| `ops/search.rs:660` `update_endpoint_if_present` (`PUT …/search/providers/{slug}`) | only inside `if let Some(endpoint) = supplied(body.endpoint)`; `validate_draft` returns `None` for a non-endpoint provider |
| `ops/search.rs:982` `select_provider` (legacy `PUT …/search`) | `endpoint.filter(\|_\| info.needs_endpoint())` — `None` when omitted or not SearXNG |
| `ops/search.rs:1023` `update_endpoint_if_present` (legacy `apply_to`) | always `Some` |
| `ops/search.rs:953` `set_enabled` (select managed) | n/a — does not take an endpoint |

`inference.rs`/`ops/inference*` `put_provider` hits are the **inference** store,
a different module. Not touched.

**Does any path clear an endpoint by passing `None`?** No. Verified above:
`supplied()` (`ops/search.rs:395-400`) turns blank into `None`, and every `None`
today is a no-op on the address, because `put_provider_locked` writes the
per-slug key only `if let Some(endpoint)` (`store.rs:413-421`) and the index
stores no address. The only ways an address is cleared are
`delete_provider_locked` (`store.rs:527`) and `DELETE …/search/key`
(`ops/search.rs:1055-1060`). The pinned test
`selecting_a_provider_without_an_address_keeps_the_one_stored_now`
(`store_test.rs:1029-1076`) states the contract: an omitted address is kept.

**Does anything still write the flat `search/endpoint` non-empty?** No.
`git grep -n "ENDPOINT_SECRET" fcfb3e1bc -- src` gives only `mod.rs:59` (the
const), `ops/search.rs:62` (import) and `:1056` (a clear), `store.rs` reads and
the delete clear, and tests. This matters in §11.

**SSRF validation is unchanged.** `validate_endpoint` (`ops/search.rs:433-455`)
runs before every stored address; this slice only changes where a validated
address is kept.

## 3. Current code

`store.rs:149-155`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    slug: String,
    #[serde(default = "yes")]
    enabled: bool,
}
```

No `deny_unknown_fields`, so a blob with extra fields already parses.

`store.rs:228-258` (comment elided):

```rust
for entry in index {
    let mut endpoint = read(company, secrets, &provider_endpoint_key(&entry.slug)).await?;
    if endpoint.is_none() && entry_zero.as_deref() == Some(entry.slug.as_str()) {
        endpoint = read(company, secrets, ENDPOINT_SECRET).await?;
    }
    providers.push(SearchProvider { slug: entry.slug, enabled: entry.enabled, endpoint });
}
```

`store.rs:269-275`:

```rust
.map(|provider| IndexEntry {
    slug: provider.slug.clone(),
    enabled: provider.enabled,
})
```

`store.rs:411-436`:

```rust
let mut providers: Vec<SearchProvider> = list_providers(company, secrets).await?;
if let Some(endpoint) = provider.endpoint.as_deref() {
    write(company, secrets, &provider_endpoint_key(&provider.slug), endpoint).await?;
}
match providers.iter_mut().find(|existing| existing.slug == provider.slug) {
    Some(existing) => *existing = provider,
    None => providers.push(provider),
}
save_index(company, secrets, &providers).await
```

`*existing = provider` with `endpoint: None` is harmless **today** only because
`save_index` drops the address. Once `save_index` writes it, the same line
would erase it. That is the one change that makes the merge in §4 mandatory.

## 4. Target code

### `IndexEntry`

```rust
/// The stored shape of one index row. Deliberately not [`SearchProvider`].
///
/// Never add `#[serde(deny_unknown_fields)]`: a binary one release older must
/// still parse a blob this one wrote (it ignores `endpoint`), and this one must
/// parse blobs written before `endpoint` existed (`serde(default)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    slug: String,
    #[serde(default = "yes")]
    enabled: bool,
    /// The instance URL, for a self-hosted provider. Not a secret. Absent from
    /// the blob (not `null`) when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    endpoint: Option<String>,
}

/// Trims, and treats blank as absent — the same rule [`read`] applies.
fn non_blank(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
```

### `list_providers` loop (replaces `:228-258`)

```rust
for entry in index {
    // 1. The index row itself — where every write since #2306 puts it.
    let mut endpoint = non_blank(entry.endpoint);
    // 2. The per-slug key — where it lived from 2026-09-11 until #2306, and
    //    where a rolled-back binary still writes it.
    if endpoint.is_none() {
        endpoint = read(company, secrets, &provider_endpoint_key(&entry.slug)).await?;
    }
    // 3. The flat entry-zero address, for the slug the flat keys describe.
    //    (Keep the existing long comment from :230-249 here, amended per §5.)
    if endpoint.is_none() && entry_zero.as_deref() == Some(entry.slug.as_str()) {
        endpoint = read(company, secrets, ENDPOINT_SECRET).await?;
    }
    providers.push(SearchProvider {
        slug: entry.slug,
        enabled: entry.enabled,
        endpoint,
    });
}
```

`non_blank(entry.endpoint)` moves one field out of `entry`; `entry.slug` and
`entry.enabled` are still usable afterwards (partial move, no `Drop` impl).

The synthesised entry-zero row above the loop (`:218-226`) is unchanged.

### `save_index` map (replaces `:271-274`)

```rust
.map(|provider| IndexEntry {
    slug: provider.slug.clone(),
    enabled: provider.enabled,
    endpoint: non_blank(provider.endpoint.clone()),
})
```

### `put_provider_locked` (replaces the body `:411-436`)

```rust
let mut providers: Vec<SearchProvider> = list_providers(company, secrets).await?;
let mut provider = provider;
provider.endpoint = non_blank(provider.endpoint.take());

if let Some(endpoint) = provider.endpoint.as_deref() {
    // **Also** the per-slug key, for one release (#2306). The index entry is
    // what this binary reads first; this copy is what a binary rolled back to
    // before #2306 reads, since it ignores `IndexEntry.endpoint`. Remove this
    // write only in a release after the one that ships the index field.
    write(company, secrets, &provider_endpoint_key(&provider.slug), endpoint).await?;
}

// (keep the "In place, not moved to the end" comment from :423-428)
match providers
    .iter_mut()
    .find(|existing| existing.slug == provider.slug)
{
    Some(existing) => {
        existing.enabled = provider.enabled;
        // **Merge, never replace.** An omitted address keeps the stored one:
        // a toggle, a legacy select with no address, or a key replace sends
        // `None`, and replacing the row would wipe a SearXNG URL — agents then
        // fall back to managed search and the bill moves to the platform.
        // Nothing clears an address through here; removal does that.
        if provider.endpoint.is_some() {
            existing.endpoint = provider.endpoint;
        }
    }
    None => providers.push(provider),
}
save_index(company, secrets, &providers).await
```

`set_enabled` (`:444-462`) and `delete_provider_locked` (`:515-566`) need **no code
change**: `set_enabled` edits a row read by `list_providers`, which now carries
the address, and `save_index` writes it back; delete filters the row out
(taking the field with it) and still clears `provider_endpoint_key(slug)` at
`:527`.

## 5. Ordered edit list

1. `store.rs:149-155` — replace `IndexEntry` with the §4 version; add
   `non_blank` directly below `fn yes()` (`:157-159`).
2. `store.rs:228-258` — replace the loop with §4. Amend the comment at
   `:245-249` ("Read-side fallback rather than a copy at write time…"): it is
   now both. Add one sentence: "Since #2306 the next index mutation also writes
   this address into the row, which is safe because nothing writes
   `search/endpoint` any more — it can only be cleared."
3. `store.rs:269-275` — add `endpoint` to the `IndexEntry` built in `save_index`.
4. `store.rs:411-436` — replace the body of `put_provider_locked` with §4.
5. `store.rs:125-128` — doc of `provider_endpoint_key`: "The per-slug
   instance-address slot. Read as a fallback when an index row carries no
   `endpoint`, and still written alongside the row for one release so a
   rolled-back binary finds the address (#2306). Not a secret."
6. `store.rs:642-646` — in `select_provider`'s doc, replace "the index carries
   no address, and [`put_provider_locked`] writes one only when given one" with
   "[`put_provider_locked`] merges: a `None` address keeps the stored one."
7. `store.rs:14-26` — module diagram: in the right box change
   `search/providers        index` to `search/providers  index (+endpoint)`
   and `search/provider/<slug>/endpoint` to
   `search/provider/<slug>/endpoint  fallback`. Keep the box width.
8. `store_test.rs` — §7.
9. `docs/modules/search/data-model.md:74-86` — same two diagram edits as step 7;
   below the diagram add: "Since #2306 the address is stored in the index row
   (`[{"slug":"searxng","enabled":true,"endpoint":"https://search.example"}]`).
   Reads take the row, else `search/provider/<slug>/endpoint`, else
   `search/endpoint` for entry zero. Writes go to the row and, for one release,
   also to the per-slug key."
10. `docs/spec/runtime/search.md:76-82` — the table lists only the flat keys;
    add rows `search/providers` (read back: yes; holds: the connected providers,
    each `{slug, enabled, endpoint?}`) and `search/provider/<slug>/endpoint`
    (read back: yes; holds: fallback address, see data-model.md). Change
    "Three secret-store keys" to "Secret-store keys".
11. `cargo fmt --all -- --check`. Commit: `Store the search endpoint in the provider index`.

## 6. Data carry-over

No boot migration. Nothing is written until an operator action mutates the index.
Read paths give the same answer before and after the upgrade for every stored
shape below. Keys not shown are unchanged.

**A. Connected through the list (per-slug address).** Before, and after the
upgrade until the next mutation:

```json
search/providers                  = [{"slug":"searxng","enabled":true},{"slug":"brave","enabled":true}]
search/provider/searxng/endpoint  = "https://search.example"
```

After any index mutation (for example, `brave` switched off):

```json
search/providers                  = [{"slug":"searxng","enabled":true,"endpoint":"https://search.example"},{"slug":"brave","enabled":false}]
search/provider/searxng/endpoint  = "https://search.example"
```

**B. Legacy entry zero (flat keys, no index).** Before:

```json
search/provider  = "searxng"
search/endpoint  = "https://search.example"
```

After switching it off (the address is copied into the row; flat keys kept;
per-slug key **not** written — a rolled-back binary still reads the flat key):

```json
search/providers = [{"slug":"searxng","enabled":false,"endpoint":"https://search.example"}]
search/provider  = "searxng"
search/endpoint  = "https://search.example"
```

**C. Re-address** to `https://search2.example` on the new binary:

```json
search/providers                  = [{"slug":"searxng","enabled":true,"endpoint":"https://search2.example"}]
search/provider/searxng/endpoint  = "https://search2.example"
```

**D. Remove `searxng`** (from C):

```json
search/providers                  = []
search/provider/searxng/endpoint  = ""
search/provider/searxng/key       = ""
```

**E. Rollback** to a binary without this slice, from C. It parses the blob
(unknown field ignored), reads `search2` from the per-slug key, and on its next
index write re-serialises without `endpoint`. Rolling forward again reads the
per-slug key (step 2), which that binary kept current. No state loses the
address.

## 7. Tests

All in `src/company/search/store_test.rs`, using its existing `MemSecrets`,
`seed`, `company()`. Add this helper beside `seed`:

```rust
fn stored_index(secrets: &MemSecrets) -> serde_json::Value {
    let raw = secrets.map.lock().unwrap().get(PROVIDER_INDEX_KEY).cloned().expect("index written");
    serde_json::from_str(&raw).expect("index is JSON")
}
```

Addresses below are fake (`*.acme.internal`).

| # | Name | Setup | Call | Assert |
|---|---|---|---|---|
| 1 | `an_index_row_without_an_endpoint_reads_the_per_slug_address` | seed `PROVIDER_INDEX_KEY` = `[{"slug":"searxng","enabled":true}]`, `search/provider/searxng/endpoint` = `http://old.acme.internal` | `list_providers` | `[0].endpoint == Some("http://old.acme.internal")` |
| 2 | `an_index_row_without_an_endpoint_falls_back_to_the_flat_address_for_entry_zero` | seed `PROVIDER_SECRET`=`searxng`, `ENDPOINT_SECRET`=`http://flat.acme.internal`, index `[{"slug":"searxng","enabled":false}]` | `list_providers` | one row; endpoint is the flat URL; `enabled == false` |
| 3 | `the_flat_address_is_never_read_for_a_slug_that_is_not_entry_zero` | seed `PROVIDER_SECRET`=`exa`, `ENDPOINT_SECRET`=`http://flat.acme.internal`, index `[{"slug":"searxng","enabled":true}]` | `list_providers` | the `searxng` row has `endpoint == None` |
| 4 | `the_index_row_address_wins_over_the_per_slug_one` | index `[{"slug":"searxng","enabled":true,"endpoint":"http://row.acme.internal"}]`, per-slug `http://slug.acme.internal` | `list_providers` | endpoint is `http://row.acme.internal` |
| 5 | `a_blank_address_in_the_index_row_reads_as_absent` | index row with `"endpoint":"  "`, per-slug `http://slug.acme.internal` | `list_providers` | endpoint is the per-slug URL |
| 6 | `re_addressing_writes_the_index_row_and_the_per_slug_key` | `put_provider` searxng `Some(old)` | `update_endpoint_if_present(…, Some("http://new.acme.internal"))` | `stored_index[0]["endpoint"] == "http://new.acme.internal"`; `map["search/provider/searxng/endpoint"] == "http://new.acme.internal"` |
| 7 | `an_omitted_address_survives_toggle_select_reconnect_and_a_second_provider` | `put_provider` searxng `Some("http://kept.acme.internal")`; **then seed per-slug key to `""`** so only the row holds it | in order: `set_enabled(searxng,false)`; `select_provider(searxng, None, None)`; `update_endpoint_if_present(searxng, None)`; `put_provider(brave, None)`; `claim_provider(exa, None)` → `true` | after **each** call: `list_providers` searxng endpoint `== Some(kept)` and `stored_index` searxng entry `"endpoint" == kept`. After `select_provider`: `enabled == true` and default is `searxng` |
| 8 | `removing_a_provider_takes_its_address_with_it` | `put_provider` searxng `Some(url)` | `delete_provider(searxng)`; then `put_provider(searxng, None)` | after delete: `stored_index == json!([])`, per-slug `== ""`; after re-add: endpoint `None` (no resurrection) |
| 9 | `an_index_blob_written_before_endpoint_existed_still_parses` | index `[{"slug":"brave","enabled":true},{"slug":"exa"}]` | `list_providers` | 2 rows; `exa.enabled == true`; both endpoints `None` |
| 10 | `a_row_with_no_address_is_stored_without_the_field` | empty store | `put_provider(brave, None)` | raw `map[PROVIDER_INDEX_KEY] == r#"[{"slug":"brave","enabled":true}]"#` (no `"endpoint":null`) |
| 11 | `a_blob_with_endpoint_parses_as_the_row_shape_before_it` | none | define in the test `#[derive(serde::Deserialize)] struct PreviousIndexEntry { slug: String, #[serde(default = "yes")] enabled: bool }` (the `fcfb3e1bc` shape, **no** `deny_unknown_fields`); `serde_json::from_str::<Vec<PreviousIndexEntry>>(r#"[{"slug":"searxng","enabled":true,"endpoint":"http://row.acme.internal"}]"#)` | `is_ok()`; slug `searxng`. Rollback safety |
| 12 | `switching_off_entry_zero_copies_its_flat_address_into_the_row` | seed `PROVIDER_SECRET`=`searxng`, `ENDPOINT_SECRET`=`http://flat.acme.internal` | `set_enabled(searxng,false)` | `stored_index[0]["endpoint"] == flat`; `map[ENDPOINT_SECRET]` still `http://flat.acme.internal`; `map` has **no** `search/provider/searxng/endpoint` entry |

**Existing tests to change:**

- `a_self_hosted_endpoint_round_trips_on_its_own_address` (`:331-351`) — rename to
  `a_self_hosted_endpoint_round_trips_through_the_index_row`; keep its assert and
  add `stored_index(&secrets)[0]["endpoint"] == "https://search.acme.internal"`
  and the per-slug key equal to the same URL.
- `a_legacy_searxng_address_survives_the_slug_entering_the_index`
  (`:662-748`) — no change required; it must still pass unmodified.

**Existing tests that must pass unmodified** (they pin the merge):
`selecting_a_provider_without_an_address_keeps_the_one_stored_now`,
`a_re_address_refuses_rather_than_recreating_a_removed_row`,
`re_addressing_a_provider_keeps_its_place_in_the_list`,
`a_refused_claim_writes_nothing`, `a_failed_legacy_address_clear_is_still_retried_as_entry_zero`,
and in `search_byo.rs` `the_marked_provider_is_the_one_the_harness_wires` and
`moving_the_marker_moves_which_credential_is_used`, which seed old-shape blobs
(`search_byo.rs:512-520`, `:540-548`). No test is deleted.

`store_test.rs` is ungated (`company::search` always compiles), so it runs in
the `Rust` CI job; `search_byo.rs` tests run in `Rust (openhuman, tinymemory)`.

## 8. Console / UI

None. `SearchProviderView.endpoint` (`ops/search.rs:350`) and the status DTO
(`:305-323`) are built from `list_providers`, whose output is unchanged. No
route, DTO, copy or testid changes. No browser check is needed for this slice.

## 9. Must not touch

- `search/provider`, `search/api_key`, `search/endpoint` semantics, and the
  entry-zero synthesis at `store.rs:216-226`.
- `search/provider/<slug>/key`, `search/default`, `store_provider_key`,
  `load_provider_key`, the lock (`INDEX_LOCKS`, `index_guard`).
- The per-slug **read** (step 2 of the chain) and the per-slug **clear** on
  delete (`store.rs:527`). Both stay in this PR.
- `validate_endpoint` / `validate_draft` / `supplied` in `ops/search.rs`.
- Row order: an existing row is edited in place, a new slug is appended.
- The inference store (`src/company/inference/store.rs`), which has its own
  `PROVIDER_INDEX_KEY`.
- No new secret-store key.

## 10. Done when

- Every test in §7 exists and the renamed test is renamed.
- `cargo fmt --all -- --check` is clean locally.
- Pushed; on the head SHA, `gh api "repos/tinyhumansai/opencompany/actions/runs?head_sha=$SHA"`
  → the CI run's jobs show `Rust` and `Rust (openhuman, tinymemory)` green,
  zero failures **and** zero pending across all jobs.
- `git grep -n "provider_endpoint_key" -- src` still shows the read in
  `list_providers`, the write in `put_provider_locked`, and the clear in
  `delete_provider_locked`.
- `docs/modules/search/data-model.md` and `docs/spec/runtime/search.md` updated;
  both files stay at 500 lines or fewer.

## 11. Gotchas

- **`skip_serializing_if` is required**, not just `serde(default)`. Without it
  every row gains `"endpoint":null`; test 10 fails and blob diffs get noisy.
- **Never `*existing = provider`.** That line is the regression the rundown
  warned about: once the row carries the address, a toggle or a legacy select
  with no address would erase it, silently moving agents to managed search.
- **Test 7 must blank the per-slug key first.** Without that, the per-slug
  fallback hides a broken merge and the test passes for the wrong reason.
- **The flat address is copied into the row on the next mutation** (case B).
  This contradicts the older comment at `store.rs:245-249`, which is why step 2
  amends it. It is safe only because nothing writes `search/endpoint`
  non-empty any more (§2). If some future change writes it again, the row copy
  would shadow it.
- **Failure between the two writes.** `put_provider_locked` writes the per-slug
  key first, then the index (same order as today). If the index write fails,
  the call returns an error and the row still holds the old address, which is
  what this binary reads. Retrying the save fixes it. Do not reorder.
- **Precedence is row → per-slug → flat.** A per-slug value newer than the row
  can only come from a rolled-back binary, which always rewrites the index
  without the field. So the row never goes stale against the per-slug key.
- **Rundown part 2 says "clear the per-slug key in the same operation".** Do
  not. Rundown part 5 (PR2) and this brief write both for one release. Clearing
  it is a later release's change, together with dropping the per-slug read.
- `IndexEntry` is private; tests reach it because `store_test.rs` is a child
  module (`#[path = "store_test.rs"] mod store_test;`, `store.rs:718-720`), but
  prefer asserting raw JSON through `stored_index` as above.
