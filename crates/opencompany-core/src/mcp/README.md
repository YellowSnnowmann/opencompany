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
| `decl/mod.rs` | ungated | `McpServerDecl`, `AuthMaterial`, `McpHealth`, secret-store keys, and the three-layer merge `effective_mcp_servers`. Also reachable as `company::mcp`. |
| `decl/store.rs` | ungated | Runtime index, credential and health reads/writes, and `resolve_effective`. |
| `decl/validate.rs` | ungated | Declaration validation (HTTP only, reserved names, endpoint credentials) and default-server normalization. |
| `decl/file.rs` | ungated | The bundle's `mcp.json`. |
| `decl/endpoint.rs` | ungated | The rule for whether two records name the same server. |
| `decl/server_info.rs` | ungated | What a server says about itself (`serverInfo`), and its icon. |
| `decl/families.rs` | ungated | The persona brief naming which dispatch tool reaches which server. |
| `policy/mod.rs` | ungated | Per-tool approval policy: tiers, stored overrides, inventory, the allow set. |
| `policy/agent.rs` | ungated | Per-agent narrowing of that policy. |
| `probe.rs` | `openhuman` | Probing a declared server, classifying failures, `McpFailureQueue`. |
| `runtime.rs` | `openhuman` | `McpRuntime`: the company-scoped registry store, directory search, installs, connections. |
| `agent/mod.rs` | `openhuman` | Spec attachments (`embed_servers_for_agent`), the capability brief, live discovery. |
| `agent/resolve.rs` | `openhuman` | `resolve_for_agent` → `AgentMcp`: one agent's attachments, registry tools, bridge names and brief. |
| `agent/registry_list.rs` | `openhuman` | `mcp_registry_installed_list`, filtered by the agent's grants. |
| `agent/registry_scoped.rs` | `openhuman` | `OcMcpRegistryScopedTool`: grant and per-tool policy checks before a registry tool runs. |

Tests sit beside each file as `<stem>_tests.rs`. `agent/agent_turn_tests.rs`
drives a live harness turn and needs the `mcp` feature; the CI `mcp-module`
lane runs every `mcp::` test under `openhuman,mcp,media`.
