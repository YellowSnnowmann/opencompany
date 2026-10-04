#!/usr/bin/env bash
#
# Is this PR commit being run on the EX63 by CI Fast (ci-fast.yml)?
#
# Writes outsider=true|false to $GITHUB_OUTPUT: false when ci-fast.yml routed
# the commit to the EX63 (an org member's PR, with the CI_EX63 switch on), true
# otherwise. ci-fast-hosted.yml runs the hosted lanes only for outsiders, and
# ci-gate.yml then requires whichever flow ran. Ported from OpenHuman's
# scripts/ci/ci-fast-route.sh (its #6549).
#
# Why not decide here: ci-fast.yml runs from main with the CI_MEMBERSHIP_TOKEN
# secret, so it also sees private members. A fork's `pull_request` run gets no
# secrets and sees those members only as COLLABORATOR, so deciding here too
# would run their PRs twice. Instead, find ci-fast.yml's run for this head
# commit and read its outcome: lanes skipped means outsider, anything else
# means the EX63 has it.
#
# Env: GH_TOKEN, REPO, HEAD_SHA, AUTHOR, ACTOR, ASSOCIATION, EVENT.
set -euo pipefail

# Merge-queue commits, pushes and dispatches have no PR and no ci-fast.yml run
# (pull_request_target only fires for PRs); they always run here.
case "${EVENT:-pull_request}" in
  pull_request) ;;
  *)
    echo "[ci][route] event=${EVENT}: no ci-fast.yml run exists for it; running the hosted lanes"
    echo "outsider=true" >> "$GITHUB_OUTPUT"
    exit 0
    ;;
esac

decide_from_ci_fast() {
  local run_id="" lanes="" route=""
  for _ in $(seq 1 36); do
    run_id="$(gh api "repos/${REPO}/actions/workflows/ci-fast.yml/runs?head_sha=${HEAD_SHA}&per_page=5" \
      --jq '[.workflow_runs[] | select(.event == "pull_request_target")] | sort_by(.created_at) | last | .id // empty' 2>/dev/null || true)"
    if [ -n "${run_id}" ]; then
      # A member run materialises the reusable workflow's jobs ("Lanes / ...");
      # an outsider run leaves the skipped call job "Lanes" alone.
      lanes="$(gh api "repos/${REPO}/actions/runs/${run_id}/jobs?per_page=50" \
        --jq 'if any(.jobs[]; .name == "Lanes / CI Fast (EX63)") then "member"
              elif any(.jobs[]; .name == "Lanes" and .conclusion == "skipped") then "outsider"
              else "" end' 2>/dev/null || true)"
      route="$(gh api "repos/${REPO}/actions/runs/${run_id}/jobs?per_page=50" \
        --jq '[.jobs[] | select(.name | startswith("Route"))][0].conclusion // empty' 2>/dev/null || true)"
      if [ "${route}" = "success" ]; then
        case "${lanes}" in
          outsider) echo "true ci-fast run ${run_id}: EX63 lanes skipped"; return 0 ;;
          member) echo "false ci-fast run ${run_id}: EX63 lanes running"; return 0 ;;
          *) ;; # jobs not materialised yet
        esac
      fi
    fi
    sleep 5
  done
  return 1
}

if verdict="$(decide_from_ci_fast)"; then
  outsider="${verdict%% *}"
  reason="${verdict#* }"
else
  # No ci-fast.yml decision within 3 minutes (it does not exist yet on the PR
  # that adds it, or the API is down): run here. Running a member's PR on both
  # flows costs minutes; running it on neither would leave the gate pending.
  outsider=true
  reason="fallback: no ci-fast.yml decision within 3 minutes"
fi
echo "[ci][route] author=${AUTHOR} actor=${ACTOR} association=${ASSOCIATION} outsider=${outsider} (${reason})"
echo "outsider=${outsider}" >> "$GITHUB_OUTPUT"
