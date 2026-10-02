// `node --test scripts/release/assert-latest-unchanged.test.mjs`

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const script = path.join(path.dirname(fileURLToPath(import.meta.url)), 'assert-latest-unchanged.sh');

const FAKE_GH = `#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  api) cat "$FAKE_DIR/latest" ;;
  release) echo "$*" >> "$FAKE_DIR/edits" ;;
  *) echo "unexpected gh call: $*" >&2; exit 99 ;;
esac
`;

function run(before, after) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'latest-guard-'));
  fs.mkdirSync(path.join(dir, 'bin'));
  fs.writeFileSync(path.join(dir, 'bin', 'gh'), FAKE_GH, { mode: 0o755 });
  fs.writeFileSync(path.join(dir, 'latest'), `${after}\n`);
  const result = spawnSync('bash', [script], {
    encoding: 'utf8',
    env: {
      ...process.env,
      PATH: `${path.join(dir, 'bin')}${path.delimiter}${process.env.PATH}`,
      FAKE_DIR: dir,
      REPO: 'example/repo',
      COMPANION: 'v1.2.3-windows',
      LATEST_BEFORE: before,
      GH_TOKEN: 'unused',
    },
  });
  const edits = path.join(dir, 'edits');
  return { ...result, edits: fs.existsSync(edits) ? fs.readFileSync(edits, 'utf8').trim() : '' };
}

test('latest unchanged passes and edits nothing', () => {
  const r = run('v1.2.3', 'v1.2.3');
  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.edits, '');
});

test('latest moved is restored to the prior release and the step fails', () => {
  const r = run('v1.2.3', 'v1.2.3-windows');
  assert.notEqual(r.status, 0);
  assert.equal(r.edits, 'release edit v1.2.3 -R example/repo --latest');
  assert.match(r.stderr, /moved to 'v1\.2\.3-windows'.*restored to v1\.2\.3/);
});

test('a missing LATEST_BEFORE fails without touching releases', () => {
  const r = run('', 'v1.2.3');
  assert.notEqual(r.status, 0);
  assert.equal(r.edits, '');
});
