# Agentic Product Team

> A product organization of agents that triages bugs, grooms the backlog, defends a roadmap, and keeps a current read on competitors — with a human keeping the prioritization calls.

Nobody here writes the product's code. This team owns the decisions *around* the
code and hands each of them to whoever builds; the engineering counterpart is
[`agentic_software_company`](../agentic_software_company/).

## What it can do

- Turn raw reports into reproduced, deduplicated, classified bugs.
- Keep a backlog small enough that the order at the top means something.
- Sequence ready work into themes and bets, with a reason attached to every
  move.
- Keep a dated, sourced picture of what competitors ship and charge.
- Turn support threads and usage into findings the roadmap can be argued from.

## Agent roster

| Agent | Responsibility |
| --- | --- |
| Head of Product | Decide what the team works on next, and defend the decision. |
| Bug Triager | Turn raw reports into reproduced, classified, deduplicated bugs. |
| Backlog Curator | Dedupe, split, age out, and make every item ready. |
| Roadmap Planner | Sequence ready work into themes, bets, and an order. |
| Competitor Analyst | Keep a current, sourced picture of the market. |
| User Researcher | Turn support threads and usage into evidence. |

Every teammate carries a checked-in brief under
[`agents/prompts/`](agents/prompts/), named by its `prompt_files` entry. The
brief is where the role's actual working rules live — the `.toml` beside it
carries only the wiring (tier, grants, write scope). Print what any of them
assembles into with:

```sh
./scripts/dump-prompt.sh --company companies/agentic_product_team
```

## Workflows

| Workflow | What it does |
| --- | --- |
| `bug_triage` | A raw report becomes a merge or a new, ready bug row. |
| `roadmap_review` | The weekly loop: refresh evidence, re-sequence, record the calls. |
| `competitor_scan` | Re-read one competitor from public sources and report what changed. |

`roadmap_review` also runs unattended on a Monday-morning `[[schedule]]`.

## Human in the loop

Humans keep the **prioritization calls and roadmap sign-off**; the agents run
everything that produces the evidence for them. Anything that is genuinely the
operator's — cost, headcount, a customer commitment, a strategy change — is
parked with options and a recommendation rather than decided.

## Run it

```sh
cargo run --bin opencompany -- serve --company companies/agentic_product_team
```
