#!/usr/bin/env bash
#
# Every `uses:` in .github/workflows/ names a commit SHA, and the vendored
# submodule init is called from its one script rather than inlined.
#
# Moved out of the old ci.yml's `Actions pinned` job so both CI profiles (the
# EX63 microVM and GitHub-hosted runners, scripts/ci/lanes/) run the same text.
#
# Pins: a tag is a pointer its owner can move, so a job naming one runs
# whatever it points at on the day. Resolve one with
#   gh api repos/<owner>/<action>/commits/<tag> --jq .sha
# and record the version in a trailing comment.
#
# Submodule init (issue #592): it used to be copied into four jobs and a fifth
# in a release workflow, and the copies drifted — the release copy was missing
# `vendor/tinyhumans-sdk` and the nested `tinycortex` init, which fails
# `cargo build --locked` at manifest resolution, and stayed invisible because
# that workflow had never run. The pattern is the inline-copy signature only
# (`git -C vendor/<path> submodule update`), not the drift report's
# `rev-parse` / `fetch`, nor deploy-staging.yml's root-level
# `git submodule update --init`.
#
# Both checks also fail when they match nothing (issue #555): a grep that
# selects zero lines would otherwise be green while asserting nothing.
set -euo pipefail

cd "$(dirname "$0")/../.."

failed=0

# Match the `uses:` value only, anchored to the step form, so prose that says
# "uses:" in a comment is ignored. A LOCAL reusable workflow
# (`uses: ./.github/workflows/x.yml`) takes no ref — it always runs at the
# caller's own commit — so it is exempt by its leading `./`, and only that.
unpinned=$(grep -rnE '^[[:space:]]*-?[[:space:]]*uses:' .github/workflows/ \
  | grep -vE 'uses:[[:space:]]*\./\.github/workflows/[^[:space:]]+\.ya?ml([[:space:]]|$|#)' \
  | grep -vE 'uses:[[:space:]]*[^@]+@[0-9a-f]{40}([[:space:]]|$|#)' || true)
if [ -n "$unpinned" ]; then
  echo "::error title=Unpinned action::A \`uses:\` reference names a tag or branch rather than a 40-hex commit SHA. Resolve it with: gh api repos/<owner>/<action>/commits/<tag> --jq .sha — then record the version in a trailing comment." >&2
  echo "$unpinned" >&2
  failed=1
fi
count=$(grep -rhcE '^[[:space:]]*-?[[:space:]]*uses:' .github/workflows/ | awk '{ n += $1 } END { print n + 0 }')
if [ "$count" -eq 0 ]; then
  echo "::error title=Pin check matched nothing::The grep selected zero \`uses:\` lines, so this check is green while checking nothing. The pattern or the workflow layout changed." >&2
  failed=1
fi

inlined=$(grep -rnE 'git[[:space:]]+-C[[:space:]]+vendor/[^[:space:]]+[[:space:]]+submodule[[:space:]]+update' \
  .github/workflows/ || true)
if [ -n "$inlined" ]; then
  echo "::error title=Inlined vendored submodule init::A workflow initializes the vendored submodules inline. That happens in exactly one place, scripts/ci/init-vendored-submodules.sh, which derives the crate list from the pinned vendor/openhuman/.gitmodules — call the script instead." >&2
  echo "$inlined" >&2
  failed=1
fi

# Anchored to the whole `run:` line, so it counts CALL SITES, not mentions —
# an unanchored search would match this script's own prose.
callers=$(grep -rlE '^[[:space:]]*run:[[:space:]]*scripts/ci/init-vendored-submodules\.sh[[:space:]]*$' \
  .github/workflows/ || true)
if [ -z "$callers" ]; then
  echo "::error title=Submodule init script unreferenced::No workflow calls scripts/ci/init-vendored-submodules.sh, so the check above is green while guarding nothing. Restore the call: \`run: scripts/ci/init-vendored-submodules.sh\`." >&2
  failed=1
fi

[ "$failed" -eq 0 ] || exit 1
echo "$count \`uses:\` references, all pinned to a commit SHA."
echo "Vendored submodule init is inlined nowhere and called from:"
echo "$callers"
