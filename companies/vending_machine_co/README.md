# Northgate Vending

> **Migration note.** The trace-grammar hive this bundle was written against
> (`[group_chat.hive]`: quorum, turn budget, blind round, per-seat moves,
> asides) has been replaced by the company hive — one TinyHiveMind
> `Coordinator` per company, desks as hives, agents talking with the
> `hivemind_*` tools — configured by `[group_chat.routing]`
> ([`docs/spec/runtime/hive.md`](../../docs/spec/runtime/hive.md)). A
> `[group_chat.hive]` or `[group_chat.routing.referral]` block is refused at
> load with a migration hint; cross-desk questions are direct messages now, and
> the grammar paragraphs describe the design the bundle was tuned under, not
> what runs.

A vending-machine operator: eight machines at five host sites, a finite
warehouse behind them, and host contracts that renew whether or not anybody
prepared for it. Three hive-mind desks run it.

Every other hive bundle here answers a **question** — `hive_math_lab` solves a
stated Project Euler problem and stops. This one runs an **operation**. Work
arrives on its own, through `[[schedule]]` cadences and the
`POST /hooks/{company}/{channel}` webhook path, and nothing in the world
restocks itself.

## What this bundle is for: agent-to-agent communication

The decisions worth watching here are the ones no single desk can make
correctly:

- *Which machines does the van visit today?* is an **ops** question whose right
  answer depends on which host site is about to renew badly — which is
  **commercial's** knowledge.
- *Do we raise the price of the energy line at the gym?* is a **commercial**
  question whose right answer depends on whether that machine's chiller is
  reliable — which is **ops'** knowledge.

So a member of any desk messages a member of a peer desk directly
(`hivemind_send_agent`): the peer takes a real turn on it, and the answer comes
back to the asker on a later turn.

One rule governs that, and it is what makes this sound rather than merely
chatty: **what crosses a desk boundary carries information, never support.** An
answer from another desk adds no supporter and moves no option toward a
decision — the asking desk still has to convince itself. A desk that could
import a quorum from elsewhere would be a desk that never had to be convinced.
See [`hive.md`](../../docs/spec/runtime/hive.md).

## The three desks

| Desk | Members | Quorum / budget | Owns |
| --- | --- | --- | --- |
| `ops` | `route_planner`, `fleet_tech`, `stock_controller`, `field_realist` | 2 of 4, 12 turns | The van's finite day: route, shelves, faults |
| `commercial` | `account_manager`, `pricing_analyst`, `contract_counsel` | 2 of 3, 9 turns | Margin, prices, host sites, contracts |
| `intel` | `market_scout`, `demand_analyst` | 2 of 2, 8 turns | What the market did, and whether it changes a plan |

Each desk restricts its seats' moves (`[group_chat.hive.moves]`), and the
restrictions carry the design:

- On `ops` only the **route planner** may `!propose`. Three seats proposing
  three near-identical routes is a room that votes rather than deliberates —
  the live failure the trace-grammar hive spec recorded. The fleet
  technician and stock controller ground or `!refute` the plan from the two
  constraints that actually bind, and the **field realist** may never
  `!support` at all: a seat that both objects and supports drifts into being a
  second planner, and the room loses the one member whose incentive is purely
  to find the flaw.
- On `commercial` **two** seats may propose, because a commercial question has
  two genuinely different framings — what we charge, and what we sign — and
  forcing both through one seat produces a plan that is only ever one of them.
  **Counsel proposes nothing and may `!refute` anything**: its whole job is to
  be the seat that says no with a citation.
- `intel` is two members, the smallest room that is still a room, at a quorum
  of two — unanimity. A room of two that could carry on one supporter is a
  single responder with extra steps.

`require_evidential` is on for **`ops` and `commercial`**, so a `!support` with
no `^citation` adds nothing to quorum. In this company that bites hardest on the
stock controller: "we have enough stock" is a claim about a number
`warehouse_status` will actually print.

`intel` deliberately does **not** have it, and that is the one setting here
chosen from measurement rather than argument. Stacking `require_evidential` on a
two-member desk whose quorum is already unanimity means a decision needs both
seats to file a citation that chains to a stated fact. The deliberation
benchmark (`vendor/tinyhivemind/crates/tinyhivemind-hive/examples/bench`) puts
numbers on it — at `--agents 2` it takes the decided rate from **91.6% to
36.2%** and correctness from 63.6% to 30.7% over 2000 seeded rooms — and live
runs agreed: the signals desk spent its budget without deciding in nearly every
episode it opened. Its budget went from 6 turns to 8 for the same reason. The
larger desks keep the rule, because their seats read hard figures off the
`vending` MCP on almost every turn, so a citation that reaches a fact is cheap
there in a way it is not on a desk whose job is to weigh a signal nobody has
measured yet.

## Tool servers

The company's entire work environment is one MCP server, `vending`, declared in
[`mcp.json`](mcp.json). It exposes the fleet, the warehouse and lead times,
margin, host clients, incidents and the market feed — plus the writes that
change any of them: restock, service, order, price, renegotiate. Every read an
agent makes and every change it causes is a tool call, which is what makes a
run auditable and replayable.

`vending` **ships disabled**, pointing at a placeholder hosted URL and naming an
`authSecret`. That is deliberate: a bundle-declared server must be `https` and
must not ship enabled while it needs a credential nobody has provisioned. For a
local run you do not enable it — you run the simulator on loopback and register
it at **runtime**, which is the only layer where an `http://` endpoint is
accepted. `scripts/vending-sim.py` does that for you.

The grant that reaches it is `mcp:vending`, named explicitly in `[tools].allow`.
A wildcard cannot reach an MCP server by design (`grants_cover_server`), so a
company that wildcarded its belt does not silently acquire every server an
operator later installs.

## Running it

```bash
# 1. Memory. Company memory is OpenHuman memory v2
#    (docs/spec/runtime/memory-engine.md): the `OPENCOMPANY_MEMORY*` variables
#    and the host-side CortexDB driver are gone, so there is nothing to export.
#    Sign in with a TinyHumans credential (or configure OpenHuman's
#    `engine = "cortexdb"` with a stored key) and each seat gets a pack and logs
#    its turns. With no engine, memory is off and the run still works.
#    `scripts/cortexdb-up.sh` still starts a local CortexDB if you want one for
#    the `cortexdb` engine; its printed exports are legacy.
#
# 2. The company.
OPENCOMPANY_INFERENCE_URL=http://127.0.0.1:6969/v1 \
OPENCOMPANY_INFERENCE_KEY=$LADDER_API_KEY \
OPENCOMPANY_AUTH_MODE=none \
  cargo run --features openhuman,mcp --bin opencompany -- \
    serve --company companies/vending_machine_co

# 3. The world, the MCP server, and the trigger loop — one command.
python3 scripts/vending-sim.py --days 14 --out /tmp/vending-run.json
```

Memory matters more here than in a one-shot bundle. A desk carries what it
learned between episodes (`hive/<desk>/…`), and this company runs for simulated
weeks — so "we already decided not to visit Vulcan on Tuesdays, and why" is
knowledge day 9 needs and day 2 produced. Without a durable store behind it
every morning starts from nothing, and the desks re-litigate the same route.

`vending-sim.py` starts the simulator, registers it as a runtime MCP server,
then advances the clock a day at a time. For each desk it posts the day's
triggers and waits for the room to close, pumping the approvals queue
meanwhile — the chat POST holds open for the whole episode and a parked
`place_order` would otherwise deadlock it. It prints each episode's turns,
the direct messages that crossed desks, and the close. Exit status is the number of days on which no desk
decided anything, so zero means the company was awake throughout.

## The ledgers

The operational truth lives in the MCP server; duplicating it here would produce
two records that disagree by the end of the week. These four hold what the
server cannot:

- **`episodes`** — the *reasoning*. A transcript scrolls out of a
  thirty-message window and nobody re-reads one to learn why the van skipped
  Vulcan for a fortnight. The `not_doing` field is the most useful and the most
  likely to be left empty. (Named `episodes` and not `decisions` because
  `decisions` is a built-in ledger the runtime ships and a declared slug may not
  shadow one — and the two are different records anyway: the built-in holds the
  company's standing calls, this holds what one deliberation concluded.)
- **`clients`** — what each host was *promised*, and by whom. A promise nobody
  wrote down is one this company will break by accident.
- **`incidents`** — the *pattern*. One jam is an event; VM-301 jamming four
  times in a month is a machine that needs replacing, and no list of open
  incidents will ever say so.
- **`signals`** — observation kept apart from inference, because a correct
  observation with a wrong inference and a wrong observation with a lucky one
  look identical afterwards.

## What the operator decides

`[policy]` is `supervised`, and the desks act on the fleet themselves —
restocking and pricing are the job. What stays with a human is money and
commitments: `place_order` and `renegotiate_contract` are on `always_approve`,
so a headless run has to pump the approvals queue (the driver does).
