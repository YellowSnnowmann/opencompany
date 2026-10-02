# Per-tool permissions for MCP servers

Split out of [MCP Servers](mcp.md), which holds everything else about
per-tenant tool servers: where they come from, how credentials are stored, how
agents are scoped to them, and the directory.

Each server carries a policy document that says, per remote tool, what happens
when an agent calls it. It is stored at `mcp/{name}/tool_policies` (and
`mcp_registry/{server_id}/tool_policies` for a directory install), separate from
the credential and from the declaration.

Three modes:

| Mode | Effect |
|---|---|
| `always_allow` | Runs without parking for a human. |
| `needs_approval` | Parks under the standing approval rules, as every bridge call does by default. |
| `blocked` | Refused before the call reaches the transport. No approver can wave it through. |

And three tiers a tool can be grouped under — `read_only`, `interactive`,
`write_delete` — each of which can carry a bulk default so "allow everything
read-only on this server" is one decision rather than one per tool.

## What resolves a call

Two ladders. The tier is an operator's reclassification, else a suggestion, else
`interactive`. The mode is the tool's own override, else the tier's stored bulk
default, else a hardcoded fallback.

**A suggested tier never grants `always_allow` on its own.** The suggestion
comes from a name heuristic (`get_`/`list_`/`read_`/`search_` reads;
`delete_`/`remove_`/`drop_` destroys), and a heuristic deciding who skips the
approval gate would mean that the day it gains a verb, calls that used to park
quietly stop parking. A suggestion groups a row and pre-selects a control; a
stored tier default or a per-tool override is what actually allows. The same
reasoning is why a server's own `readOnlyHint`/`destructiveHint` annotations are
not a source: they are self-reported by whoever runs the server, and a directory
install can come from anyone.

## One teammate at a time

The same document carries an `agents` map, keyed by agent id, holding one
teammate's own per-tool modes. It resolves *after* the company answer and may
only **narrow** it, along `always_allow < needs_approval < blocked`.

```json
{"tierDefaults": {"read_only": "always_allow"},
 "overrides": {"delete_page": {"mode": "blocked"}},
 "agents": {"writer": {"overrides": {"search_pages": {"mode": "blocked"}}}}}
```

Narrow-only for three reasons, in ascending order of how badly the alternative
fails. Every other per-agent layer in the crate is an intersection, so a
widening one would be the single exception to the operator's model. The
enforcement seam can only express restriction — the attached server carries a
deny list and the transport consults deny before allow — so a widening rule would
be silently ignored, which is worse than refusing to express it. And
`resolve_policy` already refuses to let a mere suggestion reach `always_allow`; a
per-agent widening would be a second route to it, invisible on the server's own
page.

It is a property of the type rather than a rule to remember:
`ApprovalMode::max_restrictive` is the only way a per-agent mode reaches a
resolved mode. A stored setting the clamp discards is **reported**, not dropped —
`PolicySource::AgentClamped` — so a console can name a setting nothing honours
instead of rendering a control whose value the host throws away.

**No per-agent tier defaults.** A tier classifies the tool, not the teammate.
Per-agent tiers would give the tier a second source as well as the mode, doubling
the audit problem for no product requirement: "the writer gets only
`write_page`" is a statement about modes.

`allowed_tools` / `disallowed_tools` stay company-wide membership, because
per-agent restriction is already fully expressible as `mode: blocked` — one field
feeds the transport's deny list instead of four sources per row.

**No migration.** A document with no `agents` key gives an empty map, so
`agents.get(agent)` is `None` and the clamp never runs: every agent id, including
one nobody has written a rule for, resolves to exactly what the company document
resolved to before, and the attachment's deny list is identical in contents and
in order. A reset prunes per-tool rows and then the emptied teammate, and the
`agents` key is skipped when the map is empty — residue would resolve identically
but hash differently, moving the fingerprint on a write that changed nothing and
rebuilding every roster.

## The legacy declaration is still live

A server's `read_only_tools` list is the baseline the stored document layers
over, field by field — not a one-shot migration input. A stored entry naming
only a mode keeps the baseline's tier, and editing one row cannot retire the
declaration's remaining rows.

An **unreadable** document is not the same as an absent one. Absent means the
declaration is the whole policy. Unreadable drops the declaration too and parks
everything, because the damaged document may have carried a refusal, and falling
back to the declaration would restore an allow the operator had taken away. The
degrade is scoped to the one server named in the warning: the loader never
surfaces the failure, because MCP resolution's caller treats an error as "this
company gets no MCP servers at all".

## Where a block is enforced

A company agent does not reach a declared server through this crate's bridge
tool. `AgentSpec::mcp` attaches the granted servers to the agent, and the names
in `OPENHUMAN_NATIVE_TOOLS` — `mcp_call_tool` among them — always resolve to
OpenHuman's own implementation over those attachments. The bridge tool's guard
runs only where that tool is the one dispatched.

So a blocked tool is denied where the server is attached: its name goes on the
attachment's deny list, which the transport filters on before anything is
listed or dialled, and where deny outranks allow. The declaration's own
`disallowed_tools` is kept — the policy adds to that list rather than replacing
it. Only `blocked` is denied this way; a tool that merely parks stays reachable,
because parking is what the approval gate is for.

The deny list is resolved **for the agent being built**, so one teammate's
refusal reaches only that teammate's attachment. On a document with no per-agent
rules it is the company list, in the same order, which is why the upgrade changes
no attachment.

The gate's own read set is narrowed the same way, and additionally by
`grants_cover_server` — which the company-wide answer never applied at all, so a
teammate's gate used to treat a pair on a server it cannot dial as a declared
read. Both narrowings only remove pairs, and reach is affirmative-membership-only,
so a smaller set can only park more.

**`needs_approval` still parks nothing on this build.** `ApprovalPolicy::check`
returns `Allow` at the `policy_hitl_enabled` bypass, and every roster build
disables policy HITL, so only `blocked` and `always_allow` differ observably: a
per-agent `needs_approval` behaves as allow. The honest sentence is "block works,
ask does not yet" — the product requirement is fully expressible with `blocked`,
and the tests assert on the resolved mode and the deny list, never on parking.

That is resolved when the agent is built, so a block reaches the native path on
the next roster build — and the stored document is a term of the fingerprint
`HarnessPool::ensure` compares, so writing one *is* what triggers that build.
Without the term the deny list would be resolved once and a tool set to `blocked`
would stay callable until the host restarted; the write and reset responses carry
`NEXT_TURN_NOTE` because the invalidation is what makes the promise true. The
fold is canonical (tiers read totally, overrides through a `BTreeMap`) so an
unchanged document does not rebuild the roster on every turn. The per-agent map
needs no such treatment — it is already a `BTreeMap` of `BTreeMap`s, which is most
of the reason the layer lives inside this document rather than in one of its own.

A **directory install** is the other shape: it is addressed by a
`server_id` argument at call time rather than by the grant its tool was wired
under, so there is no build-time snapshot to attach a policy to. Its scoping
decorator reads the install's document when the call arrives, resolved for the
teammate it was wired for — the grant answers whether this agent may name the
install at all, the policy whether that tool may run, and both refuse before
anything is dialled. The refusal text names no teammate: whose rule refused the
call is not an agent's business. A deployment with no secret
store cannot read a policy and does not invent one; the grant stays the whole
gate.

An install carries no `read_only_tools`: that is a manifest affordance of a
declared server, so for the registry the stored document is the whole policy and
the persisted inventory is the only thing that can say which tier a tool is in.

The guard in the bridge tool stays as the same refusal for any path that does
dispatch it, and both paths word it with one function so an agent cannot tell
from the message which one refused it.

## What the agent is told

Per-agent policy creates a state company-wide policy barely could: a teammate that
**reaches** a server and can call nothing on it. "The writer gets only
`write_page`" on a forty-tool server means thirty-nine refusals, and one careless
bulk action makes it forty. In that state `mcp_list_tools` returns empty and the
agent cannot tell "server down" from "you may call nothing".

So the server-family brief names such a server **with the refusal said out loud**.
Dropping the line instead is its own lie — an operator asking "do you have
notion?" would hear no when the answer is yes, and nothing on it is callable. A
server no probe has reached is never called refused: "nothing callable" and
"nothing known" are different facts, and only one of them is evidence. The clause
rides the existing line, so the brief still names only servers and the key that
addresses each, never an individual remote tool.

The console twin of that sentence is the per-teammate read's `fullyRefused` flag,
so the state is visible where it is created as well as where it lands.

## The routes

`GET`/`PUT`/`DELETE {scope}/mcp/servers/{name}/tools/policy` and its registry
sibling `…/mcp/registry/{server_id}/tools/policy` take an optional `?agent=`.
A query parameter rather than a third level of nesting in the body: the
two-level partial-merge contract stays intact, and refusing a per-agent tier
becomes one check on a shape that cannot express it twice. A blank value is the
company document, not a teammate named `""`.

In an agent scope the merge is one level shallower — a teammate has modes, not
tiers — so a `tierDefaults` body and a `tools` entry naming a `tier` are both
`400`, refused rather than dropped. An entry naming no mode resets that
teammate's row; a `DELETE` with `?agent=` clears only that teammate, which means
it has to read first and an unreadable document is a `409` there. The
company-scoped `DELETE` stays the repair for one that will not parse.

Reads are member-open in both scopes and writes stay admin-only: answering "what
can this teammate call" changes nothing, while setting it settles something on
behalf of the company.

Every row carries a **host-resolved** `source` — `server_inherited`,
`server_pinned`, `agent_pinned` or `agent_clamped` — plus `agentMode` (the
teammate's stored mode, present even when the clamp discarded it) and
`differingAgents` (the teammates whose mode differs from the company's). The
console never re-derives them: one of the four values names a *discarded*
setting, which no client can infer from the mode alone.

`GET {scope}/team/{agent_id}/mcp/permissions` is the read for the other
direction — one teammate, every configured declared server, reached or not, with
the grant that would reach it. One route rather than N so the host resolves
`source` once, and because reached-or-not can only be answered cheaply for every
server at once. One damaged document degrades its own block, never the page.
Writes still go through the per-server route, so there stays one write path per
document.

## Where the tiers come from

A tier default can only reach tools something has named. Discovery persists an
inventory — tool name to suggested tier — beside the policy, from the same
listing the health probe already performs, so a bulk "block everything
write/delete on this server" reaches the tools that server actually has. A
failed probe leaves the previous inventory standing rather than emptying it, and
neither write can fail the probe.

An inventory on its own grants and blocks nothing. It is a proposal the console
renders and the resolver reads as a suggestion; only a stored decision changes
what happens to a call.
