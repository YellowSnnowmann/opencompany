# The global baseline

What every company gets, whichever vertical it started from. A company bundle
under `companies/<name>/` describes one vertical; this directory describes the
part that is the same in all of them.

| Surface | Lives in | In every company because |
| --- | --- | --- |
| Agents | `agents/*.toml` | Research, writing, and keeping the record straight are not a vertical. |
| Workflows | `workflows/*.toml` | A weekly review and a research request run the same in a law firm and a game studio. |
| Ledgers | `ledgers/*.toml` | Risks, promises made outward, and what was learned are axes every company keeps, whatever it sells. |
| Setup cards | `tasks.toml` | Every company has the same first week of setup — a brief, its first goals, its standing decisions, its top risks, its connections — whatever it goes on to sell. Seeded onto the board once, in To-do. |
| Skills | `skills/*/SKILL.md`, named by `[skills].always` in `globals.toml` | Web research, a weekly report and a meeting brief are installed in every company rather than offered. |
| Tools | `[tools].default_allow` in `globals.toml` | Every vertical starts from the same belt; where that belt is *authored* is global, and a company can still narrow it. |

The contents are embedded into the binary at build time, so they are present in
a platform-provisioned container that has no repository checkout beside it —
the same reason `src/ledger/registry.rs` carries its built-in ledgers as code.

## A company always wins

On an id collision the company's own
definition supersedes the global one outright — its `agents/researcher.toml`
replaces this directory's, its `workflows/weekly_review.toml` replaces this
directory's. Nothing merges field-by-field, because a half-global teammate is
nobody's design.

To drop a global instead of replacing it, name it in the manifest:

```toml
[globals]
disable = [
  "agent:researcher", "workflow:weekly_review", "skill:meeting-brief",
  "ledger:risks", "task:name-the-top-risks",
]
```

Every entry is `<kind>:<id>` and must name a global that exists — a typo is a
validation error, not a silently ignored line.

Global agents load **after** the company's own roster, and no global agent is
tagged `tier = "orchestrator"`. Both facts protect the same thing: which
teammate runs the company is decided by the company, and
[`orchestrator_id`](../../crates/opencompany-core/src/company/types.rs) falls back to the first agent
declared when nobody is tagged.

## Ledgers are seeded, not resolved

Agents, workflows and skills are resolved on every read, so a change here
reaches every company on its next load. Ledgers are different: they are written
into the company's own store **once**, at first boot, because a company owns its
record and a person may retire a ledger. A baseline that re-asserted itself on
the next restart would take that call back.

So a ledger added to this directory reaches new companies, not existing ones —
which is the same trade `companies/<name>/workspace/**` makes, and for the same
reason. The declarations here are held to every rule
`docs/spec/runtime/ledgers.md` states: they cannot shadow a built-in, cannot
claim another ledger's derived file, and count against the 12-ledger cap like
any other, which is why there are three of them and not eight.

The full contract is `../../docs/spec/runtime/globals.md`.
