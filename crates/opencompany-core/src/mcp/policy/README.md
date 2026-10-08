# `src/mcp/policy/` — per-tool approval policy

Which MCP tools an agent may call, and at what approval tier. Ungated: the
console route that edits a policy ships without the harness, and the gate that
enforces one ships with it.

| File | What lives there |
| --- | --- |
| `mod.rs` | The tier vocabulary, stored overrides, the tool inventory, and the ladder that resolves a tier from them; the allow set. |
| `agent.rs` | Per-agent narrowing of that policy: the resolved tier for one agent and tool, and the tools it is blocked from. |

Tests sit beside each file as `<stem>_tests.rs`.
