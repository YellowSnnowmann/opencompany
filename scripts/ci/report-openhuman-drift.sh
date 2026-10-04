#!/usr/bin/env bash
#
# Issue #499: say out loud how far the vendored openhuman pin has drifted.
#
# The pin reached 102 commits behind openhuman main with nothing reporting it,
# because a submodule pointer is invisible to every other check: nothing fails
# until a symbol the pinned copy lacks is finally needed, and by then the
# catch-up is a migration rather than a pointer move.
#
# DELIBERATELY NON-FATAL (the lane plan marks it report-only, and it exits 0).
# The distance is a function of how fast a different repository moves, so
# failing on it would block PRs that have nothing to do with the vendored tree.
#
# `unknown` rather than a wrong number is the whole discipline. A network blip,
# a shallow checkout with no reachable merge base, or a renamed default branch
# must not fail CI over a report — but nor may any of them read as "up to
# date". Hence the ancestry gate before the count: `rev-list --count
# HEAD..main` is 0 both when the pin EQUALS main and when it is AHEAD of it.
# The fetch is gated too: a failed fetch can leave a stale FETCH_HEAD.
#
# Moved out of the old ci.yml's Rust job unchanged.
set -u

cd "$(dirname "$0")/../.."

pin="$(git -C vendor/openhuman rev-parse --short HEAD)"
if git -C vendor/openhuman fetch --quiet --no-tags --depth 500 origin main \
  && git -C vendor/openhuman merge-base --is-ancestor HEAD FETCH_HEAD 2>/dev/null; then
  behind="$(git -C vendor/openhuman rev-list --count HEAD..FETCH_HEAD 2>/dev/null || echo unknown)"
else
  behind=unknown
fi
if [ "$behind" = unknown ]; then
  echo "::notice title=Vendored openhuman drift::pin $pin — distance from openhuman main could not be computed: the fetch failed, or the pin is not an ancestor of main (ahead of it, or on a branch of its own)."
elif [ "$behind" -eq 0 ]; then
  echo "vendor/openhuman is at openhuman main ($pin)."
elif [ "$behind" -le 25 ]; then
  echo "::notice title=Vendored openhuman drift::pin $pin is $behind commit(s) behind openhuman main."
else
  echo "::warning title=Vendored openhuman drift::pin $pin is $behind commits behind openhuman main. Past ~25 the catch-up stops being a pointer move — see issue #499, where it reached 102 and the reorg in between turned the bump into a migration."
fi
exit 0
