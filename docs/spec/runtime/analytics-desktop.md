# Product analytics on the desktop

**Status: implemented.** The desktop app reports product analytics to OpenPanel
**by default**, with a user opt-out, from the Rust host. It is the one
deployment that does; every other kind still needs a deliberate opt-in
([analytics.md](analytics.md)). What is sent, and what never is, is exactly the
contract in [analytics.md](analytics.md) and [analytics-wire.md](analytics-wire.md):
shape and outcome, never content, under an opaque id.

## Why it is the Rust host, not the webview

The webview's CSP (`crates/opencompany-app/tauri.conf.json`) limits `connect-src`
to `'self'`, `ipc:` and the crash-reporting host. That is unchanged: no browser
sender, no analytics origin in the CSP. The embedded host is the same `opencompany`
crate a hosted tenant runs, so it reports through the same `Tracker`, payload
builder and transport. The browser client of the shared console stays silent on
desktop (see the console note in [analytics.md](analytics.md)).

## What a launched app does

A double-clicked `.app` has no environment, so the shell supplies the four
answers the core resolver needs, through `DesktopAnalyticsEnv`
(`crates/opencompany-app/src/analytics.rs`, modelled on `crash::DesktopEnv`):

| Variable | Desktop answer |
|---|---|
| `OPENCOMPANY_DEPLOYMENT` | always `desktop`, whatever the process env says |
| `OPENCOMPANY_ANALYTICS` | `off` if the user turned it off (this wins over an env `on`); else any non-blank env value (an env `off` wins); else `on` |
| `OPENCOMPANY_ANALYTICS_CLIENT_ID` | the env value if non-blank, else `DESKTOP_CLIENT_ID` |
| `OPENCOMPANY_ANALYTICS_ENDPOINT` | the env value if non-blank, else `https://panel.tinyhumans.ai/api/track` |
| `OPENCOMPANY_ANALYTICS_ID_KEY`, `OPENCOMPANY_TENANT_ID` | always unset |

Everything then goes through the unchanged core `analytics::config::resolve`, so
a blank is "not set", an unreadable value is `Unreadable` (silence, with a
reason), and a plain-`http` endpoint to a non-loopback host is refused. The core
resolver itself is untouched: a *bare* `Deployment::Desktop` with an empty
environment is still `Silent(NotHosted)`, which is why `opencompany serve` and the
TUI send nothing. The desktop opts in by saying so, in its own environment.

`DESKTOP_CLIENT_ID` is a single `pub const`. It currently reuses the TinyHumans
write client the console and hosted tenants ship; a bundle is readable by anyone
who unzips it, so a **dedicated, revocable desktop write client** (OpenPanel
`ignoreCorsAndSecret = true`) should replace it. Changing that one constant is
the whole migration.

## Identity is the instance, not the person

Events carry `profileId = i_<instance-id>`: the random 128-bit id in
`<data-dir>/instance-id` (`app/instance.rs`), the same id the console keys its
connection on. It names an install, not a human: no account, email, hostname or
username feeds it, and the tenant variables answer nothing so a keyed or hashed
tenant id can never replace it. Deleting the data root mints a new one. Each
local instance the shell runs has its own id and reports on its own.

Events also carry `shell_version` (the desktop app's version) beside the usual
envelope, emitted only when a shell names itself.

## The opt-out

The choice lives in `<data-dir>/preferences.json`, `{"analytics": bool}`, written
atomically (temp file, fsync, rename). A missing file, a missing key, or an
unreadable file (logged) all mean "the user never chose": **on**.

- **Opting out is immediate.** `ConsentGate` flips and every host's `GatedTracker`
  stops forwarding `track`; the same call runs `discard_pending()` on each
  host's tracker, so events queued before the user said no are not delivered after
  it. The gate flips *before* the file is written, so a full disk cannot keep
  events flowing.
- **Opting in applies at the next launch.** The core installs its tracker into a
  `OnceLock` at boot; a launch that began opted out installed a no-op that cannot
  be swapped. The preference is saved at once and `restart_required` says so.
  (Opting out and back in within one reporting launch simply resumes.)
- `OPENCOMPANY_ANALYTICS=off` in the environment always wins and is reported as
  `source: "env"`.

The Tauri commands (`commands_analytics.rs`):

- `oc_analytics_preference() -> {enabled, source, status, restart_required}`;
  `source` is `default`, `setting` or `env`; `status` is the default instance's
  tracker status (the same object `/spec` serves under `analytics`, see
  [analytics-status.md](analytics-status.md)), with `consent` filled in.
- `oc_set_analytics_preference(enabled)` persists, flips the gate, discards on
  off, and returns the same object.

## The Privacy page and the first-launch notice

Both are console UI over the two commands above, and **desktop only**: the
setting lives in the shell, so a browser console neither lists the page nor calls
the commands (`desktopOnly` in `frontend/src/views/settings-pages.ts`;
`#/settings/privacy` falls back to General outside the shell). The webview CSP is
untouched: the console never sends analytics, it only asks the shell.

- **Settings → Privacy** (`views/settings/PrivacyView.tsx`): the on/off switch
  (`oc_set_analytics_preference`), the last-send status read from
  `oc_analytics_preference().status` (result, HTTP status, time, accepted and
  dropped counts, redacted endpoint; re-read every 10 seconds), the "takes effect
  at next launch" note when `restart_required`, a locked switch with an
  explanation when `source` is `env`, a plain-language list of what is sent and
  what never is, the note that the collector sees the IP address of any request,
  and a link to this page.
- **First-launch notice** (`components/analytics-disclosure.tsx`): a floating,
  non-blocking card with "Got it" and "Turn off". Shown while `source` is
  `default` and analytics is on. **No new storage:** either button saves a choice
  (`true` or `false`), which flips `source` to `setting`, so "shown once" is the
  preference that already exists. "Got it" therefore writes an explicit
  `{"analytics": true}`. An `env` decision is never asked about.
- **Copy** is one file, `frontend/src/lib/analytics-copy.ts`, each line checked
  against `types.rs`, `types/event.rs` and [analytics.md](analytics.md). Legal and
  operator approval of that wording is pending before release.
- **Tests:** `privacy-view.test.ts`, `analytics-disclosure.test.ts`, and the CSP
  pins in `openpanel-loader.test.ts` (no `connect-src` entry, release or dev,
  names `panel.tinyhumans.ai` or `openpanel.dev`; the browser loader exits under
  `__TAURI_INTERNALS__`).

## `/spec` and the boot line

`/spec.analytics.consent` is no longer always `null`: on the desktop it is the
gate (`true`/`false`), the one deployment that asks. The boot line reads
`analytics: on (desktop default; turn off in Settings → Privacy or
OPENCOMPANY_ANALYTICS=off) — reporting to <endpoint>`, or `analytics: off (turned
off by the user)`; other deployments keep their wording.

## Flushing on exit

The transport drains on a 30-second timer, so the last seconds of a session would
die with the process. `lib.rs` builds the app and flushes every running host from
the `RunEvent::Exit` callback (`block_on` with a 2-second timeout), not from code
after `.run()`, which on some platforms never executes.

## Wiring

- `opencompany-app/Cargo.toml` adds `analytics` to the `opencompany-core` line,
  beside `tinyhumans` and `crash-reporting`. It is `["dep:reqwest"]` and `oauth`
  already pulls reqwest, so `Cargo.lock` is byte-identical (`cargo metadata
  --locked`). `assert-desktop-features.sh` needs no change: it guards only the
  command-line `acp,composio` pair.
- `embedded::start_with_analytics` takes an `AnalyticsSetup {gate, env}`. It wires
  a gated tracker into the state before any company is built, then installs it
  and logs the boot line once the listener is bound. `start_with` keeps its
  signature and defaults to a **disabled** setup, so the test suites stay silent
  and network-free.
- `desktop::desktop_builder` now calls `.with_analytics(state.analytics())`, as
  `serve` and provisioning do, so desktop companies meter into the tracker.

## The self-test

`opencompany analytics-test` (the server binary) and
`opencompany-desktop analytics-test` (hidden argv mode, runs before Tauri starts,
so it works headless on CI, through `DesktopAnalyticsEnv`) share
`analytics::selftest`. It resolves exactly like boot, then:

| Exit | Meaning |
|---|---|
| `2` | analytics is off in this process; the reason is printed |
| `0` | one `analytics_self_test` event was sent and the collector **accepted** it |
| `1` | anything else (`401`, unreachable, a build without the transport) |

The event is reported under a throwaway `s_<128 random bits>` profile, a third id
space beside `i_` and `t_`, printed alone on stdout so a CI step can look it up.
It can never be attributed to a real install.

## Tests

`analytics_tests.rs` (resolution, opt-out precedence, the gate),
`preferences_tests.rs`, `commands_analytics_tests.rs`, `embedded_analytics_tests.rs`
(an embedded host against a loopback collector: one `instance_started` with
`deployment=desktop` under `i_<instance-id>`, stable across relaunches), and in
core `selftest_tests.rs` and the exact-headers test in
`openpanel_collector_tests.rs`. The shell's collector is hand-rolled on tokio
(`test_collector.rs`) so there is no new dev-dependency.
