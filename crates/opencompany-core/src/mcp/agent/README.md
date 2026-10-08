# `src/mcp/agent/` — what one agent reaches over MCP

Compiled only under `feature = "openhuman"`. `agent_turn_tests.rs` drives a
live harness turn and needs the `mcp` feature.

| File | What lives there |
| --- | --- |
| `mod.rs` | Spec attachments (`embed_servers_for_agent`), the registry built from declarations, the capability brief, live discovery. |
| `resolve.rs` | `resolve_for_agent` → `AgentMcp`: one agent's attachments, registry tools, bridge names and brief. |
| `registry_list.rs` | `mcp_registry_installed_list`, filtered by the agent's grants. |
| `registry_scoped.rs` | `OcMcpRegistryScopedTool`: grant and per-tool policy checks before a registry tool runs. |
| `registry_outcome.rs` | The `McpCallOutcome` a `mcp_registry_tool_call` result carries. |

Tests sit beside each file as `<stem>_tests.rs`.
