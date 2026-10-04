# Store Module

The store module is the default, file-based implementation of the persistence
ports: `CompanyStore`, `EventLog`, `TraceStore`, and
`SecretStore`. It writes the human-inspectable bundle layout from
[`docs/spec/runtime/lifecycle.md`](../../spec/runtime/lifecycle.md) — `company.toml`,
append-only `events.jsonl` and `ledger.jsonl`, and the `memory/` (cycle traces only),
`keys/`, and `secrets/` directories — one bundle per `CompanyId`.

Append-only files are never rewritten; each company gets its own namespace so
isolation holds. Alternate backends (sqlite, operator-supplied)
implement the same ports.
