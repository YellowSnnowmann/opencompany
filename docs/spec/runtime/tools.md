# Tool grants and scoping

*How a company decides what each of its agents can reach.*

Terms: [glossary](../glossary.md). The approval gate — a separate axis, covering
whether a granted tool's call needs a human first — is
[company-brain/grants.md](../company-brain/grants.md).

---

## The three levels

A teammate's tool belt is resolved by intersecting three declarations, in order:

```
[tools].allow  ∩  desk.tools  ∩  agent.tools
```

Every level is **narrow-only**. There is no path through the resolution that
yields a grant `[tools].allow` does not already cover, which is what makes the
lower two levels safe to hand to an operator: the worst a desk or an agent
declaration can do is remove capability.

Each level is **optional**, and an omitted level is a pass-through rather than a
denial. This matters more than it looks:

> An empty grant list means **"inherit"**, not **"nothing"**.

An agent with no `tools` line holds its desk's ceiling; a desk with no `tools`
line imposes the company's. Any surface that renders an empty list as "no tools"
has inverted the meaning — see `AgentToolsDto` in
`src/server/ops/team_agent.rs`, whose field docs carry the same warning.

Resolution lives in one function,
[`agent_scoped_grants`](../../../src/runtime/builder.rs), and every reader goes
through it: the harness that wires the belt, the console's agent card, and the
roster list. A second implementation anywhere is a bug — the console showing a
tool the gate refuses is the exact failure this single-source rule prevents.

### Levels in detail

**Company — `[tools].allow`.** The ceiling. Defaults to
`["*", "media", "composio"]`.

**Desk — `[[group_chat]].tools`.** A department's ceiling. A company organises
its teammates into desks — a finance desk, a creative desk — and this is where
"nobody on this desk reaches the web" is stated once instead of repeated on
every member and hoped for on the next member added.

**Agent — `agents/<id>.toml` `tools`.** The individual's request.

### Agents on several desks take the union

A teammate on more than one desk takes the **union** of those desks' ceilings
before the intersection with the company grant.

Union rather than intersection, because desk membership is additive: joining the
growth desk is how a marketer gains the ad tools. Intersecting would make each
extra desk silently *revoke* capability, so adding someone to a desk could break
work they were already doing.

The consequence, which MUST be understood before relying on a desk ceiling: **a
desk with no ceiling narrows nothing**, so a teammate on both a restricted desk
and an unrestricted one ends up unrestricted. A company that means to restrict a
teammate states the ceiling on every desk that teammate sits on, or states it on
the teammate. This is the safe direction — the widest it can resolve to is the
company grant itself — but it is not the intuitive one.

## The wildcard does not mean everything

`*` covers `files`, `docs`, `shell`, `code`, `web` and `subagent`. It
deliberately does **not** confer four namespaces, each of which must be named:

| Namespace | Why it must be named |
| --- | --- |
| `media` | Spends real money per generated image or video. |
| `composio` | Reaches the tenant's connected third-party accounts and moves real side effects — sends email, opens PRs. |
| `search` | The queries leave the building, and a call is billed — to the managed platform, or to the company's own provider account. See [search.md](search.md). |
| `repo` | Materializes a third party's source inside a sandbox where the agent may also hold `shell`. |

`repo.write` is tighter still: only the exact string confers it. A bare `repo`
grant carries read access and nothing else, because read and write are separate
decisions — a company that adopted the read tier must not silently acquire
agents that push.

Each rule has its own predicate beside the manifest types
(`grants_media_explicit` and siblings in `src/company/types.rs`). Nothing may
re-derive these answers from the generic glob matcher: it reports `*` as
covering everything, which is right for the ordinary families and wrong for
these four.

## The catalog

`src/company/tool_catalog.rs` enumerates everything a company can grant —
built-in families, `[[mcp_server]]` entries, and `[tools.composio]` toolkits —
in one vocabulary, served at `GET {scope}/tools/catalog`.

It is a **projection, never a source of truth**. Every entry carries the exact
grant token an operator would write, and resolves it through the same matcher
the roster build uses. An entry advertising a grant the gate does not honour is
a bug in the catalog, not a new kind of permission; a test asserts the
round-trip.

Two flags on each entry exist because the naive rendering is wrong:

- `granted` — whether `[tools].allow` currently confers it, resolved through the
  per-namespace predicates above rather than the glob matcher.
- `coveredByWildcard` — whether `*` would confer it, so a console rendering `*`
  as "everything" can say which four families it does not in fact cover.

A disabled `[[mcp_server]]` is listed with `granted: false` rather than omitted:
an operator needs to see that the server exists and is switched off, which an
absent row cannot say.

## Runtime overrides

An operator may narrow a desk from the console. The override is stored on the
company record (`overlay_desk_tools`) and read through
`CompanyRecord::effective_desk_tools`, never directly — the same
`overlay_* → effective_*` discipline the spend cap and approval tier follow, so
the write path and both read surfaces cannot drift.

Two properties are load-bearing:

**Version control wins when it speaks.** On a rebuild, a desk's override is
dropped if that desk's seed `tools` changed
(`carry_desk_tool_overrides`, `src/runtime/builder.rs`). A console override
outliving a seed that narrowed the desk would be a runtime widening surviving
the operator revoking it in version control — the failure the `[tools]` /
`[policy]` seed-wins rule exists to prevent, and a per-desk grant is squarely
within it. The check is **per desk**, not whole-block: editing the finance desk
says nothing about the creative desk.

**A change reaches the next turn, not the next restart.** A tool belt is wired
once per roster, not once per call, so desk scoping participates in the roster
staleness fingerprint (`desk_scope_fingerprint`, `src/harness/mod.rs`). The
fingerprint covers which desks exist, who sits on them, and each ceiling —
because seating a teammate on a restricted desk narrows its belt just as surely
as editing that desk's ceiling does.

## What this does not do

Grants decide which tools are **wired**. They do not decide whether a wired
tool's call proceeds — that is the approval gate
([company-brain/grants.md](../company-brain/grants.md)) — nor how much a
namespace may spend, which is the capability plan (`[plan]`), nor what an agent
can reach once it holds `shell`. On that last point see
[security/agent-isolation.md](../security/agent-isolation.md), which is blunt
about what is not enforced.
