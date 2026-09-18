# Connections: hosted providers and releasing legacy native OAuth

Split out of [README.md](README.md) when that file reached the 500-line cap.
The read plane is `ops::connections_read`; Composio owns every currently
actionable provider connection.

## Retired native OAuth

The former native OAuth flow used this host's own provider application, completed
a real handshake, and stored `oauth/{provider}`. Nothing under `src/harness/`
can resolve that credential, so a successful native connection never gave an
agent any capability.

The console stopped offering that flow in #828. #838 keeps its endpoints only as
a bounded compatibility bridge for console bundles cached before #979:

| Route | Until 2026-09-30 | Afterward |
| --- | --- | --- |
| `POST …/connections/{provider}/start` | `410 Gone` JSON: explains that native credentials were unreachable by agents, points callers to Composio, and carries a Sunset date | removed by #1023 |
| `GET /api/v1/oauth/callback` | `410 Gone` HTML: explains to a browser already returning from a provider that nothing was saved | removed by #1023 |
| `POST …/connections/{provider}/disconnect` | blanks and best-effort revokes a credential written before #828 | stays while legacy credentials exist |

The callback intentionally ignores every query parameter. Once `start` refuses
it cannot mint a new signed state, but a consent screen opened seconds before a
deploy can still redirect a real browser afterward. Explaining that outcome,
rather than accepting the code or returning a 404, prevents an in-flight
authorization from creating the same agent-unreachable credential.

A native credential already stored remains visible in `GET …/connections` with
`via: ["native"]` and can be released with Disconnect. It does not satisfy a
planning prerequisite and must not be read as an agent capability.

`credentialSource: "static"` now means only that a legacy native credential
exists. A host-level `OPENCOMPANY_OAUTH_<PROVIDER>_ID` / `_SECRET` pair does
not make a connection route available; it is retained solely for best-effort
revocation of that historical provider grant.

## Hosted connections

Issue `#319` remains the decision point for hosted provider connections. The platform
backend owns provider applications and token custody; OpenCompany agents reach
providers through the supported Composio path, never through
`oauth/{provider}`.

The read projection's `attested` source deliberately requires the projected-file tier, not
`TinyhumansTokenSource::from_env` as a whole: that resolver also accepts a
long-lived `TINYHUMANS_API_KEY`, which a self-hoster commonly sets to buy
inference. Accepting it here would tell such an operator their working Connect
button is platform-managed and take it away. Both the REST route and the GraphQL
`Company.connections` resolver project the field through the same
`connect_route_from_env`, so the two read shapes cannot drift.

**Provider mapping to the platform backend.** Its registered OAuth providers are
`notion`, `google`, `gmail`, `github`, `twitter`, `discord` and `instagram`. Two
consequences for the console catalog: `gmail` is a registered provider *name* but
not a separate provider application — it is Google's app requested with the Gmail
skill scopes, so a Gmail connect and a Google connect share one grant (which is
why the backend merges scopes incrementally rather than replacing them). And
there is **no Slack provider** at all (the backend's only Slack credential is an
internal alerting bot), so Slack has no hosted route except Composio, which runs
its own OAuth.

## One connection status, for one console list (issue #582)

`GET …/connections` is the **only** answer to "what is connected". It reconciles
the native `oauth/{provider}` catalog with a live Composio probe into one row per
provider, marking which namespaces reported it in `via` — and the console renders
exactly one provider grid from it (`frontend/src/lib/provider-grid.ts`).

That took removing a gate. `composio_view` used to discard the Composio half
unless the company explicitly granted the `composio` tool namespace, while
`GET …/composio/connections` — which the console's *other* provider list read —
never consulted the grant. The two lists therefore disagreed by construction, not
by timing: 13 of the 21 shipped companies grant no `composio`, so for most of
them one screen said both "connected" and "not connected" about the same account,
and the second list's Connect button was actionable.

The grant governs whether **agents receive Composio tools**. It never governed
whether a handshake completes — `resolve_tenant` reads the credential and the
toolkit allowlist and nothing else, so the sign-in the gate hid worked perfectly
well from the surface that ignored it. It is now reported (`granted` on
`GET …/composio`) and stated as a caveat next to the connected badge, rather than
silently deciding what the page may show.

Two consequences worth knowing:

- The probe now runs for any company holding a credential, not only granting
  ones. It stays bounded by `COMPOSIO_PROBE_TIMEOUT` and still degrades to
  `unverified` rather than to a confident "not connected".
- `reconcile` is split from `project_connections` purely as a test seam: the
  probe is a network call with no injection point, and the removed gate lived on
  the far side of it, which is how it survived unasserted.

## The capabilities panel's Composio verdict is a tier, not a stored token (issue #886)

`GET …/capabilities` is the panel an operator checks first when a tool looks
missing, so a wrong answer there sends the whole debugging session the wrong
way. It used to compute its Composio verdict from `composio::token_configured`,
which reads exactly one secret slot — the BYO override `composio/tinyhumans/key`.

The credential is resolved over **three** tiers, and the toolbelt gates on all
three (`composio::resolve_credential`, the seam issue #586 established): the BYO
override, then the company's own TinyHumans key, then this instance's platform
identity. On a hosted tenant nobody pastes a BYO token — the third tier answers,
and the tools wire up and are ready to attempt calls. Credential resolution
proves bearer presence, not that a later call succeeds — that is the probe's
job (above), a separate axis. The one-tier probe reported `false` throughout.

So the route now sends **both**, answering two different questions:

| Field | Question | Shape |
| --- | --- | --- |
| `composioTokenConfigured` | did *this company* paste a BYO token? | boolean, unchanged meaning |
| `composioCredentialSource` | which Composio credential tier resolves? | `attested` for the projected platform identity, `company` for the company's TinyHumans key, `static` for a BYO or static instance key, and `none` when no credential resolves |

Three properties are load-bearing:

- **The tier comes from the resolver, never from a second copy of its
  precedence.** This is the same rule the `GET …/composio` status route already
  follows; #886 is the copy that route's migration missed. The console must
  never be able to name a tier the agents are not on.
- **An unreadable secret store omits the field**, rather than reporting `none`.
  `none` is a verdict — "nothing resolves, no tools are wired" — and claiming it
  on a transient hiccup sends an operator to paste a token they already have.
  The console treats an absent field as unknown and must not render it in the
  alarm colour.
- **It is a resolution verdict, not a liveness one.** `attested` says a bearer
  can be obtained, not that Composio answered or that any account is connected.
  `GET …/connections` above is the axis that answers those, and a company with a
  valid bearer and zero connections is a working empty account, not a fault.

The evidence pack the planning station builds reads the same resolver, for the
same reason: it used to tell operators "this company has no Composio credential,
so no Composio account can be reached" on a card whose own evidence listed the
connectors as connected.

## The capabilities panel reports publishing, in two rungs not three (issue #1192)

Publishing was the one capability on that panel with **no field at all**. Media,
Composio, search and `repo` each carry a Granted/InBuild shape; whether an
agent could hand its work over as a deliverable was answerable only by reading
the manifest and knowing that `publish_artifact` rides the `files`/`docs` grant.

`GET …/capabilities` now sends two fields:

| Field | Question | Shape |
| --- | --- | --- |
| `publishGranted` | do this company's grants confer `publish_artifact`? | boolean |
| `publishInBuild` | is the harness carrying the tool compiled into this build? | boolean; there is no `publish` Cargo feature, it rides `openhuman` exactly as `searchInBuild` does |

Two things about the shape are deliberate and easy to get wrong:

- **A bare `*` DOES confer publishing**, unlike every `*Granted` neighbour on
  this panel. Publishing spends nothing and reaches nothing outside the
  company's own board, so it rides the ordinary namespace rule rather than the
  opt-in-by-name rule the real-money surfaces use. `grants_files_or_docs` is
  therefore **not** a `grants_*_explicit` sibling, and folding it into that
  family would silently revoke publishing for the majority of shipped manifests,
  which grant `*` and nothing else. `repoGranted` on the same response answers
  the opposite way for the same input; that asymmetry is intended.
- **There is no third rung, and adding one would be the #886 mistake again.**
  The other capabilities carry a credential/config flag because each can be
  granted and still wire nothing. Publishing has neither a credential nor a
  store toggle: the artifact store is non-optional on the runtime ops bundle and
  the single production `HarnessDeps` literal always sets it, so an
  `artifactStoreConfigured` field could only ever serialize a hardcoded `true`
  for every company on every deployment. The fail-closed "no artifact store
  configured" branch in `build_agent` is reachable from tests only.

The verdict comes from `company::grants_files_or_docs`, which is the same
predicate `build_agent`'s `wants_files` gate calls — one derivation, so the
panel cannot report a capability the toolbelt does not wire. That equality is
asserted over a grant matrix by
`harness::build::tests::the_capability_verdict_matches_what_the_toolbelt_wires`,
which is the standard #886 stated: prove the two agree by running both, not by
reading them.

## Two Composio accounts to choose between

`GET …/composio` carries a `mode` alongside `credentialSource`, and the two
answer different questions: the source names *whose identity* a call presents,
the mode names *which host* it is presented to.

- `managed` (the default) — Composio through the OpenHuman backend. Nothing to
  paste; the backend holds the Composio account, enforces its toolkit allowlist
  and bills the calls.
- `byok` — straight to `backend.composio.dev` with this company's own Composio
  API key, set through `PUT …/composio/api-key` (admin-only, write-only, never
  echoed). An empty `apiKey` clears the key and returns the company to managed.

`backendUrl` reports the host the calls actually reach, so it changes with the
mode — a status still naming the managed backend after a switch would read as
though nothing had happened.

The console renders this as one picker in `ComposioSection`, with a
confirmation on the first switch away from managed: the providers connected
through OpenHuman's Composio account live in *that* account, so the provider
grid goes empty until they are connected again in the company's own. That is
recoverable — clearing the key puts the company back — and saying so before the
switch is cheaper than explaining an empty grid after it.

Under `byok` the console hides the managed route's token card entirely: the
BYO backend token and the instance identity are out of play for **live
Composio calls** — authorize, execute, list connections — so offering a
control over them would be offering a control that changes nothing about
those calls. The company's TinyHumans key is the one exception: it still
authenticates the curated-catalog fetch (`TenantComposio::catalog`), so
clearing it while a company is on BYOK degrades the provider grid to the
account's own directory even though every Composio call keeps working. See
`docs/spec/runtime/credentials.md`'s "The catalog is OpenHuman's, even under
BYOK" for the full precedence.

Everything else the panel offers is unchanged, disconnecting included: the host
revokes through Composio's own `DELETE /api/v3/connected_accounts/{id}` rather
than the backend's route, which is a difference in who is dialled, not in what
the console can offer. `disconnectRouteFor` therefore takes no mode argument —
the route is resolved from what a tile is connected through, never from which
Composio the company uses.

## Releasing a connection: two routes, not interchangeable (issue #404)

There are two disconnects and they act on different things:

- `POST …/connections/{provider}/disconnect` blanks this host's own
  `oauth/{provider}` secret and best-effort revokes it upstream. It has never
  touched Composio.
- `DELETE …/composio/connections/{connection_id}` revokes **one connected
  account** at Composio. Addressed by connection id, because a company can hold
  two accounts for one toolkit and exactly one of them is being released.

Until the second route existed the console sent every Disconnect to the first,
so a Composio-connected provider answered `200`, reported success, and was still
connected on the next refresh. The console now routes by what the provider is
actually connected through (`disconnectRouteFor` in
`frontend/src/lib/provider-grid.ts`), preferring the Composio account when a
provider is connected through both — the native secret is inert until #396
lands, so blanking it would release nothing an agent can feel.

`GET …/composio/connections` carries the accounts a revoke is addressed to:
`accounts[]` per toolkit, each with Composio's verbatim `status`, its
`createdAt` when Composio published one, and the account label when the provider
did. `toolkit` and `connected` keep their previous meaning, so the callers that
read only those two are untouched.

**Which account an agent acts as is not decided here.** `composio_execute` posts
`{tool, arguments}` and no connection id (`src/harness/composio.rs`), so
Composio resolves it for the entity — nothing in this codebase selects, orders
or defaults an account, and the console's provider detail view says so rather
than marking one as the default. Changing that means sending a connection id on
execute, which is a harness change, not a console one.
