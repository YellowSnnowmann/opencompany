# `src/hive/`

Hive desks: one `OpenHumanHive` + `CompletionDriver` per `[[group_chat]]`
over the process-wide `openhuman_embed::Runtime`. The spec is
`docs/spec/runtime/hive.md`; this folder is its implementation, one seam per
file.

| File | What lives there |
| --- | --- |
| `mod.rs` | Index only. |
| `jev.rs` / `jev_tests.rs` | `TinyHumansSystemOne`, the `SystemOneTransport` over the TinyHumans System One proxy (`OPENCOMPANY_JEV_URL`), and `jev_router`, which answers `None` without a TinyHumans key so the driver routes by lead and mention. Gated on `openhuman`. |
| `mcp_server.rs` / `mcp_server_tests.rs` | The `opencompany` MCP server (`McpHost`): JSON-RPC over Streamable HTTP at `POST /internal/mcp/{company}/{runtime_agent_id}` on a dedicated loopback listener, one bearer per agent, `tools/list` = the speech tools plus the agent's OpenCompany tools, `tools/call` decided by the agent's `ApprovalPolicy`. `attach_opencompany_mcp` fixes it on an `AgentSpec`; `mount` puts the route on a caller's router. Gated on `openhuman`. |
| `tools.rs` / `tools_tests.rs` | `InFlightRegistry` — one in-flight turn per agent, keyed by runtime agent id — the `InFlight` speech fold (`speech::interpret`, one action per turn, `dm` recipient check), `InFlightContext` (the `ToolRunContext` a served tool runs under) and `McpToolAdapter`. Gated on `openhuman`. |
