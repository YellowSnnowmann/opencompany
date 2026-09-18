#!/usr/bin/env bash
#
# Fail if any Rust source file under `crates/*/src` is over 750 lines, or
# still carries an inline `#[cfg(test)] mod tests { ... }` block instead of a
# sibling `*_tests.rs` file.
#
# Both rules come from the same place as the Markdown cap in
# `assert-md-line-cap.sh`: a convention nothing measures is a convention that
# drifts. Before the pass that introduced this script, 220 files were over the
# cap — `server/operator.rs` had reached 19,000 lines, `company/runtime.rs`
# 16,000 — and none of them arrived there in one commit. A file that size is
# not read, it is grepped, and the module boundary it should have had is
# instead a comment banner somewhere in the middle that nobody keeps current.
#
# The test-file rule exists because an inline test module doubles the length
# of the file it tests and hides the production surface behind it. The
# convention (repo `CLAUDE.md`, "Testing Guidelines") is a sibling file named
# after the source stem — `foo.rs` → `foo_tests.rs`, `foo/mod.rs` →
# `foo/foo_tests.rs` — declared from the source file as
#
#     #[cfg(test)]
#     #[path = "foo_tests.rs"]
#     mod tests;
#
# so `super::*` and every `foo::tests::name` path keep working, and the test
# file can be split by topic (`foo_<topic>_tests.rs`) when it grows.
#
# Fixing a size failure is a split, never a deletion: keep `foo.rs` as the
# module root, move a coherent seam into `foo/<part>.rs`, and re-export so
# every existing `crate::...` path still resolves. Integration targets under
# `crates/*/tests/` and `examples/` are exempt — CI selects those per file, and
# splitting one silently changes which lane runs it (issue #475).
#
# USAGE: assert-rs-source-layout.sh [tests|lines|all]   (default: all)
#
# The two halves landed in separate PRs — the test-file convention first, the
# line cap once the splits that satisfy it had merged — so `ci.yml` names the
# half it is entitled to enforce. Once both are green in CI the argument is
# `all`; the modes stay because a local run of one half is useful on its own.
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 1

MODE="${1:-all}"
case "$MODE" in
  tests|lines|all) ;;
  *) echo "usage: $0 [tests|lines|all]" >&2; exit 2 ;;
esac

LIMIT=750
status=0

OVER=""
[ "$MODE" != "tests" ] && OVER=$(
  find crates -path '*/src/*' -name '*.rs' \
    -not -path '*/target/*' \
    -print0 \
  | xargs -0 wc -l \
  | awk -v limit="$LIMIT" '$1 > limit && $2 != "total" { print $1 "\t" $2 }' \
  | sort -rn
)

if [ -n "$OVER" ]; then
  echo
  echo "✗ Rust source files over the ${LIMIT}-line cap"
  echo "  Split each along a real seam into <stem>/<part>.rs child modules,"
  echo "  keep the original file as the module root, and re-export so existing"
  echo "  paths still resolve. See CLAUDE.md -> 'Coding Style'."
  echo
  echo "$OVER" | sed 's/^/    /'
  echo
  status=1
fi

# An inline test module is `#[cfg(test)]` followed (allowing other attributes
# and blank lines in between) by `mod <name> {`. The convention's declaration
# is `mod tests;` — a semicolon, so it does not match. `*_tests.rs` files are
# skipped: nested helper modules inside a test file are fine.
INLINE=""
[ "$MODE" != "lines" ] && INLINE=$(
  find crates -path '*/src/*' -name '*.rs' \
    -not -name '*_tests.rs' \
    -not -path '*/target/*' \
    -print0 \
  | xargs -0 awk '
      /^[[:space:]]*#\[cfg\(test\)\]/ { pending = 1; next }
      pending && /^[[:space:]]*#\[/ { next }
      pending && /^[[:space:]]*$/ { next }
      pending && /^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[[:space:]]*\{/ {
        print FILENAME ":" FNR
      }
      { pending = 0 }
    '
)

if [ -n "$INLINE" ]; then
  echo
  echo "✗ inline #[cfg(test)] mod blocks — move each to a sibling <stem>_tests.rs"
  echo "  declared with '#[cfg(test)] #[path = \"<stem>_tests.rs\"] mod tests;'."
  echo
  echo "$INLINE" | sed 's/^/    /'
  echo
  status=1
fi

if [ "$status" -eq 0 ]; then
  case "$MODE" in
    tests) echo "✓ rust source layout: tests live in *_tests.rs (line cap not checked in this mode)" ;;
    lines) echo "✓ rust source layout: every crates/*/src file is ${LIMIT} lines or fewer" ;;
    all)   echo "✓ rust source layout: every crates/*/src file is ${LIMIT} lines or fewer and tests live in *_tests.rs" ;;
  esac
fi

exit "$status"
