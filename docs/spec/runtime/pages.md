# Agent-authored internal dashboard pages

A company can hand an agent the ability to build its own small internal
dashboard pages — a metrics view, a pipeline board, a status page — as real
React, rendered inside the operator console. This is not a template or a
markdown render: the agent writes TSX, the server compiles it, and the
console runs the result. That means running agent-authored code in the
browser, which is the one thing this repository had, until now, no
infrastructure for at all: `GET …/workspace/blob/{id}`
([`workspace.rs`](../../../src/server/ops/workspace.rs)) *actively refuses*
to render anything inline for exactly this reason (issue #667). Pages answers
that refusal with real isolation rather than routing around it — see
"The security model, in two halves" below.

## Storage: `pages/<slug>/` in the existing workspace tree

No new port. A page lives at `pages/<slug>/` in the company's
[`WorkspaceStore`](../../../src/ports/workspace.rs), the same store
`agents/<agent-id>/` and the rest of the shared note tree already use:

```text
pages/<slug>/
  page.toml          # title, description, icon, nav_visible — the manifest
  page.tsx            # the agent-authored source, a text node
  page.compiled.mjs   # server-compiled output, a binary node, mime application/javascript
```

`page.tsx` is an ordinary text node; `page.compiled.mjs` is a binary node —
mime, size and sha256 computed by the store, same as any upload. Both ride
the workspace's existing `[workspace] max_blob_mb` / `tree_quota_gb` quotas;
no new limit exists for pages specifically.

`page.compiled.mjs` may also be a plain text node — an operator creating the
file through the console, or a test seeding the tree over the workspace API,
stores text, not a payload. The bundle route serves whichever kind the node
is; the bytes are the compiled module either way.

`slug` is restricted to `^[a-z0-9][a-z0-9-]*$` — narrower than an ordinary
workspace path segment, because a slug is also a URL path segment
(`GET …/pages/{slug}`) and has to survive that role without escaping or
ambiguity. The layout constants (`Pages`, `page.toml`, `page.tsx`,
`page.compiled.mjs`, the compiled mime) live in
[`company::workspace_scaffold`](../../../src/company/workspace_scaffold.rs)
rather than only in the harness tool module, because the HTTP routes that
serve a page must exist — and 404 correctly — in a build compiled without the
`openhuman` feature, which is the only build the harness tools compile under.

## The tool namespace: `pages`

[`harness::pages_tools`](../../../src/harness/pages_tools.rs) exposes four
tools, mirroring the shape of `harness::workspace_tools`:

- `pages_list` — every page's slug and manifest.
- `pages_read` — one page's manifest and `page.tsx` source.
- `pages_write` — create or update a page's manifest and/or source.
- `pages_delete` — remove a page's whole bundle.

Unlike `workspace`, there is no `pages.write` split behind a separate,
explicit grant: `pages` rides the default `"*"` grant whole, the same as
`files`/`docs`/`shell`/`code`. A company that has not deliberately withheld
tools gets all four the moment it names an agent for the job — the global
`page_builder` agent (`companies/_globals/agents/page_builder.toml`) is exactly that.
`pages` is also **not** in `GATEABLE_NAMESPACES`
(`src/company/types.rs`), for the same reason `workspace`/`docs`/`files`
are not: an agent should not lose the ability to fix a broken page under
token-budget pressure.

## The compile contract

`pages_write` compiles `page.tsx` synchronously, whenever `source` is given,
using [`swc_core`](https://github.com/swc-project/swc) — a pure-Rust
TypeScript/JSX compiler, chosen specifically because the runtime image has no
Node (`deploy/Dockerfile`'s builder stage; only the separate frontend Docker build
stage does). Compilation therefore has to be a Rust-native step, done inside
this binary, at request time.

The pipeline, in [`pages_tools::compile_page`](../../../src/harness/pages_tools.rs):

1. **Parse** as TSX (`Syntax::Typescript { tsx: true, .. }`).
2. **Check the import allow-list**, on the freshly parsed AST, before any
   transform runs. Every specifier a page references must name exactly one
   of: `"react"`, `"react-dom/client"`, `"react/jsx-runtime"`,
   `"@opencompany/site"`. The check is a full AST walk, so it covers all
   three forms that carry a module specifier — a static `import`, a
   re-export (`export * from "…"` / `export { x } from "…"`), and a dynamic
   `import("…")` — not just the top-level `import` statements (a page could
   otherwise smuggle a bare specifier through a form the allow-list never
   looked at, and the browser would fetch it outside the served import map).
   Anything else — `"node:fs"`, a bare npm package, a relative import —
   fails the whole call with a diagnostic naming the disallowed specifier.
   This is a compile-time allow-list check, not a sandbox: the runtime
   isolation is the sandboxed iframe (see below), and the allow-list exists
   so a page cannot even *reference* something the pages SDK does not intend
   to serve, catching a mistake at write time instead of at render time.
3. **Strip TypeScript** types (`ecma_transforms_typescript::strip`).
4. **Transform JSX** via the automatic runtime
   (`ecma_transforms_react::react` with `Runtime::Automatic`), which rewrites
   JSX elements into `jsx`/`jsxs` calls importing from `"react/jsx-runtime"`
   — no `React` import is needed in page source.
5. **Render** the transformed AST back to JS text (`ecma_codegen`).

A parse error, a rejected import, or a codegen failure returns the
diagnostic as the tool's error result — the same ergonomics as a failing
`cargo build` — and **writes nothing**: neither `page.tsx` nor
`page.compiled.mjs` changes until a call compiles cleanly. `pages_write`
also carries a required `expected_updated_at` compare-and-swap token
whenever it overwrites a page that already has a `page.tsx`, the same
read-before-write invariant `workspace_write` enforces.

## The HTTP routes

[`server::ops::pages`](../../../src/server/ops/pages.rs) serves four routes,
scoped to the addressed company exactly like every other console route (this
is an internal dashboard page, not a public site):

| Route | Serves |
| --- | --- |
| `GET {scope}/pages` | Every page's manifest as JSON — `[{ "slug", "title", "description", "icon", "navVisible" }]` — for the console nav. |
| `GET {scope}/pages/{slug}` | A fixed HTML shell: an import map pointing `react` / `react-dom/client` / `react/jsx-runtime` at `/pages-sdk/react.mjs` and `@opencompany/site` at `/pages-sdk/index.mjs`, plus a `<script type="module">` that loads the SDK unconditionally — `@opencompany/site` is present even for a page that does not import it itself, so the gesture-relay listener below is always there — and a second `<script type="module">` that imports `./{slug}/bootstrap.mjs?oc_cap=…` (a path relative to the shell's own URL at `…/pages/{slug}`) and mounts the page with `ReactDOM.createRoot`. |
| `GET {scope}/pages/{slug}/bootstrap.mjs` | The fixed mounting module, served with the shell-minted capability threaded into its own import of the bundle. |
| `GET {scope}/pages/{slug}/bundle.mjs` | The page's `page.compiled.mjs`, streamed with `Content-Type: application/javascript` and `Content-Disposition: inline`. |

All four set:

```text
Content-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; connect-src 'none'; frame-ancestors 'self'
X-Content-Type-Options: nosniff
Cache-Control: no-store
```

`Cache-Control: no-store` on every route: the shell, manifest and bundle are
authenticated, company-specific content, so a browser — or an intermediary —
must never serve a cached copy of one company's (or one session's) page to
another request.

### Why the module graph is gated by a capability, not a session

The sandboxed iframe is opaque-origin (`sandbox="allow-scripts"`, no
`allow-same-origin`), which is what makes the module graph a CORS request with
`Origin: null` **and no session cookie**: a browser does not attach cookies to
subresource requests from an opaque-origin frame, no matter what credentials
mode the module element asks for. So the two module routes cannot be
session-authenticated the way the shell is. The shell is loaded by an iframe
`src` *navigation*, which does attach the operator's HttpOnly cookie; it mints
a short-lived, unguessable capability bound to this company and page and
embeds it in the module URLs (`?oc_cap=…` on the bootstrap, threaded through
the bootstrap's static import of the bundle). The module routes accept
**that** instead of a session. The capability is minted fresh on every shell
load, expires within a minute, and authorizes only the two module URLs for
that one page — it cannot list pages, read another page's bundle, or reach any
other route.

The module responses additionally carry `Access-Control-Allow-Origin: null`
and `Access-Control-Allow-Credentials: true`. The opaque origin reports itself
as the literal `"null"` origin, and module scripts always fetch with CORS, so
the response must admit that origin explicitly or the browser refuses it.
`Access-Control-Allow-Credentials: true` is harmless — the frame sends no
credentials — and keeps the pair consistent across every module the shell
references. The console's fixed `/pages-sdk/*.mjs` assets carry the same CORS
response headers. This is narrowly for the same-origin console deployment, as
defense in depth on top of the iframe sandbox described below, which is the
boundary that actually holds.

### Why `Content-Disposition: inline` is the right call here, and wrong at `workspace/blob/{id}`

`workspace/blob/{id}` forces `attachment` and a fixed allow-list of *safe to
render* image/PDF types, because the bytes behind it are an **untrusted
upload** — anything a caller sent, of any claimed mime, with no verification
that the bytes match the claim. Rendering that inline on the console's own
origin would hand a malicious upload the operator's session cookie.

`page.compiled.mjs` is a different kind of bytes: it is not upload input, it
is the **validated output of the compile step** in the previous section — a
source that already passed TSX parsing, the import allow-list, and a
successful codegen. Serving it as `application/javascript` with `inline` is
serving trusted output, not routing around the blob route's caution; the
blob route's refusal and this route's `inline` are the same policy applied
to two different inputs.

The HTML shell in the middle route is not agent content at all — it is a
fixed Rust format string this route builds itself, with the slug
(pre-validated as `^[a-z0-9][a-z0-9-]*$`) as its only interpolated value —
so there is no injection surface there either.

## The security model, in two halves

**Server half (this document).** CSP headers, a validated slug, and a
compiled bundle that already passed an import allow-list before it was ever
written. None of this is the actual isolation boundary — it is defense in
depth around a payload that is trusted because of how it was produced, not
because the server contained it after the fact.

**Client half (frontend, a separate concern from this doc).** The console
embeds a page in a sandboxed iframe — `sandbox="allow-scripts"`, deliberately
**without** `allow-same-origin` — so the frame is opaque-origin: it cannot
read `document.cookie`, cannot reach the parent frame's DOM, and cannot ride
the operator's session on a credentialed request of its own. Live data
reaches the page through a postMessage bridge to the parent console tab
instead of a credential handed into the frame; the parent is the only party
that holds the operator's authenticated session, and it executes the page's
requests on the page's behalf. The bridge runs over a `MessageChannel` port
the console transfers to exactly the loaded document: a document the page
navigates itself to never receives the port, so it cannot send through the
bridge — or observe a reply — even if it captures something the page that was
there before it knew. That bridge forwards full GraphQL — queries **and**
mutations — so a page can read and write company data with the same authority
the operator's own session has; the sandbox stops the page from touching
cookies, the parent DOM, or making its own credentialed requests, but it does
not limit what an authorized request can *do* once it crosses the bridge. The
iframe embedding, the bridge, and the nav view that lists pages are frontend
concerns and are not described further here.

**The gesture relay.** The one thing that travels the bridge the other way —
parent to page — is a forwarded gesture. A toast in the console can cover a
control inside the Pages view (issue #1303); a DOM event cannot cross the
sandboxed-frame boundary, so when the toast is clicked the console posts the
gesture's coordinates to the frame's document instead: `oc:relay-click` or
`oc:relay-pointerdown`, with `x`/`y` shifted into frame-relative viewport
coordinates, and the pointer's `pointerId`/`pointerType`/`button`/`buttons`
fields on the pointerdown variant. Pointer capture cannot reach into another
document, so a press is relayed whole: the console keeps posting the rest of
the sequence — `oc:relay-pointermove`, `oc:relay-pointerup`,
`oc:relay-pointercancel` — to the same frame until the press ends, and the
SDK routes the continuations to the element that took the press so a drag or
press-state control completes instead of getting stuck. The page SDK accepts
a relay only from `window.parent` — a frame the page embeds itself surfaces
as its own window, not the parent, so the source check is the whole trust
boundary — and turns it back into a real click or `pointer` sequence on
whatever `elementFromPoint(x, y)` finds in the frame's own document. The
re-dispatched events are programmatic and therefore untrusted: like the
console's own synthetic clicks, they carry no transient user activation, so
a control that requires activation (a file input, `showPicker()`,
`window.open()`) stays unreachable through an overlay — a browser will not
transfer activation across the sandbox boundary (that is the clickjacking
defense) — and the relay targets the ordinary click- and pointer-driven
controls a toast-over-page gesture is actually for.

**Normative: pages require a same-origin console.** The page shell is loaded by
an iframe `src` navigation, which can only attach the credentials a browser
attaches to a same-origin request — the operator's HttpOnly session cookie. It
cannot attach the API client's `authorization` or `x-opencompany-session`
header, and the module graph that follows rides a capability only that
authenticated shell request can mint. `pages` MUST therefore only be served to
a console that is same-origin with the host. A cross-origin console gets no
pages: its shell request is unauthenticated, so no capability is minted and no
module can load.

**Normative: the bridge's residual privilege.** A page that the console's parent
frame loads through the bridge described above MUST be assumed able to perform
every query and mutation the operator's session authorizes, unless the console
imposes an operation allow-list at the point the bridge forwards a request.
Possession of the transferred `MessageChannel` port — with the per-document
capability layered on top — authenticates the caller as the exact document the
console loaded, but does NOT restrict the scope of operations an
authorized message can request. The operational consequence feature
(`pages_write`, `pages_delete`) gates *persisting* a page — approval is
single-use and covers only that one storage operation; every later GraphQL
request the rendered page fires through the bridge is ungated. This is the
deliberate trade-off described in the client half above: the sandbox protects
the operator's *session credential* from the page, but does not protect the
operator's *authority* from what a page asks to do with it.
