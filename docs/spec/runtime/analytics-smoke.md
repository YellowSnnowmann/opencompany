# Analytics capture proof (send, then read back)

`opencompany analytics-test` proves an event LEFT a binary and the collector
answered 2xx ([analytics-status.md](analytics-status.md),
[analytics-desktop.md](analytics-desktop.md)). It cannot prove the collector
KEPT it. A collector can accept and discard; this is the same blind spot the
Sentry gate closes for crash reports. This page is the automated proof, so nobody
checks a dashboard by hand to learn whether analytics works.

## The pieces

| Piece | What it proves |
|---|---|
| `scripts/ci/verify-analytics-capture.sh <profileId>` | The profile's events are **stored**: reads them back from OpenPanel's MCP endpoint (`https://panel.tinyhumans.ai/api/mcp`) |
| `scripts/ci/verify-analytics-capture.sh --selftest` | The parsing and polling logic, against canned JSON and SSE fixtures in `scripts/ci/fixtures/analytics-capture/`, no network. Run in the ungated `Actions pinned` job of `ci.yml` via `scripts/ci/test-verify-analytics-capture.sh` |
| `scripts/ci/assert-desktop-analytics.sh <app>` | The **built desktop app's wire contract**: runs `analytics-test` against a loopback capture server (`scripts/ci/analytics-capture-server.py`) and asserts exit 0, an `s_` profile id, `openpanel-client-id` and `openpanel-sdk-name: opencompany`, and no `Origin`/`Authorization`/`Cookie`. Runs in `build-desktop.yml` on the aarch64 leg only (the x86_64 leg is cross-compiled and cannot execute) |
| `.github/workflows/analytics-smoke.yml` | Daily (`17 4 * * *`) and on dispatch: the published tenant image (`ghcr.io/tinyhumansai/opencompany:latest`) and the latest aarch64 desktop DMG each send and read back |
| Release gates | A `Verify product analytics` step after `Verify crash reporting` in `release-production.yml`, and after the push in `deploy-staging.yml` |

## How the read-back works

Streamable-HTTP MCP, with the real handshake (a bare `tools/list` returns no
`result`): `initialize` (capturing `mcp-session-id` when the server sends one),
`notifications/initialized`, then `tools/call` `query_events` for the profile
over UTC yesterday..tomorrow. Because the `eventNames` filter has returned `[]`
for names that exist, a second query goes by `profileId` alone and the match is on
the returned events' `profile_id`. Plain JSON and SSE `data:` framing are both
parsed. It polls every 10 s, up to 18 times (180 s).

Failures are distinct on purpose:

- HTTP 401/403 fails at once: **read token lacks access** (fix the secret).
- Timeout after 2xx: **collector accepted but the event was never stored**
  (fix OpenPanel, not this repository).
- `OPENPANEL_MCP_BEARER` empty: `::warning::analytics read-back skipped:
  OPENPANEL_MCP_BEARER not set` and exit 0, unless `VERIFY_ANALYTICS_REQUIRED=1`,
  which fails. The release and staging gates leave it unset so they warn until the
  secret exists; the scheduled smoke sets it to `1`.

The bearer is environment only: masked with `::add-mask::`, passed to `curl` as a
config on stdin (`-K -`), so it is in neither argv nor a file.

## Operator: create the read client and set the secret

1. In OpenPanel (`https://panel.tinyhumans.ai`), open the project, then
   Settings, Clients (API keys), and create a client with mode **read**. Copy its
   client id and secret. Do not reuse a write (track) client.
2. Build the token as `base64(clientId:clientSecret)` and store it without it
   touching disk or shell history:

   ```sh
   printf '%s:%s' "$READ_CLIENT_ID" "$READ_CLIENT_SECRET" | base64 | tr -d '\n' \
     | gh secret set OPENPANEL_MCP_BEARER --repo tinyhumansai/opencompany
   ```

   (`gh secret set` reads the value from stdin when no `--body` is given.)
3. Dispatch **Analytics smoke** once. When it is green, set
   `VERIFY_ANALYTICS_REQUIRED: '1'` on the two gate steps to make a missing secret
   block a release.

## Manual checks this does not replace

A notarized DMG on a clean Mac (including Cmd-Q within 10 s of launch), the
toggle off and on, offline then online, sleep and resume, and the Little Snitch
prompt are not automatable here. The desktop leg of the smoke can only pass once
a release carrying the `analytics-test` mode has shipped.
