# Web search

How a company's agents find a page they do not already have a URL for, and
which account pays for the looking.

Two surfaces sit behind one tool name:

| Surface | Credential | Billed to | Capped by |
| --- | --- | --- | --- |
| **Managed** (default) | the instance's platform identity, from the environment | the platform | `[tools].search_daily_calls`, per company per UTC day |
| **Company provider** (BYO) | a key the company pasted into Settings → Search | that company's own provider account | whatever the company set where the key was issued |

Whichever is live, the agent is offered one tool called `web_search`. A
provider switch changes what a search costs and which index answers it; it
never changes what a teammate is told it can do — the shipped research skills
name `web_search` in their instructions, and a belt where that name came and
went with a settings change is how a model learns to invent URLs instead.

## What is inherited from OpenHuman

OpenHuman owns the search domain (`oh::search`): the engines, the result
rendering, the HTTP clients. OpenCompany selects from it rather than
reimplementing it.

- The **managed** surface posts to the same backend route through the same
  `IntegrationClient` and deserializes OpenHuman's own `SearchResponse`. It is
  its own tool (`crate::harness::search`) only because a *metered* tool has to
  read `cost_usd` off the response, which upstream's renderer drops. See that
  module's header for the full list of deliberate divergences.
- The **company provider** surface (`crate::harness::search_byo`) calls
  OpenHuman's own tool constructors with the company's key. A Brave result an
  OpenCompany agent reads is the text OpenHuman renders, because it *is*
  OpenHuman's tool.

Neither calls `oh::search::build_search_tools`: that entry point takes
OpenHuman's process-wide `Config`, and two companies on one host search through
two different accounts.

## Providers

| Slug | Credential | Extra tools beside `web_search` |
| --- | --- | --- |
| `managed` | none — the platform's | — |
| `brave` | API key | `brave_news_search`, `brave_image_search`, `brave_video_search` |
| `exa` | API key | `exa_find_similar`, `exa_get_contents` |
| `querit` | API key | — |
| `searxng` | an instance URL, no account | — |

The extras keep their upstream names, because they are genuinely different
affordances and a borrowed name is one an operator can look up.

## The gates

A search tool reaches an agent only when **all** of these hold:

1. The company's manifest **explicitly** grants `search`. A catch-all `*` does
   not confer it (`grants_search_explicit`) — see [tools.md](tools.md).
2. The agent's own `tools` request intersects it, as for any namespace.
3. A credential exists: either a company provider that is *complete*, or the
   managed backend. Neither, and nothing is wired — fail-closed, with a warning
   naming which half is missing.

A company provider **replaces** the managed tool rather than joining it. Two
"search the web" tools on one belt would let a model spend the platform's
metered budget for a company that pasted its own key, which is the bill-swap
this surface exists to prevent.

## Configuration

`GET`/`PUT` `…/companies/{id}/search`, and `DELETE …/search/key` to fall back
to managed. Writes require an admin: which index reads a company's queries, and
under whose retention policy, is not an ordinary member's edit. The console
renders it at Settings → Search.

Three secret-store keys, per company (`crate::company::search`):

| Key | Read back? | Holds |
| --- | --- | --- |
| `search/provider` | yes | the selected slug |
| `search/api_key` | **never** | the BYO key |
| `search/endpoint` | yes | the SearXNG instance URL |

There is deliberately **no environment fallback** for a company key: an ambient
one could only ever be somebody else's account. A selected-but-incomplete
provider is not an error — it falls back to managed, which is what OpenHuman's
own registry does for a BYO engine with no key. The status route reports
`provider` (what was picked) and `effectiveProvider` (what actually answers)
separately for exactly that reason.

The connection is re-resolved every turn and folded into the harness
fingerprint, so a key set, switched or cleared in the console takes effect on
the next turn with no restart — the contract Composio and hosting already have.

## Metering

The managed tool meters every call and reserves against a shared, company-keyed,
UTC-day ledger *before* the request goes out; over-cap returns a loud tool error
naming the ceiling, never an empty result set. See `crate::harness::search`.

A BYO call carries neither. The calls are billed by Brave or Exa to the
company's own account, under rate limits that company chose; applying the
platform's cap to them would be this host throttling a bill it does not pay, and
metering them would produce a number nobody can reconcile.

## Workflows

A `tool_call` node on `web_search` follows the same rule: the company's own
provider serves it when there is one, the managed surface otherwise, and a
node granted `search` on a deployment with neither is refused with a message
naming both remedies (`crate::workflows::caps::tools`).
