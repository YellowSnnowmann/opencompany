// `node --test scripts/release/publish-updater-manifest.test.mjs`

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const script = path.join(here, 'publish-updater-manifest.sh');
const conf = path.join(here, '..', '..', 'crates', 'opencompany-app', 'tauri.conf.json');
const version = JSON.parse(fs.readFileSync(conf, 'utf8')).version;

const FAKE_GH = `#!/usr/bin/env bash
set -euo pipefail
case "$1 $2" in
  "release view") cat "$FAKE_DIR/assets.txt" ;;
  "release download")
    while [ $# -gt 0 ]; do
      case "$1" in
        --pattern) pattern="$2"; shift 2 ;;
        --dir) dir="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    cp "$FAKE_DIR/sigs/$pattern" "$dir/$pattern"
    ;;
  "release upload") cp "$4" "$FAKE_DIR/uploaded.json" ;;
  *) echo "unexpected gh call: $*" >&2; exit 99 ;;
esac
`;

const ASSETS = {
  mac: [`OpenCompany_${version}_aarch64.app.tar.gz`, `OpenCompany_${version}_x64.app.tar.gz`],
  win: [`OpenCompany_${version}_x64-setup.exe`],
};

function run(platforms, assets) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'updater-manifest-'));
  fs.mkdirSync(path.join(dir, 'bin'));
  fs.mkdirSync(path.join(dir, 'sigs'));
  fs.writeFileSync(path.join(dir, 'bin', 'gh'), FAKE_GH, { mode: 0o755 });
  fs.writeFileSync(path.join(dir, 'assets.txt'), assets.flatMap((a) => [a, `${a}.sig`]).join('\n') + '\n');
  for (const a of assets) fs.writeFileSync(path.join(dir, 'sigs', `${a}.sig`), `sig-of-${a}\n`);

  const env = {
    ...process.env,
    PATH: `${path.join(dir, 'bin')}${path.delimiter}${process.env.PATH}`,
    FAKE_DIR: dir,
    TAG: `v${version}`,
    REPO: 'example/repo',
    GH_TOKEN: 'unused',
  };
  delete env.PLATFORMS;
  delete env.VERSION;
  if (platforms !== undefined) env.PLATFORMS = platforms;

  const result = spawnSync('bash', [script], { encoding: 'utf8', env });
  const uploaded = path.join(dir, 'uploaded.json');
  const manifest = fs.existsSync(uploaded) ? JSON.parse(fs.readFileSync(uploaded, 'utf8')) : null;
  return { ...result, manifest };
}

test('default platforms is macOS only', () => {
  const r = run(undefined, [...ASSETS.mac, ...ASSETS.win]);
  assert.equal(r.status, 0, r.stderr);
  assert.deepEqual(Object.keys(r.manifest.platforms).sort(), ['darwin-aarch64', 'darwin-x86_64']);
});

test('macos-only selects darwin entries and ignores a windows asset on the release', () => {
  const r = run('macos', [...ASSETS.mac, ...ASSETS.win]);
  assert.equal(r.status, 0, r.stderr);
  assert.deepEqual(Object.keys(r.manifest.platforms).sort(), ['darwin-aarch64', 'darwin-x86_64']);
  assert.equal(r.manifest.version, version);
});

test('macos,windows adds windows-x86_64 with its url and signature', () => {
  const r = run(' macos , windows ', [...ASSETS.mac, ...ASSETS.win]);
  assert.equal(r.status, 0, r.stderr);
  assert.deepEqual(Object.keys(r.manifest.platforms).sort(), [
    'darwin-aarch64',
    'darwin-x86_64',
    'windows-x86_64',
  ]);
  const win = r.manifest.platforms['windows-x86_64'];
  const name = `OpenCompany_${version}_x64-setup.exe`;
  assert.equal(win.url, `https://github.com/example/repo/releases/download/v${version}/${name}`);
  assert.equal(win.signature.trim(), `sig-of-${name}`);
});

test('windows-only produces just the windows entry', () => {
  const r = run('windows', ASSETS.win);
  assert.equal(r.status, 0, r.stderr);
  assert.deepEqual(Object.keys(r.manifest.platforms), ['windows-x86_64']);
});

test('requesting windows without its installer fails and uploads nothing', () => {
  const r = run('macos,windows', ASSETS.mac);
  assert.notEqual(r.status, 0);
  assert.equal(r.manifest, null);
});

test('an unknown platform family fails', () => {
  const r = run('macos,linux', [...ASSETS.mac, ...ASSETS.win]);
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /unknown platform family 'linux'/);
  assert.equal(r.manifest, null);
});

test('an empty PLATFORMS falls back to the macOS default', () => {
  const r = run('', [...ASSETS.mac, ...ASSETS.win]);
  assert.equal(r.status, 0, r.stderr);
  assert.deepEqual(Object.keys(r.manifest.platforms).sort(), ['darwin-aarch64', 'darwin-x86_64']);
});

test('a PLATFORMS of only separators selects nothing and fails', () => {
  const r = run(' , ', [...ASSETS.mac, ...ASSETS.win]);
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /selects no platform/);
  assert.equal(r.manifest, null);
});
