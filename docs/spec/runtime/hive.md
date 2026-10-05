# The company hive

Every conversation a company's agents have — an operator line on a desk or on
`#general`, an operator DM to a teammate, one teammate messaging another — runs
through **one TinyHiveMind `Coordinator` per company** (OC-2). The Coordinator
owns who runs and when: each agent has one durable inbox and one serialized
turn stream across every hive it sits in, a desk message opens an **episode**
the conductor runs until its members complete it, and a direct message is
delivered to its recipient's inbox and answered on a later turn. OpenCompany
hosts it: it owns the runtime, the agents, the storage, the journal and the
turn hooks.

The mechanics come from `vendor/tinyhivemind`: `tinyhivemind-hives`
(`Coordinator`, `Storage`, `RetentionPolicy`) and `tinyhivemind-openhuman`
(`OpenHumanHost`, `TurnHooks`, `SendAuthorizer`, the permanent `hivemind_*`
tools). Its own migration note is `vendor/tinyhivemind/docs/opencompany-migration.md`.

Isolated turns — a dispatched card, a workflow node, a copilot thread, a
background task — stay on the harness pool (`HarnessPool`), each in a fresh
session, and never enter the hive.

## The shape

```text
one process ── one openhuman_embed::Runtime
                 └─ one openhuman_embed::Agent per (company, agent)
company ────── one CompanyHive  (hive::runtime::for_company, keyed by company + HiveStore)
                 ├─ Coordinator  over PortStorage(HiveStore)       ← state row + transcript, CAS
                 ├─ OpenHumanHost                                  ← every agent registered, one session each
                 │    ├─ ReachPolicy  (SendAuthorizer)
                 │    └─ HiveHooks    (TurnHooks)
                 ├─ hives: one per desk + `General` (#general)
                 └─ Projector  transcript ─▶ company journal
```

- **One Coordinator per company, not per pool or process.** A company with
  several `built_in` harnesses has one pool per harness; every pool registers
  its agents into the same `CompanyHive`. Coordinator agent ids are runtime ids
  (`session_key::runtime_agent_id` → `{company}--{agent}`), so one process can
  host many tenants without `hivemind_list_agents` leaking another's roster.
- **One writer per store.** `Coordinator::new` claims the store's
  `writer_epoch`; an older Coordinator on the same store gets `Fenced` on every
  write and from `run()`. The run loop treats `Fenced` as "another process owns
  this company" and stops for good, logging it — no retry loop. Any other
  run-loop error restarts it after a second.
- **Storage is the `HiveStore` port** (`docs/spec/runtime/ports.md`),
  implemented by the filesystem, SQLite and MongoDB backends, so a hosted
  tenant's hive lives in its own database (or its namespace of the shared one)
  exactly as its journal does. `hive::storage::PortStorage` adapts it; a
  revision mismatch is `RevisionConflict { expected, actual }`, which the
  Coordinator reloads and retries.
- **One session per agent.** Registration binds each agent to its existing
  OpenHuman conversation (`register_agent_in_session`), so an agent continues
  one session across every hive and DM. A roster rebuild replaces the handle
  (`OpenHumanHost::replace_agent`) after the pool drops its own clones.

## Desks, `#general` and DMs

`CompanyHive::sync` runs on every roster rebuild and record refresh: one hive
per desk (`delegation_tools::desk_ids`, overlay desks included) holding the
desk's effective members that are registered, plus `#general`. A desk that
disappears keeps its hive (the transcript is permanent) with every member
removed.

TinyHiveMind core reserves `general` and `main` (any case) as desk identities,
except for its own default desk whose id and name are both `General`. So:

| Console chat | Hive id | Note |
| --- | --- | --- |
| `general` | `General` | core's default desk |
| a desk called `main` | `desk-main` | prefixed, mapped back by `chat_for_hive` |
| any other desk id | the same id | |

A desk whose display name is `General` or `Main` runs under `"<name> (<id>)"`.
A hive created under a reserved identity is accepted by the Coordinator but
fails every episode it opens and stalls the run loop, so every boundary
between a console chat and a hive goes through `hive::hive_id_for_chat` /
`chat_for_hive`.

An operator line in a teammate's DM (`dm:<agent>` or a bare roster id) is a
direct `send_as_host` to that agent: no episode, one turn, and its reply comes
back to the host as a returned reply that the projector journals on the DM.

## Where an operator line goes

`HarnessBrain::send_to_hive` (`harness/built_in/brain/hive_chat.rs`) is the
operator path. It hands the line to the Coordinator with `send_as_host` and
returns; the turns that answer it run on the hive's own task, and their output
reaches the console through the journal. The message id is `op:{seq}` — the
journal row the line was said as — so a redelivered cycle resends
idempotently (`MessageConflict` is "already accepted").

| Outcome | What happens |
| --- | --- |
| accepted | `HiveAccepted { message_id, sequence, chat_id, source, starters, route }` is journaled |
| `InvalidThread` | the line answers a thread the hive cannot see; resent at the top level |
| `InboxFull { agent_id, limit }` | backpressure: a system line in the chat says the teammate has `limit` messages waiting and to resend later |
| no hive (no `HiveStore` wired) | the brain answers with a pooled turn instead |

**Starters** (`hive::route::choose`) are who the episode starts with:

1. **Mention** — the named members of this hive (a teammate, a desk, `@everyone`).
2. **Jev** — an unaddressed desk line is put to System One with the desk's
   members as candidates (`route.rs`, `jev.rs`); skipped without a TinyHumans
   key and always on `#general`.
3. **Default** — the desk lead; on `#general`, the orchestrator.

`route` on `HiveAccepted` says which rung decided (`mention`, `jev`,
`default`); `opencompany measure` counts them.

## An episode

Each hive has at most one active episode; later lines queue. The conductor
(TinyHiveMind's `CompletionDriver` / `Conductor`) assigns the starters, runs
rounds of at most `round_width` concurrent turns, nudges, and settles the
episode when every assigned member has called `hivemind_complete`, or fails it
at the round cap or turn wall. A turn's plain reply is recorded as a post in
the episode, not as completion; completion is always an explicit
`hivemind_complete { episode_id, body }`.

The Coordinator's options are folded from every desk's `[group_chat.routing]`
block (`hive::routing::coordinator_options`): the widest `round_width`, the
largest `max_rounds`, a turn wall of `round_width × max_rounds`, and the
adapter's per-turn timeout is the longest `turn_timeout_secs`
(`routing::turn_timeout`). Defaults: `round_width=5`, `max_rounds=12`,
`turn_timeout_secs=600`.

### Retention

The state row is bounded (`RetentionPolicy`, `hive/routing.rs`); the
transcript is never pruned:

| Field | Value | What it bounds |
| --- | --- | --- |
| `settled_episodes` | 256 | settled episodes kept on the row (each is journaled as `HiveEpisodeSettled` as it settles) |
| `deliveries` | 1024 | acknowledged direct deliveries (bookkeeping; the message stays in the transcript) |
| `interrupted` | 256 | interruption records (journaled as `HiveTurnInterrupted` as they appear) |
| `pending_per_agent` | 64 | undelivered direct messages one agent may have waiting; a send past it is `InboxFull` |

## Talking: the `hivemind_*` tools

Every registered agent carries the adapter's permanent family, admitted by the
agent's tool scope (`hive::HIVEMIND_TOOLS`):

| Tool | Does |
| --- | --- |
| `hivemind_list_hives` / `hivemind_list_agents` | where the agent sits; who it can message |
| `hivemind_read` | a hive (optionally a thread) or its direct transcript with one peer, after an exclusive cursor; starts no turn |
| `hivemind_send_hive` | enqueue a line on a hive it belongs to (opens or queues an episode) |
| `hivemind_send_agent` | enqueue a direct message to a teammate; their reply arrives on a later turn |
| `hivemind_post` / `hivemind_ask` / `hivemind_broadcast` / `hivemind_complete` | act in its active episode, by explicit `episode_id` |

Sends return a receipt immediately and never wait for the peer. `agent_id`
arguments are Coordinator ids; the team brief lists each teammate's
(`` `acme--writer` as the `agent_id` of a `hivemind_*` tool``), and a send that
passes a bare manifest id is refused with the id to use.

**Reach** (`hive::policy::ReachPolicy`, the host's `SendAuthorizer`) is the rule
the retired `delegate_to_teammate` enforced: a teammate may message the people
it shares a desk with, plus the members of every desk its manifest
`delegates_to` names — anybody when that list is empty or `"*"`, never itself.
A refusal reaches the model as the tool's error text and its turn continues.

## A coordinator turn

The adapter runs the agent's turn on its own handle and calls
`HiveHooks` (`harness/built_in/hive_hooks.rs`) around it:

1. **`prepare`** finds the `CompanyAgent` behind the coordinator id and roots
   the turn in the agent's workspace.
2. **`progress`** streams the turn live to the desk or DM it answers.
3. **`wrap_turn`** (`hive_hooks/settle.rs`) admits the turn (total ceiling,
   monthly budget, daily cap — `HarnessPool::admit`), journals `TurnStarted`
   with `hive: { hiveId, episodeId }`, claims the per-turn queues (approval
   scope, publishes, outputs, `spawn_task` cards, the episode's card budget),
   runs the model inside the agent's `TurnEnvelope` (turn lock, tool executor,
   stop hooks), then files what the turn left — published files on a card,
   cards it opened, approvals it asked for — and journals `TurnSettled` /
   `TurnFailed`.
4. **`after_turn`** answers `Parked` when the turn put an approval in front of
   the operator; the Coordinator then holds the agent.

**Cards.** One episode may open at most three cards with no repeated title
(`EpisodeCards`); a fourth `spawn_task` is refused in-turn. A publish with no
card in scope mints one; a line the chat handler already carded keeps that one.

**Approvals.** A parked turn's approvals are keyed `hive-turn:{agent}:{episode}`
(`runtime::hive_resume`) and carry `origin: Hive { agent_id, episode_id }` on
`ApprovalParked`, surfaced as `hive` on the approval summary. When the last
decision the agent waits on lands, every decision is rendered as one release
note and `Brain::release_hive_agent` calls `Coordinator::release_with`; the
agent reads it as `Host resumption note: …` at the top of its next turn and
redeems an approved call itself under the single-use grant. If no hive takes
the release, the operator is told the teammate is no longer waiting.

## What lands in the journal

The `Projector` (`hive/projector.rs`) tails `Coordinator::read_transcript` on
every committed revision and appends:

| Transcript row | Journal row |
| --- | --- |
| a member's line on a hive, visible to all | `AgentReply` on the desk chat, threaded under what it answers, `hive: { sequence, episodeId?, thread? }` |
| a reply to an operator DM | `AgentReply` on the DM chat |
| a direct line between two agents | `HiveMessage { destination: Agent(to) }` |
| a private desk line (`only_for`) | `HiveMessage { destination: Hive(desk), only_for }` |
| an episode settled / failed | `HiveEpisodeSettled { episode_id, hive_id, opened_at, thread, failure }` |
| a turn interrupted by a restart | `HiveTurnInterrupted` |

Each also streams on `/events` (`docs/spec/runtime/events.md`). A teammate's
direct lines are read back with `GET {scope}/agents/{agent_id}/messages?after=`.

## Measuring

`opencompany measure --company <id>` (`hive/measure.rs`) and
`scripts/measure-coordination.mjs` (over `/events`) fold the same rows into
the same report: the concurrency peak and same-agent overlaps from the turn
brackets, episodes settled / failed, direct and private contacts and distinct
pairs, and the starter-route histogram. Thresholds: at least two turns at
once, no agent overlapping itself, one agent→agent contact, two distinct
pairs, every episode settled.

## Testing

- `src/hive/*_tests.rs`: routing, reach, storage, projector, mapping, measure.
- `harness/built_in/hive_hooks_tests.rs`, `hive_hooks/settle_tests.rs`.
- `tests/hive_e2e.rs`: a real company over HTTP with a scripted model keyed on
  the coordinator turn's prompt (`tests/support/room.rs`) — desk, `#general`,
  desk of one, a direct message answered, a shared agent never running twice,
  the approval release, and the measurement.
- `tests/one_card_per_message.rs`: one operator line opens one card.
- CI: the `hive-coordinator` lane runs `hive::` under
  `openhuman,mcp,media`; the HiveStore conformance runs in the SQLite and
  MongoDB lanes (`store::sqlite`, `store::mongodb`).
