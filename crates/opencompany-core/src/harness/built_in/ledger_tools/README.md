# `harness/built_in/ledger_tools/`

Child modules of `harness/built_in/ledger_tools.rs`.

| File | What lives there |
|------|------------------|
| `list.rs` | `list_ledgers`: the tool that names every ledger the agent may see with its statuses and open/closed counts, and `counts_line`, which reports a ledger whose rows could not be read as unavailable rather than empty. |

Tests are in `harness/built_in/ledger_tools_tests.rs` and `ledger_tools_unreadable_tests.rs`.
