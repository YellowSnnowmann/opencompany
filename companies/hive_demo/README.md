# Hive Demo Co

> The smallest company whose desks answer as **rooms**: two desks of two
> members each, sharing the CEO, in one company hive (a TinyHiveMind
> `Coordinator`). Every message on a desk opens an episode on that desk's hive,
> members run **concurrently** in rounds of two, any member may message a
> teammate on the other desk directly (`hivemind_send_agent`), and the episode
> settles when its members call `hivemind_complete`.

It exists to be measured. `companies/openhuman_demo` is the same three agents
with one seat per desk — a one-seat desk runs no round — so this is the copy
that exercises the thing the runtime promises about coordination: two desks'
rounds overlap, and one agent's turns never do.

## Roster and desks

| Agent | Desks | Responsibility |
| --- | --- | --- |
| Chief Executive | engineering, content | Sets direction; the **shared seat**. |
| Engineer | engineering (lead) | Explains how things are built and proposes technical plans. |
| Writer | content (lead) | Turns rough notes into short, clear written drafts. |

Both desks declare `[group_chat.routing] round_width = 2` (see
`docs/spec/runtime/manifest-semantics.md`). There is no remote MCP server: the
only tools an agent needs to talk are the `hivemind_*` tools every registered
agent carries (`docs/spec/runtime/hive.md`).

## Measuring it

Against the scripted mock brain, deterministic and offline:

```bash
scripts/measure-coordination.sh --mock
```

which boots `frontend/test/e2e/mock-brain.mjs`, a host built with
`--features openhuman,mcp` serving this company, posts one task to
`engineering`, tails `/events` until every episode settles, and prints the
numbers against the thresholds (max concurrent turns ≥ 2, ≥ 1 agent→agent
contact, ≥ 2 distinct pairs, every episode settled; the exit code is the
number of failures). Against a real model:

```bash
TINYHUMANS_API_KEY=<jwt> scripts/measure-coordination.sh
```

The console shows the same run live: open `#/chat/engineering` to watch both
members answer and the episode settle; `#/company/comms` draws who messaged
whom.

`npm run e2e:hive` in `frontend/` drives the same company from a browser.

## Human in the loop

You keep **asking the desks questions and approving anything costly**; the
seats run everything else. The company's output is **decisions and short
drafts, reached by desks answering together**.
