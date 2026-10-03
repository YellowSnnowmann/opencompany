#!/usr/bin/env bash
#
# Fail when the resolve graph holds a second copy of any OpenHuman-family crate.
#
# Issue #1524 (PR 1). The nested-vendor path deps must stay byte-identical to
# openhuman's own, or cargo resolves the same crate from two directories and
# two `MemoryProvider` traits exist in one process (the hazard the `[patch]`
# comment in the root `Cargo.toml` records). `cargo tree -d` prints duplicate
# versions of a package; any family line in that report is the failure.
#
# Widened to the whole family with the hive-desks work: `tinyhivemind-openhuman`
# names `openhuman-embed` by git URL and the root
# `[patch."https://github.com/tinyhumansai/openhuman"]` redirects it onto
# `vendor/openhuman`. If the tinyhivemind `rev` and our gitlink drift, or the
# patch entry is lost, cargo resolves a SECOND OpenHuman tree from the git
# source — two `openhuman_embed::Agent` types — and this is the only place that
# reads as a failure rather than a confusing type mismatch three crates away.
#
# Moved out of the old ci.yml's gated job unchanged, so both CI profiles run it.
set -euo pipefail

cd "$(dirname "$0")/../.."

if ! report="$(cargo tree --locked -p opencompany-core -e normal --features openhuman,mcp,tinymemory -d)"; then
  echo "::error::cargo tree failed to resolve the graph" >&2
  exit 1
fi
if grep -E "^(openhuman|tinymcp|tinyagents|tinyinference|tinymemory|tinyflows|tinybus)" <<< "$report"; then
  echo "::error::duplicated OpenHuman-family package in the resolve graph" >&2
  exit 1
fi
if cargo metadata --locked --format-version 1 \
    | jq -e '[.packages[] | select(.source != null and (.source | test("github.com/tinyhumansai/openhuman")))] | length > 0' >/dev/null; then
  echo "::error::an OpenHuman package resolved from its git source instead of vendor/openhuman" >&2
  exit 1
fi
echo "No duplicated OpenHuman-family crates; every one resolves from vendor/openhuman."
