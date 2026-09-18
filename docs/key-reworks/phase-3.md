# Phase 3 — each agent has its own provider + model

Part of [the keys rework](README.md). After phase 2. Code references are on
`upstream/main @ fcfb3e1bc` (2026-09-14).

| Slice | File | Dump items | One line |
|---|---|---|---|
| 3a | [phase-3a-agent-pair-backend.md](phase-3a-agent-pair-backend.md) | 13c, 19; Q5; F5, F6 | `provider` on the agent records, validation, resolver, boot |
| 3b | [phase-3b-agent-pair-editor.md](phase-3b-agent-pair-editor.md) | 13c, 19 | Provider + Model controls in Agent detail |

## What changes

- `Agent`, `OverlayAgent` and `AgentOverride` gain `provider: Option<String>`
  next to the existing `model`. On a `built_in` harness, `provider` and `model`
  together are the agent's pair; one without the other is refused. On an `acp`
  harness `model` keeps its meaning and `provider` is refused.
- The resolver order becomes: agent pair → named-harness `[harness.inference]`
  → company default → legacy → "choose a model" error.
- A pair naming a missing or switched-off provider fails that agent's turn with
  a sentence naming the agent. It never falls back (F6).
- Agent detail offers a Provider select (connected, enabled providers) and the
  normal model picker, or "Company default · <provider> · <model>".

## Where

| Area | Files |
|---|---|
| Records | `src/company/types.rs:634-684`, `src/company/agent_file.rs`, `src/ports/types.rs` (`OverlayAgent` ~:3410-3453, `AgentOverride` ~:3455-3562, upsert/merge) |
| Validation | `src/company/manifest.rs` model/harness loop, `src/server/ops/team_agent.rs` (`EditAgent`, cross-field check, persist, DTO) |
| Resolver | `src/harness/built_in/provider.rs` (`HarnessModel`, `TenantProvider`), `src/harness/built_in/build.rs:1159-1162`, `src/runtime/builder.rs` (`configured`) |
| Rebuild | `src/harness/built_in/mod.rs` (`overlay_agent_to_manifest`, fingerprint) |
| Console | `frontend/src/api/types.ts`, `frontend/src/lib/agent.ts`, `frontend/src/views/team/AgentDetailView.tsx`, `frontend/src/inference/ModelField.tsx` |

## Use cases

1. **Researcher on a larger model.** The company has an `anthropic` row (key
   `sk-not-a-real-key`) and a default `{"provider":"openrouter","model":"acme/test-model"}`.
   Admin opens Team → Researcher → Harness & model, picks Provider `Anthropic`,
   Model `test-model-large`, Save. The researcher's turns go to Anthropic with
   `test-model-large`; everyone else stays on the default.
2. **Web-search agent on a smaller model.** Same, `anthropic` / `test-model-small`
   — a larger model for research, a smaller one for search, as the operator
   described (item 19).
3. **Writer with nothing set** uses the company default.
4. **Anthropic row deleted** while the researcher points at it. The
   researcher's next turn fails with "agent `researcher` is set to `anthropic`,
   which this company does not have. Choose another in Team → researcher →
   Model." (the agent id, as `HarnessModel::pinned` receives it). Its spend
   never moves to another account.
5. **An agent on TinyHumans.** The model picker for the `tinyhumans` provider
   lists exactly what `…/agent-integrations/openrouter/models` returns; any id
   it returns may be picked. Nothing is filtered or preferred by vendor or name.
6. **A company whose only inference is one pinned agent** boots the harness
   brain, not echo.
7. **A hand-authored `company.toml`** declares `provider = "groq"`,
   `model = "test-model-b"` on an agent. The manifest accepts the
   syntax; if no `groq` row exists, that agent fails closed at its first turn.
8. **An ACP agent** (runs on the desktop's `claude` CLI) keeps its model picker;
   the Provider select is not shown.

## Handled

Items 13(c) and 19; Q5 including the named-harness precedence; F5 and F6.

## Not handled (and why)

- Agent pairs for internal passes — they carry no agent; Q13 keeps them on the
  default.
- Validating a manifest pair against console providers at boot — the manifest
  cannot see console data (F5); the turn fails closed instead.
- A new `inference/agents` key — D-set forbids a new key and the records
  already persist `harness`/`model`.

## Data carry-over

None. An absent `provider` means no pair; every existing record deserializes as
today.

```toml
# company.toml before
[[agent]]
id = "researcher"
tier = "reasoning"
# after (hand-authored pair)
[[agent]]
id = "researcher"
tier = "reasoning"
provider = "anthropic"
model = "test-model-large"
```

## Commit order

1. 3a records + conformance samples + manifest validation + tests.
2. 3a route + DTO + resolver + builder + tests.
3. 3b editor + unit/e2e tests + browser check (light, dark, 15-agent roster).

## Rollback

An older binary ignores `provider` (serde default) and refuses `model` on a
`built_in` agent at manifest load (`manifest.rs:1066-1073`). A console-set pair
lives in the overlay records, which the older binary deserializes without the
field, so it loads and uses the default. A hand-authored manifest pair would be
refused by an older binary — say so in the PR body.
