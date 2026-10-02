#!/usr/bin/env bash
# Fail, after restoring it, if publishing a companion release moved
# `/releases/latest` off the release that held it beforehand.
#
# Required environment:
#   REPO           owner/name on GitHub
#   COMPANION      the companion release tag that was just published
#   LATEST_BEFORE  the tag `/releases/latest` resolved to before publishing
#   GH_TOKEN       a token with release write scope
set -euo pipefail

: "${REPO:?REPO is required}"
: "${COMPANION:?COMPANION is required}"
: "${LATEST_BEFORE:?LATEST_BEFORE is required}"

latest="$(gh api "repos/${REPO}/releases/latest" --jq .tag_name)"
if [ "$latest" != "$LATEST_BEFORE" ]; then
  gh release edit "$LATEST_BEFORE" -R "$REPO" --latest
  echo "::error::/releases/latest moved to '$latest' after publishing $COMPANION; restored to $LATEST_BEFORE. Check that updates resolve." >&2
  exit 1
fi
