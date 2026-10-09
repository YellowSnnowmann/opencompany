# `src/mcp/decl/` — MCP server declarations

What a company declares about its MCP servers and where the rest of that
state lives. Ungated, so its tests run in the default lane; also reachable as
`company::mcp`.

| File | What lives there |
| --- | --- |
| `mod.rs` | `McpServerDecl`, `AuthMaterial`, `McpHealth`, the secret-store keys, and the three-layer merge `effective_mcp_servers`. |
| `store.rs` | Runtime index, credential and health reads/writes, and `resolve_effective`. |
| `validate.rs` | Declaration validation (HTTP only, reserved names, endpoint credentials) and default-server normalization. |
| `file.rs` | The bundle's `mcp.json`, read through tinymcp's `config_doc::parse_with` with this host's fields registered. |
| `endpoint.rs` | The rule for whether two records name the same server. |
| `server_info.rs` | What a server says about itself (`serverInfo`), and its icon. |
| `families.rs` | The persona brief naming which dispatch tool reaches which server. |

Tests sit beside each file as `<stem>_tests.rs`.
