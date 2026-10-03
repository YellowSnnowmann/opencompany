# Cutting a release

Three workflows, dispatched in order, each with one decision to make. The
version is computed from the tree by the workflow; nobody types a tag.

```text
main ──promote──▶ release ──staging cut──▶ v0.2.4-staging   (artifacts, no Release)
                     │
                     └──production cut──▶ v0.2.5           (GitHub Release + DMGs + Windows installers + latest.json)
                                              │
                                              └── merged back into main
```

| Workflow | Dispatch from | The one decision | Produces |
|---|---|---|---|
| **Promote main to release** (`promote-main-to-release.yml`) | `main` | — | a merge commit on `release`, and a CI run for it |
| **Release Staging** (`release-staging.yml`) | `main` or `release` | which branch | tag `v<X.Y.Z>-staging`, signed DMGs and the Windows installers as Actions artifacts, a throw-away Docker build |
| **Release Production** (`release-production.yml`) | `release` | `release_type`: patch / minor / major | tag `v<X.Y.Z>`, a published GitHub Release carrying both DMGs, the Windows `-setup.exe` and `.msi`, their updater artifacts and `latest.json` |

Every cut bumps every version file, commits `chore(release): vX.Y.Z [skip ci]`
(or `chore(staging): …`) to the branch it was cut from, tags that commit, and
builds **the tag** — never the branch head, which may have moved by the time a
build starts. A cut from `release` is merged back into `main` at the end, so
`main`'s version never falls behind the last release.

## Day to day

```bash
# 1. Snapshot main into release. Re-running is a no-op when release already has main.
gh workflow run promote-main-to-release.yml --ref main -R tinyhumansai/opencompany

# 2. Optional: a staging build for testers, from release (or from main).
gh workflow run release-staging.yml --ref release -R tinyhumansai/opencompany

# 3. Ship. patch is the default; minor/major when the release warrants it.
gh workflow run release-production.yml --ref release -R tinyhumansai/opencompany -f release_type=patch
```

`--ref` is the branch in the web UI's "Use workflow from" dropdown, and it is
load-bearing: staging refuses anything but `main` or `release`, production
refuses anything but `release`. The dropdown lists every branch and tag; the
guard step is what makes picking the wrong one a five-second failure instead
of a build.

A fix found on `release` is a PR **against `release`**. It runs CI like any
other PR; the next cut picks it up and the back-merge carries it to `main`.
Re-dispatching the promotion afterwards merges — it never resets — so fixes on
`release` survive it.

## What a production cut does, in order

1. **`prepare-build`** — `scripts/release/bump-version.mjs <release_type>`
   moves the version in every file that carries it (the list is
   `scripts/release/version-files.mjs`: the workspace `Cargo.toml` and lock,
   the desktop shell's `Cargo.toml` and lock, `tauri.conf.json`,
   `frontend/package.json` and its lock), refuses a tree whose files already
   disagree, commits, tags `vX.Y.Z`, pushes, merges `release` back into `main`.
2. **`create-release`** — `scripts/release/generate-release-notes.mjs` writes
   the notes (OpenAI-polished when `OPENAI_API_KEY` is set, deterministic
   otherwise — the job log says which) and creates the Release **as a draft**.
3. **`build-desktop`** (`build-desktop.yml`) — both macOS architectures,
   Developer-ID signed and notarized, plus the updater's `.app.tar.gz` + `.sig`
   built from the stapled bundle; and, with the `windows` input (default on),
   the Windows x64 `OpenCompany_X.Y.Z_x64-setup.exe` and
   `OpenCompany_X.Y.Z_x64_en-US.msi`, each with its updater `.sig`.
   Everything attaches to the draft. A failed Windows leg fails the cut the
   same way a failed macOS leg does.
4. **`publish-docker`** — the tenant image, built from the tag (so `/spec`
   reports the bumped version) with the feature set in `deploy-staging.yml`,
   run through the `sentry-test` gate, then pushed to
   `ghcr.io/tinyhumansai/opencompany:vX.Y.Z`. boat.dev sandboxes
   (`tinyhumansai/opencompany-sandbox-manager`) follow the newest GitHub
   Release, so the image has to exist before the Release does. The GHCR package
   must be public for their anonymous pull.
5. **`updater-manifest`** — `latest.json` assembled from both macOS
   architectures' assets and the Windows setup's (`PLATFORMS=macos,windows`),
   uploaded to the draft. See [desktop-updates.md](desktop-updates.md).
   **`promote-image`** then retags `:latest` onto `:vX.Y.Z` (same digest), only
   once everything else has passed.
6. **`publish-release`** — every required asset is checked to be on the draft,
   then it is flipped public and marked latest. This repository has immutable
   releases: the asset list freezes at that moment, which is why nothing is
   published until it is complete.
7. **`cleanup-failed-release`** — if anything after the tag failed, the draft
   and the tag are deleted, so the next dispatch bumps cleanly and no
   half-built version is reachable. The bump commit stays; that is harmless.

`create_release: false` is a rehearsal: bump and build — no tag, no image push, no
Release, installers as Actions artifacts. The version still moves.

`windows: false` cuts a macOS-only release: no Windows leg, no Windows assets
required, no `windows-x86_64` in `latest.json`. It exists for a Windows-only
breakage that should not hold a macOS release; Windows users then stay on the
last release that carried them.

Separately, every push to `main` that touches the image's inputs publishes
`:staging` and `:sha-<short>` from `deploy-staging.yml` — main's head for
staging sandboxes, never `:latest`.

A staging cut is steps 1, 3 (both platforms, without the updater artifacts)
and a throw-away image build (no push), tagged
`vX.Y.Z-staging`, with no Release at all — see
[desktop-updates.md](desktop-updates.md#a-staging-cut-ships-no-update-anybody-can-reach)
for why that is the right shape for the auto-updater.

## Why it is shaped this way

**The version is computed, not typed.** The previous flow took a tag as an
input to two separate workflows, and the tag had to be created by hand first
— which is how v0.1.5 shipped with `crates/opencompany-app/Cargo.toml` still at
0.1.4: a hand bump touched five files and missed the sixth. Now one script
owns the list, `verify-version-sync.mjs` runs in CI on every PR, and the only
version-shaped input anywhere is patch/minor/major.

**One dispatch per cut.** The previous flow was `release.yml` (notes + draft)
followed by `Release Desktop (macOS DMG)` with `sign: true` and
`create_release: true` typed by hand, with the same tag typed into both. A
forgotten second dispatch left an asset-less draft that nobody could see from
the front page — v0.1.5 sat that way for a day. Now the second half cannot be
forgotten because it is not a separate thing.

**A long-lived `release` branch.** `main` moves continuously; a release needs a
point that can be fixed without taking everything that landed since. Fixes
land on `release` as PRs and reach `main` through the back-merge, so neither
branch loses them.

**No separate build-and-test job.** Everything on `release` came through a PR
that ran the CI lanes, the promotion runs them again on the snapshot, and the
desktop and Docker jobs compile the tag `--locked` anyway. A third compile of
the same tree cost ~30 minutes per cut and never found anything new.

**Pushes by the workflow do not trigger CI.** The bump commit and the
promotion merge are pushed with `GITHUB_TOKEN`, which GitHub deliberately
excludes from firing `push` workflows. The promotion's push to `release` is
made by a GitHub App instead, which does fire `ci-fast-hosted.yml`, and the
lanes force every area on for a `release` push or a dispatch, since their path
filter would otherwise see an empty diff. The bump commit is version numbers
only and is verified by the cut itself.

## Backfilling Windows for an older release

Releases cut before the Windows leg existed (v0.3.0 and earlier) carry no
Windows installers, and cannot be given any: the repository has immutable
releases, and a published release's asset list is frozen. So the installers go
into a **companion release** instead:

```bash
gh workflow run backfill-windows-desktop.yml --ref main -R tinyhumansai/opencompany -f tag=v0.3.0
```

`backfill-windows-desktop.yml` checks that `v0.3.0` is a published release
whose `tauri.conf.json` says `0.3.0`, creates tag `v0.3.0-windows` at the same
commit, drafts `OpenCompany v0.3.0 — Windows`, runs `build-desktop.yml` with
`macos: false, windows: true` against `v0.3.0`, and publishes the companion
with `--latest=false`. It then checks `/releases/latest` is still `v0.3.0` —
that is where every installed client finds `latest.json`, and the companion has
none — and re-marks `v0.3.0` as latest (failing the run) if it moved. On a
failure it deletes the companion draft and tag, and nothing else. It shares
`release-production`'s concurrency group, so it never overlaps a cut.

A Windows install from a companion reports "no update" until the next release
that ships Windows builds, then updates normally.

## Windows code signing (DigiCert KeyLocker)

Windows installers are Authenticode-signed only when all five DigiCert
KeyLocker secrets are in the `Production` environment: `SM_HOST`, `SM_API_KEY`,
`SM_CLIENT_CERT_FILE_B64` (the client `.p12`, base64), `SM_CLIENT_CERT_PASSWORD`
and `KEYPAIR_ALIAS`. With none of them the build ships **unsigned** installers —
SmartScreen warns on first run — and the run summary says so; with only some,
`guard` fails before anything compiles.

When configured, the Windows leg decodes the client certificate into the
runner's temp directory, installs DigiCert's `smctl`, runs `smctl healthcheck`
and `smctl windows certsync` **before** the compile, passes
`bundle.windows.signCommand` (`smctl.exe sign --keypair-alias=… --input %1`,
from `scripts/release/prepare-tauri-config.mjs`) to the Tauri build so the
binaries and both installers are signed as they are produced, and checks
`Get-AuthenticodeSignature` reports `Valid` on the setup and the MSI. The
certificate file is deleted at the end of the job whatever happened.

## When something goes wrong

| Symptom | What happened | What to do |
|---|---|---|
| "Tag vX.Y.Z already exists" in `prepare-build` | The tree's version is behind a tag that was already cut — usually a back-merge that never landed. | Merge `release` into `main` by hand (or the other way), check `node scripts/release/verify-version-sync.mjs`, re-dispatch. |
| "the tree does not agree on its current version" | Someone edited one version file by hand. | Fix the odd file to match, open a PR; CI's `verify-version-sync` step names it. |
| Production run red, no Release visible | `cleanup-failed-release` deleted the draft and the tag. | Read the failed job, fix on `release` via PR, re-dispatch. The next cut takes the next patch number. |
| "release→main back-merge hit conflicts" warning | Main and release diverged on the same lines. The release itself is fine and published. | Merge `release` into `main` by hand and resolve. |
| Release is published but `latest.json` is missing | Cannot happen through this flow — `publish-release` checks for it. If you see it, the release was published some other way. | Cut the next version; a published release cannot be amended. |
| Windows leg red, "partially configured" | Some DigiCert secrets are set, not all five. | Set the rest in `Production`, or remove them all to ship unsigned. |
| Need a build from a commit behind `main`'s head | — | That is what `release` is for: put `release` at that point (promote, or a PR against `release`) and cut from `release`. A bump commit built on an older point cannot be pushed to a branch without force, so there is no "cut this SHA" input. |

## What is needed once

Secrets in the `Production` GitHub environment (branch-policied to `main` and
`release`, no admin bypass): `APPLE_CERTIFICATE_BASE64`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` and `APPLE_TEAM_ID` for
Developer-ID signing; `APP_STORE_CONNECT_API_KEY_ID`,
`APP_STORE_CONNECT_API_PRIVATE_KEY_BASE64` (the `.p8`, base64) and
`APP_STORE_CONNECT_ISSUER_ID` for notarization, which authenticates with an
App Store Connect API key rather than an Apple ID and password;
`TAURI_SIGNING_PRIVATE_KEY` (+ `_PASSWORD`) for the updater
([desktop-updates.md](desktop-updates.md#operator-setup)); optionally the five
DigiCert KeyLocker secrets for Windows signing (above); and optionally
`OPENAI_API_KEY` for polished notes. Every job that reads one declares
`environment: Production` itself — `build-desktop.yml`'s `guard` and `build`,
and `create-release` — and each caller passes every secret name explicitly,
never `secrets: inherit`. `guard` fails in seconds, naming the missing secret,
before any build starts.
