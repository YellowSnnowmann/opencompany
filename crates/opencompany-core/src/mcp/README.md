# `src/mcp/` — MCP tool servers

Everything OpenCompany knows about remote MCP servers: what a company
declares, what each tool is allowed to do, how a server is probed, the
company-scoped registry store, and what one agent reaches. The HTTP routes stay
in `src/server/ops/` and call into here. OpenCompany's own `opencompany` MCP
server is a different thing and lives in `src/hive/mcp_server*`.

Design docs: [MCP servers](../../../../docs/modules/mcp.md),
[per-tool permissions](../../../../docs/modules/mcp-tool-permissions.md),
[directory installs](../../../../docs/modules/mcp-registry.md).

| Path | Gate | What lives there |
| --- | --- | --- |
| `mod.rs` | — | Module root. |
| `decl/` | ungated | Declarations, credentials, health, validation and the three-layer merge; see its `README.md`. |
| `policy/` | ungated | Per-tool approval policy and its per-agent narrowing; see its `README.md`. |
| `probe.rs` | `openhuman` | Probing a declared server, classifying failures, including a call's structured outcome (`classify_call_error`). |
| `observe.rs` | `openhuman` | `McpCallObserver` / `AgentMcpObserver`: a turn's MCP call outcomes into `OauthCall` metering and the failures the brain drains. |
| `runtime.rs` | `openhuman` | `McpRuntime`: the company-scoped registry store, directory search, installs, connections. |
| `agent/` | `openhuman` | What one agent reaches: spec attachments, registry tools, briefs; see its `README.md`. |

Tests sit beside each file as `<stem>_tests.rs`. `agent/agent_turn_tests.rs`
drives a live harness turn and needs the `mcp` feature; the CI `mcp-module`
lane runs every `mcp::` test under `openhuman,mcp,media`.
