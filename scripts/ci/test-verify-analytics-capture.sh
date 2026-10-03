#!/usr/bin/env bash
#
# Cheap, network-free CI check of the analytics read-back tooling: the
# parsing/polling selftest of verify-analytics-capture.sh against canned JSON and
# SSE fixtures, plus a syntax pass over the sibling scripts. docs/spec/runtime/analytics-smoke.md.
set -euo pipefail
cd "$(dirname "$0")"
for script in verify-analytics-capture.sh assert-desktop-analytics.sh test-verify-analytics-capture.sh; do
  bash -n "$script"
done
python3 -m py_compile analytics-capture-server.py
rm -rf __pycache__
./verify-analytics-capture.sh --selftest
