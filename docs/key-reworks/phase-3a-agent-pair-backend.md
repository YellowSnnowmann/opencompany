# Phase 3a — agent `{provider, model}` pair: record, validation, resolver

Slice 3a of the keys rework (issue #2306). Written 2026-09-14 against
`upstream/main @ fcfb3e1bc`. Continued in
[phase-3a-agent-pair-backend-part2.md](phase-3a-agent-pair-backend-part2.md)
(data carry-over, tests, console, must-not-touch, done-when, gotchas).

**Starts only after:** slices 2a, 2b, 2c and 2d are committed on
`feat/key-reworks`. PR #2305 was closed on 2026-09-14; nothing here depends on
it. Every `file:line` below is on `fcfb3e1bc`; re-find each one by its quoted
excerpt before editing, because slices 2a–2d move lines in `provider.rs`,
`build.rs` and `inference.rs`.

## 1. Goal

An agent on a `built_in` harness may carry its own `{provider, model}` pair on
its existing record, and that agent's own turns resolve agent pair → named
harness `[harness.inference]` → company default, failing closed when the pair
names a missing or switched-off provider.

- Dump items **13(c)** and **19**; decisions **Q5**, **F5**, **F6**; README goal 3–4.
- Operator's examples: the researcher on `anthropic` / `test-model-large`, the
  web-search agent on `anthropic` / `test-model-small` (a larger model for
  research, a smaller one for search, as the operator described, item 19), the
  writer with nothing set → company default.

Contract names used verbatim: `Agent/OverlayAgent/AgentOverride.provider`, `store::ModelChoice` (2b),
`resolve_for_turn(…, pin, …)` (2b), `check_model_id` (2c), and — changed from the README on
2026-09-14 so the turn error names the agent — `HarnessModel::pinned(&self, agent_id: &str, choice: &ModelChoice)`.

## 2. Files (with `fcfb3e1bc` lines)

| File | Where | What changes |
|---|---|---|
| `src/company/types.rs` | `Agent` `:634-684`; `harness` `:670`, `model` `:684` | new `provider` field |
| `src/company/agent_file.rs` | `AgentFile` `:75-102`; `Ok(Agent {…})` `:278-293`; test `:511` | carry `provider` |
| `src/ports/types.rs` | `OverlayAgent` `:3410-3453` (`model` `:3445`, `harness` `:3452`) | new field |
| `src/ports/types.rs` | `AgentOverride` `:3493-3562` (`model` `:3556`, `harness` `:3561`) | new field |
| `src/ports/types.rs` | `upsert_agent_override` `:5553-5583` | copy `provider` |
| `src/ports/types.rs` | merge in `effective_manifest_agent` `:5681-5686` | merge `provider` |
| `src/ports/types.rs` | `retain_nonempty_agent_edits` `:5864-5874` | count `provider` |
| `src/ports/types.rs` | tests `:8901`, `:8953` | extend + add |
| `src/harness/built_in/mod.rs` | `overlay_fingerprint` `:4983`, hashes `:5022-5023` and `:5042-5043` | hash `provider` |
| `src/harness/built_in/mod.rs` | `override_fingerprint` hashes `:5356-5357` | hash `provider` |
| `src/harness/built_in/mod.rs` | `overlay_agent_to_manifest` `:5712-5724` | carry `provider` |
| `src/harness/lanes.rs` | `agent_models_on` doc `:210-219` | comment only |
| `src/store/conformance.rs` | `sample_overlay_agents` `:183-228`; `sample_agent_overrides` `:278-298` | set `provider` |
| `src/company/manifest.rs` | model loop `:1036-1076`; test `:3053` | pair rule |
| `src/server/ops/team_agent.rs` | `EDITABLE_FIELDS` `:118-136`; DTO `:190-203`; `declared_model` `:337-352`; `EditAgent` `:558-575`; admin gate `:775`; hoists `:797-803`; cross-field `:836-880`; `routing_changed` `:883`; persist `:913-918`, `:954-959`; DTO fill `:1218-1219`; tests `mod tests` `:2193` | pair write |
| `src/harness/built_in/provider.rs` | `HarnessModel` `:85-129`; `TenantProvider` `:1999-2027`; `new` `:2030-2048`; `resolve` `:2096-2133` | `pinned`, `pin` |
| `src/harness/built_in/build.rs` | model string `:1159-1162`; `.chat_model(deps.provider…)` `:1233` | per-agent provider |
| `src/runtime/builder.rs` | `configured` `:3083-3104`; overlays in scope `:2702-2712`; tests `:4638` | count pairs |

`HarnessDeps.provider` is `mod.rs:257`. Production `TenantProvider`s: `builder.rs:3315-3328`
(default harness), `lanes.rs:424-437` (each named `built_in` harness).

**Earlier-slice overlap.** Slices 2a (TinyHumans on the proxy) and 2b edit
`provider.rs`; 2b replaces the body of `TenantProvider::resolve` with a
`resolve_for_turn` call. Re-read `resolve` on the branch head before step 13.

## 3. Current code (quoted so it can be found)

`src/company/types.rs:683-684` — `model` today is ACP-only:

```rust
    /// … validation rejects it on a `built_in`-harness agent rather than
    /// silently ignoring it …
    #[serde(default)]
    pub model: Option<String>,
```

`src/ports/types.rs:3556-3561` (`AgentOverride`):

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Cleared the same way as [`Self::model`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
```

`src/ports/types.rs:5681-5686` (merge):

```rust
        if let Some(model) = entry.model.as_ref() {
            merged.model = Some(model.clone()).filter(|text| !text.is_empty());
        }
        if let Some(harness) = entry.harness.as_ref() {
            merged.harness = Some(harness.clone()).filter(|text| !text.is_empty());
        }
```

`src/company/manifest.rs:1061-1073` — the refusal F5 changes:

```rust
                Some(harness) => {
                    problems.push(format!(
                        "agent `{}` names a `model` but runs on harness `{}` (`kind = \"{}\"`), \
                         which has no ACP transport to forward it to. Bind this agent to an \
                         `acp` harness, or drop `model`.",
```

`src/server/ops/team_agent.rs:849-857` — the route's copy of that refusal:

```rust
        let bound = record.manifest.harness_by_id(&resulting_harness_id);
        if bound.as_ref().map(|h| h.kind.as_str()) != Some("acp") {
            return Err(ApiError(OpenCompanyError::InvalidRequest(format!(
                "`{model_value}` names a model, but this teammate's harness has no ACP \
```

`src/server/ops/team_agent.rs:775` and `:883`:

```rust
    if body.tools.is_some() || body.model.is_some() || body.harness.is_some() {
        require_admin(&headers, &state, &company.runtime, peer).await?;
    }
    …
    let routing_changed = model.is_some() || harness.is_some();
```

`src/harness/built_in/build.rs:1159-1162` and `:1233`:

```rust
    let model = deps
        .model_override
        .clone()
        .unwrap_or_else(|| model_for_tier(manifest_agent.tier.as_deref()));
    …
        .chat_model(deps.provider.clone() as Arc<dyn tinyinference::model::ChatModel<()>>)
```

(If 2d changed the model line, e.g. to `deps.turn_model_hint()`, keep whatever 2d left.)

`src/runtime/builder.rs:3087-3104`:

```rust
                        let configured = inference::resolve_effective(
                            &id, &effective_manifest, env_default.as_ref(), secrets.as_ref(),
                        ).await.unwrap_or_else(|err| { … None }).is_some();
```

## 4. Target code

### 4.1 Record fields

`Agent` (`types.rs`), inserted directly after `model` (`:684`). Rewrite the
`model` doc's "validation rejects it on a `built_in`-harness agent" sentence to
"on a `built_in` harness it is the model half of the agent pair; see
[`provider`](Self::provider)".

```rust
    /// The provider half of this agent's own `{provider, model}` pair (keys
    /// rework slice 3a, issue #2306): a slug in the company's
    /// `inference/providers` list, e.g. `anthropic`.
    ///
    /// Only on a `built_in` harness, and only together with
    /// [`model`](Self::model). Both set: this agent's own turns use that
    /// provider and model, ahead of the harness `[harness.inference]` and the
    /// company default. Neither set: the agent follows the default. One
    /// without the other, or `provider` on an `acp` harness, is refused by
    /// `CompanyManifest::validate`. Never checked against the provider list at
    /// load — that list is console data a manifest cannot see — so a slug that
    /// is not there fails the agent's first turn (F6), with no fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
```

`AgentFile` (`agent_file.rs`), after `model` (`:102`), and `provider: file.provider,`
after `model: file.model,` (`:293`):

```rust
    /// The provider half of the agent pair; see `Agent::provider`. Cross-checked
    /// in `CompanyManifest::validate`, like `model`.
    #[serde(default)]
    provider: Option<String>,
```

`OverlayAgent` (`ports/types.rs`), after `harness` (`:3452`):

```rust
    /// The provider half of this teammate's `{provider, model}` pair — see
    /// [`Agent::provider`](crate::company::types::Agent::provider). `None` (the
    /// default, and every record written before this field) means no pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
```

`AgentOverride` (`ports/types.rs`), after `harness` (`:3561`):

```rust
    /// The provider half of the pair, as an overlay on the blueprint.
    ///
    /// Cleared the same way as [`Self::model`]: `Some("")` is the stored
    /// "cleared" form, `None` is "never edited".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
```

`upsert_agent_override`, after the `harness` block (`:5580-5582`):

```rust
            if entry.provider.is_some() {
                held.provider = entry.provider;
            }
```

Merge, after the `harness` block (`:5685-5686`):

```rust
        if let Some(provider) = entry.provider.as_ref() {
            merged.provider = Some(provider.clone()).filter(|text| !text.is_empty());
        }
```

`retain_nonempty_agent_edits` (`:5872-5873`): add `|| entry.provider.is_some()`.

`overlay_agent_to_manifest` (`mod.rs`), after `harness: overlay.harness.clone(),`
(`:5724`): `provider: overlay.provider.clone(),`.

Fingerprints (`mod.rs`), after each `model`/`harness` hash pair:
`edit.provider.hash(&mut hasher);` (`:5023`), `agent.provider.hash(&mut hasher);`
(`:5043`), `entry.provider.hash(&mut hasher);` (`:5357`). And `is_avatar_only`
(`:4957-4965`) lists every field: add `&& edit.provider.is_none()` after
`edit.harness.is_none()`, or a provider-only edit is skipped by the fingerprint.

Every other `Agent` / `ManifestAgent` / `OverlayAgent` literal without `..` gains
`provider: None` (`Agent` derives no `Default` on `fcfb3e1bc`). Find them with:

```sh
git grep -nE "\b(Agent|ManifestAgent|OverlayAgent) \{$" -- src tests
```

### 4.2 Manifest validation (`manifest.rs:1036-1076`)

Replace the loop body. Keep the "unknown harness" skip (`None => {}`) and the
existing empty/runner messages byte for byte.

```rust
        for agent in &self.agents {
            let provider = agent.provider.as_deref();
            let model = agent.model.as_deref();
            if provider.is_none() && model.is_none() {
                continue;
            }
            if provider.is_some_and(|p| p.trim().is_empty()) {
                problems.push(format!(
                    "agent `{}`'s `provider` is set but empty. Drop the key to use the \
                     company default provider and model.",
                    agent.id
                ));
                continue;
            }
            if model.is_some_and(|m| m.trim().is_empty()) {
                /* today's "is set but empty" message, unchanged */
                continue;
            }
            match self.harness_for(&agent.id) {
                Some(harness) if harness.kind == "acp" => {
                    if provider.is_some() {
                        problems.push(format!(
                            "agent `{}` names a `provider` but runs on harness `{}` \
                             (`kind = \"acp\"`), which brings its own provider. Drop \
                             `provider`, or bind a `built_in` harness.",
                            agent.id, harness.id
                        ));
                    }
                    if model.is_some() && /* transport == "runner" */ {
                        /* today's runner message, unchanged */
                    }
                }
                Some(harness) => match (provider, model) {
                    (Some(p), Some(m)) => {
                        if store::slugify(p) != p || p.chars().count() > store::MAX_PROVIDER_NAME_CHARS {
                            problems.push(format!(
                                "agent `{}`'s `provider` `{p}` is not a provider slug: lowercase \
                                 letters, digits and `-`, at most {} characters.",
                                agent.id, store::MAX_PROVIDER_NAME_CHARS
                            ));
                        }
                        if let Err(why) = check_model_id(m) {
                            problems.push(format!("agent `{}`'s `model`: {why}", agent.id));
                        }
                    }
                    (None, Some(_)) => problems.push(format!(
                        "agent `{}` names a `model` but no `provider`, on harness `{}` \
                         (`kind = \"{}\"`). Set `provider` too, or bind an `acp` harness.",
                        agent.id, harness.id, harness.kind
                    )),
                    (Some(_), None) => problems.push(format!(
                        "agent `{}` names a `provider` but no `model`. Set `model` too, or drop \
                         `provider` to use the company default.",
                        agent.id
                    )),
                    (None, None) => unreachable!("both-absent returned above"),
                },
                None => {}
            }
        }
```

`store` is `crate::company::inference::store` (`slugify` `:320`,
`MAX_PROVIDER_NAME_CHARS` `:281`). Call `check_model_id` exactly as slice 2c
defines it; if it returns something other than `Result<_, impl Display>`, adapt
the `Err` arm only. **Check `harness_for` on a manifest with no `[[harness]]`
at all** (see part 2 §11 G2) before trusting `None => {}`.

### 4.3 Route (`team_agent.rs`)

1. `EDITABLE_FIELDS: [&str; 9]`, `"provider"` after `"model"`. Fix the doc's
   "Seven since …" count sentence to name the keys rework.
   `EDITABLE_FIELDS_MEMBER` (`:147`) is unchanged.
2. `EditAgent`, after `harness` (`:575`):

   ```rust
       /// The provider half of this teammate's `{provider, model}` pair (keys
       /// rework slice 3a). Same double option and admin gate as `model`:
       /// absent leaves it alone, `null` clears it (back to the company
       /// default), a string sets it. Sent together with `model` by the console.
       #[serde(default, deserialize_with = "double_option")]
       provider: Option<Option<String>>,
   ```

3. Admin gate (`:775`): `|| body.provider.is_some()`.
4. Hoist beside `harness` (`:801-803`):
   `let provider = body.provider.map(|t| t.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));`
5. `declared_provider(record, agent_id) -> Option<String>`, a copy of
   `declared_model` (`:337-352`) reading `.provider`.
6. Replace the cross-field block (`:836-880`) with:

   ```rust
       let resulting_harness_id = harness
           .clone()
           .unwrap_or_else(|| declared_harness(&record, &agent_id))
           .unwrap_or_else(|| record.manifest.default_harness_id());
       let bound = record.manifest.harness_by_id(&resulting_harness_id);
       let on_acp = bound.as_ref().map(|h| h.kind.as_str()) == Some("acp");
       let resulting_model = model.clone().unwrap_or_else(|| declared_model(&record, &agent_id));
       let resulting_provider =
           provider.clone().unwrap_or_else(|| declared_provider(&record, &agent_id));

       if on_acp {
           if let Some(p) = &resulting_provider {
               return Err(bad_request(format!(
                   "`{p}` names a provider, but harness `{resulting_harness_id}` is an ACP \
                    harness, which brings its own. Clear the provider, or bind a built-in harness."
               )));
           }
           if let Some(model_value) = &resulting_model {
               /* today's runner-transport refusal (`:866-879`), verbatim */
           }
       } else {
           match (&resulting_provider, &resulting_model) {
               (None, None) => {}
               (Some(p), None) => return Err(bad_request(format!(
                   "Choose a model for `{p}`, or clear the provider to use the company default."
               ))),
               (None, Some(m)) => return Err(bad_request(format!(
                   "`{m}` names a model but no provider. Choose a provider too, or clear the \
                    model to use the company default."
               ))),
               (Some(p), Some(m)) => {
                   // Only when this request touches the pair or the binding: a name
                   // edit must not 400 because a provider was switched off since.
                   if provider.is_some() || model.is_some() || harness.is_some() {
                       match inference::store::get_provider(company.id(), secrets, p).await? {
                           None => return Err(bad_request(format!(
                               "This company has no provider `{p}`. Add it in Settings → \
                                Inference first."
                           ))),
                           Some(row) if !row.enabled => return Err(bad_request(format!(
                               "Provider `{p}` is switched off. Switch it on in Settings → \
                                Inference, or choose another."
                           ))),
                           Some(_) => {}
                       }
                       check_model_id(m).map_err(|why| bad_request(why.to_string()))?;
                   }
               }
           }
       }
   ```

   `bad_request(msg)` stands for the file's existing
   `ApiError(OpenCompanyError::InvalidRequest(msg)).into()`; write it out
   inline or add a one-line private helper. `secrets` is the company's secret
   store: copy how `src/server/ops/inference/providers.rs` obtains the handle it
   passes to `store::get_provider` (`git grep -n "get_provider(" src/server`).
   `get_provider` is `store.rs:514-525` and includes entry zero.

7. `routing_changed` (`:883`): `model.is_some() || harness.is_some() || provider.is_some()`.
8. Persist. Manifest branch after `entry.harness` (`:916-918`):
   `if let Some(provider) = provider { entry.provider = Some(provider.unwrap_or_default()); }`.
   Overlay branch after `agent.harness` (`:957-959`):
   `if let Some(provider) = provider { agent.provider = provider; }`.
9. DTO: after `model` (`:203`) add
   `#[serde(skip_serializing_if = "Option::is_none")] provider: Option<String>,`
   with a doc naming the pair; fill `provider: declared_provider(record, agent_id),`
   after `model:` (`:1219`). Rewrite the `model` DTO doc (`:196-202`): on a
   built-in harness it is the model half of the pair.

### 4.4 Resolver (`provider.rs`)

Trait, after `telemetry_model` (`:127`):

```rust
    /// A sibling of this model that resolves every turn against agent
    /// `agent_id`'s own `{provider, model}` pair (keys rework slice 3a), or
    /// `None` when this implementation cannot pin (test doubles). Only that
    /// agent's own turns use it; internal passes keep the default (Q13).
    fn pinned(&self, _agent_id: &str, _choice: &ModelChoice) -> Option<Arc<dyn HarnessModel>> {
        None
    }
```

Beside `TenantProvider` add one struct, used everywhere a pin is held:

```rust
/// One agent's pair, carried with the agent's id so a refusal can name it.
#[derive(Clone, Debug)]
pub(crate) struct AgentPin {
    pub agent_id: String,
    pub choice: ModelChoice,
}
```

`TenantProvider`: add `pin: Option<AgentPin>` after `scope` (doc: "`Some` only on
a sibling built by `pinned`"), `pin: None` in `new`. In `impl HarnessModel for TenantProvider`:

```rust
    fn pinned(&self, agent_id: &str, choice: &ModelChoice) -> Option<Arc<dyn HarnessModel>> {
        Some(Arc::new(TenantProvider {
            company: self.company.clone(), secrets: self.secrets.clone(),
            manifest: self.manifest.clone(), env_default: self.env_default.clone(),
            client: self.client.clone(), slug: RwLock::new("subscription"),
            model: RwLock::new(None), scope: self.scope.clone(),
            pin: Some(AgentPin { agent_id: agent_id.to_string(), choice: choice.clone() }),
        }))
    }
```

In `resolve`, **before** the `resolve_for_turn` call, check the pin so the
refusal names the agent (phase-3.md use case 4); then pass
`self.pin.as_ref().map(|p| p.choice.clone())` as `pin`:

```rust
        if let Some(AgentPin { agent_id, choice }) = &self.pin {
            match store::get_provider(&self.company, self.secrets.as_ref(), &choice.provider).await? {
                None => anyhow::bail!(
                    "agent `{agent_id}` is set to `{}`, which this company does not have. \
                     Choose another in Team → {agent_id} → Model.", choice.provider),
                Some(row) if !row.enabled => anyhow::bail!(
                    "agent `{agent_id}` is set to `{}`, which is switched off. Switch it on in \
                     Settings → Inference, or choose another in Team → {agent_id} → Model.", row.label),
                Some(_) => {}
            }
        }
```

Slice 2b's `resolve_for_turn` still refuses a gone pin itself ("this agent is
set to …", no id); that only shows if the row vanishes between the two reads.
If a field is not `Clone`, clone what `new` + `with_scope` need instead.

### 4.5 Roster (`build.rs:1159-1233`)

Directly above the `let model = …` line:

```rust
    // Keys rework slice 3a: this agent's own pair, when it has one. Only
    // `built_in` lanes reach `build_agent`, and validation refuses a pair on
    // an `acp` agent, so no harness-kind check is needed here.
    let pin = match (manifest_agent.provider.as_deref(), manifest_agent.model.as_deref()) {
        (Some(p), Some(m)) if !p.trim().is_empty() && !m.trim().is_empty() => {
            Some(ModelChoice { provider: p.trim().to_string(), model: m.trim().to_string() })
        }
        _ => None,
    };
    let chat_model: Arc<dyn HarnessModel> = match &pin {
        Some(choice) => deps.provider.pinned(&manifest_agent.id, choice).unwrap_or_else(|| {
            tracing::warn!(agent = %manifest_agent.id, "this provider cannot pin; the agent pair is ignored");
            deps.provider.clone()
        }),
        None => deps.provider.clone(),
    };
```

Use 2b's constructor for `ModelChoice` if it has one. The model string stays as
2d left it (the pin supplies `chosen_model`; the string is ignored). At `:1233`
use `.chat_model(chat_model.clone() as Arc<dyn tinyinference::model::ChatModel<()>>)`.
Then `git grep -n "deps.provider" src/harness/built_in/build.rs` and switch any
other use **inside `build_agent`** that stands for this agent's turn model to
`chat_model`; leave internal-pass evaluators on `deps.provider`.

### 4.6 Boot (`builder.rs:3087-3104`)

Moved to [part 2 §4.6](phase-3a-agent-pair-backend-part2.md#46-boot-builderrs3087-3104).
