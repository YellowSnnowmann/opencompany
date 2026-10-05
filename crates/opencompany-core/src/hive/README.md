# `src/hive/`

The company hive (OC-2): one TinyHiveMind `Coordinator` per company, its
agents registered on a `tinyhivemind-openhuman` `OpenHumanHost` over the
process-wide `openhuman_embed::Runtime`, one hive per desk plus `#general`.
The spec is `docs/spec/runtime/hive.md`; this folder is its implementation,
one seam per file. The turn hooks a coordinator turn runs under live beside
the harness (`harness/built_in/hive_hooks.rs`), because they are a harness
turn.

| File | What lives there |
| --- | --- |
| `mod.rs` / `hive_tests.rs` | Index; `HIVEMIND_TOOLS`, the permanent tool family the adapter attaches; `GENERAL_HIVE_ID` and `hive_id_for_chat` / `chat_for_hive` / `hive_name`, the one-to-one mapping around the desk identities TinyHiveMind core reserves (`general`, `main`). |
| `runtime.rs` / `runtime_tests.rs` | `CompanyHive`: the Coordinator over `PortStorage`, the `OpenHumanHost` with the reach policy and turn hooks, the projector task, and the run loop (a `Fenced` writer stops for good; any other error restarts after a pause). `for_company` keys live hives by company and `HiveStore` identity; `register` / `replace` / `release_handle` move agent handles; `sync` makes one hive per desk plus `General` with the desk's registered members; `release` hands a parked agent its approval note. Gated on `openhuman`. |
| `storage.rs` / `storage_tests.rs` | `PortStorage`: `tinyhivemind_hives::Storage` over the `HiveStore` port, revision strings to `u64`, a CAS miss to `RevisionConflict`, a store failure to `Error::Storage`. Default build. |
| `projector.rs` / `projector_tests.rs` | `Projector`: tails `Coordinator::read_transcript` and journals each row as `AgentReply` (a public hive line, threaded), `HiveMessage` (a direct or private line), `HiveEpisodeSettled` and `HiveTurnInterrupted`; `HiveRoster` maps Coordinator ids to manifest ids; `TurnMetaBoard` carries a turn's cost/steps onto its reply. Default build. |
| `route.rs` / `route_tests.rs` | Who starts an operator line on a hive: `choose` walks mention → Jev → default (desk lead; the orchestrator on `#general`) and names the route; `host_router` resolves the Jev transport. Default build, Jev half gated on `openhuman`. |
| `policy.rs` / `policy_tests.rs` | `ReachPolicy`, the `SendAuthorizer` behind `hivemind_send_agent`: desk peers plus `delegates_to` desks, anybody when unrestricted, never oneself; refusals name teammates by the Coordinator id the tool takes. Default build, authorizer impl gated on `openhuman`. |
| `routing.rs` / `routing_tests.rs` | The `[group_chat.routing]` block (`RoutingConfig`), its resolution (`EffectiveRouting`: `round_width=5`, `choice_option_limit=8`, `max_rounds=12`, `turn_timeout_secs=600`), overlay precedence, the wire DTOs, and `coordinator_options` / `turn_timeout` folding every desk into the one Coordinator's options with bounded retention (`RETAINED_SETTLED_EPISODES=256`, `RETAINED_DELIVERIES=1024`, `RETAINED_INTERRUPTIONS=256`, `PENDING_PER_AGENT=64`). Default build. |
| `measure.rs` / `measure_tests.rs` | The fold behind `opencompany measure`: turn brackets to the concurrency peak and same-agent overlaps, episodes from `hive.episodeId` and `HiveEpisodeSettled`, contacts from `HiveMessage`, starter routes from `HiveAccepted`; `Thresholds` mirror `scripts/lib/coordination-metrics.mjs`. Default build. |
| `jev.rs` / `jev_tests.rs` | `TinyHumansSystemOne`, the `SystemOneTransport` over the TinyHumans System One proxy (`OPENCOMPANY_JEV_URL`), and `jev_router`, which answers `None` without a TinyHumans key so routing falls back to mention and lead. Gated on `openhuman`. |
| `mcp_server.rs`, `mcp_server/handler.rs` / `mcp_server_tests.rs` | The `opencompany` MCP server (`McpHost`): JSON-RPC over Streamable HTTP on a loopback listener, one bearer per agent, serving the agent's custom OpenCompany tools under its in-flight turn. Gated on `openhuman`. |
| `tools.rs` / `tools_tests.rs` | `InFlightRegistry` — one in-flight turn per agent, keyed by runtime agent id, with its `HiveScope` (hive, episode, thread) — `InFlightContext` (the `ToolRunContext` a served tool runs under) and `McpToolAdapter`. Gated on `openhuman`. |
