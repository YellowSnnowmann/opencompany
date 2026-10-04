#!/usr/bin/env bash
#
# Assert that CI listens for GitHub merge-queue checks, does not silently skip
# its conditional lanes for the queue's synthetic merge commit, and posts the
# required status on that commit.
#
# Issue #793. A `merge_group` trigger alone is not enough. The lane plan's path
# filter is designed around pull-request metadata, which a merge-group event
# does not carry; if it answered false, every conditional Rust, console and
# desktop lane would be skipped and the queue could treat a no-op run as
# evidence that a batched merge is safe. So ci-lanes.yml skips the filter for
# those events and forces every area on. A `push` to `release` and
# `workflow_dispatch` ride the same wiring: promote-main-to-release.yml pushes
# a fresh `release` snapshot whose diff against the default branch is empty.
#
# Since the lanes moved out of the monolithic ci.yml there are three more
# halves: the hosted flow must take merge-queue commits (no ci-fast.yml run
# exists for them, because pull_request_target only fires for PRs), and the
# gate must evaluate merge-group runs, or the queue would wait forever on a
# `PR CI Gate` status nobody posts. These textual assertions make the wiring
# fail loudly if a later cleanup removes one half of it.
set -euo pipefail

cd "$(dirname "$0")/../.."

hosted=.github/workflows/ci-fast-hosted.yml
lanes=.github/workflows/ci-lanes.yml
gate=.github/workflows/ci-gate.yml
gate_script=scripts/ci/ci-gate.mjs
route=scripts/ci/ci-fast-route.sh

for f in "$hosted" "$lanes" "$gate" "$gate_script" "$route"; do
  if [ ! -f "$f" ]; then
    echo "assert-merge-group-workflow: $f is missing" >&2
    exit 1
  fi
done

require_line() {
  local file=$1 description=$2 line=$3
  if ! grep -qF -- "$line" "$file"; then
    echo "assert-merge-group-workflow: $file is missing $description:" >&2
    echo "  $line" >&2
    exit 1
  fi
}

require_line "$hosted" "merge_group trigger" "  merge_group:"
require_line "$hosted" "checks-requested merge-group activity" "    types: [checks_requested]"
require_line "$hosted" "manual dispatch trigger" "  workflow_dispatch: {}"
require_line "$route" "non-PR events routed to the hosted lanes" '  pull_request) ;;'

require_line "$lanes" "queue-safe path-filter guard (merge_group)" "github.event_name != 'merge_group' &&"
require_line "$lanes" "queue-safe path-filter guard (dispatch)" "github.event_name != 'workflow_dispatch' &&"
require_line "$lanes" "queue-safe path-filter guard (release push)" "!(github.event_name == 'push' && github.ref == 'refs/heads/release')"
require_line "$lanes" "forced areas when the filter is skipped" "FORCED: \${{ steps.filter.outcome == 'skipped' }}"
require_line "$lanes" "every area forced on" 'if [ "${FORCED}" = "true" ]; then RUST=true FRONTEND=true DESKTOP=true; fi'

require_line "$gate" "merge-group runs evaluated by the gate" "github.event.workflow_run.event == 'merge_group'"
require_line "$gate_script" "merge-group runs counted by the gate" '"merge_group",'

echo "CI listens for merge-group checks, selects every lane for them, and gates their commit."
